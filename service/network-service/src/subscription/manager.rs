//! Subscription lifecycle (design doc v1.1 §25–§27):
//! ADD -> DOWNLOAD -> DECODE -> PARSE -> NORMALIZE -> VALIDATE -> STORE.
//! Any failure preserves the previous node set; storage swap is atomic.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::common::{mask_url, now_millis, SvcError, SvcResult};
use crate::config::crypto;
use crate::config::manager::{atomic_write_json, read_json_or_warn, ConfigManager};
use crate::ipc::events::{ev, EventBus};
use crate::nodes::manager::NodeManager;
use crate::subscription::decoder::decode_body;
use crate::subscription::downloader::Downloader;
use crate::subscription::parser;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePolicy {
    pub interval_secs: i64,
    pub last_updated: i64,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            interval_secs: 86_400,
            last_updated: 0,
        }
    }
}

/// On-disk representation. `url` is a DPAPI ciphertext, never plaintext.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    #[serde(default)]
    pub update: UpdatePolicy,
    #[serde(default)]
    pub node_ids: Vec<String>,
}

impl Subscription {
    fn plaintext_url(&self) -> SvcResult<String> {
        crypto::unprotect(&self.url)
    }

    /// Safe view for the UI: URL is masked (design doc §52 log/UI hygiene).
    pub fn to_view(&self) -> serde_json::Value {
        let masked = self
            .plaintext_url()
            .map(|u| mask_url(&u))
            .unwrap_or_else(|_| "****".to_string());
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "url": masked,
            "enabled": self.enabled,
            "update": self.update,
            "node_ids": self.node_ids,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionInput {
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub interval_secs: i64,
}

fn default_true() -> bool {
    true
}

pub struct SubscriptionManager {
    items: Arc<RwLock<HashMap<String, Subscription>>>,
    in_flight: Arc<Mutex<HashSet<String>>>,
    config: ConfigManager,
    nodes: Arc<NodeManager>,
    downloader: Downloader,
    bus: EventBus,
}

impl SubscriptionManager {
    pub fn load(
        config: ConfigManager,
        nodes: Arc<NodeManager>,
        downloader: Downloader,
        bus: EventBus,
    ) -> Self {
        let items: Vec<Subscription> =
            read_json_or_warn(&config.paths.subscriptions(), "subscriptions");
        let map = items.into_iter().map(|s| (s.id.clone(), s)).collect();
        Self {
            items: Arc::new(RwLock::new(map)),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            config,
            nodes,
            downloader,
            bus,
        }
    }

    pub fn list_views(&self) -> Vec<serde_json::Value> {
        let mut v: Vec<_> = self.items.read().values().map(|s| s.to_view()).collect();
        v.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        v
    }

    pub fn add(&self, input: SubscriptionInput) -> SvcResult<Subscription> {
        if input.name.trim().is_empty() || input.url.trim().is_empty() {
            return Err(SvcError::ConfigInvalid(
                "subscription name/url required".into(),
            ));
        }
        let sub = Subscription {
            id: format!("sub_{}", uuid::Uuid::new_v4().simple()),
            name: input.name,
            url: crypto::protect(input.url.trim())?,
            enabled: input.enabled,
            update: UpdatePolicy {
                interval_secs: if input.interval_secs > 0 {
                    input.interval_secs
                } else {
                    86_400
                },
                last_updated: 0,
            },
            node_ids: vec![],
        };
        self.items.write().insert(sub.id.clone(), sub.clone());
        self.persist()?;
        Ok(sub)
    }

    pub fn update_fields(
        &self,
        id: &str,
        name: Option<String>,
        url: Option<String>,
        enabled: Option<bool>,
        interval_secs: Option<i64>,
    ) -> SvcResult<()> {
        let mut g = self.items.write();
        let sub = g
            .get_mut(id)
            .ok_or_else(|| SvcError::NotFound(format!("subscription {id}")))?;
        if let Some(n) = name {
            sub.name = n;
        }
        if let Some(u) = url {
            if !u.trim().is_empty() {
                sub.url = crypto::protect(u.trim())?;
            }
        }
        if let Some(e) = enabled {
            sub.enabled = e;
        }
        if let Some(i) = interval_secs {
            if i > 0 {
                sub.update.interval_secs = i;
            }
        }
        drop(g);
        self.persist()
    }

    pub fn delete(&self, id: &str) -> SvcResult<()> {
        let removed = self.items.write().remove(id).is_some();
        if !removed {
            return Err(SvcError::NotFound(format!("subscription {id}")));
        }
        // Remove the nodes it owned.
        self.nodes.replace_subscription_nodes(id, vec![]).ok();
        self.persist()
    }

    fn persist(&self) -> SvcResult<()> {
        let snapshot: Vec<Subscription> = self.items.read().values().cloned().collect();
        atomic_write_json(&self.config.paths.subscriptions(), &snapshot)
    }

    /// Full refresh pipeline. Concurrent refresh of the SAME subscription is
    /// rejected per design doc §65.
    pub async fn refresh(&self, id: &str) -> SvcResult<u32> {
        {
            let mut g = self.in_flight.lock();
            if !g.insert(id.to_string()) {
                return Err(SvcError::AlreadyRunning(format!("refresh {id}")));
            }
        }
        let result = self.refresh_inner(id).await;
        self.in_flight.lock().remove(id);
        match &result {
            Ok(count) => self.bus.emit(
                ev::SUBSCRIPTION_UPDATED,
                &crate::ipc::proto::SubscriptionEventPayload {
                    subscription_id: id.to_string(),
                    name: self
                        .items
                        .read()
                        .get(id)
                        .map(|s| s.name.clone())
                        .unwrap_or_default(),
                    error: String::new(),
                    node_count: *count,
                },
            ),
            Err(e) => {
                tracing::warn!(subscription = %id, error = %e, "subscription refresh failed; keeping old nodes");
                self.bus.emit(
                    ev::SUBSCRIPTION_FAILED,
                    &crate::ipc::proto::SubscriptionEventPayload {
                        subscription_id: id.to_string(),
                        name: self
                            .items
                            .read()
                            .get(id)
                            .map(|s| s.name.clone())
                            .unwrap_or_default(),
                        error: e.to_string(),
                        node_count: 0,
                    },
                );
            }
        }
        result
    }

    async fn refresh_inner(&self, id: &str) -> SvcResult<u32> {
        let (url, current) = {
            let g = self.items.read();
            let sub = g
                .get(id)
                .ok_or_else(|| SvcError::NotFound(format!("subscription {id}")))?;
            (sub.plaintext_url()?, sub.clone())
        };

        // DOWNLOAD -> DECODE -> PARSE (all before touching stored state).
        let body = self.downloader.fetch(&url).await?;
        let lines = decode_body(&body);
        if lines.is_empty() {
            return Err(SvcError::Subscription(
                "no parseable node URIs in subscription".into(),
            ));
        }
        let (nodes, skipped) = parser::parse_all(&lines);
        if nodes.is_empty() {
            return Err(SvcError::Subscription(
                "all subscription lines failed to parse".into(),
            ));
        }

        // NORMALIZE + VALIDATE happened per-node in the parser; one extra pass.
        for n in &nodes {
            n.validate().map_err(SvcError::Subscription)?;
        }

        // STORE: node swap is a single atomic write inside the node manager;
        // only then do we update subscription metadata.
        let node_ids = self.nodes.replace_subscription_nodes(id, nodes)?;
        {
            let mut g = self.items.write();
            if let Some(sub) = g.get_mut(id) {
                sub.node_ids = node_ids.clone();
                sub.update.last_updated = now_millis();
            }
        }
        self.persist()?;
        tracing::info!(
            subscription = %current.name,
            nodes = node_ids.len(),
            skipped,
            "subscription refreshed"
        );
        Ok(node_ids.len() as u32)
    }

    /// Background refresh of all due/enabled subscriptions (called on start
    /// and periodically). A subscription is due when it has never been
    /// refreshed or its configured interval has elapsed.
    pub async fn refresh_due(&self) {
        let now = now_millis();
        let due: Vec<String> = self
            .items
            .read()
            .values()
            .filter(|s| {
                if !s.enabled {
                    return false;
                }
                if s.update.last_updated == 0 {
                    return true;
                }
                let interval_ms = s.update.interval_secs.max(60).saturating_mul(1000);
                now.saturating_sub(s.update.last_updated) >= interval_ms
            })
            .map(|s| s.id.clone())
            .collect();
        if due.is_empty() {
            return;
        }
        tracing::info!("{} subscription(s) due for background refresh", due.len());
        for id in due {
            let _ = self.refresh(&id).await;
        }
    }
}

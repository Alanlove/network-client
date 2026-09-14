//! Application wiring root + IPC method dispatch.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::common::{SvcError, SvcResult};
use crate::config::manager::ConfigManager;
use crate::config::schema::RouteMode;
use crate::diagnostics::engine::DiagnosticEngine;
use crate::diagnostics::ping;
use crate::dns::manager::{DnsManager, ResolvePolicy};
use crate::ipc::events::EventBus;
use crate::ipc::proto::{
    ConnectionStatePayload, Empty, TextPayload, TrafficPayload, VersionPayload,
};
use crate::nodes::manager::{tcp_probe, NodeManager};
use crate::nodes::model::Node;
use crate::proxy::adapter::ProxyCoreAdapter;
use crate::proxy::manager::ExternalCoreAdapter;
use crate::route::manager::RouteManager;
use crate::routing::engine::RoutingEngine;
use crate::statistics::manager::StatsManager;
use crate::subscription::manager::{SubscriptionInput, SubscriptionManager};
use crate::tun::manager::TunManager;

use super::lifecycle::ConnectionManager;
use super::state::StateSnapshot;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct AppContext {
    pub config: ConfigManager,
    pub bus: EventBus,
    pub connection: Arc<ConnectionManager>,
    pub nodes: Arc<NodeManager>,
    pub subscriptions: Arc<SubscriptionManager>,
    pub routing: Arc<RoutingEngine>,
    pub dns: Arc<DnsManager>,
    pub stats: Arc<StatsManager>,
    pub core: Arc<ExternalCoreAdapter>,
    pub route: Arc<RouteManager>,
    pub tun: Arc<TunManager>,
    pub diagnostics: Arc<DiagnosticEngine>,
    pub(crate) diag_running: Arc<Mutex<Option<CancellationToken>>>,
}

impl AppContext {
    pub async fn dispatch(&self, method: &str, payload: &[u8]) -> SvcResult<Vec<u8>> {
        match method {
            // ---- system ---------------------------------------------------
            "system.getVersion" => Ok(encode(&VersionPayload {
                protocol_version: PROTOCOL_VERSION,
                service_version: SERVICE_VERSION.into(),
                core_version: self.config.app().core.kind.clone(),
            })),

            // ---- connection ----------------------------------------------
            "connection.getStatus" => Ok(encode(&state_payload(&self.connection.state.get()))),
            "connection.connect" => {
                let req = decode::<crate::ipc::proto::ConnectRequest>(payload)?;
                self.connection
                    .connect(&req.node_id, if req.mode.is_empty() { "smart" } else { &req.mode })
                    .await?;
                Ok(encode(&state_payload(&self.connection.state.get())))
            }
            "connection.disconnect" => {
                self.connection.disconnect().await?;
                Ok(encode(&state_payload(&self.connection.state.get())))
            }
            "connection.reconnect" => {
                let req = decode::<crate::ipc::proto::ConnectRequest>(payload)?;
                let (node_id, mode) = {
                    let snap = self.connection.state.get();
                    (
                        if req.node_id.is_empty() { snap.node_id } else { req.node_id },
                        if req.mode.is_empty() { snap.mode } else { req.mode },
                    )
                };
                self.connection.disconnect().await?;
                self.connection.connect(&node_id, &mode).await?;
                Ok(encode(&state_payload(&self.connection.state.get())))
            }

            // ---- nodes ----------------------------------------------------
            "node.list" => Ok(json_text(&self.nodes.list())?),
            "node.get" => {
                let id = text_payload(payload)?;
                Ok(json_text(&self.nodes.get(&id)?)?)
            }
            "node.add" => {
                let node: Node = serde_json::from_str(&text_payload(payload)?)?;
                Ok(json_text(&self.nodes.add(node)?)?)
            }
            "node.update" => {
                let node: Node = serde_json::from_str(&text_payload(payload)?)?;
                Ok(json_text(&self.nodes.update(node)?)?)
            }
            "node.delete" => {
                self.nodes.delete(&text_payload(payload)?)?;
                Ok(encode(&Empty::default()))
            }
            "node.select" => {
                self.nodes.select(&text_payload(payload)?)?;
                Ok(encode(&Empty::default()))
            }
            "node.test" => {
                let id = text_payload(payload)?;
                let stats = self.nodes.test_node(&id).await?;
                Ok(json_text(&stats)?)
            }
            "node.batchTest" => {
                let map = self.nodes.clone().batch_test().await?;
                Ok(json_text(&map)?)
            }

            // ---- subscriptions -------------------------------------------
            "subscription.list" => Ok(json_text(&self.subscriptions.list_views())?),
            "subscription.add" => {
                let input: SubscriptionInput =
                    serde_json::from_str(&text_payload(payload)?)?;
                let sub = self.subscriptions.add(input)?;
                Ok(json_text(&sub.to_view())?)
            }
            "subscription.update" => {
                let v: serde_json::Value = serde_json::from_str(&text_payload(payload)?)?;
                let id = v
                    .get("id")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| SvcError::ConfigInvalid("subscription.id required".into()))?;
                self.subscriptions.update_fields(
                    id,
                    v.get("name").and_then(|x| x.as_str()).map(str::to_string),
                    v.get("url").and_then(|x| x.as_str()).map(str::to_string),
                    v.get("enabled").and_then(|x| x.as_bool()),
                    v.get("interval_secs").and_then(|x| x.as_i64()),
                )?;
                Ok(encode(&Empty::default()))
            }
            "subscription.delete" => {
                self.subscriptions.delete(&text_payload(payload)?)?;
                Ok(encode(&Empty::default()))
            }
            "subscription.refresh" => {
                let id = text_payload(payload)?;
                let subs = self.subscriptions.clone();
                tokio::spawn(async move {
                    if let Err(e) = subs.refresh(&id).await {
                        tracing::warn!("subscription refresh {id} failed: {e}");
                    }
                });
                Ok(encode(&Empty::default()))
            }

            // ---- routing --------------------------------------------------
            "routing.getConfig" => Ok(json_text(&self.routing.config())?),
            "routing.setMode" => {
                let v: serde_json::Value = serde_json::from_str(&text_payload(payload)?)?;
                let mode = v
                    .get("mode")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| SvcError::ConfigInvalid("mode required".into()))?;
                self.routing.set_mode(RouteMode::parse(mode))?;
                Ok(json_text(&self.routing.config())?)
            }
            "routing.listRules" => Ok(json_text(&self.routing.list_rules())?),
            "routing.addRule" => {
                let rule: crate::routing::rule::Rule =
                    serde_json::from_str(&text_payload(payload)?)?;
                Ok(json_text(&self.routing.add_rule(rule)?)?)
            }
            "routing.updateRule" => {
                let rule: crate::routing::rule::Rule =
                    serde_json::from_str(&text_payload(payload)?)?;
                self.routing.update_rule(rule)?;
                Ok(encode(&Empty::default()))
            }
            "routing.deleteRule" => {
                self.routing.delete_rule(&text_payload(payload)?)?;
                Ok(encode(&Empty::default()))
            }
            "routing.test" => {
                let input = text_payload(payload)?;
                Ok(json_text(&self.routing.test_match(&input)?)?)
            }

            // ---- dns ------------------------------------------------------
            "dns.getConfig" => Ok(json_text(&self.dns.config())?),
            "dns.setConfig" => {
                let cfg = serde_json::from_str(&text_payload(payload)?)?;
                self.dns.set_config(cfg)?;
                Ok(json_text(&self.dns.config())?)
            }
            "dns.test" => {
                let domain = text_payload(payload).unwrap_or_default();
                Ok(json_text(&self.dns.test_all(&domain).await)?)
            }
            "dns.flushCache" => {
                self.dns.flush_cache();
                Ok(encode(&Empty::default()))
            }

            // ---- statistics ----------------------------------------------
            "statistics.get" | "statistics.getToday" => {
                Ok(encode::<TrafficPayload>(&self.stats.snapshot()))
            }
            "statistics.getRange" => {
                let s = text_payload(payload)?;
                let req: serde_json::Value = serde_json::from_str(s.trim())?;
                let now = crate::common::now_millis();
                let end = req.get("end").and_then(|v| v.as_i64()).unwrap_or(now);
                let start = req.get("start").and_then(|v| v.as_i64()).unwrap_or(end - 120_000);
                let rows = self.stats.db().query_range(start, end)?;
                let samples: Vec<serde_json::Value> = rows.iter().map(|(ts, up, down)| {
                    serde_json::json!({"ts": ts, "up": up, "down": down})
                }).collect();
                Ok(json_text(&serde_json::json!({"samples": samples}))?)
            }

            // ---- diagnostics ---------------------------------------------
            "diagnostics.run" => {
                let mut g = self.diag_running.lock().await;
                if g.is_some() {
                    return Err(SvcError::AlreadyRunning("diagnostics".into()));
                }
                let token = CancellationToken::new();
                *g = Some(token.clone());
                let engine = self.diagnostics.clone();
                let port = self.core.inbound_port();
                let active = self.connection.state.get().state == super::state::ConnState::Connected;
                let running = self.diag_running.clone();
                tokio::spawn(async move {
                    engine
                        .run(if active { Some(port) } else { None }, token)
                        .await;
                    running.lock().await.take();
                });
                Ok(text("started"))
            }
            "diagnostics.cancel" => {
                if let Some(token) = self.diag_running.lock().await.take() {
                    token.cancel();
                }
                Ok(encode(&Empty::default()))
            }
            "diagnostics.ping" => {
                let v: serde_json::Value =
                    serde_json::from_str(&text_payload(payload).unwrap_or_default())
                        .unwrap_or(serde_json::Value::Null);
                let host = v.get("host").and_then(|x| x.as_str()).unwrap_or("223.5.5.5");
                let port = v.get("port").and_then(|x| x.as_u64()).unwrap_or(443) as u16;
                match tcp_probe(host, port, Duration::from_secs(3)).await {
                    Ok(ms) => Ok(json_text(&serde_json::json!({"ok": true, "latency_ms": ms}))?),
                    Err(e) => Ok(json_text(&serde_json::json!({"ok": false, "error": e.to_string()}))?),
                }
            }
            "diagnostics.dns" => Ok(json_text(&self.dns.test_all("").await)?),
            "diagnostics.proxy" => {
                let health = self.core.health_check().await;
                Ok(json_text(&serde_json::json!({
                    "running": health.is_healthy(),
                    "health": format!("{health:?}").to_lowercase(),
                    "port": self.core.inbound_port(),
                    "pid": self.core.pid(),
                }))?)
            }
            "diagnostics.speed" => {
                let item = crate::diagnostics::engine::speed_probe(self.core.inbound_port()).await;
                Ok(json_text(&item)?)
            }
            "diagnostics.packetLoss" => {
                let (rate, avg) = tokio::task::spawn_blocking(|| {
                    ping::ping_loss(Ipv4Addr::new(223, 5, 5, 5), 5, 2000)
                })
                .await
                .unwrap_or((0.0, None));
                Ok(json_text(&serde_json::json!({
                    "success_rate": rate,
                    "avg_latency_ms": avg,
                }))?)
            }
            "diagnostics.resolve" => {
                let name = text_payload(payload).unwrap_or_default();
                match self.dns.resolve(&name, ResolvePolicy::Any).await {
                    Ok(ips) => Ok(json_text(
                        &ips.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                    )?),
                    Err(e) => Err(e),
                }
            }

            "logs.tail" => {
                let n = text_payload(payload)
                    .map(|s| s.trim().parse::<usize>().unwrap_or(200))
                    .unwrap_or(200);
                let log_dir = self.config.paths.logs_dir.clone();
                let today: String = chrono::Local::now().format("%Y-%m-%d").to_string();
                let path = log_dir.join(format!("service.log.{today}"));
                let lines: Vec<String> = if path.exists() {
                    let content = std::fs::read_to_string(&path).unwrap_or_default();
                    let all: Vec<&str> = content.lines().collect();
                    let start = all.len().saturating_sub(n);
                    all[start..].iter().map(|s| s.to_string()).collect()
                } else {
                    Vec::new()
                };
                Ok(json_text(&serde_json::json!({"lines": lines}))?)
            }

            other => Err(SvcError::NotFound(format!("method {other}"))),
        }
    }
}

pub(crate) fn state_payload(s: &StateSnapshot) -> ConnectionStatePayload {
    ConnectionStatePayload {
        state: s.state.as_str().to_string(),
        node_id: s.node_id.clone(),
        node_name: s.node_name.clone(),
        mode: s.mode.clone(),
        latency_ms: s.latency_ms,
        error_code: s.error_code.clone(),
        error_message: s.error_message.clone(),
        timestamp: crate::common::now_millis(),
    }
}

fn encode<M: Message>(m: &M) -> Vec<u8> {
    let mut buf = Vec::with_capacity(m.encoded_len());
    m.encode(&mut buf).expect("prost encode");
    buf
}

fn decode<M: Message + Default>(bytes: &[u8]) -> SvcResult<M> {
    M::decode(bytes).map_err(|e| SvcError::ConfigInvalid(format!("bad payload: {e}")))
}

fn text_payload(bytes: &[u8]) -> SvcResult<String> {
    Ok(TextPayload::decode(bytes)
        .map_err(|e| SvcError::ConfigInvalid(format!("bad TextPayload: {e}")))?
        .text)
}

fn text(s: &str) -> Vec<u8> {
    encode(&TextPayload {
        text: s.to_string(),
    })
}

fn json_text<T: serde::Serialize>(v: &T) -> SvcResult<Vec<u8>> {
    Ok(encode(&TextPayload {
        text: serde_json::to_string(v)?,
    }))
}

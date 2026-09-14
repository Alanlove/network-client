//! Node storage, CRUD and Level-1 (TCP connect) reachability probes
//! (design doc v1.1 §23/§24, v1.0 §28).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use tokio::net::TcpStream;

use crate::common::{now_millis, SvcError, SvcResult};
use crate::config::manager::{atomic_write_json, read_json_or_warn, ConfigManager};
use crate::ipc::events::{ev, EventBus};
use crate::nodes::health::{select_best, NodeRuntimeStats, StatsTable};
use crate::nodes::model::Node;

pub struct NodeManager {
    nodes: Arc<RwLock<HashMap<String, Node>>>,
    selected: Arc<RwLock<Option<String>>>,
    stats: StatsTable,
    in_flight: Arc<Mutex<HashSet<String>>>,
    config: ConfigManager,
    bus: EventBus,
}

impl NodeManager {
    pub fn load(config: ConfigManager, bus: EventBus) -> Self {
        let nodes: Vec<Node> = read_json_or_warn(&config.paths.nodes(), "nodes");
        let map = nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        Self {
            nodes: Arc::new(RwLock::new(map)),
            selected: Arc::new(RwLock::new(None)),
            stats: StatsTable::default(),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            config,
            bus,
        }
    }

    pub fn stats(&self) -> StatsTable {
        self.stats.clone()
    }

    pub fn list(&self) -> Vec<Node> {
        let mut v: Vec<Node> = self.nodes.read().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn get(&self, id: &str) -> SvcResult<Node> {
        self.nodes
            .read()
            .get(id)
            .cloned()
            .ok_or_else(|| SvcError::NodeNotFound(id.to_string()))
    }

    pub fn add(&self, mut node: Node) -> SvcResult<Node> {
        node.validate().map_err(SvcError::ConfigInvalid)?;
        if node.id.is_empty() {
            node.id = format!("node_{}", uuid::Uuid::new_v4().simple());
        }
        if node.created_at == 0 {
            node.created_at = now_millis();
        }
        self.nodes.write().insert(node.id.clone(), node.clone());
        self.persist()?;
        Ok(node)
    }

    pub fn update(&self, node: Node) -> SvcResult<Node> {
        node.validate().map_err(SvcError::ConfigInvalid)?;
        if !self.nodes.read().contains_key(&node.id) {
            return Err(SvcError::NodeNotFound(node.id));
        }
        self.nodes.write().insert(node.id.clone(), node.clone());
        self.persist()?;
        Ok(node)
    }

    pub fn delete(&self, id: &str) -> SvcResult<()> {
        let mut g = self.nodes.write();
        if g.remove(id).is_none() {
            return Err(SvcError::NodeNotFound(id.to_string()));
        }
        drop(g);
        self.stats.clear(id);
        if self.selected.read().as_deref() == Some(id) {
            *self.selected.write() = None;
        }
        self.persist()
    }

    pub fn select(&self, id: &str) -> SvcResult<()> {
        if !self.nodes.read().contains_key(id) {
            return Err(SvcError::NodeNotFound(id.to_string()));
        }
        *self.selected.write() = Some(id.to_string());
        Ok(())
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected.read().clone()
    }

    /// Bulk replace of all nodes belonging to one subscription.
    /// Called by the subscription manager after a successful atomic refresh.
    pub fn replace_subscription_nodes(
        &self,
        subscription_id: &str,
        incoming: Vec<Node>,
    ) -> SvcResult<Vec<String>> {
        let mut g = self.nodes.write();
        g.retain(|_, n| n.subscription_id != subscription_id);
        let mut ids = Vec::with_capacity(incoming.len());
        for mut n in incoming {
            n.subscription_id = subscription_id.to_string();
            n.validate().map_err(SvcError::ConfigInvalid)?;
            ids.push(n.id.clone());
            g.insert(n.id.clone(), n);
        }
        drop(g);
        self.persist()?;
        Ok(ids)
    }

    fn persist(&self) -> SvcResult<()> {
        let snapshot: Vec<Node> = self.list();
        atomic_write_json(&self.config.paths.nodes(), &snapshot)
    }

    /// Level-1 probe: raw TCP connect to the node endpoint (design doc §28).
    pub async fn test_node(&self, id: &str) -> SvcResult<NodeRuntimeStats> {
        {
            let mut g = self.in_flight.lock();
            if !g.insert(id.to_string()) {
                return Err(SvcError::AlreadyRunning(format!("test {id}")));
            }
        }
        let result = self.test_node_inner(id).await;
        self.in_flight.lock().remove(id);
        result
    }

    async fn test_node_inner(&self, id: &str) -> SvcResult<NodeRuntimeStats> {
        let node = self.get(id)?;
        let probe =
            tcp_probe(&node.endpoint.host, node.endpoint.port, Duration::from_secs(3)).await;
        let stats = self.stats.mutate(id, |s| {
            let (ok, latency) = match probe {
                Ok(ms) => (true, Some(ms)),
                Err(_) => (false, None),
            };
            s.record_probe(ok, latency, now_millis());
        });
        let payload = crate::ipc::proto::NodeStatsPayload {
            node_id: id.to_string(),
            latency_ms: stats.latency_ms,
            packet_loss: stats.packet_loss,
            download_speed: stats.download_speed,
            upload_speed: stats.upload_speed,
            availability: stats.availability,
            score: stats.score,
            last_test: stats.last_test,
            success_count: stats.success_count,
            failure_count: stats.failure_count,
        };
        self.bus.emit(ev::NODE_LATENCY_CHANGED, &payload);
        self.bus.emit(ev::NODE_HEALTH_CHANGED, &payload);
        Ok(stats)
    }

    /// Test all enabled nodes with bounded concurrency. A single failure
    /// never aborts the batch; the per-node stats record the failure.
    pub async fn batch_test(self: &Arc<Self>) -> SvcResult<HashMap<String, NodeRuntimeStats>> {
        let ids: Vec<String> = self
            .list()
            .into_iter()
            .filter(|n| n.enabled)
            .map(|n| n.id)
            .collect();
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut handles = Vec::new();
        for id in ids {
            let permit = sem.clone().acquire_owned().await.ok();
            let this = Arc::clone(self);
            handles.push(tokio::spawn(async move {
                let _p = permit;
                let res = this.test_node(&id).await;
                (id, res.ok())
            }));
        }
        let mut out = HashMap::new();
        for h in handles {
            if let Ok((id, stats)) = h.await {
                if let Some(s) = stats {
                    out.insert(id, s);
                }
            }
        }
        Ok(out)
    }

    /// Smart selection: rank, give the top 5 a fresh final probe, then apply
    /// hysteresis before deciding to switch (design doc §30/§31).
    pub async fn smart_select(&self, threshold: f64) -> SvcResult<Option<String>> {
        let scores: Vec<(String, f64)> = self
            .list()
            .into_iter()
            .filter(|n| n.enabled)
            .filter_map(|n| self.stats.get(&n.id).map(|s| (n.id, s.score)))
            .collect();
        if scores.is_empty() {
            return Ok(None);
        }
        let mut ranked = scores;
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (id, _) in ranked.iter().take(5) {
            let _ = self.test_node(id).await;
        }
        let fresh: Vec<(String, f64)> = self
            .list()
            .into_iter()
            .filter_map(|n| self.stats.get(&n.id).map(|s| (n.id, s.score)))
            .collect();
        Ok(select_best(
            &fresh,
            self.selected.read().as_deref(),
            threshold,
        ))
    }
}

pub async fn tcp_probe(host: &str, port: u16, timeout: Duration) -> SvcResult<i64> {
    let started = std::time::Instant::now();
    match tokio::time::timeout(timeout, TcpStream::connect((host, port))).await {
        Ok(Ok(_)) => {
            // Round (don't truncate) sub-ms handshakes; a successful connect
            // is reported as >=1 ms so zero can never mean "success/instant".
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            Ok((ms.round() as i64).max(1))
        }
        Ok(Err(e)) => Err(SvcError::Other(e.to_string())),
        Err(_) => Err(SvcError::Timeout),
    }
}

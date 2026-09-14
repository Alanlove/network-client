//! Connection orchestration (design doc §48–§51).
//!
//! Connect order (each step validated, failures unwind what was started):
//!   validate node -> start core -> (optional TUN) -> system proxy ->
//!   stats session -> health -> CONNECTED
//! Disconnect closes everything in reverse and is idempotent.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::common::{now_millis, SvcError, SvcResult};
use crate::config::manager::ConfigManager;
use crate::config::schema::{RouteMode, RuntimePersist};
use crate::dns::manager::DnsManager;
use crate::ipc::events::EventBus;
use crate::nodes::manager::NodeManager;
use crate::nodes::model::Node;
use crate::proxy::adapter::ProxyCoreAdapter;
use crate::proxy::manager::ExternalCoreAdapter;
use crate::route::bypass;
use crate::route::manager::RouteManager;
use crate::routing::engine::RoutingEngine;
use crate::statistics::manager::StatsManager;
use crate::tun::manager::TunManager;

use super::state::{ConnState, StateStore};

pub struct ConnectionManager {
    pub config: ConfigManager,
    pub bus: EventBus,
    pub nodes: Arc<NodeManager>,
    pub routing: Arc<RoutingEngine>,
    pub dns: Arc<DnsManager>,
    pub core: Arc<ExternalCoreAdapter>,
    pub route: Arc<RouteManager>,
    pub tun: Arc<TunManager>,
    pub stats: Arc<StatsManager>,
    pub state: StateStore,
    guard: Mutex<()>,
    active_node: RwLock<Option<Node>>,
    monitor: Mutex<Option<(CancellationToken, JoinHandle<()>)>>,
    restart_count: AtomicU32,
}

impl ConnectionManager {
    pub fn new(
        config: ConfigManager,
        bus: EventBus,
        nodes: Arc<NodeManager>,
        routing: Arc<RoutingEngine>,
        dns: Arc<DnsManager>,
        core: Arc<ExternalCoreAdapter>,
        route: Arc<RouteManager>,
        tun: Arc<TunManager>,
        stats: Arc<StatsManager>,
    ) -> Self {
        let state = StateStore::new(bus.clone());
        Self {
            config,
            bus,
            nodes,
            routing,
            dns,
            core,
            route,
            tun,
            stats,
            state,
            guard: Mutex::new(()),
            active_node: RwLock::new(None),
            monitor: Mutex::new(None),
            restart_count: AtomicU32::new(0),
        }
    }

    pub fn active_node(&self) -> Option<Node> {
        self.active_node.read().clone()
    }

    fn persist_runtime(&self, state: ConnState, node: Option<&Node>, mode: RouteMode) {
        let rt = RuntimePersist {
            last_state: state.as_str().to_string(),
            node_id: node.map(|n| n.id.clone()).unwrap_or_default(),
            mode: mode.as_str().to_string(),
            updated_at: now_millis(),
        };
        if let Err(e) = self.config.save_runtime(rt) {
            tracing::warn!("save runtime state failed: {e}");
        }
    }

    pub async fn connect(self: &Arc<Self>, node_id: &str, mode: &str) -> SvcResult<()> {
        let _g = self.guard.lock().await;
        self.connect_locked(node_id, mode).await
    }

    /// Connect body. Callers MUST hold `self.guard` (public `connect`, boot
    /// recovery). The serialized sections also protect system-proxy state,
    /// the proxy backup snapshot and the core child handle.
    async fn connect_locked(self: &Arc<Self>, node_id: &str, mode: &str) -> SvcResult<()> {
        // In-place switch: a connect() on a live session with a different
        // node (or mode) tears down just the core and dials the new target.
        // The system proxy and the user's pre-connect backup stay intact —
        // the local inbound port never changes.
        if self.state.state() == ConnState::Connected {
            let same_target = self.active_node().map(|n| n.id == node_id).unwrap_or(false)
                && self.state.get().mode.eq_ignore_ascii_case(mode);
            if same_target {
                return Err(SvcError::AlreadyRunning("connection".into()));
            }
            tracing::info!("switching connection to {node_id} (mode {mode})");
            self.state.transition(ConnState::Stopping)?;
            self.stop_monitor().await;
            self.tun.stop().await;
            self.core.stop().await;
            self.stats.end_session();
            *self.active_node.write() = None;
            self.state.transition(ConnState::Starting)?;
        } else if matches!(
            self.state.state(),
            ConnState::Starting | ConnState::Connecting | ConnState::Stopping
        ) {
            return Err(SvcError::AlreadyRunning("connection".into()));
        }

        let mode = RouteMode::parse(mode);
        if self.state.state() != ConnState::Starting {
            self.state.transition(ConnState::Starting)?;
        }

        let node = match self.nodes.get(node_id) {
            Ok(n) if n.enabled => n,
            Ok(_) => {
                let e = SvcError::ConfigInvalid("node disabled".into());
                self.abort_start(&e, mode, None);
                return Err(e);
            }
            Err(e) => {
                self.abort_start(&e, mode, None);
                return Err(e);
            }
        };
        self.nodes.select(&node.id)?;
        self.routing.set_mode(mode)?;

        if let Err(e) = self.state.set_context(
            ConnState::Connecting,
            &node.id,
            &node.name,
            mode.as_str(),
        ) {
            self.abort_start(&e, mode, Some(&node));
            return Err(e);
        }

        // 1. Start the proxy core.
        if let Err(e) = self.core.start(&node, &self.dns.config()).await {
            tracing::error!("core start failed: {e}");
            self.cleanup_partial().await;
            self.abort_start(&e, mode, Some(&node));
            return Err(e);
        }

        // 2. Optional TUN adapter (graceful: fall back to system proxy).
        if self.config.app().tun.enabled {
            if let Err(e) = self.tun.start().await {
                tracing::warn!("TUN unavailable, using system proxy: {e}");
            }
        }

        // 3. System proxy -> local mixed inbound.
        let port = self.core.inbound_port();
        let route = self.route.clone();
        let bypass = bypass::default_bypass();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            route.set_system_proxy(&format!("127.0.0.1:{port}"), &bypass)
        })
        .await
        .unwrap_or_else(|e| Err(SvcError::Other(e.to_string())))
        {
            self.cleanup_partial().await;
            self.abort_start(&e, mode, Some(&node));
            return Err(e);
        }

        // 4. Stats + state.
        self.stats.start_session(&node.id);
        *self.active_node.write() = Some(node.clone());
        self.restart_count.store(0, Ordering::SeqCst);
        self.state.set_context(
            ConnState::Connected,
            &node.id,
            &node.name,
            mode.as_str(),
        )?;
        self.persist_runtime(ConnState::Connected, Some(&node), mode);

        // 5. Best-effort first latency reading.
        let nid = node.id.clone();
        let latency = crate::nodes::manager::tcp_probe(
            &node.endpoint.host,
            node.endpoint.port,
            std::time::Duration::from_secs(3),
        )
        .await
        .unwrap_or(-1);
        if let Ok(stats) = self.nodes.test_node(&nid).await {
            self.state.set_latency(stats.latency_ms);
        } else {
            self.state.set_latency(latency);
        }

        tracing::info!("connected: {} ({})", node.name, mode.as_str());
        self.clone().spawn_monitor().await;
        Ok(())
    }

    pub async fn disconnect(&self) -> SvcResult<()> {
        let _g = self.guard.lock().await;
        let from_error = self.state.state() == ConnState::Error;
        if !from_error && self.state.state() != ConnState::Connected
            && self.state.state() != ConnState::Reconnecting
        {
            return Ok(()); // already disconnected — idempotent
        }
        if !from_error {
            self.state.transition(ConnState::Stopping)?;
        }
        self.stop_monitor().await;
        self.cleanup_partial().await;
        let mode = RouteMode::parse(&self.state.get().mode);
        self.persist_runtime(ConnState::Stopped, None, mode);
        *self.active_node.write() = None;
        if from_error {
            self.state.transition(ConnState::Stopped)?;
        } else {
            self.state.transition(ConnState::Stopped)?;
        }
        tracing::info!("disconnected");
        Ok(())
    }

    async fn cleanup_partial(&self) {
        self.tun.stop().await;
        let route = self.route.clone();
        tokio::task::spawn_blocking(move || route.clear_system_proxy())
            .await
            .ok();
        self.core.stop().await;
        self.stats.end_session();
    }

    fn abort_start(&self, e: &SvcError, mode: RouteMode, node: Option<&Node>) {
        self.state.fail(ConnState::Error, &e.code().to_string(), &e.to_string());
        self.persist_runtime(ConnState::Error, node, mode);
        *self.active_node.write() = None;
    }

    /// Recovery bookkeeping shared by every successful recovery branch:
    /// reset the restart budget and persist the live state. The health
    /// supervisor loop (which calls `recover`) re-enters its tick phase on
    /// its own once this returns and the state is CONNECTED again.
    fn mark_recovered(&self, node: &Node) {
        self.restart_count.store(0, Ordering::SeqCst);
        let mode = self.state.get().mode;
        let _ = self
            .state
            .set_context(ConnState::Connected, &node.id, &node.name, &mode);
        self.persist_runtime(ConnState::Connected, Some(node), RouteMode::parse(&mode));
        tracing::info!("connection recovered on node {}", node.name);
    }

    /// Called by the health monitor after repeated failures.
    pub async fn recover(self: &Arc<Self>) {
        let Some(node) = self.active_node() else {
            return;
        };
        if self.state.state() != ConnState::Connected {
            return;
        }
        let max = self.config.app().core.restart_max;
        let attempt = self.restart_count.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::warn!("connection recovery attempt {attempt}/{}", max);
        let _ = self.state.transition(ConnState::Reconnecting);

        if attempt <= max {
            if self
                .core
                .restart(&node, &self.dns.config())
                .await
                .is_ok()
                && self.core.health_check().await.is_healthy()
            {
                self.mark_recovered(&node);
                return;
            }
        } else if self.routing.config().mode == RouteMode::Smart {
            // Out of restarts: smart mode tries another node once.
            let threshold = self.config.app().smart.switch_threshold;
            if let Ok(Some(new_id)) = self.nodes.smart_select(threshold).await {
                if new_id != node.id {
                    if let Ok(new_node) = self.nodes.get(&new_id) {
                        self.core.stop().await;
                        if self.core.start(&new_node, &self.dns.config()).await.is_ok()
                            && self.core.health_check().await.is_healthy()
                        {
                            self.nodes.select(&new_id).ok();
                            *self.active_node.write() = Some(new_node.clone());
                            self.mark_recovered(&new_node);
                            return;
                        }
                    }
                }
            }
        }

        // Give up — tear everything down so the system proxy never dangles
        // at a dead local port, then surface ERROR. NOTE: we are running
        // INSIDE the supervisor task, so it must not await its own handle;
        // the supervisor exits once it sees the non-CONNECTED state.
        tracing::error!("proxy core unreachable after {max} recovery attempts; giving up");
        self.cleanup_partial().await;
        *self.active_node.write() = None;
        self.state.fail(
            ConnState::Error,
            "601",
            "proxy core unreachable after recovery attempts",
        );
        self.persist_runtime(
            ConnState::Error,
            Some(&node),
            RouteMode::parse(&self.state.get().mode),
        );
    }

    /// Run once at service boot. Responsibilities:
    /// 1. Kill proxy-core processes orphaned by a hard-killed service instance.
    /// 2. Honour `restore_previous_connection`: reconnect automatically,
    ///    preserving the user's original proxy snapshot for later restore.
    /// 3. Otherwise (or on reconnect failure) restore the user's proxy
    ///    settings / clear a stale dead-port proxy.
    pub async fn startup_recover(self: &Arc<Self>) {
        // Hold the connection guard for the ENTIRE recovery: IPC clients
        // issuing connect/disconnect while stale cores are being reaped (or
        // while the registry is being restored) would race the teardown.
        let _g = self.guard.lock().await;

        self.core.reap_stale_cores().await;

        let runtime = self.config.runtime();
        if runtime.node_id.is_empty() {
            return;
        }
        let port = self.config.app().core.mixed_port;
        let mode = RouteMode::parse(&runtime.mode);

        // Automatic reconnection only applies to a session that was live
        // when the previous instance died. Clean STOPPED/ERROR shutdowns
        // still get their stale proxy cleaned below, but never auto-dial.
        let was_connected = runtime.last_state == ConnState::Connected.as_str();
        let node = self
            .nodes
            .get(&runtime.node_id)
            .ok()
            .filter(|n| n.enabled);

        let restored = if was_connected && self.config.app().restore_previous_connection {
            match node {
                Some(node) => {
                    tracing::info!(
                        "restore_previous_connection=true; reconnecting to {}",
                        node.name
                    );
                    // The live registry already points at our inbound; keep
                    // the crash backup as the "before" snapshot so clean
                    // disconnect restores the USER's original settings.
                    self.route.prime_from_backup();
                    match self.connect_locked(&node.id, mode.as_str()).await {
                        Ok(()) => {
                            // connect() skips registry writes when a saved
                            // snapshot already exists; make sure the proxy is
                            // actually live even if it was changed while down.
                            let bypass = bypass::default_bypass();
                            self.route.ensure_proxy_applied(
                                &format!("127.0.0.1:{}", self.core.inbound_port()),
                                &bypass,
                            );
                            true
                        }
                        Err(e) => {
                            tracing::error!("auto-restore failed: {e}");
                            false
                        }
                    }
                }
                None => {
                    tracing::warn!(
                        "previous node {} no longer exists/disabled; cannot restore",
                        runtime.node_id
                    );
                    false
                }
            }
        } else {
            false
        };

        if !restored {
            self.route.restore_after_crash(port);
            self.persist_runtime(ConnState::Stopped, None, mode);
        }
    }

    pub async fn spawn_monitor(self: Arc<Self>) {
        let token = CancellationToken::new();
        let task_token = token.clone();
        let interval =
            std::time::Duration::from_secs(self.config.app().smart.health_interval_secs.max(10));
        let cm = self.clone();
        let handle = tokio::spawn(async move {
            crate::health::monitor::supervise(cm, interval, task_token).await;
        });
        *self.monitor.lock().await = Some((token, handle));
    }

    async fn stop_monitor(&self) {
        if let Some((token, handle)) = self.monitor.lock().await.take() {
            token.cancel();
            let _ = handle.await;
        }
    }
}


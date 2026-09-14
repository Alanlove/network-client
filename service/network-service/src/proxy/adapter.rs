//! ProxyCoreAdapter boundary (design doc v1.1 §20).
//!
//! The UI, node manager and routing engine never know which proxy core is in
//! use. Implementations: external process (sing-box/xray), later in-process
//! cores or remote services.

use async_trait::async_trait;
use serde::Serialize;

use crate::common::SvcResult;
use crate::config::schema::DnsConfig;
use crate::nodes::model::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreStatus {
    Stopped,
    Starting,
    Running,
    Error,
}

/// Liveness tri-state. Distinguishing a dead local process from a dead
/// uplink lets the supervisor react immediately to a crashed core while
/// tolerating transient remote hiccups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreHealth {
    /// Inbound accepts AND a real request traverses the tunnel.
    Healthy,
    /// The local inbound does not accept — the core process is gone.
    /// A definitive signal that needs no failure tolerance.
    LocalDead,
    /// Inbound is up but a request THROUGH the tunnel failed. The remote
    /// side may be having a transient outage; tolerate a few strikes.
    UplinkDead,
}

impl CoreHealth {
    pub fn is_healthy(self) -> bool {
        matches!(self, CoreHealth::Healthy)
    }
}

#[async_trait]
pub trait ProxyCoreAdapter: Send + Sync {
    async fn start(&self, node: &Node, dns: &DnsConfig) -> SvcResult<()>;
    async fn stop(&self);
    async fn restart(&self, node: &Node, dns: &DnsConfig) -> SvcResult<()>;
    fn status(&self) -> CoreStatus;
    fn pid(&self) -> Option<u32>;
    /// Local inbound port the rest of the service should send traffic to.
    fn inbound_port(&self) -> u16;
    /// Liveness probe: local inbound TCP check followed by a real request
    /// through the tunnel. See [`CoreHealth`] for tri-state semantics.
    async fn health_check(&self) -> CoreHealth;
}

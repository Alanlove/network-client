//! In-process pub/sub for service -> UI push events (design doc §7).

use prost::Message;
use tokio::sync::broadcast;

use crate::common::now_millis;

// Event type constants — frozen surface, mirrored by the C# client.
pub mod ev {
    pub const CONNECTION_STATE_CHANGED: &str = "connection.stateChanged";
    pub const NODE_LATENCY_CHANGED: &str = "node.latencyChanged";
    pub const NODE_HEALTH_CHANGED: &str = "node.healthChanged";
    pub const SUBSCRIPTION_UPDATED: &str = "subscription.updated";
    pub const SUBSCRIPTION_FAILED: &str = "subscription.failed";
    pub const TRAFFIC_UPDATED: &str = "traffic.updated";
    pub const NETWORK_CHANGED: &str = "network.changed";
    pub const DNS_CHANGED: &str = "dns.changed";
    pub const CORE_STARTED: &str = "core.started";
    pub const CORE_STOPPED: &str = "core.stopped";
    pub const CORE_ERROR: &str = "core.error";
    pub const DIAGNOSTICS_PROGRESS: &str = "diagnostics.progress";
    pub const DIAGNOSTICS_COMPLETED: &str = "diagnostics.completed";
}

#[derive(Clone, Debug)]
pub struct EventMessage {
    pub event_type: String,
    pub payload: Vec<u8>,
    pub timestamp: i64,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<EventMessage>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(16));
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventMessage> {
        self.tx.subscribe()
    }

    pub fn emit_raw(&self, msg: EventMessage) {
        // No subscribers is a normal state (UI closed) — ignore the error.
        let _ = self.tx.send(msg);
    }

    pub fn emit<M: Message>(&self, event_type: &str, payload: &M) {
        let mut buf = Vec::with_capacity(payload.encoded_len());
        if payload.encode(&mut buf).is_ok() {
            self.emit_raw(EventMessage {
                event_type: event_type.to_string(),
                payload: buf,
                timestamp: now_millis(),
            });
        }
    }

    pub fn emit_empty(&self, event_type: &str) {
        self.emit(event_type, &crate::ipc::proto::Empty {});
    }
}

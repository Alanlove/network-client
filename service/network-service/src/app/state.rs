//! Connection state machine (design doc §12/§50). Every transition is
//! validated and broadcasts `connection.stateChanged`.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::common::{now_millis, SvcError, SvcResult};
use crate::ipc::events::{ev, EventBus};
use crate::ipc::proto::ConnectionStatePayload;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnState {
    #[serde(rename = "STOPPED")]
    Stopped,
    #[serde(rename = "INITIALIZING")]
    Initializing,
    #[serde(rename = "READY")]
    Ready,
    #[serde(rename = "STARTING")]
    Starting,
    #[serde(rename = "CONNECTING")]
    Connecting,
    #[serde(rename = "CONNECTED")]
    Connected,
    #[serde(rename = "RECONNECTING")]
    Reconnecting,
    #[serde(rename = "STOPPING")]
    Stopping,
    #[serde(rename = "ERROR")]
    Error,
}

impl Default for ConnState {
    fn default() -> Self {
        ConnState::Stopped
    }
}

impl ConnState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnState::Stopped => "STOPPED",
            ConnState::Initializing => "INITIALIZING",
            ConnState::Ready => "READY",
            ConnState::Starting => "STARTING",
            ConnState::Connecting => "CONNECTING",
            ConnState::Connected => "CONNECTED",
            ConnState::Reconnecting => "RECONNECTING",
            ConnState::Stopping => "STOPPING",
            ConnState::Error => "ERROR",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "INITIALIZING" => ConnState::Initializing,
            "READY" => ConnState::Ready,
            "STARTING" => ConnState::Starting,
            "CONNECTING" => ConnState::Connecting,
            "CONNECTED" => ConnState::Connected,
            "RECONNECTING" => ConnState::Reconnecting,
            "STOPPING" => ConnState::Stopping,
            "ERROR" => ConnState::Error,
            _ => ConnState::Stopped,
        }
    }

    fn can_transition(self, to: ConnState) -> bool {
        use ConnState::*;
        matches!(
            (self, to),
            (Stopped, Starting)
                | (Initializing, Ready)
                | (Initializing, Error)
                | (Ready, Starting)
                | (Starting, Connecting)
                | (Starting, Error)
                | (Starting, Stopping)
                | (Connecting, Connected)
                | (Connecting, Reconnecting)
                | (Connecting, Error)
                | (Connecting, Stopping)
                | (Connected, Reconnecting)
                | (Connected, Stopping)
                | (Connected, Error)
                | (Reconnecting, Connected)
                | (Reconnecting, Error)
                | (Reconnecting, Stopping)
                | (Stopping, Starting) // in-place node/mode switch
                | (Stopping, Stopped)
                | (Stopping, Error)
                | (Error, Starting)
                | (Error, Stopped)
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct StateSnapshot {
    pub state: ConnState,
    pub node_id: String,
    pub node_name: String,
    pub mode: String,
    pub latency_ms: i64,
    pub error_code: String,
    pub error_message: String,
}

#[derive(Default)]
struct Inner {
    state: ConnState,
    node_id: String,
    node_name: String,
    mode: String,
    latency_ms: i64,
    error_code: String,
    error_message: String,
}

pub struct StateStore {
    inner: RwLock<Inner>,
    bus: EventBus,
}

impl StateStore {
    pub fn new(bus: EventBus) -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
            bus,
        }
    }

    pub fn get(&self) -> StateSnapshot {
        let g = self.inner.read();
        StateSnapshot {
            state: g.state,
            node_id: g.node_id.clone(),
            node_name: g.node_name.clone(),
            mode: g.mode.clone(),
            latency_ms: g.latency_ms,
            error_code: g.error_code.clone(),
            error_message: g.error_message.clone(),
        }
    }

    pub fn state(&self) -> ConnState {
        self.inner.read().state
    }

    /// Validate and perform a bare transition; other fields unchanged.
    pub fn transition(&self, to: ConnState) -> SvcResult<()> {
        let mut g = self.inner.write();
        let from = g.state;
        if from == to {
            return Ok(());
        }
        if !from.can_transition(to) {
            return Err(SvcError::InvalidState(from.as_str().into(), to.as_str().into()));
        }
        g.state = to;
        if matches!(to, ConnState::Connected | ConnState::Ready) {
            g.error_code.clear();
            g.error_message.clear();
        }
        self.emit_locked(&g);
        Ok(())
    }

    /// Transition while binding the active node/mode context.
    pub fn set_context(&self, to: ConnState, node_id: &str, node_name: &str, mode: &str) -> SvcResult<()> {
        let mut g = self.inner.write();
        let from = g.state;
        if from != to && !from.can_transition(to) {
            return Err(SvcError::InvalidState(from.as_str().into(), to.as_str().into()));
        }
        g.state = to;
        g.node_id = node_id.to_string();
        g.node_name = node_name.to_string();
        g.mode = mode.to_string();
        self.emit_locked(&g);
        Ok(())
    }

    pub fn set_latency(&self, latency_ms: i64) {
        let mut g = self.inner.write();
        g.latency_ms = latency_ms;
        self.emit_locked(&g);
    }

    pub fn fail(&self, to: ConnState, code: &str, message: &str) {
        let mut g = self.inner.write();
        g.state = to;
        g.error_code = code.to_string();
        g.error_message = message.to_string();
        self.emit_locked(&g);
    }

    fn emit_locked(&self, g: &Inner) {
        self.bus.emit(
            ev::CONNECTION_STATE_CHANGED,
            &ConnectionStatePayload {
                state: g.state.as_str().to_string(),
                node_id: g.node_id.clone(),
                node_name: g.node_name.clone(),
                mode: g.mode.clone(),
                latency_ms: g.latency_ms,
                error_code: g.error_code.clone(),
                error_message: g.error_message.clone(),
                timestamp: now_millis(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> StateStore {
        StateStore::new(EventBus::new(4))
    }

    #[test]
    fn happy_path_transitions() {
        let s = store();
        s.transition(ConnState::Starting).unwrap();
        s.transition(ConnState::Connecting).unwrap();
        s.transition(ConnState::Connected).unwrap();
        s.transition(ConnState::Stopping).unwrap();
        s.transition(ConnState::Stopped).unwrap();
    }

    #[test]
    fn rejects_illegal_jump() {
        let s = store();
        assert!(s.transition(ConnState::Connected).is_err());
        s.transition(ConnState::Starting).unwrap();
        assert!(s.transition(ConnState::Stopped).is_err());
    }
}

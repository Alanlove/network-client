//! Periodic health supervision (design doc §52). Every interval:
//!   core inbound probe -> active node latency refresh
//! Three consecutive failures trigger `ConnectionManager::recover`
//! (restart up to restart_max, then one smart-mode node switch, then ERROR).
//!
//! This module owns ONE long-lived supervisor task per connection. It never
//! spawns replacement tasks itself: after a recovery round it simply loops
//! back into a fresh tick phase while the state is CONNECTED, which both
//! keeps failure accounting local and avoids recursive task spawning.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::app::lifecycle::ConnectionManager;
use crate::app::state::ConnState;
use crate::proxy::adapter::{CoreHealth, ProxyCoreAdapter};

/// Outcome of one ticking phase.
enum Phase {
    /// Health is fine / connection is gone / shutdown requested — stop
    /// supervising entirely.
    Finished,
    /// Probe failure threshold reached: run recovery, then decide whether
    /// to keep ticking based on the resulting state.
    NeedRecovery,
}

pub async fn supervise(cm: Arc<ConnectionManager>, every: Duration, token: CancellationToken) {
    loop {
        match tick_phase(&cm, every, &token).await {
            Phase::Finished => break,
            Phase::NeedRecovery => {
                cm.recover().await;
                if cm.state.state() != ConnState::Connected {
                    break;
                }
                // Recovered (same node or smart switch): counters reset with
                // a brand new tick phase below.
            }
        }
    }
}

async fn tick_phase(
    cm: &Arc<ConnectionManager>,
    every: Duration,
    token: &CancellationToken,
) -> Phase {
    let mut ticker = interval(every);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut failures = 0u32;
    let mut ticks = 0u32;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = token.cancelled() => return Phase::Finished,
        }
        if cm.state.state() != ConnState::Connected {
            return Phase::Finished;
        }
        ticks += 1;

        match cm.core.health_check().await {
            CoreHealth::Healthy => failures = 0,
            CoreHealth::LocalDead => {
                // The core process itself is gone — no point tolerating
                // strikes against a corpse; recover on the next tick.
                tracing::error!("core inbound dead; triggering immediate recovery");
                return Phase::NeedRecovery;
            }
            CoreHealth::UplinkDead => {
                failures += 1;
                tracing::warn!("tunnel uplink probe failed ({failures}/3)");
                if failures >= 3 {
                    return Phase::NeedRecovery;
                }
                continue;
            }
        }

        // Refresh active-node latency roughly every 5 minutes.
        if ticks % 5 == 0 {
            if let Some(node) = cm.active_node() {
                if let Ok(stats) = cm.nodes.test_node(&node.id).await {
                    cm.state.set_latency(stats.latency_ms);
                }
            }
        }
    }
}

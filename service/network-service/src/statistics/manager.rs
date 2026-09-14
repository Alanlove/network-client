//! Memory counters -> periodic SQLite flush (design doc §43). Never write per
//! packet; flush every 2 seconds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::common::{now_millis, today_midnight_millis};
use crate::config::manager::ConfigManager;
use crate::ipc::events::{ev, EventBus};
use crate::statistics::database::Database;

pub struct StatsManager {
    db: Arc<Database>,
    bus: EventBus,
    up: AtomicU64,
    down: AtomicU64,
    session: Mutex<Option<(String, String)>>,
    last: Mutex<Option<(Instant, u64, u64)>>,
}

impl StatsManager {
    pub fn load(config: ConfigManager, bus: EventBus) -> std::io::Result<Arc<Self>> {
        let db = Database::open(&config.paths.database())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok(Arc::new(Self {
            db: Arc::new(db),
            bus,
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            session: Mutex::new(None),
            last: Mutex::new(None),
        }))
    }

    pub fn db(&self) -> Arc<Database> {
        self.db.clone()
    }

    pub fn start_session(&self, node_id: &str) {
        self.end_session();
        let id = format!("sess_{}", uuid::Uuid::new_v4().simple());
        self.up.store(0, Ordering::SeqCst);
        self.down.store(0, Ordering::SeqCst);
        *self.last.lock() = Some((Instant::now(), 0, 0));
        if let Err(e) = self.db.begin_session(&id, node_id, now_millis()) {
            tracing::error!("begin_session failed: {e}");
        }
        *self.session.lock() = Some((id, node_id.to_string()));
    }

    pub fn end_session(&self) {
        let Some((id, _)) = self.session.lock().take() else {
            return;
        };
        let up = self.up.load(Ordering::SeqCst);
        let down = self.down.load(Ordering::SeqCst);
        if let Err(e) = self.db.end_session(&id, now_millis(), up, down) {
            tracing::error!("end_session failed: {e}");
        }
    }

    pub fn record_up(&self, bytes: u64) {
        self.up.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn record_down(&self, bytes: u64) {
        self.down.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn session_active(&self) -> bool {
        self.session.lock().is_some()
    }

    pub async fn spawn_flush_loop(self: Arc<Self>, token: CancellationToken) {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
        ticker.tick().await; // ignore immediate first tick
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.flush_once();
                }
                _ = token.cancelled() => {
                    self.flush_once();
                    break;
                }
            }
        }
    }

    fn flush_once(&self) {
        let up = self.up.load(Ordering::SeqCst);
        let down = self.down.load(Ordering::SeqCst);
        let (up_rate, down_rate, delta_up, delta_down, elapsed) = {
            let mut g = self.last.lock();
            let now = Instant::now();
            match *g {
                Some((t, pu, pd)) => {
                    let secs = now.duration_since(t).as_secs_f64().max(0.001);
                    let du = up.saturating_sub(pu);
                    let dd = down.saturating_sub(pd);
                    let rates = ((du as f64 / secs) as u64, (dd as f64 / secs) as u64);
                    *g = Some((now, up, down));
                    (rates.0, rates.1, du, dd, secs)
                }
                None => {
                    *g = Some((now, up, down));
                    (0, 0, 0, 0, 0.0)
                }
            }
        };
        let _ = elapsed;

        let (node, active) = {
            let g = self.session.lock();
            (
                g.as_ref().map(|(_, n)| n.clone()).unwrap_or_default(),
                g.is_some(),
            )
        };
        if active && (delta_up > 0 || delta_down > 0) {
            if let Err(e) = self.db.insert_traffic(now_millis(), &node, delta_up, delta_down) {
                tracing::debug!("traffic insert failed: {e}");
            }
        }
        // Only publish while a session is active: an idle/disconnected client
        // would otherwise receive a zeroed traffic event every two seconds.
        if !active {
            return;
        }
        let (up_today, down_today) = self.db.sum_since(today_midnight_millis()).unwrap_or((0, 0));
        self.bus.emit(
            ev::TRAFFIC_UPDATED,
            &crate::ipc::proto::TrafficPayload {
                up_rate,
                down_rate,
                up_total: up,
                down_total: down,
                up_today,
                down_today,
            },
        );
    }

    pub fn snapshot(&self) -> crate::ipc::proto::TrafficPayload {
        let (up_today, down_today) = self.db.sum_since(today_midnight_millis()).unwrap_or((0, 0));
        let (up_rate, down_rate) = {
            let g = self.last.lock();
            if let Some((t, pu, pd)) = *g {
                let secs = t.elapsed().as_secs_f64().max(0.001);
                (
                    ((self.up.load(Ordering::SeqCst).saturating_sub(pu)) as f64 / secs) as u64,
                    ((self.down.load(Ordering::SeqCst).saturating_sub(pd)) as f64 / secs) as u64,
                )
            } else {
                (0, 0)
            }
        };
        crate::ipc::proto::TrafficPayload {
            up_rate,
            down_rate,
            up_total: self.up.load(Ordering::SeqCst),
            down_total: self.down.load(Ordering::SeqCst),
            up_today,
            down_today,
        }
    }
}

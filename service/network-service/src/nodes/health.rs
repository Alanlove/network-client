//! Runtime stats, scoring and smart node selection (design doc §28–§31).
//!
//! Stats are runtime-only and never touch user config.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Reference throughput used to normalise the 15% speed component (100 MB/s).
const SPEED_REFERENCE_BPS: u64 = 100 * 1024 * 1024;
/// Latency at/above which the latency score is 0.
const LATENCY_FLOOR_MS: f64 = 300.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeRuntimeStats {
    pub latency_ms: i64,
    /// 0.0..=1.0
    pub packet_loss: f64,
    /// bytes / second
    pub download_speed: u64,
    pub upload_speed: u64,
    pub availability: f64,
    pub score: f64,
    pub last_test: i64,
    pub success_count: u64,
    pub failure_count: u64,
}

impl NodeRuntimeStats {
    /// Weighted score in [0, 100]:
    /// latency 30%, packet loss 25%, availability 20%, throughput 15%,
    /// failure rate 10% (design doc §29).
    pub fn compute_score(&mut self) {
        let latency_score = ((LATENCY_FLOOR_MS - self.latency_ms as f64)
            / LATENCY_FLOOR_MS)
            .clamp(0.0, 1.0)
            * 100.0;
        let loss_score = (1.0 - self.packet_loss.clamp(0.0, 1.0)) * 100.0;
        let availability_score = self.availability.clamp(0.0, 1.0) * 100.0;
        let throughput_score =
            (self.download_speed as f64 / SPEED_REFERENCE_BPS as f64).min(1.0) * 100.0;

        let total = self.success_count + self.failure_count;
        let failure_score = if total == 0 {
            100.0
        } else {
            (1.0 - self.failure_count as f64 / total as f64) * 100.0
        };

        self.score = latency_score * 0.30
            + loss_score * 0.25
            + availability_score * 0.20
            + throughput_score * 0.15
            + failure_score * 0.10;
    }

    /// Record a fresh TCP-level probe result.
    pub fn record_probe(&mut self, ok: bool, latency_ms: Option<i64>, at: i64) {
        if ok {
            self.success_count += 1;
            if let Some(ms) = latency_ms {
                // Exponential smoothing keeps single spikes from dominating.
                self.latency_ms = if self.latency_ms == 0 {
                    ms
                } else {
                    (self.latency_ms as f64 * 0.7 + ms as f64 * 0.3).round() as i64
                };
            }
        } else {
            self.failure_count += 1;
        }
        let total = self.success_count + self.failure_count;
        self.packet_loss = self.failure_count as f64 / total.max(1) as f64;
        self.availability = self.success_count as f64 / total as f64;
        self.last_test = at;
        self.compute_score();
    }
}

#[derive(Default, Clone)]
pub struct StatsTable {
    inner: Arc<RwLock<HashMap<String, NodeRuntimeStats>>>,
}

impl StatsTable {
    pub fn get(&self, id: &str) -> Option<NodeRuntimeStats> {
        self.inner.read().get(id).cloned()
    }
    pub fn all(&self) -> HashMap<String, NodeRuntimeStats> {
        self.inner.read().clone()
    }
    pub fn upsert(&self, id: &str, stats: NodeRuntimeStats) {
        self.inner.write().insert(id.to_string(), stats);
    }
    pub fn mutate(&self, id: &str, f: impl FnOnce(&mut NodeRuntimeStats)) -> NodeRuntimeStats {
        let mut g = self.inner.write();
        let entry = g.entry(id.to_string()).or_default();
        f(entry);
        entry.clone()
    }
    pub fn clear(&self, id: &str) {
        self.inner.write().remove(id);
    }
}

/// Smart selection with anti-flapping hysteresis (design doc §30/§31).
///
/// * filters disabled nodes
/// * ranks by score
/// * requires `threshold` score delta over the current node to switch
pub fn select_best(
    scores: &[(String, f64)],
    current_id: Option<&str>,
    threshold: f64,
) -> Option<String> {
    let mut ranked: Vec<&(String, f64)> = scores
        .iter()
        .filter(|(_, s)| s.is_finite())
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let best = ranked.first()?;

    if let Some(cur) = current_id {
        if best.0 == cur {
            return Some(best.0.clone());
        }
        let current_score = scores
            .iter()
            .find(|(id, _)| id == cur)
            .map(|(_, s)| *s)
            .unwrap_or(0.0);
        if best.1 - current_score < threshold {
            return Some(cur.to_string());
        }
    }
    Some(best.0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_weights_and_bounds() {
        let mut s = NodeRuntimeStats {
            latency_ms: 30,
            packet_loss: 0.0,
            availability: 1.0,
            download_speed: SPEED_REFERENCE_BPS,
            ..Default::default()
        };
        s.success_count = 10;
        s.compute_score();
        assert!(s.score > 95.0, "perfect node should score ~100, got {}", s.score);

        let mut bad = NodeRuntimeStats {
            latency_ms: 400,
            packet_loss: 1.0,
            availability: 0.0,
            ..Default::default()
        };
        bad.failure_count = 10;
        bad.compute_score();
        assert!(bad.score < 15.0, "terrible node should score ~0, got {}", bad.score);
    }

    #[test]
    fn hysteresis_prevents_flapping() {
        let scores = vec![
            ("cur".to_string(), 82.0),
            ("new".to_string(), 84.0),
        ];
        assert_eq!(
            select_best(&scores, Some("cur"), 10.0).as_deref(),
            Some("cur")
        );
        let scores2 = vec![
            ("cur".to_string(), 82.0),
            ("new".to_string(), 96.0),
        ];
        assert_eq!(
            select_best(&scores2, Some("cur"), 10.0).as_deref(),
            Some("new")
        );
    }
}

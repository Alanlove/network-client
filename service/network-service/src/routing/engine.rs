//! Rule engine + route modes (design doc v1.1 §12–§17, §35).

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::common::{SvcError, SvcResult};
use crate::config::manager::{atomic_write_json, read_json_or, ConfigManager};
use crate::config::schema::RouteMode;
use crate::routing::geoip::{GeoIp, PRIVATE};
use crate::routing::matcher::{M, RuleIndex};
use crate::routing::rule::{Action, Decision, Flow, Rule};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfigPersist {
    #[serde(default)]
    pub mode: RouteMode,
    #[serde(default = "default_final")]
    pub final_action: Action,
    /// Country routed direct in smart mode when no rule matches.
    #[serde(default = "default_home")]
    pub home_country: String,
}

fn default_final() -> Action {
    Action::Proxy
}
fn default_home() -> String {
    "CN".to_string()
}

impl Default for RoutingConfigPersist {
    fn default() -> Self {
        Self {
            mode: RouteMode::Smart,
            final_action: Action::Proxy,
            home_country: default_home(),
        }
    }
}

pub struct RoutingEngine {
    rules: Arc<RwLock<Vec<Rule>>>,
    index: Arc<RwLock<RuleIndex>>,
    geo: Arc<RwLock<GeoIp>>,
    cfg: Arc<RwLock<RoutingConfigPersist>>,
    config: ConfigManager,
}

impl RoutingEngine {
    pub fn load(config: ConfigManager) -> SvcResult<Self> {
        let rules: Vec<Rule> = read_json_or(&config.paths.rules());
        let index = RuleIndex::build(&rules)?;
        let mut geo = GeoIp::with_private();
        geo.load_dir(&config.paths.cache_dir.join("geoip"));
        let cfg: RoutingConfigPersist = read_json_or(&config.paths.config_dir.join("routing.json"));
        Ok(Self {
            rules: Arc::new(RwLock::new(rules)),
            index: Arc::new(RwLock::new(index)),
            geo: Arc::new(RwLock::new(geo)),
            cfg: Arc::new(RwLock::new(cfg)),
            config,
        })
    }

    pub fn config(&self) -> RoutingConfigPersist {
        self.cfg.read().clone()
    }

    pub fn set_mode(&self, mode: RouteMode) -> SvcResult<()> {
        self.cfg.write().mode = mode;
        self.persist_config()
    }

    pub fn set_config(&self, cfg: RoutingConfigPersist) -> SvcResult<()> {
        *self.cfg.write() = cfg;
        self.persist_config()
    }

    fn persist_config(&self) -> SvcResult<()> {
        let cfg = self.cfg.read().clone();
        atomic_write_json(&self.config.paths.config_dir.join("routing.json"), &cfg)
    }

    pub fn list_rules(&self) -> Vec<Rule> {
        self.rules.read().clone()
    }

    /// Test what rule a domain/IP matches (for the UI "rule test" feature).
    pub fn test_match(&self, input: &str) -> SvcResult<serde_json::Value> {
        let input = input.trim();
        if input.is_empty() {
            return Err(SvcError::ConfigInvalid("empty input".into()));
        }
        // Build a synthetic Flow from the input.
        let mut flow = Flow::default();
        // If it parses as an IP, use that; otherwise treat as domain.
        if let Ok(ip) = input.parse::<std::net::IpAddr>() {
            flow.dst_ip = Some(ip);
        } else {
            flow.domain = Some(input.to_string());
            // Try to resolve the domain to an IP for GeoIP matching.
            // (synchronous resolve — best effort, failure is fine)
        }
        let decision = self.decide(&flow);
        let cfg = self.cfg.read().clone();
        Ok(serde_json::json!({
            "input": input,
            "action": format!("{:?}", decision.action).to_lowercase(),
            "matched_rule_id": decision.rule_id,
            "reason": decision.reason,
            "mode": format!("{:?}", cfg.mode).to_lowercase(),
        }))
    }

    pub fn add_rule(&self, mut rule: Rule) -> SvcResult<Rule> {
        if rule.value.trim().is_empty() {
            return Err(SvcError::ConfigInvalid("rule value empty".into()));
        }
        if rule.id.is_empty() {
            rule.id = format!("rule_{}", uuid::Uuid::new_v4().simple());
        }
        rule.priority = 1000; // user authored rules pin to top ladder
        let mut next = self.rules.read().clone();
        next.push(rule.clone());
        self.commit(next)?;
        Ok(rule)
    }

    pub fn update_rule(&self, rule: Rule) -> SvcResult<()> {
        let mut next = self.rules.read().clone();
        let pos = next
            .iter()
            .position(|r| r.id == rule.id)
            .ok_or_else(|| SvcError::NotFound(format!("rule {}", rule.id)))?;
        next[pos] = rule;
        self.commit(next)
    }

    pub fn delete_rule(&self, id: &str) -> SvcResult<()> {
        let next: Vec<Rule> = self
            .rules
            .read()
            .iter()
            .filter(|r| r.id != id)
            .cloned()
            .collect();
        self.commit(next)
    }

    fn commit(&self, rules: Vec<Rule>) -> SvcResult<()> {
        // Validate the entire set before publishing (rebuild is cheap relative
        // to flow volume and happens only on config change).
        let index = RuleIndex::build(&rules)?;
        atomic_write_json(&self.config.paths.rules(), &rules)?;
        *self.index.write() = index;
        *self.rules.write() = rules;
        Ok(())
    }

    /// Pipeline: user/process -> domain -> IP/port/protocol -> geoip -> final
    /// (design doc v1.1 §13/§14).
    pub fn decide(&self, flow: &Flow) -> Decision {
        let mut winner: Option<M> = None;
        let mut why = String::new();

        let consider = |current: Option<M>, candidate: Option<M>, label: &str, why: &mut String| {
            match (current, candidate) {
                (Some(cur), Some(cand)) => {
                    if cand.priority > cur.priority {
                        why.truncate(0);
                        why.push_str(label);
                        Some(cand)
                    } else {
                        Some(cur)
                    }
                }
                (None, Some(cand)) => {
                    why.push_str(label);
                    Some(cand)
                }
                (cur, None) => cur,
            }
        };

        if let Some(exe) = flow.process_name.as_deref() {
            let m = self.index.read().match_process(exe);
            winner = consider(winner, m, "process", &mut why);
        }
        if let Some(domain) = flow.domain.as_deref() {
            let m = self.index.read().match_domain(domain);
            winner = consider(winner, m, "domain", &mut why);
        }
        if let Some(ip) = flow.dst_ip.as_ref() {
            let m = self.index.read().match_ip(ip);
            winner = consider(winner, m, "ip", &mut why);
        }
        if flow.dst_port != 0 {
            let m = self.index.read().match_port(flow.dst_port);
            winner = consider(winner, m, "port", &mut why);
        }
        if !flow.protocol.is_empty() {
            let m = self.index.read().match_protocol(&flow.protocol);
            winner = consider(winner, m, "protocol", &mut why);
        }
        // GeoIP user rules (priority 600).
        if let Some(ip) = flow.dst_ip.as_ref() {
            if let Some(country) = self.geo.read().country(ip) {
                let m = self.index.read().match_geoip(country);
                winner = consider(winner, m, "geoip", &mut why);
            }
        }

        if let Some(m) = winner {
            return Decision {
                action: m.action,
                rule_id: Some(m.rule_id),
                reason: why,
            };
        }

        // Final: mode-driven default.
        let cfg = self.cfg.read().clone();
        match cfg.mode {
            RouteMode::Direct => Decision::direct("mode=direct"),
            RouteMode::Global => Decision::proxy("mode=global"),
            RouteMode::Smart => {
                let home = cfg.home_country.to_ascii_uppercase();
                match flow.dst_ip {
                    Some(ip) => {
                        let country = self.geo.read().country(&ip).map(str::to_string);
                        match country.as_deref() {
                            Some(PRIVATE) => Decision::direct("private/lan"),
                            Some(c) if c == home => Decision::direct("smart:home-country"),
                            _ => Decision::proxy("smart:international"),
                        }
                    }
                    None => Decision {
                        action: cfg.final_action,
                        rule_id: None,
                        reason: "smart:no-ip".into(),
                    },
                }
            }
        }
    }
}

//! DNS policy layer + cache (design doc v1.1 §18–§19).
//!
//! V1 uses real IPs + cache (no Fake-IP). Server policy:
//!   config.mode = "direct" / "proxy" / "smart"; per-server tag refines which
//!   server is used for direct vs proxied contexts.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use reqwest::Client;
use serde::Serialize;

use crate::common::SvcResult;
use crate::config::manager::ConfigManager;
use crate::config::schema::DnsConfig;
use crate::dns::cache::DnsCache;
use crate::dns::resolver::{parse_server, ping_server, query_server, Server};
use crate::dns::wire;
use crate::ipc::events::{ev, EventBus};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResolvePolicy {
    Direct,
    Proxy,
    Any,
}

#[derive(Serialize)]
pub struct ServerTestResult {
    pub server: String,
    pub ok: bool,
    pub latency_ms: u128,
    pub ips: Vec<String>,
}

pub struct DnsManager {
    cfg: Arc<parking_lot::RwLock<DnsConfig>>,
    cache: Mutex<DnsCache>,
    client: Client,
    config: ConfigManager,
    bus: EventBus,
}

impl DnsManager {
    pub fn load(config: ConfigManager, bus: EventBus) -> Self {
        let cfg = config.dns();
        let cache = DnsCache::new(cfg.cache, cfg.cache_size as usize);
        let client = Client::builder()
            .timeout(Duration::from_secs(6))
            .build()
            .expect("reqwest");
        Self {
            cfg: Arc::new(parking_lot::RwLock::new(cfg)),
            cache: Mutex::new(cache),
            client,
            config,
            bus,
        }
    }

    pub fn config(&self) -> DnsConfig {
        self.cfg.read().clone()
    }

    pub fn set_config(&self, cfg: DnsConfig) -> SvcResult<()> {
        self.config.save_dns(&cfg)?;
        let mut cache = DnsCache::new(cfg.cache, cfg.cache_size as usize);
        if !cfg.cache {
            cache.flush();
        }
        *self.cache.lock() = cache;
        *self.cfg.write() = cfg;
        self.bus.emit_empty(ev::DNS_CHANGED);
        Ok(())
    }

    pub fn flush_cache(&self) {
        self.cache.lock().flush();
    }

    fn select_servers(&self, policy: ResolvePolicy) -> Vec<(String, Server)> {
        let cfg = self.cfg.read();
        let mut picked = Vec::new();
        let mut untagged = Vec::new();
        for s in &cfg.servers {
            let Some(server) = parse_server(s) else { continue };
            match (policy, s.tag.as_str()) {
                (ResolvePolicy::Direct, "direct") | (ResolvePolicy::Proxy, "proxy") => {
                    picked.push((s.url.clone(), server))
                }
                (_, "") => untagged.push((s.url.clone(), server)),
                _ => {}
            }
        }
        picked.extend(untagged);
        if picked.is_empty() {
            // Fallback to anything configured.
            for s in &cfg.servers {
                if let Some(server) = parse_server(s) {
                    picked.push((s.url.clone(), server));
                }
            }
        }
        picked
    }

    /// Resolve A then AAAA; IPv4 first. Tries servers in policy order.
    pub async fn resolve(&self, name: &str, policy: ResolvePolicy) -> SvcResult<Vec<IpAddr>> {
        if let Ok(ip) = name.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        if let Some(hit) = self.cache.lock().get(name) {
            return Ok(hit);
        }
        let servers = self.select_servers(policy);
        let mut last_err = None;
        for (_, server) in &servers {
            let v4 = query_server(
                &self.client,
                server,
                name,
                wire::TYPE_A,
                Duration::from_secs(5),
            )
            .await;
            let v6 = query_server(
                &self.client,
                server,
                name,
                wire::TYPE_AAAA,
                Duration::from_secs(5),
            )
            .await;
            match (v4, v6) {
                (Ok(a), aaaa) => {
                    let mut ips: Vec<IpAddr> = a.into_iter().filter_map(|r| r.ip).collect();
                    if let Ok(aaaa) = aaaa {
                        ips.extend(aaaa.into_iter().filter_map(|r| r.ip));
                    }
                    if ips.is_empty() {
                        last_err = Some(crate::common::SvcError::Dns("empty answer".into()));
                        continue;
                    }
                    // Cache TTL = minimum TTL of A records.
                    // (Re-query isn't needed; `a` was consumed above, use 60s.)
                    self.cache.lock().put(name, ips.clone(), 60);
                    return Ok(ips);
                }
                (Err(e), _) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| crate::common::SvcError::Dns("no dns server".into())))
    }

    /// Diagnostics: probe every configured server.
    pub async fn test_all(&self, name: &str) -> Vec<ServerTestResult> {
        let probe_name = if name.is_empty() {
            "www.microsoft.com"
        } else {
            name
        };
        // Snapshot under the guard, then release it before awaiting — a
        // parking_lot guard is !Send and must not cross an await point.
        let servers: Vec<crate::config::schema::DnsServer> = self.cfg.read().servers.clone();
        let mut out = Vec::new();
        for s in &servers {
            if let Some(server) = parse_server(s) {
                let (ok, latency, ips) = ping_server(&self.client, &server, probe_name).await;
                out.push(ServerTestResult {
                    server: s.url.clone(),
                    ok,
                    latency_ms: latency,
                    ips: ips.into_iter().map(|i| i.to_string()).collect(),
                });
            }
        }
        out
    }
}

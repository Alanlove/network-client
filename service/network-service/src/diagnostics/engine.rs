//! Self-diagnostic orchestration (design doc v1.0 §11, v1.1 §38).
//!
//! Steps: network reachability -> local gateway -> DNS -> proxy path ->
//! packet loss -> download speed. Progress is streamed over IPC; the full
//! report is persisted in SQLite and returned as JSON in the completed event.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::common::now_millis;
use crate::diagnostics::ping;
use crate::dns::manager::{DnsManager, ResolvePolicy};
use crate::ipc::events::{ev, EventBus};
use crate::ipc::proto::DiagnosticProgressPayload;
use crate::statistics::database::Database;

#[derive(Debug, Serialize, Clone)]
pub struct DiagnosticItem {
    pub name: String,
    pub status: String, // ok | warn | fail | skipped
    pub latency_ms: u64,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticReport {
    pub started_at: i64,
    pub finished_at: i64,
    pub items: Vec<DiagnosticItem>,
}

pub struct DiagnosticEngine {
    bus: EventBus,
    dns: Arc<DnsManager>,
    db: Option<Arc<Database>>,
}

impl DiagnosticEngine {
    pub fn new(bus: EventBus, dns: Arc<DnsManager>, db: Option<Arc<Database>>) -> Self {
        Self { bus, dns, db }
    }

    async fn emit_running(&self, index: u32, total: u32, step: &str) {
        self.bus.emit(
            ev::DIAGNOSTICS_PROGRESS,
            &DiagnosticProgressPayload {
                step: step.to_string(),
                index,
                total,
                status: "running".into(),
                detail: String::new(),
            },
        );
    }

    async fn finish_step(
        &self,
        index: u32,
        total: u32,
        item: DiagnosticItem,
    ) -> DiagnosticItem {
        self.bus.emit(
            ev::DIAGNOSTICS_PROGRESS,
            &DiagnosticProgressPayload {
                step: item.name.clone(),
                index,
                total,
                status: item.status.clone(),
                detail: item.detail.clone(),
            },
        );
        if let Some(db) = &self.db {
            let _ = db.insert_diagnostic(
                now_millis(),
                &item.name,
                &item.status,
                item.latency_ms as i64,
                &item.detail,
            );
        }
        item
    }

    pub async fn run(&self, proxy_port: Option<u16>, cancel: CancellationToken) -> DiagnosticReport {
        const TOTAL: u32 = 6;
        let started = now_millis();
        let mut items = Vec::new();

        // 1. Network reachability (public DNS anycast).
        self.emit_running(1, TOTAL, "network").await;
        let ping = tokio::task::spawn_blocking(|| {
            ping::ping_once(Ipv4Addr::new(223, 5, 5, 5), 3000)
        })
        .await
        .ok()
        .flatten();
        items.push(
            self.finish_step(
                1,
                TOTAL,
                match ping {
                    Some(ms) => DiagnosticItem {
                        name: "network".into(),
                        status: "ok".into(),
                        latency_ms: ms as u64,
                        detail: "223.5.5.5 reachable".into(),
                    },
                    None => DiagnosticItem {
                        name: "network".into(),
                        status: "fail".into(),
                        latency_ms: 0,
                        detail: "223.5.5.5 ICMP unreachable".into(),
                    },
                },
            )
            .await,
        );
        if cancel.is_cancelled() {
            return self.report(started, items);
        }

        // 2. Default gateway.
        self.emit_running(2, TOTAL, "gateway").await;
        let gw = tokio::task::spawn_blocking(ping::default_gateway)
            .await
            .ok()
            .flatten();
        let gw_item = match gw {
            Some(addr) => {
                let r = tokio::task::spawn_blocking(move || ping::ping_once(addr, 2000))
                    .await
                    .ok()
                    .flatten();
                match r {
                    Some(ms) => DiagnosticItem {
                        name: "gateway".into(),
                        status: "ok".into(),
                        latency_ms: ms as u64,
                        detail: format!("gateway {addr} reachable"),
                    },
                    None => DiagnosticItem {
                        name: "gateway".into(),
                        status: "warn".into(),
                        latency_ms: 0,
                        detail: format!("gateway {addr} does not answer ICMP"),
                    },
                }
            }
            None => DiagnosticItem {
                name: "gateway".into(),
                status: "warn".into(),
                latency_ms: 0,
                detail: "no IPv4 default route".into(),
            },
        };
        items.push(self.finish_step(2, TOTAL, gw_item).await);
        if cancel.is_cancelled() {
            return self.report(started, items);
        }

        // 3. DNS resolution.
        self.emit_running(3, TOTAL, "dns").await;
        let t = Instant::now();
        let dns_result = self.dns.resolve("www.microsoft.com", ResolvePolicy::Any).await;
        items.push(
            self.finish_step(
                3,
                TOTAL,
                match dns_result {
                    Ok(ips) => DiagnosticItem {
                        name: "dns".into(),
                        status: "ok".into(),
                        latency_ms: t.elapsed().as_millis() as u64,
                        detail: format!("resolved {} addresses", ips.len()),
                    },
                    Err(e) => DiagnosticItem {
                        name: "dns".into(),
                        status: "fail".into(),
                        latency_ms: t.elapsed().as_millis() as u64,
                        detail: e.to_string(),
                    },
                },
            )
            .await,
        );
        if cancel.is_cancelled() {
            return self.report(started, items);
        }

        // 4. Proxy data path.
        self.emit_running(4, TOTAL, "proxy").await;
        items.push(match proxy_port {
            Some(port) => {
                let item = proxy_probe(port).await;
                self.finish_step(4, TOTAL, item).await
            }
            None => {
                self.finish_step(
                    4,
                    TOTAL,
                    DiagnosticItem {
                        name: "proxy".into(),
                        status: "skipped".into(),
                        latency_ms: 0,
                        detail: "proxy core not running".into(),
                    },
                )
                .await
            }
        });
        if cancel.is_cancelled() {
            return self.report(started, items);
        }

        // 5. Packet loss (5 ICMP probes).
        self.emit_running(5, TOTAL, "packet_loss").await;
        let loss = tokio::task::spawn_blocking(|| {
            ping::ping_loss(Ipv4Addr::new(223, 5, 5, 5), 5, 2000)
        })
        .await
        .unwrap_or((0.0, None));
        let status = if loss.0 >= 0.999 {
            "ok"
        } else if loss.0 > 0.0 {
            "warn"
        } else {
            "fail"
        };
        items.push(
            self.finish_step(
                5,
                TOTAL,
                DiagnosticItem {
                    name: "packet_loss".into(),
                    status: status.into(),
                    latency_ms: loss.1.unwrap_or(0) as u64,
                    detail: format!("{:.0}% success", loss.0 * 100.0),
                },
            )
            .await,
        );
        if cancel.is_cancelled() {
            return self.report(started, items);
        }

        // 6. Download speed through the proxy (5 MB cap).
        self.emit_running(6, TOTAL, "speed").await;
        items.push(match proxy_port {
            Some(port) => {
                let item = speed_probe(port).await;
                self.finish_step(6, TOTAL, item).await
            }
            None => {
                self.finish_step(
                    6,
                    TOTAL,
                    DiagnosticItem {
                        name: "speed".into(),
                        status: "skipped".into(),
                        latency_ms: 0,
                        detail: "proxy core not running".into(),
                    },
                )
                .await
            }
        });

        let report = self.report(started, items);
        if let Ok(json) = serde_json::to_string(&report) {
            self.bus.emit(ev::DIAGNOSTICS_COMPLETED, &crate::ipc::proto::TextPayload { text: json });
        }
        report
    }

    fn report(&self, started_at: i64, items: Vec<DiagnosticItem>) -> DiagnosticReport {
        DiagnosticReport {
            started_at,
            finished_at: now_millis(),
            items,
        }
    }
}

async fn proxy_probe(port: u16) -> DiagnosticItem {
    let started = Instant::now();
    let client = match reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).expect("proxy url"))
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return DiagnosticItem {
                name: "proxy".into(),
                status: "fail".into(),
                latency_ms: 0,
                detail: format!("client build failed: {e}"),
            }
        }
    };
    match client
        .get("https://www.gstatic.com/generate_204")
        .send()
        .await
    {
        Ok(resp) if resp.status().as_u16() == 204 => DiagnosticItem {
            name: "proxy".into(),
            status: "ok".into(),
            latency_ms: started.elapsed().as_millis() as u64,
            detail: "HTTP 204 via proxy".into(),
        },
        Ok(resp) => DiagnosticItem {
            name: "proxy".into(),
            status: "warn".into(),
            latency_ms: started.elapsed().as_millis() as u64,
            detail: format!("unexpected HTTP {}", resp.status()),
        },
        Err(e) => DiagnosticItem {
            name: "proxy".into(),
            status: "fail".into(),
            latency_ms: started.elapsed().as_millis() as u64,
            detail: e.to_string(),
        },
    }
}

pub(crate) async fn speed_probe(port: u16) -> DiagnosticItem {
    let started = Instant::now();
    let client = match reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).expect("proxy url"))
        .timeout(std::time::Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return DiagnosticItem {
                name: "speed".into(),
                status: "fail".into(),
                latency_ms: 0,
                detail: e.to_string(),
            }
        }
    };
    let outcome = async {
        let resp = client
            .get("https://speed.cloudflare.com/__down?bytes=5000000")
            .send()
            .await?;
        Ok::<usize, reqwest::Error>(resp.bytes().await?.len())
    }
    .await;
    match outcome {
        Ok(bytes) => {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let mbps = (bytes as f64 * 8.0 / secs / 1_000_000.0).round() as u64;
            DiagnosticItem {
                name: "speed".into(),
                status: "ok".into(),
                latency_ms: started.elapsed().as_millis() as u64,
                detail: format!("{mbps} Mbps ({} bytes)", bytes),
            }
        }
        Err(e) => DiagnosticItem {
            name: "speed".into(),
            status: "warn".into(),
            latency_ms: started.elapsed().as_millis() as u64,
            detail: format!("speed probe failed: {e}"),
        },
    }
}

//! Stateless query transport for one configured DNS server (UDP / DoH).

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use reqwest::Client;
use tokio::net::UdpSocket;

use crate::common::{SvcError, SvcResult};
use crate::config::schema::DnsServer;
use crate::dns::doh;
use crate::dns::wire::{self, DnsRecord};

#[derive(Debug, Clone)]
pub enum Server {
    Udp(SocketAddr),
    Doh(String),
}

pub fn parse_server(s: &DnsServer) -> Option<Server> {
    match s.kind.as_str() {
        "udp" => {
            let addr: SocketAddr = if s.url.contains(':') {
                s.url.parse().ok()?
            } else {
                format!("{}:53", s.url).parse().ok()?
            };
            Some(Server::Udp(addr))
        }
        "doh" => Some(Server::Doh(s.url.clone())),
        _ => None,
    }
}

fn query_id() -> u16 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos as u16
}

pub async fn query_server(
    client: &Client,
    server: &Server,
    name: &str,
    qtype: u16,
    timeout: Duration,
) -> SvcResult<Vec<DnsRecord>> {
    let query = wire::build_query(name, qtype, query_id());
    let resp = tokio::time::timeout(timeout, async {
        match server {
            Server::Udp(addr) => {
                let bind = if addr.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                };
                let sock = UdpSocket::bind(bind).await?;
                sock.send_to(&query, addr).await?;
                let mut buf = vec![0u8; 1500];
                let (n, _) = sock.recv_from(&mut buf).await?;
                buf.truncate(n);
                Ok::<Vec<u8>, SvcError>(buf)
            }
            Server::Doh(url) => doh::query(client, url, &query).await,
        }
    })
    .await
    .map_err(|_| SvcError::Timeout)??;

    wire::parse_response(&resp)
        .ok_or_else(|| SvcError::Dns("malformed DNS response".into()))
}

/// Convenience: fire a single A probe and measure latency.
pub async fn ping_server(client: &Client, server: &Server, name: &str) -> (bool, u128, Vec<IpAddr>) {
    let started = Instant::now();
    match query_server(client, server, name, wire::TYPE_A, Duration::from_secs(5)).await {
        Ok(records) => {
            let ips: Vec<IpAddr> = records.into_iter().filter_map(|r| r.ip).collect();
            (!ips.is_empty(), started.elapsed().as_millis(), ips)
        }
        Err(_) => (false, started.elapsed().as_millis(), vec![]),
    }
}

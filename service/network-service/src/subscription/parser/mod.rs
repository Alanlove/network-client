//! URI -> Node parsers for the protocols accepted at subscription import.
//! Adding a protocol = add a submodule + one dispatch arm here; nothing else
//! in the control plane changes (design doc v1.1 §22).

pub mod ss;
pub mod vless;
pub mod vmess;
pub mod socks;
pub mod trojan;

use percent_encoding::percent_decode_str;
use serde_json::json;

use crate::nodes::model::{Endpoint, Node, TlsConfig};

/// FNV-1a 64-bit stable identity: survives re-import so runtime stats stay
/// attached to the same node across subscription refreshes.
pub fn stable_id(protocol: &str, host: &str, port: u16, identity: &str) -> String {
    let key = format!("{protocol}|{host}|{port}|{identity}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("node_{hash:016x}")
}

pub fn decode_name(fragment: Option<&str>, fallback: &str) -> String {
    fragment
        .map(|f| percent_decode_str(f).decode_utf8_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// `host:port` / `[::1]:port`, with a default port when absent.
pub fn split_hostport(s: &str, default_port: u16) -> Option<(String, u16)> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('[') {
        let (v6, tail) = rest.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default_port);
        return Some((v6.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((s.to_string(), default_port)),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_node(
    protocol: &str,
    name: String,
    host: String,
    port: u16,
    authentication: serde_json::Value,
    transport: serde_json::Value,
    tls: TlsConfig,
    identity_for_id: &str,
) -> Node {
    Node {
        id: stable_id(protocol, &host, port, identity_for_id),
        name,
        protocol: protocol.to_string(),
        endpoint: Endpoint { host, port },
        authentication,
        transport,
        tls,
        metadata: json!({}),
        subscription_id: String::new(),
        enabled: true,
        created_at: 0,
    }
}

/// Dispatch one subscription line.
pub fn parse_line(line: &str) -> Option<Node> {
    let line = line.trim();
    let (scheme, _) = line.split_once("://")?;
    match scheme.to_ascii_lowercase().as_str() {
        "ss" | "shadowsocks" => ss::parse(line),
        "vmess" => vmess::parse(line),
        "vless" => vless::parse(line),
        "trojan" => trojan::parse(line),
        "socks" | "socks5" => socks::parse(line),
        _ => None,
    }
}

/// Parse every line; returns nodes and the count of unrecognised lines.
pub fn parse_all(lines: &[String]) -> (Vec<Node>, usize) {
    let mut nodes = Vec::new();
    let mut skipped = 0;
    for line in lines {
        match parse_line(line) {
            // Providers embed announcement lines as real-looking URIs pointing
            // at unspecified addresses ("请下载新客户端" etc.) — never nodes.
            Some(n) if is_placeholder_host(&n.endpoint.host) => skipped += 1,
            Some(n) => nodes.push(n),
            None => skipped += 1,
        }
    }
    (nodes, skipped)
}

fn is_placeholder_host(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::" | "[::]")
}

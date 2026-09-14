//! Node -> Xray-core outbound/stream JSON translation.
//!
//! Xray is the reference implementation for transports sing-box does not
//! carry (notably XHTTP / `type=xhttp`). The proxy manager selects this
//! builder automatically when a node requires such a transport.
//!
//! Two local inbounds are emitted: an HTTP proxy on `mixed_port` (the port
//! WinINET is pointed at) and SOCKS on `socks_port`.

use serde_json::{json, Map, Value};

use crate::common::{SvcError, SvcResult};
use crate::config::schema::DnsConfig;
use crate::nodes::model::{Node, SHADOWSOCKS, TROJAN, VLESS};

fn auth_str(node: &Node, key: &str) -> String {
    node.authentication
        .get(key)
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string().trim_matches('"').to_string(),
        })
        .unwrap_or_default()
}

fn transport_kind(node: &Node) -> Option<String> {
    node.transport
        .get("type")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn stream_settings(node: &Node) -> SvcResult<Value> {
    let mut s = Map::new();
    let kind = transport_kind(node);
    let network = match kind.as_deref() {
        None | Some("tcp") => "tcp",
        Some("ws") => "ws",
        Some("grpc") => "grpc",
        Some("xhttp") => "xhttp",
        Some(other) => {
            return Err(SvcError::ConfigInvalid(format!(
                "xray builder: unsupported transport {other}"
            )))
        }
    };
    s.insert("network".into(), json!(network));

    // ---- TLS / Reality ---------------------------------------------------
    let reality = node.authentication.get("reality");
    if reality.is_some() {
        let r = reality.unwrap();
        s.insert("security".into(), json!("reality"));
        s.insert(
            "realitySettings".into(),
            json!({
                "serverName": if node.tls.server_name.is_empty() {
                    node.endpoint.host.clone()
                } else {
                    node.tls.server_name.clone()
                },
                "fingerprint": pick_str(r, "fingerprint").unwrap_or_else(|| "chrome".into()),
                "publicKey": pick_str(r, "public_key").unwrap_or_default(),
                "shortId": pick_str(r, "short_id").unwrap_or_default(),
                "spiderX": ""
            }),
        );
    } else if node.tls.enabled {
        s.insert("security".into(), json!("tls"));
        let mut tls = Map::new();
        tls.insert(
            "serverName".into(),
            json!(if node.tls.server_name.is_empty() {
                node.endpoint.host.clone()
            } else {
                node.tls.server_name.clone()
            }),
        );
        tls.insert("allowInsecure".into(), json!(node.tls.insecure));
        tls.insert("fingerprint".into(), json!("chrome"));
        if !node.tls.alpn.is_empty() {
            tls.insert("alpn".into(), json!(node.tls.alpn));
        }
        s.insert("tlsSettings".into(), Value::Object(tls));
    } else {
        s.insert("security".into(), json!("none"));
    }

    // ---- transport-specific settings ------------------------------------
    match network {
        "ws" => {
            let path = node.transport["path"].as_str().unwrap_or("/");
            let host = node.transport["headers"]["Host"].as_str().unwrap_or("");
            s.insert(
                "wsSettings".into(),
                json!({
                    "path": path,
                    "headers": if host.is_empty() { json!({}) } else { json!({ "Host": host }) }
                }),
            );
        }
        "grpc" => {
            s.insert(
                "grpcSettings".into(),
                json!({
                    "serviceName": node.transport["service_name"].as_str().unwrap_or(""),
                    "multiMode": false
                }),
            );
        }
        "xhttp" => {
            let mut xh = Map::new();
            if let Some(p) = node.transport.get("path").and_then(Value::as_str) {
                if !p.is_empty() {
                    xh.insert("path".into(), json!(p));
                }
            }
            if let Some(h) = node.transport.get("host").and_then(Value::as_str) {
                if !h.is_empty() {
                    xh.insert("host".into(), json!(h));
                }
            }
            if let Some(m) = node.transport.get("mode").and_then(Value::as_str) {
                if !m.is_empty() {
                    xh.insert("mode".into(), json!(m));
                }
            }
            s.insert("xhttpSettings".into(), Value::Object(xh));
        }
        _ => {}
    }
    Ok(Value::Object(s))
}

fn pick_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn node_outbound(node: &Node) -> SvcResult<Value> {
    let mut out = Map::new();
    out.insert("tag".into(), json!("node"));
    let stream = stream_settings(node)?;

    match node.protocol.as_str() {
        VLESS => {
            out.insert("protocol".into(), json!("vless"));
            let mut user = Map::new();
            user.insert("id".into(), json!(auth_str(node, "uuid")));
            user.insert("encryption".into(), json!("none"));
            let flow = auth_str(node, "flow");
            if !flow.is_empty() {
                user.insert("flow".into(), json!(flow));
            }
            out.insert(
                "settings".into(),
                json!({
                    "vnext": [{
                        "address": node.endpoint.host,
                        "port": node.endpoint.port,
                        "users": [Value::Object(user)]
                    }]
                }),
            );
        }
        TROJAN => {
            out.insert("protocol".into(), json!("trojan"));
            out.insert(
                "settings".into(),
                json!({
                    "servers": [{
                        "address": node.endpoint.host,
                        "port": node.endpoint.port,
                        "password": auth_str(node, "password")
                    }]
                }),
            );
        }
        SHADOWSOCKS => {
            out.insert("protocol".into(), json!("shadowsocks"));
            out.insert(
                "settings".into(),
                json!({
                    "servers": [{
                        "address": node.endpoint.host,
                        "port": node.endpoint.port,
                        "method": auth_str(node, "method"),
                        "password": auth_str(node, "password")
                    }]
                }),
            );
        }
        other => {
            return Err(SvcError::ConfigInvalid(format!(
                "xray builder: no mapping for protocol {other}"
            )))
        }
    }
    out.insert("streamSettings".into(), stream);
    Ok(Value::Object(out))
}

fn dns_block(dns: &DnsConfig) -> Value {
    // Keep the Xray DNS object minimal: static servers only, so no geo asset
    // dependency. Proxy-tagged DoH servers aren't wired (Xray would need a
    // detour chain); the node's own server name still resolves via system DNS.
    let mut servers = Vec::new();
    for s in &dns.servers {
        match s.kind.as_str() {
            "udp" => servers.push(json!({ "address": s.url.trim_end_matches(":53") })),
            "doh" => servers.push(json!({ "address": s.url })),
            _ => {}
        }
    }
    if servers.is_empty() {
        servers.push(json!({ "address": "223.5.5.5" }));
    }
    json!({ "servers": servers, "queryStrategy": "UseIPv4" })
}

/// Local-only dokodemo inbound that exposes Xray's gRPC StatsService; the
/// stats poller queries cumulative byte counters on this port.
pub const STATS_PORT: u16 = 15490;

pub fn build_xray_config(node: &Node, mixed_port: u16, socks_port: u16, dns: &DnsConfig) -> SvcResult<Value> {
    let sniff = json!({ "enabled": true, "destOverride": ["http", "tls", "quic"] });
    let mut node_ob = node_outbound(node)?;
    // Force stats collection on the node outbound itself.
    if let Some(obj) = node_ob.as_object_mut() {
        obj.insert("stats".into(), json!({"enabled": true}));
    }
    Ok(json!({
        "log": { "loglevel": "warning" },
        "stats": {},
        "api": { "tag": "api", "services": ["StatsService"] },
        "policy": {
            "levels": { "0": {
                "statsInboundUplink": true,
                "statsInboundDownlink": true,
                "statsOutboundUplink": true,
                "statsOutboundDownlink": true
            } }
        },
        "dns": dns_block(dns),
        "inbounds": [
            {
                "tag": "http-in",
                "listen": "127.0.0.1",
                "port": mixed_port,
                "protocol": "http",
                "settings": {},
                "sniffing": sniff
            },
            {
                "tag": "socks-in",
                "listen": "127.0.0.1",
                "port": socks_port,
                "protocol": "socks",
                "settings": { "udp": true, "auth": "noauth" },
                "sniffing": sniff
            },
            {
                "tag": "api-in",
                "listen": "127.0.0.1",
                "port": STATS_PORT,
                "protocol": "dokodemo-door",
                "settings": { "address": "127.0.0.1" }
            }
        ],
        "outbounds": [
            node_ob,
            { "protocol": "freedom", "tag": "direct" },
            { "protocol": "blackhole", "tag": "block" }
        ],
        "routing": {
            "domainStrategy": "AsIs",
            "rules": [
                {
                    "type": "field",
                    "inboundTag": ["api-in"],
                    "outboundTag": "api"
                },
                {
                    "type": "field",
                    "ip": [
                        "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16",
                        "127.0.0.0/8", "169.254.0.0/16",
                        "fc00::/7", "fe80::/10"
                    ],
                    "outboundTag": "direct"
                }
            ]
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::model::{Endpoint, TlsConfig};

    #[test]
    fn vless_xhttp_tls_stream() {
        let n = Node {
            id: "n1".into(),
            name: "HK".into(),
            protocol: VLESS.into(),
            endpoint: Endpoint { host: "hk1.example.com".into(), port: 443 },
            authentication: json!({ "uuid": "u1", "flow": "" }),
            transport: json!({ "type": "xhttp", "mode": "stream-up", "path": "/path" }),
            tls: TlsConfig {
                enabled: true,
                server_name: "update.microsoft.com".into(),
                insecure: true,
                alpn: vec![],
            },
            metadata: json!({}),
            subscription_id: String::new(),
            enabled: true,
            created_at: 0,
        };
        let cfg = build_xray_config(&n, 2080, 2081, &DnsConfig::default()).unwrap();
        let ob = &cfg["outbounds"][0];
        assert_eq!(ob["protocol"], "vless");
        assert_eq!(ob["streamSettings"]["network"], "xhttp");
        assert_eq!(ob["streamSettings"]["security"], "tls");
        assert_eq!(ob["streamSettings"]["xhttpSettings"]["mode"], "stream-up");
        assert_eq!(ob["streamSettings"]["tlsSettings"]["serverName"], "update.microsoft.com");
        assert_eq!(cfg["inbounds"][0]["port"], 2080);
    }
}

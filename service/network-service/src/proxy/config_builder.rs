//! Node -> Core-specific config translation (design doc v1.1 §20–§22).
//!
//! The control plane only knows the unified Node. This module maps it to the
//! sing-box 1.x outbound schema. Supporting another core = another builder;
//! no upper layer changes.

use serde_json::{json, Map, Value};

use crate::common::{SvcError, SvcResult};
use crate::config::schema::DnsConfig;
use crate::nodes::model::{Node, SHADOWSOCKS, SOCKS, TROJAN, VLESS, VMESS};

fn auth_str(node: &Node, key: &str) -> String {
    node.authentication
        .get(key)
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string().trim_matches('"').to_string(),
        })
        .unwrap_or_default()
}

fn tls_block(node: &Node) -> Value {
    if !node.tls.enabled {
        return json!({ "enabled": false });
    }
    let mut tls = Map::new();
    tls.insert("enabled".into(), json!(true));
    tls.insert(
        "server_name".into(),
        json!(if node.tls.server_name.is_empty() {
            node.endpoint.host.clone()
        } else {
            node.tls.server_name.clone()
        }),
    );
    tls.insert("insecure".into(), json!(node.tls.insecure));
    if !node.tls.alpn.is_empty() {
        tls.insert("alpn".into(), json!(node.tls.alpn));
    }
    Value::Object(tls)
}

fn transport_block(node: &Node) -> Option<Value> {
    let t = node.transport.as_object()?;
    if t.is_empty() {
        return None;
    }
    Some(node.transport.clone())
}

fn node_outbound(node: &Node) -> SvcResult<Value> {
    let base = json!({
        "tag": "node",
        "server": node.endpoint.host,
        "server_port": node.endpoint.port,
    });
    let mut out = base.as_object().unwrap().clone();

    match node.protocol.as_str() {
        SHADOWSOCKS => {
            out.insert("type".into(), json!("shadowsocks"));
            out.insert("method".into(), json!(auth_str(node, "method")));
            out.insert("password".into(), json!(auth_str(node, "password")));
            if let Some(tr) = transport_block(node) {
                out.insert("transport".into(), tr);
                out.insert("tls".into(), tls_block(node));
            }
        }
        VMESS => {
            out.insert("type".into(), json!("vmess"));
            out.insert("uuid".into(), json!(auth_str(node, "uuid")));
            let security = auth_str(node, "security");
            let security = if security.is_empty() {
                "auto".to_string()
            } else {
                security
            };
            out.insert("security".into(), json!(security));
            let aid: i64 = auth_str(node, "alter_id").parse().unwrap_or(0);
            out.insert("alter_id".into(), json!(aid));
            if let Some(tr) = transport_block(node) {
                out.insert("transport".into(), tr);
            }
            out.insert("tls".into(), tls_block(node));
        }
        VLESS => {
            out.insert("type".into(), json!("vless"));
            out.insert("uuid".into(), json!(auth_str(node, "uuid")));
            let flow = auth_str(node, "flow");
            if !flow.is_empty() {
                out.insert("flow".into(), json!(flow));
            }
            if let Some(tr) = transport_block(node) {
                out.insert("transport".into(), tr);
            }
            out.insert("tls".into(), tls_block(node));
        }
        TROJAN => {
            out.insert("type".into(), json!("trojan"));
            out.insert("password".into(), json!(auth_str(node, "password")));
            if let Some(tr) = transport_block(node) {
                out.insert("transport".into(), tr);
            }
            out.insert("tls".into(), tls_block(node));
        }
        SOCKS => {
            out.insert("type".into(), json!("socks"));
            out.insert("version".into(), json!("5"));
            let user = auth_str(node, "username");
            if !user.is_empty() {
                out.insert("username".into(), json!(user));
                out.insert("password".into(), json!(auth_str(node, "password")));
            }
            out.insert("tls".into(), tls_block(node));
        }
        other => {
            return Err(SvcError::ConfigInvalid(format!(
                "no core builder for protocol {other}"
            )))
        }
    }
    Ok(Value::Object(out))
}

fn dns_block(dns: &DnsConfig) -> Value {
    let mut servers = Vec::new();
    for (i, s) in dns.servers.iter().enumerate() {
        let tag = if s.tag == "proxy" {
            "dns-remote"
        } else {
            "dns-direct"
        };
        let address = match s.kind.as_str() {
            "udp" => s.url.trim_end_matches(":53").to_string(),
            "doh" => s.url.clone(),
            _ => continue,
        };
        let tag = if i == 0 { tag.to_string() } else { format!("{tag}-{i}") };
        let mut entry = json!({ "tag": tag, "address": address });
        if s.tag == "proxy" {
            entry.as_object_mut().unwrap().insert("detour".into(), json!("node"));
        }
        servers.push(entry);
    }
    if servers.is_empty() {
        servers.push(json!({ "tag": "dns-direct", "address": "223.5.5.5" }));
    }
    let final_tag = servers
        .iter()
        .find(|s| s["tag"].as_str() == Some("dns-direct"))
        .or_else(|| servers.first())
        .and_then(|s| s["tag"].as_str())
        .unwrap_or("dns-direct")
        .to_string();
    json!({
        "servers": servers,
        "final": final_tag,
        "strategy": "prefer_ipv4",
        "independent_cache": true
    })
}

pub fn build_singbox_config(node: &Node, mixed_port: u16, dns: &DnsConfig) -> SvcResult<Value> {
    let outbound = node_outbound(node)?;
    Ok(json!({
        "log": { "level": "warn", "timestamp": true },
        "dns": dns_block(dns),
        "inbounds": [{
            "type": "mixed",
            "tag": "mixed-in",
            "listen": "127.0.0.1",
            "listen_port": mixed_port,
            "sniff": true,
            "sniff_override_destination": true
        }],
        "outbounds": [
            outbound,
            { "type": "direct", "tag": "direct" },
            { "type": "block", "tag": "block" }
        ],
        "route": {
            "rules": [
                { "ip_is_private": true, "outbound": "direct" }
            ],
            "final": "node",
            "auto_detect_interface": true
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::model::{Endpoint, TlsConfig};

    fn sample_node(protocol: &str) -> Node {
        Node {
            id: "n1".into(),
            name: "N1".into(),
            protocol: protocol.into(),
            endpoint: Endpoint {
                host: "h.example.com".into(),
                port: 443,
            },
            authentication: json!({}),
            transport: json!({}),
            tls: TlsConfig::default(),
            metadata: json!({}),
            subscription_id: String::new(),
            enabled: true,
            created_at: 0,
        }
    }

    #[test]
    fn shadowsocks_outbound_shape() {
        let mut n = sample_node(SHADOWSOCKS);
        n.authentication = json!({ "method": "aes-256-gcm", "password": "pw" });
        let cfg = build_singbox_config(&n, 2080, &DnsConfig::default()).unwrap();
        let obs = &cfg["outbounds"];
        assert_eq!(obs[0]["type"], "shadowsocks");
        assert_eq!(obs[0]["server_port"], 443);
        assert_eq!(cfg["inbounds"][0]["listen_port"], 2080);
        assert_eq!(cfg["route"]["final"], "node");
    }

    #[test]
    fn vless_ws_tls_outbound() {
        let mut n = sample_node(VLESS);
        n.authentication = json!({ "uuid": "u1" });
        n.tls.enabled = true;
        n.tls.server_name = "sni.x".into();
        n.transport = json!({ "type": "ws", "path": "/p", "headers": {"Host":"cdn.x"} });
        let cfg = build_singbox_config(&n, 2080, &DnsConfig::default()).unwrap();
        assert_eq!(cfg["outbounds"][0]["tls"]["server_name"], "sni.x");
        assert_eq!(cfg["outbounds"][0]["transport"]["type"], "ws");
    }
}

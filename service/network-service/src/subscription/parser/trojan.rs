//! Trojan URI:
//!   trojan://password@host:443?security=tls&sni=..&type=ws&host=..&path=..#name

use percent_encoding::percent_decode_str;
use serde_json::json;
use url::Url;

use crate::nodes::model::{Node, TlsConfig, TROJAN};

use super::{build_node, decode_name};

pub(super) fn query(u: &Url, key: &str) -> Option<String> {
    u.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.to_string())
}

pub(super) fn transport_from_query(u: &Url, host: &str) -> serde_json::Value {
    let kind = query(u, "type").unwrap_or_else(|| "tcp".to_string());
    match kind.as_str() {
        "ws" => {
            let ws_host = query(u, "host").unwrap_or_else(|| host.to_string());
            let path = query(u, "path").unwrap_or_else(|| "/".to_string());
            json!({ "type": "ws", "path": path, "headers": { "Host": ws_host } })
        }
        "grpc" => json!({
            "type": "grpc",
            "service_name": query(u, "serviceName").unwrap_or_default()
        }),
        // Xray XHTTP transport (vless://...&type=xhttp&mode=stream-up&path=..).
        // sing-box has no XHTTP implementation; the core manager routes these
        // nodes to the Xray adapter. Only the fields shared by share-URI
        // imports are retained (mode/path/host).
        "xhttp" => {
            let mut o = serde_json::Map::new();
            o.insert("type".into(), json!("xhttp"));
            if let Some(mode) = query(u, "mode") {
                if !mode.is_empty() {
                    o.insert("mode".into(), json!(mode));
                }
            }
            if let Some(path) = query(u, "path") {
                if !path.is_empty() {
                    o.insert("path".into(), json!(path));
                }
            }
            if let Some(xh) = query(u, "host") {
                if !xh.is_empty() {
                    o.insert("host".into(), json!(xh));
                }
            }
            serde_json::Value::Object(o)
        }
        "httpupgrade" => {
            let hu_host = query(u, "host").unwrap_or_else(|| host.to_string());
            let path = query(u, "path").unwrap_or_else(|| "/".to_string());
            json!({ "type": "httpupgrade", "host": hu_host, "path": path })
        }
        _ => json!({}),
    }
}

pub fn parse(line: &str) -> Option<Node> {
    let u = Url::parse(line).ok()?;
    let host = u.host_str()?.to_string();
    let port = u.port().unwrap_or(443);
    let password = percent_decode_str(u.username()).decode_utf8_lossy().to_string();
    if password.is_empty() {
        return None;
    }
    let security = query(&u, "security").unwrap_or_else(|| "tls".to_string());
    let sni = query(&u, "sni").unwrap_or_else(|| host.clone());
    let tls = TlsConfig {
        enabled: security == "tls",
        server_name: sni,
        insecure: query(&u, "allowInsecure").as_deref() == Some("1"),
        alpn: vec![],
    };
    let name = decode_name(Some(u.fragment().unwrap_or("")), &format!("{host}:{port}"));
    Some(build_node(
        TROJAN,
        name,
        host,
        port,
        json!({ "password": password }),
        transport_from_query(&u, &u.host_str().unwrap_or_default()),
        tls,
        &password,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trojan_ws() {
        let uri = "trojan://p%40ss@h.example.com:443?security=tls&sni=sni.x&type=ws&\
                   host=cdn.x&path=%2Fws#TR";
        let n = parse(uri).unwrap();
        assert_eq!(n.authentication["password"], "p@ss");
        assert!(n.tls.enabled);
        assert_eq!(n.tls.server_name, "sni.x");
        assert_eq!(n.transport["type"], "ws");
        assert_eq!(n.transport["headers"]["Host"], "cdn.x");
        assert_eq!(n.name, "TR");
    }
}

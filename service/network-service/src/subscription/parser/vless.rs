//! VLESS URI:
//!   vless://uuid@host:443?encryption=none&security=tls&sni=..&type=ws&path=..#name

use percent_encoding::percent_decode_str;
use serde_json::json;
use url::Url;

use crate::nodes::model::{Node, TlsConfig, VLESS};

use super::{build_node, decode_name, trojan::query, trojan::transport_from_query};

pub fn parse(line: &str) -> Option<Node> {
    let u = Url::parse(line).ok()?;
    let host = u.host_str()?.to_string();
    let port = u.port().unwrap_or(443);
    let uuid = percent_decode_str(u.username()).decode_utf8_lossy().to_string();
    if uuid.is_empty() {
        return None;
    }
    let security = query(&u, "security").unwrap_or_else(|| "none".to_string());
    let sni = query(&u, "sni").unwrap_or_else(|| host.clone());
    let flow = query(&u, "flow").unwrap_or_default();
    let mut tls = TlsConfig {
        enabled: security == "tls" || security == "reality",
        server_name: sni,
        insecure: query(&u, "allowInsecure").as_deref() == Some("1"),
        alpn: vec![],
    };
    if let Some(alpn) = query(&u, "alpn") {
        tls.alpn = alpn.split(',').map(|s| s.to_string()).collect();
    }

    let mut authentication = json!({ "uuid": uuid, "flow": flow });
    // Reality parameters are retained here; the core adapter maps them when the
    // bundled core supports reality.
    if security == "reality" {
        authentication["reality"] = json!({
            "public_key": query(&u, "pbk").unwrap_or_default(),
            "short_id": query(&u, "sid").unwrap_or_default(),
            "fingerprint": query(&u, "fp").unwrap_or_default(),
        });
    }

    let name = decode_name(Some(u.fragment().unwrap_or("")), &format!("{host}:{port}"));
    Some(build_node(
        VLESS,
        name,
        host,
        port,
        authentication,
        transport_from_query(&u, &u.host_str().unwrap_or_default()),
        tls,
        &uuid,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vless_tls_ws() {
        let uri = "vless://uuid-xyz@h.example.com:443?encryption=none&security=tls&\
                   sni=sni.example&type=ws&path=%2Fvless#VL";
        let n = parse(uri).unwrap();
        assert_eq!(n.protocol, "vless");
        assert_eq!(n.authentication["uuid"], "uuid-xyz");
        assert!(n.tls.enabled);
        assert_eq!(n.transport["type"], "ws");
        assert_eq!(n.name, "VL");
    }
}

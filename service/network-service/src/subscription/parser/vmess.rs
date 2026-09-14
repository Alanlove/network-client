//! VMess URI: `vmess://base64(json config)` (V2RayN share format).

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::{json, Value};

use crate::nodes::model::{Node, TlsConfig, VMESS};

use super::{build_node, decode_name};

fn b64_either(input: &str) -> Option<Vec<u8>> {
    let pad = |s: &str| {
        let mut s = s.to_string();
        while s.len() % 4 != 0 {
            s.push('=');
        }
        s
    };
    STANDARD.decode(pad(input))
        .or_else(|_| URL_SAFE_NO_PAD.decode(pad(input)))
        .ok()
}

fn get_str(v: &Value, key: &str) -> String {
    v.get(key)
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => other.to_string().trim_matches('"').to_string(),
        })
        .unwrap_or_default()
}

pub fn parse(line: &str) -> Option<Node> {
    let body = line.strip_prefix("vmess://")?;
    let decoded = String::from_utf8(b64_either(body.trim())?).ok()?;
    let v: Value = serde_json::from_str(&decoded).ok()?;

    let host = get_str(&v, "add");
    let port: u16 = get_str(&v, "port").parse().ok()?;
    let uuid = get_str(&v, "id");
    if host.is_empty() || uuid.is_empty() {
        return None;
    }
    let aid = get_str(&v, "aid");
    let security = {
        let s = get_str(&v, "scy");
        if s.is_empty() {
            "auto".to_string()
        } else {
            s
        }
    };
    let net = get_str(&v, "net");
    let path = get_str(&v, "path");
    let ws_host = get_str(&v, "host");
    let tls_on = get_str(&v, "tls").eq_ignore_ascii_case("tls");
    let sni = get_str(&v, "sni");
    let ps = get_str(&v, "ps");

    let transport = match net.as_str() {
        "ws" => json!({
            "type": "ws",
            "path": if path.is_empty() { "/" } else { &path },
            "headers": { "Host": if ws_host.is_empty() { host.clone() } else { ws_host.clone() } }
        }),
        "grpc" => json!({
            "type": "grpc",
            "service_name": path,
        }),
        "h2" => json!({ "type": "http", "path": path, "host": ws_host }),
        _ => json!({}),
    };

    let tls = TlsConfig {
        enabled: tls_on,
        server_name: if sni.is_empty() {
            ws_host.clone()
        } else {
            sni
        },
        insecure: false,
        alpn: vec![],
    };

    let name = decode_name(None, &ps);
    Some(build_node(
        VMESS,
        name,
        host.clone(),
        port,
        json!({ "uuid": uuid, "alter_id": aid, "security": security }),
        transport,
        tls,
        &uuid,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2rayn_format() {
        let cfg = json!({
            "v": "2", "ps": "JP", "add": "jp.example.com", "port": "443",
            "id": "11111111-2222-3333-4444-555555555555", "aid": "0",
            "scy": "auto", "net": "ws", "type": "none", "host": "cdn.example.com",
            "path": "/ray", "tls": "tls", "sni": "sni.example.com"
        })
        .to_string();
        let uri = format!("vmess://{}", STANDARD.encode(cfg));
        let n = parse(&uri).unwrap();
        assert_eq!(n.name, "JP");
        assert_eq!(n.endpoint.port, 443);
        assert!(n.tls.enabled);
        assert_eq!(n.tls.server_name, "sni.example.com");
        assert_eq!(n.transport["type"], "ws");
        assert_eq!(n.transport["path"], "/ray");
        assert_eq!(n.authentication["uuid"], "11111111-2222-3333-4444-555555555555");
    }
}

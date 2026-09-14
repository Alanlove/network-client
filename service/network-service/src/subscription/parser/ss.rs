//! Shadowsocks SIP002 / legacy URI:
//!   ss://base64url(method:password)@host:port?plugin=..#name
//!   ss://base64(method:password@host:port)#name

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::json;

use crate::nodes::model::{Node, TlsConfig, SHADOWSOCKS};

use super::{build_node, decode_name, split_hostport};

fn b64_either(input: &str) -> Option<Vec<u8>> {
    let pad = |s: &str| {
        let mut s = s.to_string();
        while s.len() % 4 != 0 {
            s.push('=');
        }
        s
    };
    URL_SAFE_NO_PAD
        .decode(pad(input))
        .or_else(|_| STANDARD.decode(pad(input)))
        .ok()
}

pub fn parse(line: &str) -> Option<Node> {
    let body = line.strip_prefix("ss://")?;
    let (main, fragment) = body.split_once('#').map(|(m, f)| (m, Some(f))).unwrap_or((body, None));

    // Legacy: whole body is one base64 blob.
    if !main.contains('@') {
        let decoded = String::from_utf8(b64_either(main)?).ok()?;
        let decoded = decoded.strip_suffix('/').unwrap_or(&decoded);
        // method:password@host:port
        let (creds, hostport) = decoded.rsplit_once('@')?;
        let (method, password) = creds.split_once(':')?;
        let (host, port) = split_hostport(hostport, 8388)?;
        let name = decode_name(fragment, &format!("{host}:{port}"));
        return Some(build_node(
            SHADOWSOCKS,
            name,
            host,
            port,
            json!({ "method": method, "password": password }),
            json!({}),
            TlsConfig::default(),
            &format!("{method}:{password}"),
        ));
    }

    // SIP002.
    let (userinfo, after) = main.split_once('@')?;
    let (hostport_raw, _query) = after.split_once('?').map(|(h, q)| (h, Some(q))).unwrap_or((after, None));
    let creds = String::from_utf8(b64_either(userinfo)?).ok()?;
    let (method, password) = creds.split_once(':')?;
    let (host, port) = split_hostport(hostport_raw, 8388)?;
    let name = decode_name(fragment, &format!("{host}:{port}"));
    Some(build_node(
        SHADOWSOCKS,
        name,
        host,
        port,
        json!({ "method": method, "password": password }),
        json!({}),
        TlsConfig::default(),
        &format!("{method}:{password}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sip002() {
        // aes-256-gcm:pass -> base64url
        let user = URL_SAFE_NO_PAD.encode(b"aes-256-gcm:pass");
        let uri = format!("ss://{user}@1.2.3.4:8388#Tokyo");
        let n = parse(&uri).unwrap();
        assert_eq!(n.protocol, "shadowsocks");
        assert_eq!(n.endpoint.host, "1.2.3.4");
        assert_eq!(n.endpoint.port, 8388);
        assert_eq!(n.name, "Tokyo");
        assert_eq!(n.authentication["method"], "aes-256-gcm");
    }

    #[test]
    fn legacy() {
        let blob = STANDARD.encode(b"chacha20-ietf-poly1305:secret@5.6.7.8:8388");
        let n = parse(&format!("ss://{blob}")).unwrap();
        assert_eq!(n.authentication["password"], "secret");
        assert_eq!(n.endpoint.port, 8388);
    }
}

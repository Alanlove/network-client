//! SOCKS5 URI: `socks5://user:pass@host:1080#name` (RFC 1928 user/pass auth).

use percent_encoding::percent_decode_str;
use serde_json::json;
use url::Url;

use crate::nodes::model::{Node, TlsConfig, SOCKS};

use super::{build_node, decode_name};

pub fn parse(line: &str) -> Option<Node> {
    let u = Url::parse(line).ok()?;
    let host = u.host_str()?.to_string();
    let port = u.port().unwrap_or(1080);
    let user = percent_decode_str(u.username()).decode_utf8_lossy().to_string();
    let pass = u.password()
        .map(|p| percent_decode_str(p).decode_utf8_lossy().to_string())
        .unwrap_or_default();
    let auth = if user.is_empty() {
        json!({})
    } else {
        json!({ "username": user, "password": pass })
    };
    let name = decode_name(Some(u.fragment().unwrap_or("")), &format!("{host}:{port}"));
    Some(build_node(
        SOCKS,
        name,
        host,
        port,
        auth,
        json!({}),
        TlsConfig::default(),
        &user,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_with_auth() {
        let n = parse("socks5://alice:secret@10.0.0.1:1080#S").unwrap();
        assert_eq!(n.endpoint.host, "10.0.0.1");
        assert_eq!(n.authentication["username"], "alice");
        assert_eq!(n.authentication["password"], "secret");
        assert_eq!(n.name, "S");
    }
}

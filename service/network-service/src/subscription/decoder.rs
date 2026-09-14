//! Subscription body decoding: many providers ship the whole URI list as one
//! base64 blob (standard or url-safe). Plain text lists pass through.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;

/// Returns individual subscription lines.
pub fn decode_body(body: &str) -> Vec<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    // Fast path: already a decoded URI list.
    if trimmed.contains("://") {
        return split_lines(trimmed);
    }

    // Try url-safe first (SIP002), then standard alphabet.
    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(pad_b64(trimmed)) {
        if let Ok(text) = String::from_utf8(bytes) {
            if text.contains("://") {
                return split_lines(&text);
            }
        }
    }
    if let Ok(bytes) = STANDARD.decode(pad_b64(trimmed)) {
        if let Ok(text) = String::from_utf8(bytes) {
            if text.contains("://") {
                return split_lines(&text);
            }
        }
    }
    Vec::new()
}

fn pad_b64(s: &str) -> String {
    let mut s = s.trim().to_string();
    while s.len() % 4 != 0 {
        s.push('=');
    }
    s
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn decodes_standard_base64_list() {
        // Two physical lines: an SS SIP002 URI (inner base64 holds
        // aes-256-gcm:pass@1.2.3.4:8388) followed by a vmess URI.
        let list = "ss://YWVzLTI1Ni1nY206cGFzc0AxLjIuMy40OjgzODg=#Node\nvmess://xxx";
        let raw = STANDARD.encode(list.as_bytes());
        let lines = decode_body(&raw);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("ss://"));
        assert!(lines[1].starts_with("vmess://"));
    }

    #[test]
    fn passes_plain_list_through() {
        let body = "trojan://a@h:443#N\nsocks5://h:1080\n";
        assert_eq!(decode_body(body).len(), 2);
    }
}

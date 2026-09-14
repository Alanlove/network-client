//! Unified node model (design doc v1.1 §23).
//!
//! Protocol-specific knobs live as free-form JSON sections
//! (`authentication` / `transport` / `metadata`) instead of a god-struct, so
//! adding a protocol never touches the control plane.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VLESS: &str = "vless";
pub const VMESS: &str = "vmess";
pub const TROJAN: &str = "trojan";
pub const SHADOWSOCKS: &str = "shadowsocks";
pub const SOCKS: &str = "socks";
pub const WIREGUARD: &str = "wireguard";

pub const SUPPORTED: &[&str] = &[VLESS, VMESS, TROJAN, SHADOWSOCKS, SOCKS, WIREGUARD];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TlsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server_name: String,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub alpn: Vec<String>,
}

/// User-owned configuration. Runtime measurements are NOT stored here
/// (design doc §24) — see [`super::health::NodeRuntimeStats`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    /// Empty for manual add; NodeManager assigns a stable id.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub endpoint: Endpoint,
    #[serde(default)]
    pub authentication: Value,
    #[serde(default)]
    pub transport: Value,
    #[serde(default)]
    pub tls: TlsConfig,
    #[serde(default)]
    pub metadata: Value,
    /// Owning subscription id, empty for manually added nodes.
    #[serde(default)]
    pub subscription_id: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
}

impl Node {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("node name is empty".into());
        }
        if !SUPPORTED.contains(&self.protocol.as_str()) {
            return Err(format!("unsupported protocol: {}", self.protocol));
        }
        if self.endpoint.host.trim().is_empty() {
            return Err("endpoint.host is empty".into());
        }
        if self.endpoint.port == 0 {
            return Err("endpoint.port is zero".into());
        }
        Ok(())
    }
}

//! Persistent configuration schema (design doc §19, §46).
//!
//! Everything in this file is `Persistent State`. Runtime state
//! (connected/current node/latency/traffic) never lives here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RouteMode {
    Global,
    Smart,
    Direct,
}

impl Default for RouteMode {
    fn default() -> Self {
        RouteMode::Smart
    }
}

impl RouteMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteMode::Global => "global",
            RouteMode::Smart => "smart",
            RouteMode::Direct => "direct",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "global" => RouteMode::Global,
            "direct" => RouteMode::Direct,
            _ => RouteMode::Smart,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunConfig {
    /// TUN mode is staged in: adapter lifecycle + routes are implemented;
    /// full userspace datapath lands in M2. When false the service uses
    /// OS system-proxy (WinINET) against the core's mixed inbound.
    pub enabled: bool,
    pub name: String,
    pub mtu: u32,
    pub address_v4: String,
    pub address_v6: String,
}

impl Default for TunConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "NetworkClient".to_string(),
            mtu: 1420,
            address_v4: "10.255.0.1/24".to_string(),
            address_v6: "fdfe:dcba:9876::1/126".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    /// Adapter kind; only "sing-box" ships config builder in V1. The binary
    /// itself is supplied by the user/operator (product ships no circuits).
    pub kind: String,
    pub executable: String,
    pub work_dir: String,
    pub mixed_port: u16,
    pub socks_port: u16,
    pub api_port: u16,
    pub start_timeout_secs: u64,
    pub restart_max: u32,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            kind: "sing-box".to_string(),
            executable: "bin/sing-box.exe".to_string(),
            work_dir: "core".to_string(),
            mixed_port: 2080,
            socks_port: 2081,
            api_port: 2082,
            start_timeout_secs: 10,
            restart_max: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsServer {
    /// "udp" | "doh"
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    /// Optional policy tag: "direct" / "proxy".
    #[serde(default)]
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsConfig {
    /// "smart" | "direct" | "proxy"
    pub mode: String,
    #[serde(default)]
    pub servers: Vec<DnsServer>,
    #[serde(default = "default_true")]
    pub cache: bool,
    #[serde(default = "default_cache_size")]
    pub cache_size: u32,
}

fn default_true() -> bool {
    true
}
fn default_cache_size() -> u32 {
    4096
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            mode: "smart".to_string(),
            servers: vec![
                DnsServer {
                    kind: "udp".to_string(),
                    url: "223.5.5.5:53".to_string(),
                    tag: "direct".to_string(),
                },
                DnsServer {
                    kind: "doh".to_string(),
                    url: "https://1.1.1.1/dns-query".to_string(),
                    tag: "proxy".to_string(),
                },
            ],
            cache: true,
            cache_size: 4096,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartConfig {
    /// Minimum score delta required to switch away from current node
    /// (design doc §31 anti-flapping hysteresis).
    pub switch_threshold: f64,
    pub health_interval_secs: u64,
}

impl Default for SmartConfig {
    fn default() -> Self {
        Self {
            switch_threshold: 10.0,
            health_interval_secs: 45,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub mode: RouteMode,
    #[serde(default)]
    pub tun: TunConfig,
    #[serde(default)]
    pub core: CoreConfig,
    #[serde(default)]
    pub dns: DnsConfig,
    #[serde(default)]
    pub smart: SmartConfig,
    #[serde(default)]
    pub auto_start_proxy: bool,
    #[serde(default)]
    pub restore_previous_connection: bool,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: RouteMode::default(),
            tun: TunConfig::default(),
            core: CoreConfig::default(),
            dns: DnsConfig::default(),
            smart: SmartConfig::default(),
            auto_start_proxy: false,
            restore_previous_connection: false,
            log_level: default_log_level(),
        }
    }
}

/// Persisted hint used after a service crash (design doc §56). Never blindly
/// trusted: every restore path revalidates network, node and core.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimePersist {
    pub last_state: String,
    pub node_id: String,
    pub mode: String,
    pub updated_at: i64,
}

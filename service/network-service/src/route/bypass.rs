//! Proxy bypass list construction (design doc §29). The loopback / private
//! ranges must always bypass so local services and LAN devices keep working;
//! the remote node endpoint is added when needed to avoid routing loops.

const PRIVATE_BYPASS: &str =
    "localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;192.168.*;<local>";

/// Default bypass used by System Proxy mode.
pub fn default_bypass() -> String {
    PRIVATE_BYPASS.to_string()
}

/// Bypass list additionally excluding the proxy server endpoint (TUN mode
/// loop prevention / corporate gateway pinning).
pub fn bypass_with_server(server_host: &str) -> String {
    if server_host.is_empty() {
        return default_bypass();
    }
    format!("{PRIVATE_BYPASS};{server_host}")
}

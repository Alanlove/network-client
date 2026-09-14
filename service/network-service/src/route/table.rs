//! Route table operations for TUN mode (design doc §27–§29).
//!
//! Before installing the default route through the TUN adapter, the remote
//! node endpoint must be pinned to the physical default gateway to avoid the
//! classic routing loop (tunnel packets trying to enter the tunnel).
//! Implemented as thin `route.exe` wrappers for V1; an IP Helper API version
//! can replace them later without changing callers.

use tokio::process::Command;

use crate::common::{SvcError, SvcResult};

/// Add a /32 host route via the given gateway.
pub async fn pin_host_via_gateway(host: &str, gateway: &str) -> SvcResult<()> {
    run_route(&["add", host, "mask", "255.255.255.255", gateway]).await
}

/// Remove the /32 host route added by `pin_host_via_gateway`.
pub async fn unpin_host(host: &str) -> SvcResult<()> {
    // `route delete` returns non-zero if the entry is already gone; treat
    // that as success to keep disconnect idempotent.
    match run_route(&["delete", host]).await {
        Ok(()) => Ok(()),
        Err(_) => Ok(()),
    }
}

async fn run_route(args: &[&str]) -> SvcResult<()> {
    let mut cmd = Command::new("route");
    cmd.args(args);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = cmd.output().await.map_err(SvcError::Io)?;
    if !out.status.success() {
        return Err(SvcError::Route(format!(
            "route {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

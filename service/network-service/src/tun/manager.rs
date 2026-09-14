//! TUN lifecycle: load wintun, create/open adapter, assign address + MTU via
//! netsh, supervise shutdown (design doc §24–§26). The userspace TCP/IP stack
//! is a later milestone; V1 enables the adapter so routing experiments and
//! future packet-loop work have a stable surface.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::process::Command;

use crate::common::{SvcError, SvcResult};
use crate::config::manager::ConfigManager;
use crate::config::schema::TunConfig;
use crate::tun::adapter::{AdapterHandle, WintunLib};

pub struct TunManager {
    config: ConfigManager,
    lib: Mutex<Option<Arc<WintunLib>>>,
    adapter: Mutex<Option<AdapterHandle>>,
}

impl TunManager {
    pub fn new(config: ConfigManager) -> Self {
        Self {
            config,
            lib: Mutex::new(None),
            adapter: Mutex::new(None),
        }
    }

    fn search_dirs(&self) -> Vec<PathBuf> {
        let root = &self.config.paths.root;
        vec![root.join("bin"), root.clone()]
    }

    /// Pre-load wintun.dll; reports a graceful error when it is absent.
    pub fn ensure_loaded(&self) -> SvcResult<Arc<WintunLib>> {
        if let Some(lib) = self.lib.lock().clone() {
            return Ok(lib);
        }
        let mut last_err = None;
        for dir in self.search_dirs() {
            match WintunLib::load(&dir) {
                Ok(lib) => {
                    let lib = Arc::new(lib);
                    *self.lib.lock() = Some(lib.clone());
                    tracing::info!(version = lib.driver_version(), "wintun loaded from {}", dir.display());
                    return Ok(lib);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| SvcError::Tun("wintun.dll not found".into())))
    }

    pub fn is_up(&self) -> bool {
        self.adapter.lock().is_some()
    }

    /// Start the adapter (blocking driver calls run on a blocking thread).
    pub async fn start(&self) -> SvcResult<()> {
        if self.is_up() {
            return Err(SvcError::AlreadyRunning("tun adapter".into()));
        }
        let cfg = self.config.app().tun.clone();
        let lib = self.ensure_loaded()?;
        let name = cfg.name.clone();
        let handle = tokio::task::spawn_blocking(move || lib.start(&name))
            .await
            .map_err(|e| SvcError::Tun(e.to_string()))??;
        *self.adapter.lock() = Some(handle);

        if let Err(e) = assign_address(&cfg).await {
            // Adapter exists but address assignment failed (typically missing
            // elevation): tear down and surface the cause.
            self.adapter.lock().take();
            return Err(e);
        }
        tracing::info!(adapter = %cfg.name, "TUN adapter up");
        Ok(())
    }

    pub async fn stop(&self) {
        let handle = self.adapter.lock().take();
        drop(handle);
        tracing::info!("TUN adapter closed");
    }
}

async fn assign_address(cfg: &TunConfig) -> SvcResult<()> {
    // Split "10.255.0.1/24" into address + mask for netsh.
    let (addr, prefix) = cfg
        .address_v4
        .split_once('/')
        .ok_or_else(|| SvcError::ConfigInvalid("tun.inet4_address must be a CIDR".into()))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| SvcError::ConfigInvalid("tun.inet4_address prefix invalid".into()))?;
    let mask = prefix_to_mask(prefix);

    run_netsh(&[
        "interface",
        "ip",
        "set",
        "address",
        &format!("name={}", cfg.name),
        "static",
        addr,
        &mask,
    ])
    .await?;
    run_netsh(&[
        "interface",
        "ipv4",
        "set",
        "subinterface",
        &cfg.name,
        &format!("mtu={}", cfg.mtu),
        "store=active",
    ])
    .await?;
    Ok(())
}

fn prefix_to_mask(prefix: u8) -> String {
    let bits = u32::MAX << (32 - prefix.min(32));
    let b = bits.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

async fn run_netsh(args: &[&str]) -> SvcResult<()> {
    let out = Command::new("netsh")
        .args(args)
        .creation_flags(0x0800_0000)
        .output()
        .await
        .map_err(SvcError::Io)?;
    if !out.status.success() {
        return Err(SvcError::Route(format!(
            "netsh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::prefix_to_mask;

    #[test]
    fn masks() {
        assert_eq!(prefix_to_mask(24), "255.255.255.0");
        assert_eq!(prefix_to_mask(16), "255.255.0.0");
        assert_eq!(prefix_to_mask(32), "255.255.255.255");
    }
}

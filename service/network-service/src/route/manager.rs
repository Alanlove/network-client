//! System proxy control via WinINET (design doc §30). On connect we remember
//! the user's previous proxy configuration and restore it verbatim on
//! disconnect — shutdown must never leave a dead proxy configured system
//! wide. The "before" snapshot is also persisted to disk so a hard crash can
//! be cleaned up on the next service start.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::common::SvcResult;

#[derive(Clone, Serialize, Deserialize)]
struct SavedProxy {
    proxy: String,
    bypass: String,
    direct: bool,
}

/// True if a WinINET `ProxyServer` string references our own mixed inbound
/// (matches a bare `host:port` entry or an explicit scheme prefix). Splitting
/// on ';' avoids `:2080` matching an unrelated `:20800`.
fn references_endpoint(spec: &str, host_port: &str) -> bool {
    spec.split(';').map(str::trim).any(|part| {
        part == host_port
            || part == format!("http={host_port}")
            || part == format!("https={host_port}")
            || part == format!("socks={host_port}")
    })
}

pub struct RouteManager {
    saved: Mutex<Option<SavedProxy>>,
    backup_path: PathBuf,
}

impl RouteManager {
    pub fn new(data_root: &Path) -> Self {
        Self {
            saved: Mutex::new(None),
            backup_path: data_root.join("proxy_backup.json"),
        }
    }

    pub fn is_active(&self) -> bool {
        self.saved.lock().is_some()
    }

    /// Load the crash-persisted "before connect" snapshot, if present.
    fn load_backup(&self) -> Option<SavedProxy> {
        let raw = std::fs::read_to_string(&self.backup_path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn store_backup(&self, saved: &SavedProxy) {
        if let Err(e) = crate::config::manager::atomic_write_json(&self.backup_path, saved) {
            tracing::warn!("persist system-proxy backup failed: {e}");
        }
    }

    fn drop_backup(&self) {
        let _ = std::fs::remove_file(&self.backup_path);
    }

    /// Prime the in-memory "before connect" snapshot from the crash backup
    /// WITHOUT touching the registry. Used right before an automatic
    /// reconnect at service start: the live proxy already points at our
    /// local inbound, and clean disconnect later must restore the user's
    /// original settings, not our own.
    pub fn prime_from_backup(&self) -> bool {
        if let Some(saved) = self.load_backup() {
            *self.saved.lock() = Some(saved);
            true
        } else {
            false
        }
    }

    /// Restore proxy settings after an unclean shutdown. Returns true when a
    /// backup was restored. Also covers the case where the registry is left
    /// pointing at our dead local port without a backup.
    pub fn restore_after_crash(&self, local_port: u16) -> bool {
        if let Some(saved) = self.load_backup() {
            let result = if saved.direct {
                if !saved.proxy.is_empty() || !saved.bypass.is_empty() {
                    wininet::apply_proxy(Some((&saved.proxy, &saved.bypass)))
                        .and_then(|_| wininet::apply_proxy(None))
                } else {
                    wininet::apply_proxy(None)
                }
            } else {
                wininet::apply_proxy(Some((&saved.proxy, &saved.bypass)))
            };
            match result {
                Ok(()) => tracing::info!("restored system proxy from crash backup"),
                Err(e) => tracing::error!("failed to restore proxy from backup: {e}"),
            }
            self.drop_backup();
            return true;
        }
        // No backup: if the registry still references our local inbound from
        // a crashed session, switch it off to avoid a dead-proxy black hole.
        let current = wininet::query_proxy();
        if !current.direct && references_endpoint(&current.proxy, &format!("127.0.0.1:{local_port}")) {
            match wininet::apply_proxy(None) {
                Ok(()) => tracing::warn!("cleared stale system proxy pointing at port {local_port}"),
                Err(e) => tracing::error!("failed to clear stale system proxy: {e}"),
            }
            return true;
        }
        false
    }

    /// Re-assert the registry values without touching the saved "before"
    /// snapshot. Used after an automatic reconnect when the registry may
    /// have been changed by the user while the service was stopped.
    pub fn ensure_proxy_applied(&self, host_port: &str, bypass: &str) {
        let expected = format!("http={host_port};https={host_port}");
        let current = wininet::query_proxy();
        if current.direct || current.proxy != expected {
            match wininet::apply_proxy(Some((&expected, bypass))) {
                Ok(()) => tracing::info!("re-applied system proxy at {host_port} after restore"),
                Err(e) => tracing::error!("re-apply system proxy failed: {e}"),
            }
        }
    }

    /// Enable the system-wide HTTP/HTTPS proxy pointing at the local mixed
    /// inbound. Idempotent: calling twice does not overwrite the saved state.
    pub fn set_system_proxy(&self, host_port: &str, bypass: &str) -> SvcResult<()> {
        if self.saved.lock().is_some() {
            return Ok(());
        }
        let mut previous = wininet::query_proxy();
        // If the live registry already points at OUR inbound (orphaned state
        // from a previous hard kill without a backup), it is not the user's
        // real baseline — remember "direct" instead so disconnect doesn't
        // restore a dead proxy.
        if !previous.direct && references_endpoint(&previous.proxy, host_port) {
            previous = SavedProxy {
                proxy: String::new(),
                bypass: String::new(),
                direct: true,
            };
        }
        let proxy_spec = format!("http={host_port};https={host_port}");
        if let Err(e) = wininet::apply_proxy(Some((&proxy_spec, bypass))) {
            return Err(e);
        }
        // Persist only after the registry write succeeded.
        *self.saved.lock() = Some(previous);
        if let Some(saved) = self.saved.lock().as_ref() {
            self.store_backup(saved);
        }
        tracing::info!("system proxy enabled at {host_port}");
        Ok(())
    }

    /// Restore whatever was configured before `set_system_proxy` (or direct).
    /// Always succeeds for the caller — errors are logged only.
    pub fn clear_system_proxy(&self) {
        let saved = self.saved.lock().take();
        let Some(saved) = saved else {
            return;
        };
        let result = (|| -> SvcResult<()> {
            if saved.direct {
                // Restore the original strings verbatim first, then switch the
                // enable flag back off so the machine looks exactly as it did
                // before connect.
                if !saved.proxy.is_empty() || !saved.bypass.is_empty() {
                    wininet::apply_proxy(Some((&saved.proxy, &saved.bypass)))?;
                }
                wininet::apply_proxy(None)
            } else {
                wininet::apply_proxy(Some((&saved.proxy, &saved.bypass)))
            }
        })();
        match result {
            Ok(()) => {
                self.drop_backup();
                tracing::info!("system proxy restored to pre-connect state");
            }
            Err(e) => tracing::error!("failed to restore system proxy: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::references_endpoint;

    #[test]
    fn endpoint_matching() {
        let ep = "127.0.0.1:2080";
        assert!(references_endpoint("127.0.0.1:2080", ep));
        assert!(references_endpoint("http=127.0.0.1:2080;https=127.0.0.1:2080", ep));
        assert!(references_endpoint("http=127.0.0.1:2080 ; https=127.0.0.1:2080", ep));
        // Prefix-style collision must not match a different port.
        assert!(!references_endpoint("127.0.0.1:20800", ep));
        assert!(!references_endpoint("127.0.0.1:7897", ep));
        assert!(!references_endpoint("http=10.0.0.1:2080", ep));
    }
}

#[cfg(windows)]
mod wininet {
    use windows::Win32::Networking::WinInet::{
        InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
    };
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    use crate::common::{SvcError, SvcResult};

    use super::SavedProxy;

    const IE_SETTINGS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    /// Read the persisted WinINET proxy settings straight from the registry.
    /// `InternetQueryOption` only reflects the calling process' runtime view,
    /// while the registry is what Explorer and every other app consume.
    pub fn query_proxy() -> SavedProxy {
        let fallback = SavedProxy {
            proxy: String::new(),
            bypass: String::new(),
            direct: true,
        };
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(key) = hkcu.open_subkey_with_flags(IE_SETTINGS_KEY, KEY_READ) else {
            return fallback;
        };
        let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
        let proxy = key.get_value::<String, _>("ProxyServer").unwrap_or_default();
        let bypass = key
            .get_value::<String, _>("ProxyOverride")
            .unwrap_or_default();
        SavedProxy {
            proxy,
            bypass,
            direct: enabled != 1,
        }
    }

    /// Persist proxy settings.
    ///
    /// `InternetSetOption(INTERNET_OPTION_PROXY)` does NOT write the registry
    /// on a range of Windows builds (the call returns TRUE while
    /// ProxyEnable/ProxyServer stay untouched). v2rayN, Clash and friends set
    /// the registry values directly and then broadcast
    /// SETTINGS_CHANGED/REFRESH so running apps pick the new values up.
    pub fn apply_proxy(spec: Option<(&str, &str)>) -> SvcResult<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey_with_flags(IE_SETTINGS_KEY, KEY_READ | KEY_WRITE)
            .map_err(|e| SvcError::Route(format!("open Internet Settings key failed: {e}")))?;

        match spec {
            Some((proxy, bypass)) => {
                key.set_value("ProxyEnable", &1u32)
                    .map_err(|e| SvcError::Route(format!("set ProxyEnable failed: {e}")))?;
                key.set_value("ProxyServer", &proxy.to_string())
                    .map_err(|e| SvcError::Route(format!("set ProxyServer failed: {e}")))?;
                key.set_value("ProxyOverride", &bypass.to_string())
                    .map_err(|e| SvcError::Route(format!("set ProxyOverride failed: {e}")))?;
            }
            None => {
                // Switch off but leave the saved strings in place.
                key.set_value("ProxyEnable", &0u32)
                    .map_err(|e| SvcError::Route(format!("clear ProxyEnable failed: {e}")))?;
            }
        }

        // Notify WinINET consumers (best effort; registry is already the
        // source of truth at this point).
        unsafe {
            let _ = InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0);
            let _ = InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0);
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod wininet {
    use crate::common::SvcResult;

    use super::SavedProxy;

    pub fn query_proxy() -> SavedProxy {
        SavedProxy {
            proxy: String::new(),
            bypass: String::new(),
            direct: true,
        }
    }

    pub fn apply_proxy(_spec: Option<(&str, &str)>) -> SvcResult<()> {
        Ok(())
    }
}

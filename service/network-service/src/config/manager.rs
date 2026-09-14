//! Filesystem layout, atomic JSON persistence (design doc §46, §48, §66).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::common::{SvcError, SvcResult};
use crate::config::schema::{AppConfig, RuntimePersist};

#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
    pub config_dir: PathBuf,
    pub subscriptions_dir: PathBuf,
    pub nodes_dir: PathBuf,
    pub rules_dir: PathBuf,
    pub db_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn new() -> Self {
        // Explicit override for portable deployments and development.
        let base = match std::env::var_os("NC_DATA_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("NetworkClient"),
        };
        Self::with_root(base)
    }

    pub fn with_root(root: PathBuf) -> Self {
        let p = Self {
            config_dir: root.join("config"),
            subscriptions_dir: root.join("subscriptions"),
            nodes_dir: root.join("nodes"),
            rules_dir: root.join("rules"),
            db_dir: root.join("db"),
            logs_dir: root.join("logs"),
            cache_dir: root.join("cache"),
            root,
        };
        for dir in [
            &p.config_dir,
            &p.subscriptions_dir,
            &p.nodes_dir,
            &p.rules_dir,
            &p.db_dir,
            &p.logs_dir,
            &p.cache_dir,
        ] {
            std::fs::create_dir_all(dir).ok();
        }
        p
    }

    pub fn app_config(&self) -> PathBuf {
        self.config_dir.join("app.json")
    }
    pub fn dns_config(&self) -> PathBuf {
        self.config_dir.join("dns.json")
    }
    pub fn nodes(&self) -> PathBuf {
        self.nodes_dir.join("nodes.json")
    }
    pub fn subscriptions(&self) -> PathBuf {
        self.subscriptions_dir.join("subscriptions.json")
    }
    pub fn rules(&self) -> PathBuf {
        self.rules_dir.join("rules.json")
    }
    pub fn runtime_state(&self) -> PathBuf {
        self.root.join("runtime_state.json")
    }
    pub fn database(&self) -> PathBuf {
        self.db_dir.join("statistics.db")
    }
}

impl Default for Paths {
    fn default() -> Self {
        Self::new()
    }
}

/// Atomic write: `target.tmp` -> fsync -> rename. `std::fs::rename` on Windows
/// uses MoveFileExW with MOVEFILE_REPLACE_EXISTING, so a crash can never leave
/// a 0-byte config behind.
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> SvcResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_json_or<T: DeserializeOwned + Default>(path: &Path) -> T {
    read_json(path).unwrap_or_default()
}

/// Like `read_json_or`, but a file that *exists* yet fails to parse is loud:
/// silent fallback to an empty Vec previously masked a malformed store
/// (e.g. PowerShell writing `{...}` where `[{...}]` was expected).
pub fn read_json_or_warn<T: DeserializeOwned + Default>(path: &Path, label: &str) -> T {
    match read_json(path) {
        Ok(v) => v,
        Err(_) if !path.exists() => T::default(),
        Err(e) => {
            tracing::warn!(
                label,
                path = %path.display(),
                "failed to read store; falling back to empty default: {e}"
            );
            T::default()
        }
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> SvcResult<T> {
    let mut body = std::fs::read(path)?;
    // Windows editors (Notepad, PowerShell 5.1) routinely persist UTF-8
    // with a BOM, which serde_json rejects — strip it before parsing.
    if body.starts_with(b"\xef\xbb\xbf") {
        body = body[3..].to_vec();
    }
    Ok(serde_json::from_slice(&body)?)
}

pub fn validate_app(cfg: &AppConfig) -> SvcResult<()> {
    if cfg.tun.mtu < 576 || cfg.tun.mtu > 9000 {
        return Err(SvcError::ConfigInvalid("tun.mtu out of [576, 9000]".into()));
    }
    if cfg.core.mixed_port == 0 || cfg.dns.cache_size > 1_000_000 {
        return Err(SvcError::ConfigInvalid("core/dns config invalid".into()));
    }
    for srv in &cfg.dns.servers {
        match srv.kind.as_str() {
            "udp" | "doh" => {}
            other => {
                return Err(SvcError::ConfigInvalid(format!(
                    "unknown dns server type: {other}"
                )))
            }
        }
    }
    Ok(())
}

/// All on-disk mutations go through this manager so an in-process write
/// collision is impossible.
#[derive(Clone)]
pub struct ConfigManager {
    pub paths: Arc<Paths>,
    app: Arc<Mutex<AppConfig>>,
    runtime: Arc<Mutex<RuntimePersist>>,
}

/// Load app.json, falling back to defaults when it is absent or invalid.
/// A present-but-unreadable file is logged loudly so silent fallback can
/// never masquerade as the user's real configuration.
fn load_app_config(paths: &Paths) -> AppConfig {
    let path = paths.app_config();
    if !path.exists() {
        return AppConfig::default();
    }
    match read_json(&path) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!("app config {path:?} unreadable ({e}); using defaults");
            AppConfig::default()
        }
    }
}

impl ConfigManager {
    pub fn load() -> SvcResult<Self> {
        let paths = Arc::new(Paths::new());
        let app: AppConfig = load_app_config(&paths);
        validate_app(&app)?;
        let runtime: RuntimePersist =
            read_json_or_warn(&paths.runtime_state(), "runtime_state");
        Ok(Self {
            paths,
            app: Arc::new(Mutex::new(app)),
            runtime: Arc::new(Mutex::new(runtime)),
        })
    }

    /// Test/CI constructor with an explicit data root.
    pub fn load_with_root(root: PathBuf) -> SvcResult<Self> {
        let paths = Arc::new(Paths::with_root(root));
        let app: AppConfig = load_app_config(&paths);
        validate_app(&app)?;
        let runtime: RuntimePersist =
            read_json_or_warn(&paths.runtime_state(), "runtime_state");
        Ok(Self {
            paths,
            app: Arc::new(Mutex::new(app)),
            runtime: Arc::new(Mutex::new(runtime)),
        })
    }

    pub fn app(&self) -> AppConfig {
        self.app.lock().clone()
    }

    pub fn save_app(&self, cfg: AppConfig) -> SvcResult<()> {
        validate_app(&cfg)?;
        atomic_write_json(&self.paths.app_config(), &cfg)?;
        *self.app.lock() = cfg;
        Ok(())
    }

    pub fn dns(&self) -> crate::config::schema::DnsConfig {
        read_json_or(&self.paths.dns_config())
    }

    pub fn save_dns(&self, dns: &crate::config::schema::DnsConfig) -> SvcResult<()> {
        atomic_write_json(&self.paths.dns_config(), dns)
    }

    pub fn runtime(&self) -> RuntimePersist {
        self.runtime.lock().clone()
    }

    pub fn save_runtime(&self, state: RuntimePersist) -> SvcResult<()> {
        atomic_write_json(&self.paths.runtime_state(), &state)?;
        *self.runtime.lock() = state;
        Ok(())
    }
}

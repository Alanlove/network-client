//! External-process proxy core adapter.
//!
//! Spawns a user-provided sing-box binary (`run -c config.json`), streams its
//! logs into tracing, waits for the local mixed inbound and supervises stop.
//! The product ships no proxy lines and no core binary — both are operator
//! supplied (design doc §19/§21).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::Mutex;

use serde_json::Value;

use crate::common::{SvcError, SvcResult};
use crate::config::manager::{atomic_write_json, ConfigManager};
use crate::config::schema::DnsConfig;
use crate::ipc::events::{ev, EventBus};
use crate::nodes::model::Node;
use crate::proxy::adapter::{CoreHealth, CoreStatus, ProxyCoreAdapter};
use crate::proxy::{config_builder, xray_builder};

/// Cores the external-process adapter can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreKind {
    SingBox,
    Xray,
}

impl CoreKind {
    /// Explicit `core.kind = "xray"` forces Xray for every node; otherwise
    /// nodes carrying Xray-only transports (XHTTP) auto-route to Xray while
    /// everything else stays on sing-box.
    fn for_node(app_kind: &str, node: &Node) -> Self {
        if app_kind.eq_ignore_ascii_case("xray") {
            return CoreKind::Xray;
        }
        let needs_xray = node
            .transport
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("xhttp"));
        if needs_xray {
            CoreKind::Xray
        } else {
            CoreKind::SingBox
        }
    }

    fn binary(self) -> &'static str {
        match self {
            CoreKind::SingBox => "bin/sing-box.exe",
            CoreKind::Xray => "bin/xray.exe",
        }
    }
}

const STOPPED: u8 = 0;
const STARTING: u8 = 1;
const RUNNING: u8 = 2;
const ERROR: u8 = 3;

pub struct ExternalCoreAdapter {
    config: ConfigManager,
    bus: EventBus,
    status: AtomicU8,
    pid: AtomicU32,
    child: Mutex<Option<tokio::process::Child>>,
}

impl ExternalCoreAdapter {
    pub fn new(config: ConfigManager, bus: EventBus) -> Self {
        Self {
            config,
            bus,
            status: AtomicU8::new(STOPPED),
            pid: AtomicU32::new(0),
            child: Mutex::new(None),
        }
    }

    fn resolve_paths(&self, kind: CoreKind) -> (PathBuf, PathBuf, PathBuf) {
        let app = self.config.app();
        let root = self.config.paths.root.clone();
        // An explicit absolute executable wins; a relative one is taken
        // verbatim for the configured default core. Auto-selected Xray uses
        // its fixed per-user install path regardless of the sing-box setting.
        let configured = PathBuf::from(&app.core.executable);
        let configured_names_xray = configured
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().starts_with("xray"));
        let exe = match kind {
            // Honour an explicit xray path only when it actually points at xray.
            CoreKind::Xray if configured_names_xray && !configured.is_relative() => configured,
            CoreKind::Xray if configured_names_xray => root.join(&app.core.executable),
            CoreKind::Xray => root.join(kind.binary()),
            CoreKind::SingBox if configured.is_absolute() => configured,
            CoreKind::SingBox => root.join(&app.core.executable),
        };
        let work_dir = if PathBuf::from(&app.core.work_dir).is_absolute() {
            PathBuf::from(&app.core.work_dir)
        } else {
            root.join(&app.core.work_dir)
        };
        let config_path = work_dir.join("config.json");
        (exe, work_dir, config_path)
    }

    async fn wait_for_port(port: u16, timeout: Duration) -> bool {
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        false
    }

    /// Kill core processes left behind by a hard-killed previous service
    /// instance. We only touch processes whose command line references OUR
    /// generated config path, so unrelated xray/sing-box installs are safe.
    pub async fn reap_stale_cores(&self) {
        #[cfg(windows)]
        {
            let (_, _, config_path) = self.resolve_paths(CoreKind::Xray);
            let needle = config_path.to_string_lossy().replace('\'', "''");
            // One PowerShell query covers both xray.exe and sing-box.exe.
            let script = format!(
                "Get-CimInstance Win32_Process -Filter \"Name='xray.exe' OR Name='sing-box.exe'\" | \
                 Where-Object {{ $_.CommandLine -like '*{needle}*' }} | \
                 ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue; $_.ProcessId }}"
            );
            match tokio::time::timeout(
                Duration::from_secs(10),
                tokio::process::Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .creation_flags(0x0800_0000)
                    .output(),
            )
            .await
            {
                Ok(Ok(out)) if !out.stdout.trim_ascii().is_empty() => {
                    tracing::warn!(
                        "reaped stale proxy core(s): {}",
                        String::from_utf8_lossy(&out.stdout).trim()
                    );
                    // Give the OS a moment to release the listening sockets.
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!("stale core reaping failed to spawn: {e}"),
                Err(_) => tracing::warn!("stale core reaping timed out"),
            }
        }
        #[cfg(not(windows))]
        {
            // On non-Windows hosts stale processes are reaped by the session
            // leader (systemd/launchd); nothing to do here.
        }
    }

    /// End-to-end liveness: TCP accept on the local inbound followed by a
    /// real HTTP request THROUGH the tunnel. A core whose inbound is alive
    /// but whose uplink is dead (remote node gone) must not count as healthy.
    async fn tunnel_probe(&self, port: u16) -> bool {
        let client = match reqwest::Client::builder()
            .proxy(match reqwest::Proxy::all(format!("http://127.0.0.1:{port}")) {
                Ok(p) => p,
                Err(_) => return false,
            })
            .timeout(Duration::from_secs(8))
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        matches!(
            tokio::time::timeout(
                Duration::from_secs(10),
                client.get("https://www.gstatic.com/generate_204").send()
            )
            .await,
            Ok(Ok(resp)) if matches!(resp.status().as_u16(), 200 | 204)
        )
    }
}

#[async_trait]
impl ProxyCoreAdapter for ExternalCoreAdapter {
    async fn start(&self, node: &Node, dns: &DnsConfig) -> SvcResult<()> {
        let mut guard = self.child.lock().await;
        if guard.is_some() {
            return Err(SvcError::AlreadyRunning("proxy core".into()));
        }
        let app_cfg = self.config.app();
        let kind = CoreKind::for_node(&app_cfg.core.kind, node);
        let (exe, work_dir, config_path) = self.resolve_paths(kind);
        if !exe.exists() {
            return Err(SvcError::Core(format!(
                "core executable not found: {} — place a {} binary there or update config/core.executable",
                exe.display(),
                match kind {
                    CoreKind::SingBox => "sing-box",
                    CoreKind::Xray => "xray-core",
                }
            )));
        }
        std::fs::create_dir_all(&work_dir).ok();

        let generated = match kind {
            CoreKind::SingBox => {
                config_builder::build_singbox_config(node, app_cfg.core.mixed_port, dns)?
            }
            CoreKind::Xray => xray_builder::build_xray_config(
                node,
                app_cfg.core.mixed_port,
                app_cfg.core.socks_port,
                dns,
            )?,
        };
        atomic_write_json(&config_path, &generated)?;

        self.status.store(STARTING, Ordering::SeqCst);

        let mut cmd = Command::new(&exe);
        cmd.arg("run").arg("-c").arg(&config_path);
        cmd.current_dir(&work_dir);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        #[cfg(windows)]
        {
            // Tokio's Windows `Command` exposes this flag setters inherently.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|e| {
            self.status.store(ERROR, Ordering::SeqCst);
            SvcError::Core(format!("failed to spawn {}: {e}", exe.display()))
        })?;
        let pid = child.id();
        self.pid.store(pid.unwrap_or(0), Ordering::SeqCst);

        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::info!(target: "core", "{line}");
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(target: "core", "{line}");
                }
            });
        }

        let ready = Self::wait_for_port(
            app_cfg.core.mixed_port,
            Duration::from_secs(app_cfg.core.start_timeout_secs),
        )
        .await;

        // The port can be held open by a STALE core left behind when a
        // previous service instance was hard-killed (kill_on_drop never ran).
        // In that case our freshly spawned core exits with a bind error while
        // wait_for_port still succeeds — verify the child actually survived.
        if let Ok(Some(status)) = child.try_wait() {
            self.status.store(ERROR, Ordering::SeqCst);
            let msg = format!(
                "core exited on startup ({status}); 127.0.0.1:{} is held by another process",
                app_cfg.core.mixed_port
            );
            tracing::error!("{msg}");
            self.bus.emit(
                ev::CORE_ERROR,
                &crate::ipc::proto::TextPayload { text: msg.clone() },
            );
            return Err(SvcError::Core(msg));
        }

        if !ready {
            let _ = child.kill().await;
            self.status.store(ERROR, Ordering::SeqCst);
            self.bus.emit(
                ev::CORE_ERROR,
                &crate::ipc::proto::TextPayload {
                    text: format!(
                        "core did not open inbound 127.0.0.1:{} within {}s",
                        app_cfg.core.mixed_port, app_cfg.core.start_timeout_secs
                    ),
                },
            );
            return Err(SvcError::Core("inbound port never opened".into()));
        }

        *guard = Some(child);
        self.status.store(RUNNING, Ordering::SeqCst);
        self.bus.emit_empty(ev::CORE_STARTED);
        tracing::info!(
            core = ?kind,
            pid = pid.unwrap_or(0),
            port = app_cfg.core.mixed_port,
            "proxy core started"
        );
        Ok(())
    }

    async fn stop(&self) {
        let mut guard = self.child.lock().await;
        if let Some(mut child) = guard.take() {
            tracing::info!("stopping proxy core");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.status.store(STOPPED, Ordering::SeqCst);
        self.pid.store(0, Ordering::SeqCst);
        self.bus.emit_empty(ev::CORE_STOPPED);
    }

    async fn restart(&self, node: &Node, dns: &DnsConfig) -> SvcResult<()> {
        self.stop().await;
        // Small settle delay so the OS can release the inbound port.
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.start(node, dns).await
    }

    fn status(&self) -> CoreStatus {
        match self.status.load(Ordering::SeqCst) {
            STARTING => CoreStatus::Starting,
            RUNNING => CoreStatus::Running,
            ERROR => CoreStatus::Error,
            _ => CoreStatus::Stopped,
        }
    }

    fn pid(&self) -> Option<u32> {
        match self.pid.load(Ordering::SeqCst) {
            0 => None,
            n => Some(n),
        }
    }

    fn inbound_port(&self) -> u16 {
        self.config.app().core.mixed_port
    }

    async fn health_check(&self) -> CoreHealth {
        if self.status.load(Ordering::SeqCst) != RUNNING {
            return CoreHealth::LocalDead;
        }
        let port = self.inbound_port();
        // Cheap local precondition first; a dead process refuses the
        // connection immediately, so this verdict needs no strike tolerance.
        let local_ok = tokio::time::timeout(
            Duration::from_secs(1),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false);
        if !local_ok {
            return CoreHealth::LocalDead;
        }
        // Inbound accepts — now prove the tunnel actually forwards traffic.
        if self.tunnel_probe(port).await {
            CoreHealth::Healthy
        } else {
            CoreHealth::UplinkDead
        }
    }
}

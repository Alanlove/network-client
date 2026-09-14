//! network-service entry point: wire every module, expose the Named Pipe
//! IPC surface, supervise background loops and shut down cleanly.

mod app {
    pub mod context;
    pub mod lifecycle;
    pub mod state;
}
mod common;
mod config {
    pub mod crypto;
    pub mod manager;
    pub mod schema;
}
mod diagnostics {
    pub mod engine;
    pub mod ping;
}
mod dns {
    pub mod cache;
    pub mod doh;
    pub mod manager;
    pub mod resolver;
    pub mod wire;
}
mod health {
    pub mod monitor;
}
mod ipc {
    pub mod events;
    pub mod frame;
    pub mod proto;
    pub mod server;
}
mod nodes {
    pub mod health;
    pub mod manager;
    pub mod model;
}
mod proxy {
    pub mod adapter;
    pub mod config_builder;
    pub mod manager;
    pub mod xray_builder;
    pub mod xray_stats;
}
mod route {
    pub mod bypass;
    pub mod manager;
    pub mod table;
}
mod routing {
    pub mod engine;
    pub mod geoip;
    pub mod matcher;
    pub mod rule;
}
mod statistics {
    pub mod database;
    pub mod manager;
}
mod subscription {
    pub mod decoder;
    pub mod downloader;
    pub mod manager;
    pub mod parser;
}
mod tun {
    pub mod adapter;
    pub mod manager;
    pub mod packet;
}

use std::sync::Arc;

use anyhow::Context;
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;

use app::context::AppContext;
use app::lifecycle::ConnectionManager;
use config::manager::ConfigManager;
use diagnostics::engine::DiagnosticEngine;
use dns::manager::DnsManager;
use ipc::events::EventBus;
use nodes::manager::NodeManager;
use proxy::manager::ExternalCoreAdapter;
use route::manager::RouteManager;
use routing::engine::RoutingEngine;
use statistics::manager::StatsManager;
use subscription::downloader::Downloader;
use subscription::manager::SubscriptionManager;
use tun::manager::TunManager;

fn init_logging(config: &ConfigManager) -> tracing_appender::non_blocking::WorkerGuard {
    let level = config.app().log_level;
    let appender = tracing_appender::rolling::daily(&config.paths.logs_dir, "service.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = tracing_subscriber::EnvFilter::try_new(&level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init()
        .ok();
    guard
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ConfigManager::load().context("load configuration")?;
    let _log_guard = init_logging(&config);

    tracing::info!(
        "network-service {} starting (root: {})",
        env!("CARGO_PKG_VERSION"),
        config.paths.root.display()
    );

    let bus = EventBus::new(256);

    // ---- domain managers ------------------------------------------------
    let nodes = Arc::new(NodeManager::load(config.clone(), bus.clone()));
    let routing = Arc::new(RoutingEngine::load(config.clone()).context("routing engine")?);
    let dns = Arc::new(DnsManager::load(config.clone(), bus.clone()));
    let stats = StatsManager::load(config.clone(), bus.clone())
        .map_err(|e| anyhow::anyhow!("statistics database: {e}"))?;
    let core = Arc::new(ExternalCoreAdapter::new(config.clone(), bus.clone()));
    let route = Arc::new(RouteManager::new(&config.paths.root));
    let tun = Arc::new(TunManager::new(config.clone()));
    let downloader = Downloader::new();
    let subscriptions = Arc::new(SubscriptionManager::load(
        config.clone(),
        nodes.clone(),
        downloader,
        bus.clone(),
    ));
    let diagnostics = Arc::new(DiagnosticEngine::new(
        bus.clone(),
        dns.clone(),
        Some(stats.db()),
    ));
    let connection = Arc::new(ConnectionManager::new(
        config.clone(),
        bus.clone(),
        nodes.clone(),
        routing.clone(),
        dns.clone(),
        core.clone(),
        route.clone(),
        tun.clone(),
        stats.clone(),
    ));

    let ctx = Arc::new(AppContext {
        config: config.clone(),
        bus: bus.clone(),
        connection: connection.clone(),
        nodes,
        subscriptions: subscriptions.clone(),
        routing,
        dns,
        stats: stats.clone(),
        core,
        route,
        tun,
        diagnostics,
        diag_running: Arc::new(Mutex::new(None)),
    });

    // Boot-time recovery: reap orphaned cores, optionally restore the
    // previous session (config.restore_previous_connection), otherwise make
    // sure no dead system proxy is left behind. Runs concurrently with IPC
    // so the pipe comes up immediately; node state is already loaded.
    {
        let connection = connection.clone();
        tokio::spawn(async move {
            connection.startup_recover().await;
        });
    }

    // ---- background loops ----------------------------------------------
    let stats_token = CancellationToken::new();
    let stats_handle = tokio::spawn(stats.clone().spawn_flush_loop(stats_token.clone()));

    // Core byte counters -> StatsManager: poll Xray's gRPC StatsService every
    // 2s while a session is active and feed the deltas into record_up/down.
    {
        let stats_bg = stats.clone();
        let poll_token = stats_token.clone();
        tokio::spawn(async move {
            let http = match reqwest::Client::builder()
                .http2_prior_knowledge()
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("stats poller: http2 client build failed: {e}");
                    return;
                }
            };
            let mut prev: Option<(u64, u64)> = None;
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = poll_token.cancelled() => break,
                }
                if !stats_bg.session_active() {
                    prev = None;
                    continue;
                }
                match proxy::xray_stats::query_totals(&http).await {
                    Ok((up, down)) => {
                        tracing::debug!("stats poll: up={up} down={down}");
                        if let Some((pu, pd)) = prev {
                            stats_bg.record_up(up.saturating_sub(pu));
                            stats_bg.record_down(down.saturating_sub(pd));
                        }
                        prev = Some((up, down));
                    }
                    Err(e) => {
                        tracing::debug!("stats poll failed: {e}");
                        prev = None;
                    }
                }
            }
        });
    }

    let subs_bg = subscriptions.clone();
    let subs_handle = tokio::spawn(async move {
        // First tick fires immediately so a freshly added subscription is
        // fetched shortly after boot; then every 10 minutes for due ones.
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(600));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            subs_bg.refresh_due().await;
        }
    });

    // ---- IPC -------------------------------------------------------------
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let ipc = tokio::spawn(async move {
        if let Err(e) = ipc::server::run(ctx, shutdown_rx).await {
            tracing::error!("ipc server exited: {e}");
        }
    });

    tokio::signal::ctrl_c().await.ok();
    tracing::info!("shutdown requested");

    // Ordered teardown.
    let _ = shutdown_tx.send(true);
    connection.disconnect().await.ok();
    stats_token.cancel();
    let _ = stats_handle.await;
    subs_handle.abort();
    ipc.abort();
    tracing::info!("network-service stopped");
    drop(_log_guard);
    Ok(())
}

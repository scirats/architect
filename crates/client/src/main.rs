mod app;
mod beacon;
mod bootstrap_manager;
mod command;
mod cross_compile;
mod event;
mod grpc;
mod health;
mod leader;
mod local_executor;
mod network_discovery;
mod task_manager;
mod tui;
mod views;
mod widgets;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use tracing::{info, warn};
use uuid::Uuid;

use app::App;
use event::EventHandler;
use health::HealthSnapshot;

#[derive(Parser)]
#[command(name = "architect-client", about = "Architect distributed computing coordinator")]
struct Cli {
    #[arg(short, long)]
    listen: Option<SocketAddr>,

    #[arg(long)]
    tick_rate: Option<u64>,

    #[arg(long, help = "Path to config file (default: ~/.config/architect/config.toml)")]
    config: Option<String>,

    #[arg(long, default_value = "9878", help = "Health/metrics HTTP port")]
    health_port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Log rotation: daily rolling logs to ~/.architect/logs/
    let log_dir = architect_core::config::log_dir();
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "client.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter("client=debug,architect_core=debug")
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();

    // Load config: --config flag > default platform path
    let config = match &cli.config {
        Some(path) => architect_core::config::load_config(path),
        None => architect_core::config::load_or_create_config(),
    };

    let config_path = cli.config
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| architect_core::config::config_path().to_string_lossy().into_owned());
    info!("Config: {}", config_path);

    // Open SQLite database
    let db = architect_core::db::open_db()?;

    // CLI flags override config values
    let listen_addr = cli.listen.unwrap_or_else(|| {
        format!("0.0.0.0:{}", config.coordinator.port).parse().unwrap()
    });
    let tick_rate = cli.tick_rate.unwrap_or(250);

    let client_id = Uuid::new_v4();
    let hostname = sysinfo::System::host_name().unwrap_or_else(|| "client".into());

    info!("Starting architect client, id={}", client_id);

    // gRPC server (replaces TCP network manager)
    let (net_event_tx, net_event_rx) = tokio::sync::mpsc::channel(256);
    let cluster_token = config.coordinator.cluster_token.clone();
    info!("Cluster token: {}...{}", &cluster_token[..8], &cluster_token[cluster_token.len()-4..]);
    let net_handle = grpc::server::start_server(listen_addr, net_event_tx, cluster_token.clone()).await?;

    // UDP beacon broadcast
    if config.discovery.beacon_enabled {
        match beacon::start_beacon_broadcast(
            config.coordinator.port,
            &cluster_token,
            hostname.clone(),
            client_id.to_string(),
            config.discovery.beacon_interval_ms,
            config.discovery.beacon_port,
        )
        .await
        {
            Ok(_handle) => info!("UDP beacon broadcasting on port {}", config.discovery.beacon_port),
            Err(e) => warn!("Failed to start beacon broadcast: {}", e),
        }
    }

    // mDNS service registration
    let _mdns_daemon = if config.discovery.mdns_enabled {
        match architect_discovery::mdns::MdnsDiscovery::new(config.coordinator.port)
            .register(&hostname, config.coordinator.port)
        {
            Ok(daemon) => {
                info!("Registered coordinator via mDNS (_architect._tcp.local.)");
                Some(daemon)
            }
            Err(e) => {
                warn!("mDNS registration failed: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Health/Prometheus HTTP server
    let health_snapshot = Arc::new(Mutex::new(HealthSnapshot::default()));
    let health_addr: SocketAddr = format!("0.0.0.0:{}", cli.health_port).parse().unwrap();
    let health_snap = health_snapshot.clone();
    tokio::spawn(async move {
        health::start_health_server(health_addr, health_snap).await;
    });

    // Event handler
    let mut events = EventHandler::new(tick_rate, net_event_rx);

    // App
    let event_tx = events.inject_tx.clone();
    let mut app = App::new(app::AppInit {
        client_id,
        client_hostname: hostname,
        network: net_handle,
        algorithm: config.scheduler.algorithm,
        discovery_config: config.discovery.clone(),
        grpc_port: config.coordinator.port,
        cluster_token: config.coordinator.cluster_token.clone(),
        event_tx,
        resource_config: config.resources.clone(),
        github_repo: config.github_repo.clone(),
        config_path: std::path::PathBuf::from(&config_path),
        db,
        health_snapshot,
    });

    // TUI
    let mut terminal = tui::init()?;

    // Initial layout
    app.terminal_size = crossterm::terminal::size()?;
    app.refresh_layout();

    // Main loop
    loop {
        terminal.draw(|frame| app.render(frame))?;

        if let Some(event) = events.next().await {
            app.update(event);
        }

        if app.should_quit {
            break;
        }
    }

    // Graceful shutdown: save task journal and broadcast shutdown to agents
    app.shutdown().await;

    tui::restore()?;
    info!("Client shutdown complete");

    Ok(())
}

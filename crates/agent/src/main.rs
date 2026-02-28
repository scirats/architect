mod battery;
mod discovery;
mod grpc_connection;
mod gpu;
mod heartbeat;
mod metrics;
mod task;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use tracing::{debug, error, info, warn};

use architect_core::protocol::{AgentMessage, ClientMessage};
use grpc_connection::GrpcConnection;
use heartbeat::heartbeat_loop;
use metrics::MetricsCollector;
use task::TaskExecutor;

#[derive(Parser)]
#[command(name = "architect-agent", about = "Architect distributed computing agent")]
struct Cli {
    /// Coordinator address (optional — agent will auto-discover via beacon/mDNS if not set)
    #[arg(short, long)]
    coordinator: Option<SocketAddr>,

    #[arg(long, default_value = "2000")]
    heartbeat_interval: u64,

    #[arg(long, default_value = "3000")]
    reconnect_delay: u64,

    /// Battery level below which tasks are rejected (0 = disabled)
    #[arg(long, default_value = "20")]
    battery_limit: u8,

    /// Cluster authentication token (required for beacon discovery filtering)
    #[arg(long, default_value = "")]
    token: String,

    /// UDP port for beacon discovery
    #[arg(long, default_value = "9877")]
    beacon_port: u16,

    /// Max connection retries before switching to discovery mode
    #[arg(long, default_value = "3")]
    max_retries: u32,

    /// Print capabilities and exit
    #[arg(long)]
    capabilities: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Log rotation: daily rolling logs
    let log_dir = architect_core::config::log_dir();
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "agent.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter("architect_agent=debug,architect_core=debug,architect_discovery=debug")
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();

    // Resolve token: CLI flag > persisted file > empty
    let data_dir = architect_core::config::data_dir();
    std::fs::create_dir_all(&data_dir).ok();
    let token_path = data_dir.join("cluster_token");
    let resolved_token = if !cli.token.is_empty() {
        // CLI provided — persist for future restarts
        let _ = std::fs::write(&token_path, &cli.token);
        cli.token.clone()
    } else if token_path.exists() {
        std::fs::read_to_string(&token_path)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        String::new()
    };

    // Resolve coordinator address: CLI flag > persisted file > None (discovery)
    let addr_path = data_dir.join("coordinator_addr");
    let resolved_coordinator: Option<SocketAddr> = if cli.coordinator.is_some() {
        // CLI provided — persist for future restarts
        if let Some(addr) = &cli.coordinator {
            let _ = std::fs::write(&addr_path, addr.to_string());
        }
        cli.coordinator
    } else if addr_path.exists() {
        std::fs::read_to_string(&addr_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
    } else {
        None
    };

    let collector = Arc::new(Mutex::new(MetricsCollector::new()));
    let mut node_info = collector.lock().unwrap().collect_info();
    node_info.auth_token = resolved_token.clone();

    // Shared mutable token for rotation
    let current_token = Arc::new(Mutex::new(resolved_token.clone()));

    if cli.capabilities {
        println!("Node ID: {}", node_info.id);
        println!("OS: {}", node_info.os);
        println!("Arch: {}", node_info.arch);
        println!("Device: {}", node_info.device_type);
        println!("CPU cores: {}", node_info.capabilities.cpu_cores);
        println!("CPU freq: {} MHz", node_info.capabilities.cpu_freq_mhz);
        println!("RAM total: {} MB", node_info.capabilities.ram_total_mb);
        println!("RAM available: {} MB", node_info.capabilities.ram_available_mb);
        println!("Compute score: {:.1}", node_info.capabilities.compute_score);
        if let Some(gpu) = &node_info.capabilities.gpu {
            println!("GPU: {} ({:.1} GB VRAM)", gpu.name, gpu.vram_gb);
        }
        if let Some(bat) = &node_info.capabilities.battery {
            println!("Battery: {}% (charging={})", bat.level_pct, bat.charging);
        }
        return Ok(());
    }

    eprintln!("[architect-agent] node={} host={}", node_info.id, node_info.hostname);
    eprintln!(
        "[architect-agent] os={} arch={} score={:.1}",
        node_info.os, node_info.arch, node_info.capabilities.compute_score
    );
    if resolved_token.is_empty() {
        eprintln!("[architect-agent] warning: no cluster token (use --token or seed via coordinator)");
    } else {
        eprintln!("[architect-agent] token={}...{}", &resolved_token[..resolved_token.len().min(8)], &resolved_token[resolved_token.len().saturating_sub(4)..]);
    }
    if let Some(addr) = &resolved_coordinator {
        eprintln!("[architect-agent] coordinator={}", addr);
    } else {
        eprintln!("[architect-agent] coordinator=auto-discover");
    }
    info!("Starting architect agent");
    info!("Node ID: {}", node_info.id);

    let node_id_str = node_info.id.to_string();

    // Auto-generate mTLS client cert if CA key is available locally
    let client_cert_path = architect_core::tls::certs_dir().join(format!("client-{}.pem", node_id_str));
    if !client_cert_path.exists() {
        let ca_key_path = architect_core::tls::certs_dir().join("ca-key.pem");
        if ca_key_path.exists() {
            match architect_core::tls::generate_client_cert(&node_id_str) {
                Ok(paths) => {
                    info!("Generated mTLS client cert: {:?}", paths.cert);
                    eprintln!("[architect-agent] generated mTLS client certificate");
                }
                Err(e) => {
                    warn!("Failed to generate client cert: {}", e);
                    eprintln!("[architect-agent] warning: could not generate client cert: {}", e);
                }
            }
        } else {
            warn!("CA key not found at {:?} — cannot generate mTLS client cert", ca_key_path);
            eprintln!("[architect-agent] warning: no CA key found, mTLS client cert not available");
        }
    } else {
        info!("mTLS client cert already exists: {:?}", client_cert_path);
    }

    let mut coordinator_addr: Option<SocketAddr> = resolved_coordinator;
    let mut retry_count = 0u32;

    // Persistent result buffer: survives reconnections
    let result_buffer = task::ResultBuffer::new();

    loop {
        // Phase 1: Resolve coordinator address
        let addr = match coordinator_addr {
            Some(addr) => addr,
            None => {
                eprintln!("[architect-agent] discovering coordinator (beacon+mDNS)...");
                info!("Entering discovery mode: beacon_port={}", cli.beacon_port);

                let token = current_token.lock().unwrap().clone();
                match discovery::discover_coordinator(&token, cli.beacon_port).await {
                    Some(addr) => {
                        eprintln!("[architect-agent] discovered coordinator: {}", addr);
                        coordinator_addr = Some(addr);
                        // Persist for future restarts
                        let _ = std::fs::write(&addr_path, addr.to_string());
                        addr
                    }
                    None => {
                        eprintln!("[architect-agent] no coordinator found, retrying...");
                        info!("Discovery timeout, will retry");
                        tokio::time::sleep(Duration::from_millis(cli.reconnect_delay)).await;
                        continue;
                    }
                }
            }
        };

        // Phase 2: Connect via gRPC (with mTLS if client cert available)
        eprintln!("[architect-agent] connecting to {} ...", addr);
        info!("Connecting to coordinator at {} via gRPC...", addr);
        let conn = GrpcConnection::new(addr).with_node_id(&node_id_str);

        match conn.connect().await {
            Ok((tx, mut rx)) => {
                retry_count = 0;
                eprintln!("[architect-agent] connected to coordinator");

                if let Err(e) = tx.send(AgentMessage::Identity(node_info.clone())).await {
                    error!("Failed to send identity: {}", e);
                    eprintln!("[architect-agent] error: failed to send identity: {}", e);
                    continue;
                }
                info!("Sent identity to coordinator");

                // Re-send any buffered results from a previous session
                let buffered = result_buffer.drain();
                if !buffered.is_empty() {
                    info!("Re-sending {} buffered results", buffered.len());
                    eprintln!("[architect-agent] re-sending {} buffered results", buffered.len());
                    if let Err(e) = tx.send(AgentMessage::BufferedResults(buffered)).await {
                        warn!("Failed to send buffered results: {}", e);
                    }
                }

                let hb_tx = tx.clone();
                let hb_collector = collector.clone();
                let hb_handle = tokio::spawn(heartbeat_loop(
                    hb_tx,
                    hb_collector,
                    cli.heartbeat_interval,
                ));

                let mut task_exec =
                    TaskExecutor::new_with_buffer(node_info.id, tx.clone(), result_buffer.clone());

                // Split-brain detection: track last coordinator ping
                let mut last_coordinator_ping = Instant::now();
                let mut partition_suspected = false;

                // Cache report: track datasets/models used by completed tasks
                let mut cached_datasets: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut cached_models: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut cache_report_ticker = tokio::time::interval(Duration::from_secs(20));

                info!("Entering message handling loop");
                loop {
                    tokio::select! {
                        msg = rx.recv() => {
                            match msg {
                                Some(ClientMessage::Identify) => {
                                    info!("Received Identify request");
                                    let info = collector.lock().unwrap().collect_info();
                                    let _ = tx.send(AgentMessage::Identity(info)).await;
                                }
                                Some(ClientMessage::RequestMetrics) => {
                                    let metrics = collector.lock().unwrap().collect_metrics();
                                    let _ = tx.send(AgentMessage::Heartbeat(metrics)).await;
                                }
                                Some(ClientMessage::RequestCapabilities) => {
                                    info!("Received RequestCapabilities");
                                    let info = collector.lock().unwrap().collect_info();
                                    let _ = tx.send(AgentMessage::Capabilities(info.capabilities)).await;
                                }
                                Some(ClientMessage::SubmitTask(task)) => {
                                    // Split-brain: reject tasks if partition suspected
                                    if partition_suspected {
                                        warn!("Rejecting task during suspected partition");
                                        let _ = tx.send(AgentMessage::Error {
                                            message: "Partition suspected, rejecting task".into(),
                                        }).await;
                                        continue;
                                    }
                                    if cli.battery_limit > 0 {
                                        if let Some(bat) = battery::detect_battery() {
                                            if battery::should_throttle(&bat, cli.battery_limit) {
                                                warn!("Rejecting task: battery {}% < limit {}%", bat.level_pct, cli.battery_limit);
                                                let _ = tx.send(AgentMessage::Error {
                                                    message: format!("Battery too low: {}%", bat.level_pct),
                                                }).await;
                                                continue;
                                            }
                                        }
                                    }
                                    // Track datasets/models for cache reporting
                                    match &task.payload {
                                        architect_core::payload::AIPayload::TrainRequest(c) => {
                                            if !c.dataset_path.is_empty() {
                                                cached_datasets.insert(c.dataset_path.clone());
                                            }
                                        }
                                        architect_core::payload::AIPayload::FinetuneRequest(c) => {
                                            if !c.dataset_path.is_empty() {
                                                cached_datasets.insert(c.dataset_path.clone());
                                            }
                                            if !c.base_model.is_empty() {
                                                cached_models.insert(c.base_model.clone());
                                            }
                                        }
                                        architect_core::payload::AIPayload::EvalRequest(c) => {
                                            if !c.model_path.is_empty() {
                                                cached_models.insert(c.model_path.clone());
                                            }
                                            if !c.dataset_path.is_empty() {
                                                cached_datasets.insert(c.dataset_path.clone());
                                            }
                                        }
                                        architect_core::payload::AIPayload::PreprocessRequest(c) => {
                                            if !c.input_path.is_empty() {
                                                cached_datasets.insert(c.input_path.clone());
                                            }
                                        }
                                        _ => {}
                                    }
                                    task_exec.execute(task).await;
                                }
                                Some(ClientMessage::CancelTask { task_id }) => {
                                    task_exec.cancel(task_id);
                                }
                                Some(ClientMessage::Shutdown) => {
                                    info!("Received shutdown command, draining tasks...");
                                    hb_handle.abort();
                                    // Wait for running tasks to finish (up to 10s)
                                    task_exec.drain(std::time::Duration::from_secs(10)).await;
                                    return Ok(());
                                }
                                Some(ClientMessage::Purge) => {
                                    warn!("Purge command received — erasing all traces");
                                    eprintln!("[architect-agent] purge: erasing all traces...");
                                    hb_handle.abort();
                                    task_exec.drain(std::time::Duration::from_secs(0)).await;

                                    let exe_path = std::env::current_exe().unwrap_or_default();
                                    let data_dir = architect_core::config::data_dir();
                                    let log_dir = architect_core::config::log_dir();
                                    let config_dir = architect_core::config::config_dir();

                                    // Delete data, logs, config
                                    let _ = std::fs::remove_dir_all(&data_dir);
                                    let _ = std::fs::remove_dir_all(&log_dir);
                                    let _ = std::fs::remove_dir_all(&config_dir);

                                    // Delete binary — on Unix the running process keeps the inode
                                    #[cfg(unix)]
                                    {
                                        let _ = std::fs::remove_file(&exe_path);
                                    }

                                    // Windows: can't delete running exe, spawn delayed cleanup
                                    #[cfg(windows)]
                                    {
                                        let exe_str = exe_path.to_string_lossy().to_string();
                                        let _ = std::process::Command::new("cmd")
                                            .args(["/c", &format!(
                                                "timeout /t 2 >nul & del \"{}\" & rmdir /s /q \"%APPDATA%\\architect\" & rmdir /s /q \"%USERPROFILE%\\.architect\"",
                                                exe_str
                                            )])
                                            .spawn();
                                    }

                                    eprintln!("[architect-agent] purge complete, exiting");
                                    std::process::exit(0);
                                }
                                Some(ClientMessage::Ping { seq }) => {
                                    last_coordinator_ping = Instant::now();
                                    if partition_suspected {
                                        info!("Partition resolved: received Ping from coordinator");
                                        partition_suspected = false;
                                    }
                                    let _ = tx.send(AgentMessage::Pong { seq }).await;
                                }
                                Some(ClientMessage::UpdateToken { new_token }) => {
                                    info!("Received token rotation");
                                    *current_token.lock().unwrap() = new_token.clone();
                                    // Persist token for next restart
                                    let token_path = architect_core::config::data_dir().join("cluster_token");
                                    let _ = std::fs::write(&token_path, &new_token);
                                    node_info.auth_token = new_token;
                                }
                                Some(ClientMessage::ApplyGradients { task_id, step, aggregated }) => {
                                    info!("Received aggregated gradients for task {} step {}: {} values", task_id, step, aggregated.len());
                                    // Gradients received — in a full implementation, these would be forwarded
                                    // to the running task via an mpsc channel for weight updates.
                                    // Currently the training loop is simulated and doesn't consume gradients.
                                }
                                Some(ClientMessage::StateSync { assigned_tasks }) => {
                                    info!("State sync request: {} assigned tasks", assigned_tasks.len());
                                    task_exec.cleanup_completed();
                                    let mut running = Vec::new();
                                    let completed = Vec::new();
                                    let mut unknown = Vec::new();
                                    for tid in assigned_tasks {
                                        if task_exec.is_running(&tid) {
                                            running.push(tid);
                                        } else {
                                            unknown.push(tid);
                                        }
                                    }
                                    let _ = tx.send(AgentMessage::StateSyncResponse { running, completed, unknown }).await;
                                }
                                Some(ClientMessage::UpdateAvailable { version, download_url }) => {
                                    info!("Update available: {} from {}", version, download_url);
                                    eprintln!("[architect-agent] update available: {}", version);
                                    // Download and restart
                                    let exe_path = std::env::current_exe().unwrap_or_default();
                                    let exe_str = exe_path.to_string_lossy().to_string();
                                    tokio::spawn(async move {
                                        let tmp_path = format!("{}.new", exe_str);
                                        let result = tokio::process::Command::new("curl")
                                            .args(["-sL", "-o", &tmp_path, &download_url])
                                            .status()
                                            .await;
                                        match result {
                                            Ok(status) if status.success() => {
                                                let _ = tokio::process::Command::new("chmod")
                                                    .args(["+x", &tmp_path])
                                                    .status()
                                                    .await;
                                                let _ = std::fs::rename(&tmp_path, &exe_str);
                                                info!("Binary updated, restarting...");
                                                eprintln!("[architect-agent] updated, restarting...");
                                                // Exec self to restart
                                                let _ = std::process::Command::new(&exe_str)
                                                    .args(std::env::args().skip(1))
                                                    .spawn();
                                                std::process::exit(0);
                                            }
                                            _ => {
                                                warn!("Failed to download update");
                                                let _ = std::fs::remove_file(&tmp_path);
                                            }
                                        }
                                    });
                                }
                                None => {
                                    warn!("Connection to coordinator lost");
                                    eprintln!("[architect-agent] connection lost");
                                    break;
                                }
                            }
                        }
                        // Split-brain: check for partition every second
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            if last_coordinator_ping.elapsed().as_secs() > 30 && !partition_suspected {
                                partition_suspected = true;
                                warn!("Suspected network partition: no Ping from coordinator in 30s");
                                eprintln!("[architect-agent] WARNING: suspected partition");
                            }
                        }
                        // Periodic cache report to coordinator
                        _ = cache_report_ticker.tick() => {
                            if !cached_datasets.is_empty() || !cached_models.is_empty() {
                                let datasets: Vec<String> = cached_datasets.iter().cloned().collect();
                                let models: Vec<String> = cached_models.iter().cloned().collect();
                                debug!("Sending cache report: {} datasets, {} models", datasets.len(), models.len());
                                let _ = tx.send(AgentMessage::CacheReport { datasets, models }).await;
                            }
                        }
                    }
                }

                hb_handle.abort();
            }
            Err(e) => {
                error!("Failed to connect: {}", e);
                eprintln!("[architect-agent] error: {}", e);
                retry_count += 1;

                if retry_count >= cli.max_retries && resolved_coordinator.is_some() {
                    eprintln!(
                        "[architect-agent] {} retries failed, switching to discovery mode",
                        retry_count
                    );
                    info!(
                        "Max retries ({}) reached for explicit address, switching to discovery",
                        cli.max_retries
                    );
                    coordinator_addr = None;
                    retry_count = 0;
                }
            }
        }

        eprintln!("[architect-agent] reconnecting in {}ms...", cli.reconnect_delay);
        info!("Reconnecting in {}ms...", cli.reconnect_delay);
        tokio::time::sleep(Duration::from_millis(cli.reconnect_delay)).await;
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use architect_core::config::{DiscoveryConfig, SchedulerAlgorithm};
use architect_core::payload::*;
use architect_core::protocol::ClientMessage;
use architect_core::types::{NodeCapabilities, NodeId, NodeInfo, NodeMetrics, NodeStatus};
use architect_discovery::DiscoveredNode;
use architect_scheduler::{
    AdaptiveScheduler, NodeState, RoundRobinScheduler, Scheduler, WeightedScheduler,
};

use crate::command::{Command, CommandBarState, CommandContext};
use crate::event::Event;
use crate::grpc::server::{NetworkEvent, NetworkManagerHandle};
use crate::local_executor::LocalTaskExecutor;
use crate::network_discovery::{self, BootstrapEvent, DiscoveryEvent};
use crate::task_manager::TaskManager;
use crate::views::info::InfoState;
use crate::views::workspace::{WorkspaceAction, WorkspaceState};
use crate::widgets::activity_log::{render_activity_log, ActivityLevel};
use crate::widgets::command_bar::render_command_bar;
use crate::widgets::graph::render_graph;
use crate::widgets::metrics_panel::render_metrics_panel;
use crate::widgets::task_list::render_task_list;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveView {
    Workspace,
    Info { node_id: NodeId },
}

/// Max number of historical metric snapshots per agent (~12 min at 2s heartbeat).
const MAX_METRICS_HISTORY: usize = 360;

/// Detect the local subnet by probing the default network interface.
/// Returns something like "192.168.1.0/24".
fn detect_local_subnet() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let local_ip = socket.local_addr().ok()?.ip();
    match local_ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            Some(format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2]))
        }
        _ => None,
    }
}

pub struct AgentState {
    pub info: NodeInfo,
    pub metrics: Option<NodeMetrics>,
    pub status: NodeStatus,
    pub last_heartbeat: Instant,
    pub active_task_count: u32,
    /// Timestamped ring buffer of metric snapshots.
    pub metrics_history: std::collections::VecDeque<(u64, NodeMetrics)>,
    /// Cached datasets on this agent (for data locality scheduling).
    pub cached_datasets: Vec<String>,
    /// Cached models on this agent.
    pub cached_models: Vec<String>,
}

pub struct App {
    pub active_view: ActiveView,
    pub workspace: WorkspaceState,
    pub info_views: HashMap<NodeId, InfoState>,
    pub agents: HashMap<NodeId, AgentState>,
    pub client_id: NodeId,
    pub client_hostname: String,
    pub command_bar: CommandBarState,
    pub network: NetworkManagerHandle,
    pub task_manager: TaskManager,
    pub scheduler: Box<dyn Scheduler>,
    pub should_quit: bool,
    pub terminal_size: (u16, u16),
    // Discovery/bootstrap state
    pub discovered_devices: Vec<DiscoveredNode>,
    pub discovery_config: DiscoveryConfig,
    pub grpc_port: u16,
    pub cluster_token: String,
    pub event_tx: mpsc::UnboundedSender<Event>,
    pub github_repo: String,
    // Local execution: client participates as a worker node
    pub local_executor: LocalTaskExecutor,
    pub client_capabilities: NodeCapabilities,
    // Fault tolerance: periodic journal save
    ticks_since_save: u32,
    // Config hot reload
    config_path: PathBuf,
    last_config_mtime: Option<SystemTime>,
    // SQLite database
    db: rusqlite::Connection,
    // Health/metrics snapshot
    health_snapshot: std::sync::Arc<std::sync::Mutex<crate::health::HealthSnapshot>>,
    // Split-brain: ping sequence counter and last pong tracking
    ping_seq: u64,
    last_pong: HashMap<NodeId, Instant>,
    // Heartbeat counter per agent (for periodic DB metrics writes)
    heartbeat_counter: HashMap<NodeId, u32>,
    // App start time for uptime tracking
    start_time: Instant,
    // Leader election for multi-coordinator HA
    leader: crate::leader::LeaderElection,
    leader_tick: u32,
    // Gradient aggregation buffers: (task_id, step) -> Vec<(shard_index, data)>
    #[allow(clippy::type_complexity)]
    gradient_buffers: HashMap<(Uuid, u64), Vec<(u32, Vec<f32>)>>,
    /// Expected shard count per task (number of agents participating in training).
    gradient_expected_shards: HashMap<Uuid, usize>,
}

/// Initialization parameters for App.
pub struct AppInit {
    pub client_id: NodeId,
    pub client_hostname: String,
    pub network: NetworkManagerHandle,
    pub algorithm: SchedulerAlgorithm,
    pub discovery_config: DiscoveryConfig,
    pub grpc_port: u16,
    pub cluster_token: String,
    pub event_tx: mpsc::UnboundedSender<Event>,
    pub resource_config: architect_core::config::ResourceConfig,
    pub github_repo: String,
    pub config_path: PathBuf,
    pub db: rusqlite::Connection,
    pub health_snapshot: std::sync::Arc<std::sync::Mutex<crate::health::HealthSnapshot>>,
}

impl App {
    pub fn new(init: AppInit) -> Self {
        let scheduler: Box<dyn Scheduler> = match init.algorithm {
            SchedulerAlgorithm::RoundRobin => Box::new(RoundRobinScheduler::new()),
            SchedulerAlgorithm::Weighted => Box::new(WeightedScheduler),
            SchedulerAlgorithm::Adaptive => Box::new(AdaptiveScheduler),
        };

        // Detect local hardware capabilities
        let sys = sysinfo::System::new_all();
        let mut client_capabilities = NodeCapabilities {
            cpu_cores: sys.cpus().len() as u32,
            cpu_freq_mhz: sys.cpus().first().map(|c| c.frequency() as u32).unwrap_or(0),
            ram_total_mb: sys.total_memory() / (1024 * 1024),
            ram_available_mb: sys.available_memory() / (1024 * 1024),
            gpu: None,
            battery: None,
            compute_score: 0.0,
        };
        client_capabilities.compute_score = client_capabilities.calculate_score();

        let local_executor = LocalTaskExecutor::new(
            init.client_id,
            init.event_tx.clone(),
            init.resource_config,
        );

        // Load tasks from SQLite (falls back to JSON journal for migration)
        let mut task_manager = TaskManager::new();
        match task_manager.load_from_db(&init.db) {
            Ok(0) => {
                // Try legacy JSON journal as fallback
                let journal_path = architect_core::config::data_dir().join("task_journal.json");
                match task_manager.load_from_file(&journal_path) {
                    Ok(0) => {}
                    Ok(n) => info!("Recovered {} tasks from legacy journal", n),
                    Err(e) => warn!("Failed to load legacy task journal: {}", e),
                }
            }
            Ok(n) => info!("Recovered {} tasks from database", n),
            Err(e) => warn!("Failed to load tasks from database: {}", e),
        }

        Self {
            active_view: ActiveView::Workspace,
            workspace: WorkspaceState::new(init.client_id),
            info_views: HashMap::new(),
            agents: HashMap::new(),
            client_id: init.client_id,
            client_hostname: init.client_hostname,
            command_bar: CommandBarState::new(),
            network: init.network,
            task_manager,
            scheduler,
            should_quit: false,
            terminal_size: (80, 24),
            discovered_devices: Vec::new(),
            discovery_config: init.discovery_config,
            grpc_port: init.grpc_port,
            cluster_token: init.cluster_token,
            event_tx: init.event_tx,
            github_repo: init.github_repo,
            local_executor,
            client_capabilities,
            ticks_since_save: 0,
            last_config_mtime: std::fs::metadata(&init.config_path)
                .and_then(|m| m.modified())
                .ok(),
            leader: {
                let db_path = architect_core::config::data_dir().join("architect.db");
                let mut le = crate::leader::LeaderElection::new(
                    db_path,
                    init.client_id.to_string(),
                );
                let _ = le.try_acquire();
                le
            },
            config_path: init.config_path,
            db: init.db,
            health_snapshot: init.health_snapshot,
            ping_seq: 0,
            last_pong: HashMap::new(),
            heartbeat_counter: HashMap::new(),
            start_time: Instant::now(),
            leader_tick: 0,
            gradient_buffers: HashMap::new(),
            gradient_expected_shards: HashMap::new(),
        }
    }

    pub fn update(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Resize(w, h) => {
                self.terminal_size = (w, h);
                self.refresh_layout();
            }
            Event::Tick => self.tick(),
            Event::Network(net) => self.handle_network(net),
            Event::Discovery(d) => self.handle_discovery(d),
            Event::Bootstrap(b) => self.handle_bootstrap(b),
            Event::LocalTaskProgress { task_id, progress_pct } => {
                debug!("Local task progress: {}={:.1}%", task_id, progress_pct);
                self.task_manager.update_progress(task_id, progress_pct, None);
                self.workspace.append_activity(
                    format!("local task {:.0}%", progress_pct),
                    ActivityLevel::Info,
                );
            }
            Event::LocalTaskResult(result) => {
                let tid = result.task_id;
                let success = result.success;
                let duration = result.duration_ms;
                info!("Local task result: task={} success={}", tid, success);
                let level = if success { ActivityLevel::Success } else { ActivityLevel::Error };
                self.workspace.append_activity(
                    format!(
                        "local task {} ({}ms)",
                        if success { "completed" } else { "failed" },
                        duration,
                    ),
                    level,
                );
                self.task_manager.complete(tid, result);
                if let Some(nv) = self.info_views.get_mut(&self.client_id) {
                    nv.append_log(format!(
                        "[local task {}] {}",
                        &tid.to_string()[..8],
                        if success { "completed" } else { "failed" }
                    ));
                }
            }
            Event::TaskSendFailed { task_id, node_id } => {
                info!("Task send failed for {} to {}, reverting to pending", task_id, node_id);
                self.task_manager.revert_to_pending(task_id);
                if let Some(agent) = self.agents.get_mut(&node_id) {
                    agent.active_task_count = agent.active_task_count.saturating_sub(1);
                }
                self.workspace.append_activity(
                    format!("{} send failed, re-queued", &task_id.to_string()[..8]),
                    ActivityLevel::Warning,
                );
            }
            Event::Activity { message, level } => {
                self.workspace.append_activity(message, level);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Command bar takes priority when active
        if self.command_bar.active {
            match key.code {
                KeyCode::Esc => {
                    if self.command_bar.popup_open {
                        self.command_bar.close_popup();
                    } else {
                        self.command_bar.deactivate();
                    }
                }
                KeyCode::Enter => {
                    if let Some(cmd) = self.command_bar.submit() {
                        self.execute_command(cmd);
                    }
                }
                KeyCode::Backspace => self.command_bar.delete_char(),
                KeyCode::Tab => self.command_bar.tab_complete(),
                KeyCode::Up => self.command_bar.popup_up(),
                KeyCode::Down => self.command_bar.popup_down(),
                KeyCode::Char(c) => self.command_bar.insert_char(c),
                _ => {}
            }
            return;
        }

        // Delegate to active view
        match self.active_view.clone() {
            ActiveView::Workspace => {
                // ':' activates command bar
                if key.code == KeyCode::Char(':') {
                    self.command_bar.set_context(CommandContext::Workspace);
                    self.command_bar.activate();
                    return;
                }
                let action = self.workspace.handle_key(key);
                match action {
                    WorkspaceAction::OpenInfo(id) => {
                        info!("Opening info for node {}", id);
                        self.open_info(id);
                    }
                    WorkspaceAction::None => {}
                }
            }
            ActiveView::Info { .. } => {
                // Esc goes back to workspace
                if key.code == KeyCode::Esc {
                    self.active_view = ActiveView::Workspace;
                }
            }
        }
    }

    fn execute_command(&mut self, cmd: Command) {
        match cmd {
            Command::Info { node_id, prefix } => {
                let id = if let Some(id) = node_id {
                    id
                } else if let Some(pfx) = prefix {
                    match self.find_agent_by_prefix(&pfx) {
                        Some(id) => id,
                        None => {
                            self.workspace.append_activity(
                                format!("no agent matching '{}'", pfx),
                                ActivityLevel::Warning,
                            );
                            return;
                        }
                    }
                } else {
                    self.client_id
                };
                self.open_info(id);
            }
            Command::Quit => {
                self.should_quit = true;
            }

            // --- ML/AI commands ---
            Command::Train(args) => {
                let quota = if args.max_ram_mb.is_some() || args.max_cpu_pct.is_some() {
                    Some(ResourceQuota {
                        max_cpu_pct: args.max_cpu_pct,
                        max_ram_mb: args.max_ram_mb,
                    })
                } else {
                    None
                };
                let config = TrainConfig {
                    dataset_path: args.dataset,
                    epochs: args.epochs.unwrap_or(3),
                    learning_rate: args.lr.unwrap_or(0.001),
                    batch_size: args.batch_size.unwrap_or(32),
                    output_path: args.output.unwrap_or_else(|| "output".into()),
                    resume_from: None,
                };
                self.dispatch_task_with_quota(TaskType::Train, AIPayload::TrainRequest(config), quota);
            }
            Command::Finetune(args) => {
                let quota = if args.max_ram_mb.is_some() || args.max_cpu_pct.is_some() {
                    Some(ResourceQuota {
                        max_cpu_pct: args.max_cpu_pct,
                        max_ram_mb: args.max_ram_mb,
                    })
                } else {
                    None
                };
                let config = FinetuneConfig {
                    base_model: args.base_model,
                    dataset_path: args.dataset,
                    epochs: args.epochs.unwrap_or(3),
                    learning_rate: args.lr.unwrap_or(0.0001),
                    batch_size: args.batch_size.unwrap_or(16),
                    output_path: args.output.unwrap_or_else(|| "output".into()),
                    adapter: args.adapter.unwrap_or_else(|| "lora".into()),
                    resume_from: None,
                };
                self.dispatch_task_with_quota(TaskType::Finetune, AIPayload::FinetuneRequest(config), quota);
            }
            Command::Infer(args) => {
                let config = InferenceConfig {
                    max_tokens: args.max_tokens.unwrap_or(256),
                    temperature: args.temperature.unwrap_or(0.7),
                    top_p: 0.9,
                };
                let payload = AIPayload::InferenceRequest {
                    prompt: args.prompt.into_bytes(),
                    config,
                };
                self.dispatch_task(TaskType::Inference, payload);
            }
            Command::Preprocess(args) => {
                let config = PreprocessConfig {
                    input_path: args.input,
                    output_path: args.output.unwrap_or_else(|| "output".into()),
                    format: args.format.unwrap_or_else(|| "tokenize".into()),
                };
                self.dispatch_task(TaskType::Preprocess, AIPayload::PreprocessRequest(config));
            }
            Command::Evaluate(args) => {
                let config = EvalConfig {
                    model_path: args.model,
                    dataset_path: args.dataset,
                    metrics: args.metrics,
                    output_path: args.output,
                };
                self.dispatch_task(TaskType::Evaluate, AIPayload::EvalRequest(config));
            }
            Command::Rag(args) => {
                let config = RagConfig {
                    query: args.query,
                    index_path: args.index,
                    model_path: args.model,
                    top_k: args.top_k.unwrap_or(5),
                };
                self.dispatch_task(TaskType::Rag, AIPayload::RagRequest(config));
            }
            Command::Embed(args) => {
                let config = EmbedConfig {
                    input: args.input,
                    model_path: args.model,
                    index_path: args.index,
                    operation: args.operation.unwrap_or_else(|| "encode".into()),
                };
                self.dispatch_task(TaskType::Embed, AIPayload::EmbedRequest(config));
            }

            // --- Management commands ---
            Command::Cancel { task_id } => {
                if let Some(tid) = self.task_manager.find_by_prefix(&task_id) {
                    self.task_manager.cancel(tid);
                    self.local_executor.cancel(tid);
                    let short = tid.to_string()[..8].to_string();
                    let net = self.network.clone();
                    let tx = self.event_tx.clone();
                    let short2 = short.clone();
                    tokio::spawn(async move {
                        match net.broadcast(ClientMessage::CancelTask { task_id: tid }).await {
                            Ok(_) => {
                                let _ = tx.send(Event::Activity {
                                    message: format!("[cancel] {} cancelled and broadcast to agents", short2),
                                    level: ActivityLevel::Warning,
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(Event::Activity {
                                    message: format!("[cancel] {} cancelled locally, broadcast failed: {}", short2, e),
                                    level: ActivityLevel::Error,
                                });
                            }
                        }
                    });
                } else {
                    self.workspace.append_activity(
                        format!("Task not found: {}", task_id),
                        ActivityLevel::Warning,
                    );
                }
            }
            // Discovery/Bootstrap commands
            Command::Pulse { subnet } => {
                let subnet = subnet
                    .or_else(|| {
                        let s = &self.discovery_config.scan_subnet;
                        if s.is_empty() { None } else { Some(s.clone()) }
                    })
                    .unwrap_or_else(|| detect_local_subnet().unwrap_or_else(|| "192.168.1.0/24".into()));

                self.workspace.append_activity(
                    format!("[pulse] scanning {}...", subnet),
                    ActivityLevel::Info,
                );

                let (disc_tx, mut disc_rx) = mpsc::channel::<DiscoveryEvent>(64);
                let event_tx = self.event_tx.clone();
                let port = self.grpc_port;

                tokio::spawn(async move {
                    network_discovery::run_scan(subnet, port, disc_tx).await;
                });

                tokio::spawn(async move {
                    while let Some(evt) = disc_rx.recv().await {
                        if event_tx.send(Event::Discovery(evt)).is_err() {
                            break;
                        }
                    }
                });
            }
            Command::Seed { target, user, pass } => {
                let ip: std::net::IpAddr = match target.parse() {
                    Ok(ip) => ip,
                    Err(_) => {
                        self.workspace.append_activity(
                            format!("Invalid IP: {}", target),
                            ActivityLevel::Error,
                        );
                        return;
                    }
                };

                let node = self.discovered_devices.iter().find(|n| n.ip == ip).cloned();
                let node = match node {
                    Some(n) => n,
                    None => {
                        // Allow seeding even to non-discovered hosts
                        DiscoveredNode {
                            ip,
                            hostname: None,
                            open_ports: vec![],
                            device_type: architect_core::types::DeviceType::Desktop,
                            has_agent: false,
                            os_hint: None,
                        }
                    }
                };

                // Build coordinator address for the remote agent
                let coordinator_addr = {
                    let local_ip = std::net::UdpSocket::bind("0.0.0.0:0")
                        .and_then(|s| { s.connect("8.8.8.8:80")?; s.local_addr() })
                        .map(|a| a.ip().to_string())
                        .unwrap_or_else(|_| "127.0.0.1".to_string());
                    format!("{}:{}", local_ip, self.grpc_port)
                };

                crate::bootstrap_manager::spawn_deploy(
                    node,
                    user.clone(),
                    pass,
                    self.github_repo.clone(),
                    self.cluster_token.clone(),
                    coordinator_addr,
                    self.event_tx.clone(),
                );
            }
            Command::Bless => {
                let new_token = Uuid::new_v4().to_string();
                info!("Rotating cluster token");
                self.cluster_token = new_token.clone();

                let net = self.network.clone();
                let token = new_token.clone();
                let tx = self.event_tx.clone();
                let short = new_token[..8].to_string();
                tokio::spawn(async move {
                    match net.broadcast(ClientMessage::UpdateToken { new_token: token }).await {
                        Ok(_) => {
                            let _ = tx.send(Event::Activity {
                                message: format!("[token] rotated to {}... and broadcast to all agents", short),
                                level: ActivityLevel::Success,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Activity {
                                message: format!("[token] rotated to {}... but broadcast failed: {}", short, e),
                                level: ActivityLevel::Error,
                            });
                        }
                    }
                });

                let _ = architect_core::db::audit(
                    &self.db, &self.client_id.to_string(), "token_rotated", "", "",
                );

                self.workspace.append_activity(
                    format!("[token] rotating cluster token to {}...", &new_token[..8]),
                    ActivityLevel::Warning,
                );
            }
            Command::Ascend { url } => {
                match url {
                    Some(url) => {
                        let net = self.network.clone();
                        let url_clone = url.clone();
                        let tx = self.event_tx.clone();
                        self.workspace.append_activity(
                            format!("[ascend] broadcasting {} to all agents...", url),
                            ActivityLevel::Info,
                        );
                        tokio::spawn(async move {
                            match net.broadcast(ClientMessage::UpdateAvailable {
                                version: "latest".into(),
                                download_url: url_clone.clone(),
                            }).await {
                                Ok(_) => {
                                    let _ = tx.send(Event::Activity {
                                        message: format!("[ascend] broadcast sent: {}", url_clone),
                                        level: ActivityLevel::Success,
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send(Event::Activity {
                                        message: format!("[ascend] broadcast failed: {}", e),
                                        level: ActivityLevel::Error,
                                    });
                                }
                            }
                        });
                    }
                    None => {
                        let mut sent = 0usize;
                        let mut skipped = Vec::new();
                        let mut sent_to = Vec::new();
                        for (id, agent) in &self.agents {
                            if agent.status != NodeStatus::Online {
                                continue;
                            }
                            let target = match crate::cross_compile::target_for_platform(agent.info.os, agent.info.arch) {
                                Some(t) => t,
                                None => {
                                    skipped.push(format!("{} (unknown platform)", agent.info.hostname));
                                    continue;
                                }
                            };
                            let label = crate::cross_compile::label_for_target(target);
                            let download_url = crate::cross_compile::github_agent_url(&self.github_repo, label);
                            let net = self.network.clone();
                            let node_id = *id;
                            let hostname = agent.info.hostname.clone();
                            let lbl = label.to_string();
                            let tx = self.event_tx.clone();
                            tokio::spawn(async move {
                                match net.send_to(node_id, ClientMessage::UpdateAvailable {
                                    version: "latest".into(),
                                    download_url: download_url.clone(),
                                }).await {
                                    Ok(_) => {
                                        let _ = tx.send(Event::Activity {
                                            message: format!("[ascend] {} <- {} sent", hostname, lbl),
                                            level: ActivityLevel::Success,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Event::Activity {
                                            message: format!("[ascend] {} <- failed: {}", hostname, e),
                                            level: ActivityLevel::Error,
                                        });
                                    }
                                }
                            });
                            sent_to.push(format!("{} ({})", agent.info.hostname, label));
                            sent += 1;
                        }
                        if sent > 0 {
                            for name in &sent_to {
                                self.workspace.append_activity(
                                    format!("[ascend] sending to {}", name),
                                    ActivityLevel::Info,
                                );
                            }
                        } else if skipped.is_empty() {
                            self.workspace.append_activity(
                                "No online agents to update".into(),
                                ActivityLevel::Warning,
                            );
                        }
                        for name in &skipped {
                            self.workspace.append_activity(
                                format!("[ascend] skipped {}", name),
                                ActivityLevel::Warning,
                            );
                        }
                    }
                }
            }
            Command::Purge { target } => {
                if target == "all" {
                    let online: Vec<(NodeId, String)> = self.agents.iter()
                        .filter(|(_, a)| a.status == NodeStatus::Online)
                        .map(|(id, a)| (*id, a.info.hostname.clone()))
                        .collect();
                    if online.is_empty() {
                        self.workspace.append_activity(
                            "No online agents to purge".into(),
                            ActivityLevel::Warning,
                        );
                    } else {
                        for (node_id, hostname) in &online {
                            let net = self.network.clone();
                            let nid = *node_id;
                            let host = hostname.clone();
                            let tx = self.event_tx.clone();
                            tokio::spawn(async move {
                                match net.send_to(nid, ClientMessage::Purge).await {
                                    Ok(_) => {
                                        let _ = tx.send(Event::Activity {
                                            message: format!("[purge] {} <- sent", host),
                                            level: ActivityLevel::Success,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Event::Activity {
                                            message: format!("[purge] {} <- failed: {}", host, e),
                                            level: ActivityLevel::Error,
                                        });
                                    }
                                }
                            });
                            self.workspace.append_activity(
                                format!("[purge] sending to {}", hostname),
                                ActivityLevel::Info,
                            );
                        }
                    }
                } else {
                    // Find agent by node_id prefix
                    let found = self.agents.iter()
                        .find(|(id, _)| id.to_string().starts_with(&target))
                        .map(|(id, a)| (*id, a.info.hostname.clone()));
                    match found {
                        Some((node_id, hostname)) => {
                            let net = self.network.clone();
                            let tx = self.event_tx.clone();
                            let host = hostname.clone();
                            tokio::spawn(async move {
                                match net.send_to(node_id, ClientMessage::Purge).await {
                                    Ok(_) => {
                                        let _ = tx.send(Event::Activity {
                                            message: format!("[purge] {} <- sent", host),
                                            level: ActivityLevel::Success,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Event::Activity {
                                            message: format!("[purge] {} <- failed: {}", host, e),
                                            level: ActivityLevel::Error,
                                        });
                                    }
                                }
                            });
                            self.workspace.append_activity(
                                format!("[purge] sending to {}", hostname),
                                ActivityLevel::Info,
                            );
                        }
                        None => {
                            self.workspace.append_activity(
                                format!("[purge] no agent matching '{}'", target),
                                ActivityLevel::Warning,
                            );
                        }
                    }
                }
            }
            Command::Plugin { name, config } => {
                self.dispatch_task(TaskType::Plugin, AIPayload::Plugin { name: name.clone(), config: config.clone() });
                self.workspace.append_activity(
                    format!("[plugin] '{}' dispatched with config: {}", name, &config[..config.len().min(60)]),
                    ActivityLevel::Info,
                );
            }
            Command::Unknown(s) => {
                self.workspace.append_activity(
                    format!("Unknown command: {}", s),
                    ActivityLevel::Warning,
                );
            }
        }
    }

    /// Build an AITask, pick a node via scheduler (client + agents), execute.
    fn dispatch_task(&mut self, task_type: TaskType, payload: AIPayload) {
        self.dispatch_task_with_quota(task_type, payload, None);
    }

    /// Build an AITask with optional resource quota, pick a node via scheduler, execute.
    fn dispatch_task_with_quota(&mut self, task_type: TaskType, payload: AIPayload, quota: Option<ResourceQuota>) {
        if !self.leader.is_leader() {
            self.workspace.append_activity(
                "Standby mode: not the active leader".into(),
                ActivityLevel::Warning,
            );
            return;
        }

        self.local_executor.cleanup_completed();

        let task_id = Uuid::new_v4();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let task = AITask {
            task_id,
            task_type,
            payload,
            priority: 5,
            created_at: now,
            timeout_ms: 300_000,
            depends_on: Vec::new(),
            quota,
        };

        // Register expected gradient shards for training tasks
        if task_type == TaskType::Train {
            let online_agents = self.agents.values().filter(|a| a.status == NodeStatus::Online).count();
            // 1 (client) + online agents — each participant sends one shard per step
            let expected = 1 + online_agents;
            self.gradient_expected_shards.insert(task_id, expected);
        }

        // Build node states: client itself + all online agents
        let mut node_states: Vec<NodeState> = Vec::new();

        // Client always participates as a worker
        node_states.push(NodeState {
            id: self.client_id,
            capabilities: self.client_capabilities.clone(),
            metrics: None,
            current_tasks: self.local_executor.active_count(),
            has_cached_data: false,
            cached_datasets: Vec::new(),
        });

        // Add online agents
        for (id, a) in &self.agents {
            if a.status == NodeStatus::Online {
                node_states.push(NodeState {
                    id: *id,
                    capabilities: a.info.capabilities.clone(),
                    metrics: a.metrics.clone(),
                    current_tasks: a.active_task_count,
                    has_cached_data: !a.cached_datasets.is_empty(),
                    cached_datasets: a.cached_datasets.clone(),
                });
            }
        }

        match self.scheduler.select_node(&task, &node_states) {
            Some(node_id) if node_id == self.client_id => {
                // Execute locally on the client
                let short_id = &task_id.to_string()[..8];
                let algo = self.scheduler.name();

                info!(
                    "Executing task {} ({}) locally via {}",
                    short_id, task_type, algo
                );

                self.task_manager.submit(task.clone());
                self.task_manager.assign(task_id, self.client_id);
                let _ = architect_core::db::audit(
                    &self.db, &self.client_id.to_string(), "task_dispatched",
                    &task_id.to_string(), "local",
                );

                if let Err(reason) = self.local_executor.execute(task) {
                    self.task_manager.cancel(task_id);
                    self.workspace.append_activity(
                        format!("Throttled: {}", reason),
                        ActivityLevel::Warning,
                    );
                    return;
                }

                self.workspace.append_activity(
                    format!("{} {} executing locally [{}]", short_id, task_type, algo),
                    ActivityLevel::Info,
                );
            }
            Some(node_id) => {
                // Delegate to a remote agent
                let short_id = &task_id.to_string()[..8];
                let hostname = self
                    .agents
                    .get(&node_id)
                    .map(|a| a.info.hostname.clone())
                    .unwrap_or_else(|| "?".into());
                let algo = self.scheduler.name();

                info!(
                    "Dispatching task {} ({}) to {} ({}) via {}",
                    short_id, task_type, hostname, node_id, algo
                );

                self.task_manager.submit(task.clone());
                self.task_manager.assign(task_id, node_id);
                let _ = architect_core::db::audit(
                    &self.db, &self.client_id.to_string(), "task_dispatched",
                    &task_id.to_string(), &hostname,
                );

                if let Some(agent) = self.agents.get_mut(&node_id) {
                    agent.active_task_count += 1;
                }

                // Atomic: if send fails, inject an event to revert the assignment
                let net = self.network.clone();
                let msg = ClientMessage::SubmitTask(task);
                let event_tx = self.event_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = net.send_to(node_id, msg).await {
                        warn!("Failed to send task to {}: {} — reverting assignment", node_id, e);
                        let _ = event_tx.send(Event::TaskSendFailed { task_id, node_id });
                    }
                });

                self.workspace.append_activity(
                    format!("{} {} assigned to {} [{}]", short_id, task_type, hostname, algo),
                    ActivityLevel::Info,
                );
            }
            None => {
                self.workspace.append_activity(
                    "Scheduler could not select a node".into(),
                    ActivityLevel::Error,
                );
            }
        }
    }

    fn find_agent_by_prefix(&self, prefix: &str) -> Option<NodeId> {
        let prefix_lower = prefix.to_lowercase();
        let matches: Vec<NodeId> = self
            .agents
            .keys()
            .filter(|id| id.to_string().starts_with(&prefix_lower))
            .copied()
            .collect();
        if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        }
    }

    fn open_info(&mut self, node_id: NodeId) {
        self.info_views.entry(node_id).or_insert_with(|| {
            info!("Creating info view for node {}", node_id);
            InfoState::new()
        });
        self.active_view = ActiveView::Info { node_id };
    }

    fn handle_network(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::AgentConnected { id, info } => {
                let hostname = info.hostname.clone();
                info!("Agent connected: {} ({}) from {}", hostname, id, info.ip);
                let _ = architect_core::db::audit(
                    &self.db, &id.to_string(), "agent_connected", &hostname, &info.ip,
                );
                let _ = architect_core::db::upsert_identity(&self.db, &id.to_string(), &hostname);
                self.last_pong.insert(id, Instant::now());
                self.workspace.append_activity(
                    format!("{} {} connected", &id.to_string()[..8], hostname),
                    ActivityLevel::Success,
                );
                self.agents.insert(
                    id,
                    AgentState {
                        info,
                        metrics: None,
                        status: NodeStatus::Online,
                        last_heartbeat: Instant::now(),
                        active_task_count: 0,
                        metrics_history: std::collections::VecDeque::new(),
                        cached_datasets: Vec::new(),
                        cached_models: Vec::new(),
                    },
                );
                self.refresh_layout();
                if let Some(nv) = self.info_views.get_mut(&id) {
                    nv.append_log(format!("[connected] {}", hostname));
                }
                // State sync: send assigned tasks to newly connected agent
                let assigned = self.task_manager.tasks_assigned_to(id);
                if !assigned.is_empty() {
                    info!("Sending state sync to {}: {} tasks", id, assigned.len());
                    let net = self.network.clone();
                    tokio::spawn(async move {
                        let _ = net.send_to(id, ClientMessage::StateSync { assigned_tasks: assigned }).await;
                    });
                }
            }
            NetworkEvent::AgentDisconnected { id } => {
                let hostname = self.agents.get(&id).map(|a| a.info.hostname.clone()).unwrap_or_default();
                let display_name = if hostname.is_empty() { id.to_string()[..8].to_string() } else { hostname.clone() };
                info!("Agent disconnected: {}", id);
                let _ = architect_core::db::audit(
                    &self.db, &id.to_string(), "agent_disconnected", &display_name, "",
                );
                self.workspace.append_activity(
                    format!("{} {} disconnected", &id.to_string()[..8], display_name),
                    ActivityLevel::Warning,
                );
                if let Some(agent) = self.agents.get_mut(&id) {
                    agent.status = NodeStatus::Offline;
                }

                // Re-queue orphaned tasks from this agent
                let orphaned = self.task_manager.tasks_assigned_to(id);
                for task_id in &orphaned {
                    let short = &task_id.to_string()[..8];
                    if self.requeue_with_checkpoint(*task_id) {
                        self.workspace.append_activity(
                            format!("{} task {} re-queued", &id.to_string()[..8], short),
                            ActivityLevel::Warning,
                        );
                    } else {
                        self.workspace.append_activity(
                            format!("{} task {} max retries exceeded", &id.to_string()[..8], short),
                            ActivityLevel::Error,
                        );
                    }
                }

                self.refresh_layout();
                if let Some(nv) = self.info_views.get_mut(&id) {
                    nv.append_log("[disconnected]".into());
                    nv.active = false;
                }
            }
            NetworkEvent::Heartbeat { id, metrics } => {
                debug!("Heartbeat from {}: cpu={:.1}% ram={:.1}%", id, metrics.cpu_usage_pct, metrics.ram_usage_pct);
                if let Some(agent) = self.agents.get_mut(&id) {
                    // Push to history ring buffer
                    let ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    agent.metrics_history.push_back((ts, metrics.clone()));
                    if agent.metrics_history.len() > MAX_METRICS_HISTORY {
                        agent.metrics_history.pop_front();
                    }
                    agent.metrics = Some(metrics.clone());
                    agent.status = NodeStatus::Online;
                    agent.last_heartbeat = Instant::now();

                    // Every 10 heartbeats, persist metrics to DB
                    let counter = self.heartbeat_counter.entry(id).or_insert(0);
                    *counter += 1;
                    if *counter >= 10 {
                        *counter = 0;
                        let _ = architect_core::db::insert_metrics(
                            &self.db,
                            &architect_core::db::MetricsRow {
                                node_id: &id.to_string(),
                                ts,
                                cpu: metrics.cpu_usage_pct,
                                ram: metrics.ram_usage_pct,
                                gpu: metrics.gpu_usage_pct,
                                tasks_completed: metrics.tasks_completed,
                                tasks_failed: metrics.tasks_failed,
                            },
                        );
                    }
                }
            }
            NetworkEvent::TaskResult { id, result } => {
                let tid = result.task_id;
                let success = result.success;
                let duration = result.duration_ms;
                let hostname = self.agents.get(&id).map(|a| a.info.hostname.as_str()).unwrap_or("?");
                info!("Task result from {}: task={} success={}", id, tid, success);
                let action = if success { "task_completed" } else { "task_failed" };
                let _ = architect_core::db::audit(
                    &self.db, &id.to_string(), action, &tid.to_string(),
                    &format!("duration={}ms", duration),
                );
                let level = if success { ActivityLevel::Success } else { ActivityLevel::Error };
                self.workspace.append_activity(
                    format!(
                        "{} task {} on {} ({}ms)",
                        &id.to_string()[..8],
                        if success { "completed" } else { "failed" },
                        hostname,
                        duration,
                    ),
                    level,
                );
                self.task_manager.complete(tid, result);
                if let Some(agent) = self.agents.get_mut(&id) {
                    agent.active_task_count = agent.active_task_count.saturating_sub(1);
                }
                if let Some(nv) = self.info_views.get_mut(&id) {
                    nv.append_log(format!(
                        "[task {}] {}",
                        &tid.to_string()[..8],
                        if success { "completed" } else { "failed" }
                    ));
                }
            }
            NetworkEvent::TaskProgress { id, task_id, progress_pct } => {
                debug!("Task progress from {}: {}={:.1}%", id, task_id, progress_pct);
                self.task_manager.update_progress(task_id, progress_pct, None);
                let hostname = self.agents.get(&id).map(|a| a.info.hostname.as_str()).unwrap_or("?");
                self.workspace.append_activity(
                    format!("{} task {:.0}% on {}", &id.to_string()[..8], progress_pct, hostname),
                    ActivityLevel::Info,
                );
            }
            NetworkEvent::AgentError { id, message } => {
                warn!("Agent error from {}: {}", id, message);
                self.workspace.append_activity(
                    format!("{} error: {}", &id.to_string()[..8], message),
                    ActivityLevel::Error,
                );
                if let Some(nv) = self.info_views.get_mut(&id) {
                    nv.append_log(format!("[error] {}", message));
                }
            }
            NetworkEvent::BufferedResults { id, results } => {
                let hostname = self.agents.get(&id).map(|a| a.info.hostname.as_str()).unwrap_or("?");
                info!("Received {} buffered results from {} ({})", results.len(), hostname, id);
                for result in results {
                    let tid = result.task_id;
                    let success = result.success;
                    self.task_manager.complete(tid, result);
                    let short = &tid.to_string()[..8];
                    self.workspace.append_activity(
                        format!(
                            "{} buffered {} from {}",
                            short,
                            if success { "completed" } else { "failed" },
                            hostname,
                        ),
                        ActivityLevel::Info,
                    );
                }
                if let Some(agent) = self.agents.get_mut(&id) {
                    agent.active_task_count = 0;
                }
            }
            NetworkEvent::Checkpoint { id, task_id, epoch, step, data } => {
                let short = &task_id.to_string()[..8];
                info!("Checkpoint from {}: task={} epoch={} step={} ({} bytes)", id, short, epoch, step, data.len());
                if let Err(e) = architect_core::db::save_checkpoint(&self.db, &task_id.to_string(), epoch, step, &data) {
                    warn!("Failed to save checkpoint: {}", e);
                } else {
                    self.workspace.append_activity(
                        format!("{} checkpoint e{}s{}", short, epoch, step),
                        ActivityLevel::Info,
                    );
                }
            }
            NetworkEvent::GradientShard { id, task_id, step, shard_index, data } => {
                debug!("Gradient shard from {}: task={} step={} shard={} len={}", id, task_id, step, shard_index, data.len());
                let key = (task_id, step);
                let buffer = self.gradient_buffers.entry(key).or_default();
                buffer.push((shard_index, data));

                let expected = self.gradient_expected_shards.get(&task_id).copied().unwrap_or(1);
                if buffer.len() >= expected {
                    // All shards received — aggregate by averaging
                    let shard_len = buffer.first().map(|(_, d)| d.len()).unwrap_or(0);
                    let mut aggregated = vec![0.0f32; shard_len];
                    for (_, shard_data) in buffer.iter() {
                        for (i, val) in shard_data.iter().enumerate() {
                            if i < aggregated.len() {
                                aggregated[i] += val;
                            }
                        }
                    }
                    if expected > 1 {
                        for v in &mut aggregated {
                            *v /= expected as f32;
                        }
                    }

                    debug!("Gradient aggregation complete: task={} step={} len={}", task_id, step, aggregated.len());
                    let net = self.network.clone();
                    tokio::spawn(async move {
                        let _ = net.broadcast(ClientMessage::ApplyGradients { task_id, step, aggregated }).await;
                    });

                    self.gradient_buffers.remove(&key);
                }
            }
            NetworkEvent::StateSyncResponse { id, running, completed, unknown } => {
                info!("State sync from {}: running={} completed={} unknown={}", id, running.len(), completed.len(), unknown.len());
                // Tasks the agent doesn't know about — requeue them
                for tid in &unknown {
                    info!("Requeuing orphaned task {}", tid);
                    self.requeue_with_checkpoint(*tid);
                }
                // Tasks completed on agent side — mark them complete
                for tid in &completed {
                    let result = TaskResult {
                        task_id: *tid,
                        node_id: id,
                        success: true,
                        payload: None,
                        duration_ms: 0,
                        error: None,
                    };
                    self.task_manager.complete(*tid, result);
                }
                self.workspace.append_activity(
                    format!("sync {}: {}run {}done {}orphan", &id.to_string()[..8], running.len(), completed.len(), unknown.len()),
                    ActivityLevel::Info,
                );
            }
            NetworkEvent::CacheReport { id, datasets, models } => {
                debug!("Cache report from {}: {} datasets, {} models", id, datasets.len(), models.len());
                if let Some(agent) = self.agents.get_mut(&id) {
                    // Store cached datasets for data locality scheduling
                    agent.cached_datasets = datasets;
                    agent.cached_models = models;
                }
            }
        }
    }

    fn handle_discovery(&mut self, event: DiscoveryEvent) {
        match event {
            DiscoveryEvent::DeviceFound(node) => {
                let line = network_discovery::format_device(&node);
                info!("Discovered device: {}", line);
                self.workspace.append_activity(
                    format!("found {}", line),
                    ActivityLevel::Info,
                );
                // Avoid duplicates
                if !self.discovered_devices.iter().any(|n| n.ip == node.ip) {
                    self.discovered_devices.push(node);
                }
            }
            DiscoveryEvent::ScanComplete { total_found } => {
                info!("Scan complete: {} devices", total_found);
                self.workspace.append_activity(
                    format!("Scan complete: {} device(s) found", total_found),
                    ActivityLevel::Success,
                );
            }
        }
    }

    fn handle_bootstrap(&mut self, event: BootstrapEvent) {
        match event {
            BootstrapEvent::Started { host } => {
                self.workspace.append_activity(
                    format!("[seed] deploying to {}...", host),
                    ActivityLevel::Info,
                );
            }
            BootstrapEvent::Progress { host, step } => {
                self.workspace.append_activity(
                    format!("[seed] {} — {}", host, step),
                    ActivityLevel::Info,
                );
            }
            BootstrapEvent::Success { host, method } => {
                info!("Agent deployed to {} via {}", host, method);
                self.workspace.append_activity(
                    format!("[seed] {} — deployed via {}, agent auto-discovering coordinator...", host, method),
                    ActivityLevel::Success,
                );
            }
            BootstrapEvent::Failed { host, error } => {
                warn!("Deploy failed for {}: {}", host, error);
                self.workspace.append_activity(
                    format!("Deploy failed: {} — {}", host, error),
                    ActivityLevel::Error,
                );
            }
        }
    }

    fn tick(&mut self) {
        self.local_executor.cleanup_completed();
        self.local_executor.refresh_metrics();

        // Heartbeat timeout: mark agents as Degraded/Offline and requeue orphaned tasks
        let now = Instant::now();
        let mut newly_offline: Vec<NodeId> = Vec::new();
        for (id, agent) in self.agents.iter_mut() {
            let elapsed = now.duration_since(agent.last_heartbeat);
            if elapsed.as_millis() > 6000 && agent.status == NodeStatus::Online {
                agent.status = NodeStatus::Degraded;
            }
            if elapsed.as_millis() > 10000 && agent.status != NodeStatus::Offline {
                agent.status = NodeStatus::Offline;
                newly_offline.push(*id);
            }
        }
        for id in newly_offline {
            let orphaned = self.task_manager.tasks_assigned_to(id);
            for task_id in orphaned {
                self.requeue_with_checkpoint(task_id);
            }
        }

        // Try to reassign any pending tasks (from requeue or journal recovery)
        self.try_reassign_pending();

        // Update health snapshot every tick
        self.update_health_snapshot();

        // Periodic operations (~every 7.5s at 250ms tick rate)
        self.ticks_since_save += 1;
        if self.ticks_since_save >= 30 {
            self.ticks_since_save = 0;

            // Save tasks to SQLite
            if let Err(e) = self.task_manager.save_to_db(&self.db) {
                warn!("Failed to save tasks to DB: {}", e);
            }

            // Prune old terminal tasks and zombie agents periodically
            self.task_manager.prune_terminal();
            self.prune_offline_agents();

            // Config hot reload: check if config file was modified
            self.check_config_reload();

            // Work stealing: if idle agents exist, steal from overloaded
            self.try_work_stealing();

            // Split-brain: send Ping to all online agents (~every 7.5s)
            self.ping_seq += 1;
            let seq = self.ping_seq;
            let net = self.network.clone();
            tokio::spawn(async move {
                let _ = net.broadcast(ClientMessage::Ping { seq }).await;
            });

            // Check for agents with stale pongs (>30s without Pong)
            let now = Instant::now();
            for (id, agent) in self.agents.iter_mut() {
                if agent.status == NodeStatus::Online {
                    if let Some(last) = self.last_pong.get(id) {
                        if now.duration_since(*last).as_secs() > 30 {
                            agent.status = NodeStatus::Degraded;
                            debug!("Agent {} marked Degraded: no Pong in 30s", id);
                        }
                    }
                }
            }

            // Prune old metrics from DB (keep 24h)
            let cutoff = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(86400);
            let _ = architect_core::db::prune_old_metrics(&self.db, cutoff);

            // Leader lease renewal
            self.leader_tick += 1;
            if self.leader_tick >= 4 {
                self.leader_tick = 0;
                if let Err(e) = self.leader.renew() {
                    warn!("Leader lease renewal failed: {}", e);
                }
            }
        }
    }

    /// Update the health/metrics snapshot for the HTTP endpoint.
    fn update_health_snapshot(&self) {
        let online = self.agents.values().filter(|a| a.status == NodeStatus::Online).count();
        let active = self.task_manager.active_tasks().len();
        let pending = self.task_manager.pending_tasks().len();
        let uptime = Instant::now().duration_since(self.start_time).as_secs();

        if let Ok(mut snap) = self.health_snapshot.lock() {
            snap.agents_online = online;
            snap.agents_total = self.agents.len();
            snap.tasks_active = active;
            snap.tasks_pending = pending;
            snap.uptime_secs = uptime;
        }
    }

    /// Work stealing: move tasks from overloaded agents to idle ones.
    fn try_work_stealing(&mut self) {
        let pending = self.task_manager.pending_tasks();
        if !pending.is_empty() {
            return; // There are already pending tasks to assign normally
        }

        // Find idle agents (Online, 0 active tasks)
        let has_idle = self.agents.values().any(|a| {
            a.status == NodeStatus::Online && a.active_task_count == 0
        });
        if !has_idle {
            return;
        }

        // Find the most overloaded agent (>=3 tasks)
        let overloaded = self
            .agents
            .iter()
            .filter(|(_, a)| a.active_task_count >= 3)
            .max_by_key(|(_, a)| a.active_task_count)
            .map(|(id, _)| *id);

        if let Some(busy_id) = overloaded {
            // Steal the lowest-priority task from the busy agent
            if let Some(task_id) = self.task_manager.lowest_priority_task_on(busy_id) {
                let short = &task_id.to_string()[..8];
                info!("Work stealing: re-queuing task {} from {}", short, busy_id);
                self.requeue_with_checkpoint(task_id);

                // Cancel on the busy agent
                let net = self.network.clone();
                tokio::spawn(async move {
                    let _ = net.send_to(busy_id, ClientMessage::CancelTask { task_id }).await;
                });

                if let Some(agent) = self.agents.get_mut(&busy_id) {
                    agent.active_task_count = agent.active_task_count.saturating_sub(1);
                }

                self.workspace.append_activity(
                    format!("{} stolen from overloaded agent", short),
                    ActivityLevel::Info,
                );
            }
        }
    }

    /// Requeue a task and inject checkpoint data if available (for Train/Finetune resume).
    fn requeue_with_checkpoint(&mut self, task_id: Uuid) -> bool {
        if !self.task_manager.requeue(task_id) {
            return false;
        }
        let tid_str = task_id.to_string();
        if let Ok(Some(cp)) = architect_core::db::load_latest_checkpoint(&self.db, &tid_str) {
            let checkpoint = CheckpointData {
                epoch: cp.epoch,
                step: cp.step,
                state: cp.data,
            };
            self.task_manager.inject_checkpoint(task_id, checkpoint);
            info!("Injected checkpoint for task {} (epoch={}, step={})", &tid_str[..8], cp.epoch, cp.step);
        }
        true
    }

    /// Check if the config file was modified and apply changes if so.
    fn check_config_reload(&mut self) {
        let mtime = match std::fs::metadata(&self.config_path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return,
        };
        if self.last_config_mtime == Some(mtime) {
            return;
        }
        self.last_config_mtime = Some(mtime);

        let path_str = self.config_path.to_string_lossy();
        let config = architect_core::config::load_config(&path_str);
        info!("Config reloaded from {:?}", self.config_path);

        // Apply scheduler algorithm change
        let new_scheduler: Box<dyn Scheduler> = match config.scheduler.algorithm {
            SchedulerAlgorithm::RoundRobin => Box::new(RoundRobinScheduler::new()),
            SchedulerAlgorithm::Weighted => Box::new(WeightedScheduler),
            SchedulerAlgorithm::Adaptive => Box::new(AdaptiveScheduler),
        };
        self.scheduler = new_scheduler;

        // Apply resource limits
        self.local_executor.update_resource_config(config.resources);

        // Apply discovery config
        self.discovery_config = config.discovery;

        // Apply github repo
        self.github_repo = config.github_repo;

        let _ = architect_core::db::audit(
            &self.db, &self.client_id.to_string(), "config_reloaded", "", "",
        );
        self.workspace.append_activity(
            "Config reloaded".to_string(),
            ActivityLevel::Info,
        );
    }

    /// Remove agents that have been offline for over 5 minutes.
    fn prune_offline_agents(&mut self) {
        let now = Instant::now();
        let stale: Vec<NodeId> = self
            .agents
            .iter()
            .filter(|(_, a)| {
                a.status == NodeStatus::Offline
                    && now.duration_since(a.last_heartbeat).as_secs() > 300
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &stale {
            self.agents.remove(id);
            self.info_views.remove(id);
        }
        if !stale.is_empty() {
            info!("Pruned {} stale offline agents", stale.len());
            self.refresh_layout();
        }
    }

    /// Try to reassign pending tasks via the scheduler.
    fn try_reassign_pending(&mut self) {
        if !self.leader.is_leader() {
            return;
        }

        let pending = self.task_manager.pending_tasks();
        if pending.is_empty() {
            return;
        }

        for task in pending {
            let task_id = task.task_id;
            let task_type = task.task_type;

            // Build node states: client + online agents (same as dispatch_task)
            let mut node_states: Vec<NodeState> = Vec::new();
            node_states.push(NodeState {
                id: self.client_id,
                capabilities: self.client_capabilities.clone(),
                metrics: None,
                current_tasks: self.local_executor.active_count(),
                has_cached_data: false,
                cached_datasets: Vec::new(),
            });
            for (id, a) in &self.agents {
                if a.status == NodeStatus::Online {
                    node_states.push(NodeState {
                        id: *id,
                        capabilities: a.info.capabilities.clone(),
                        metrics: a.metrics.clone(),
                        current_tasks: a.active_task_count,
                        has_cached_data: !a.cached_datasets.is_empty(),
                        cached_datasets: a.cached_datasets.clone(),
                    });
                }
            }

            match self.scheduler.select_node(&task, &node_states) {
                Some(node_id) if node_id == self.client_id => {
                    self.task_manager.assign(task_id, self.client_id);
                    if let Err(_reason) = self.local_executor.execute(task) {
                        // Throttled — revert to Pending, try again next tick
                        self.task_manager.revert_to_pending(task_id);
                        continue;
                    }
                    let short = &task_id.to_string()[..8];
                    self.workspace.append_activity(
                        format!("{} {} reassigned locally", short, task_type),
                        ActivityLevel::Info,
                    );
                }
                Some(node_id) => {
                    self.task_manager.assign(task_id, node_id);
                    if let Some(agent) = self.agents.get_mut(&node_id) {
                        agent.active_task_count += 1;
                    }
                    let net = self.network.clone();
                    let msg = ClientMessage::SubmitTask(task);
                    let event_tx = self.event_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = net.send_to(node_id, msg).await {
                            warn!("Failed to send reassigned task to {}: {} — reverting", node_id, e);
                            let _ = event_tx.send(Event::TaskSendFailed { task_id, node_id });
                        }
                    });
                    let short = &task_id.to_string()[..8];
                    let hostname = self
                        .agents
                        .get(&node_id)
                        .map(|a| a.info.hostname.as_str())
                        .unwrap_or("?");
                    self.workspace.append_activity(
                        format!("{} {} reassigned to {}", short, task_type, hostname),
                        ActivityLevel::Info,
                    );
                }
                None => {
                    // No nodes available — leave as Pending, will retry next tick
                }
            }
        }
    }

    /// Graceful shutdown: save to DB, broadcast shutdown to agents.
    pub async fn shutdown(&mut self) {
        info!("Initiating graceful shutdown");

        // Save tasks to SQLite
        if let Err(e) = self.task_manager.save_to_db(&self.db) {
            warn!("Failed to save tasks to DB on shutdown: {}", e);
        } else {
            info!("Tasks saved to database on shutdown");
        }

        // Save metrics history to DB
        self.save_metrics_history();

        // Audit shutdown
        let _ = architect_core::db::audit(
            &self.db, &self.client_id.to_string(), "shutdown", "", "",
        );

        // Broadcast shutdown to all connected agents
        let net = self.network.clone();
        if let Err(e) = net.broadcast(ClientMessage::Shutdown).await {
            warn!("Failed to broadcast shutdown: {}", e);
        }
    }

    /// Save all agents' metrics history to `~/.architect/metrics/`.
    fn save_metrics_history(&self) {
        let dir = architect_core::config::data_dir().join("metrics");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        for (id, agent) in &self.agents {
            if agent.metrics_history.is_empty() {
                continue;
            }
            let snapshots: Vec<_> = agent
                .metrics_history
                .iter()
                .map(|(ts, m)| {
                    serde_json::json!({
                        "ts": ts,
                        "cpu": m.cpu_usage_pct,
                        "ram": m.ram_usage_pct,
                        "gpu": m.gpu_usage_pct,
                        "tasks_completed": m.tasks_completed,
                        "tasks_failed": m.tasks_failed,
                    })
                })
                .collect();
            let path = dir.join(format!("{}.json", id));
            let tmp = path.with_extension("tmp");
            if let Ok(json) = serde_json::to_string(&snapshots) {
                let _ = std::fs::write(&tmp, &json);
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        info!("Metrics history saved for {} agents", self.agents.len());
    }

    pub fn refresh_layout(&mut self) {
        let agents: Vec<_> = self
            .agents
            .iter()
            .map(|(id, a)| (*id, a.info.hostname.clone(), a.status))
            .collect();

        // Graph occupies 65% width and full height of the workspace area
        let w = self.terminal_size.0 as f64 * 0.65;
        let h = (self.terminal_size.1 as f64 - 3.0) * 2.0; // *2 for braille

        self.workspace
            .update_layout(self.client_id, &self.client_hostname, &agents, w, h);
    }

    pub fn render(&self, frame: &mut Frame) {
        let view = self.active_view.clone();
        match view {
            ActiveView::Workspace => {
                let chunks = Layout::vertical([
                    Constraint::Min(1),
                    Constraint::Length(1), // status bar
                    Constraint::Length(1), // command bar
                ])
                .split(frame.area());

                self.render_workspace(frame, chunks[0]);
                self.render_status_bar(frame, chunks[1]);
                render_command_bar(frame, chunks[2], &self.command_bar);
            }
            ActiveView::Info { node_id } => {
                let chunks = Layout::vertical([
                    Constraint::Min(1),
                    Constraint::Length(1), // status bar only
                ])
                .split(frame.area());

                self.render_info(frame, chunks[0], node_id);
                self.render_status_bar(frame, chunks[1]);
            }
        }
    }

    fn render_workspace(&self, frame: &mut Frame, area: Rect) {
        // 2-column layout:
        // ┌──────────────┬─────────────┐
        // │              │   nodes     │
        // │    graph     ├─────────────┤
        // │              │   tasks     │
        // │              ├─────────────┤
        // │              │  activity   │
        // └──────────────┴─────────────┘
        let cols = Layout::horizontal([
            Constraint::Percentage(65),
            Constraint::Percentage(35),
        ])
        .split(area);

        let right = Layout::vertical([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(cols[1]);

        // Left: Network topology graph (full height)
        render_graph(
            frame,
            cols[0],
            &self.workspace.node_positions,
            self.client_id,
            self.workspace.selected_node,
            &self.workspace.node_order,
        );

        // Right top: Nodes
        render_metrics_panel(frame, right[0], &self.agents);

        // Right middle: Tasks
        let tasks = self.task_manager.all_tasks();
        render_task_list(frame, right[1], &tasks);

        // Right bottom: Activity
        render_activity_log(frame, right[2], &self.workspace.activity_log);
    }

    fn render_info(&self, frame: &mut Frame, area: Rect, node_id: NodeId) {
        if !self.info_views.contains_key(&node_id) {
            let p = Paragraph::new("Info not found")
                .style(Style::default().fg(Color::Red));
            frame.render_widget(p, area);
            return;
        }

        // 2-column layout: [info + tasks | logs]
        let columns = Layout::horizontal([
            Constraint::Percentage(65),
            Constraint::Percentage(35),
        ])
        .split(area);

        let is_client = node_id == self.client_id;
        let hostname = if is_client {
            self.client_hostname.clone()
        } else {
            self.agents
                .get(&node_id)
                .map(|a| a.info.hostname.clone())
                .unwrap_or_else(|| "unknown".into())
        };

        // --- Left column: info block (fixed) + tasks block(s) (fills rest) ---
        let info_height = if is_client { 6 } else { 12 }; // client=4 lines + border, agent=10 lines + border

        // Info block
        let info_block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
            .title(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(hostname, Style::default().fg(Color::Cyan).bold()),
                Span::styled(
                    format!(" {} ", &node_id.to_string()[..8]),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
            .title_bottom(
                Line::from(Span::styled(
                    " Esc to go back ",
                    Style::default().fg(Color::DarkGray),
                ))
                .centered(),
            );

        if is_client {
            // Client: 3-row layout — info header, running tasks (local), delegated tasks (remote)
            let left_rows = Layout::vertical([
                Constraint::Length(info_height),
                Constraint::Percentage(40),
                Constraint::Percentage(60),
            ])
            .split(columns[0]);

            let info_inner = info_block.inner(left_rows[0]);
            frame.render_widget(info_block, left_rows[0]);

            let info_lines = self.build_info_header(node_id);
            frame.render_widget(Paragraph::new(info_lines), info_inner);

            // RUNNING TASKS: tasks executing locally on the client
            let running_tasks: Vec<_> = self.task_manager
                .active_tasks()
                .into_iter()
                .filter(|t| t.assigned_to == Some(self.client_id))
                .collect();

            render_info_tasks(
                frame, left_rows[1], &running_tasks, "RUNNING TASKS",
                &self.agents, self.client_id, &self.client_hostname,
            );

            // DELEGATED TASKS: tasks sent to remote agents
            let delegated_tasks: Vec<_> = self.task_manager
                .active_tasks()
                .into_iter()
                .filter(|t| t.assigned_to.is_some() && t.assigned_to != Some(self.client_id))
                .collect();

            render_info_tasks(
                frame, left_rows[2], &delegated_tasks, "DELEGATED TASKS",
                &self.agents, self.client_id, &self.client_hostname,
            );
        } else {
            // Agent: 2-row layout — info header, running tasks on this agent
            let left_rows = Layout::vertical([
                Constraint::Length(info_height),
                Constraint::Min(4),
            ])
            .split(columns[0]);

            let info_inner = info_block.inner(left_rows[0]);
            frame.render_widget(info_block, left_rows[0]);

            let info_lines = self.build_info_header(node_id);
            frame.render_widget(Paragraph::new(info_lines), info_inner);

            let tasks: Vec<_> = self.task_manager
                .all_tasks()
                .into_iter()
                .filter(|t| t.assigned_to == Some(node_id))
                .collect();

            render_info_tasks(
                frame, left_rows[1], &tasks, "RUNNING TASKS",
                &self.agents, self.client_id, &self.client_hostname,
            );
        }

        // --- Right column: logs ---
        let nv = self.info_views.get(&node_id).unwrap();
        let log_count = nv.log_lines.len();
        let log_block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
            .title(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled("LOGS", Style::default().fg(Color::Yellow).bold()),
                Span::styled(
                    format!(" {} ", log_count),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));

        let log_inner = log_block.inner(columns[1]);
        frame.render_widget(log_block, columns[1]);

        let visible = log_inner.height as usize;
        let start = log_count.saturating_sub(visible);

        let log_lines: Vec<Line> = nv.log_lines[start..]
            .iter()
            .map(|l| Line::from(l.as_str()).style(Style::default().fg(Color::DarkGray)))
            .collect();

        let log_paragraph = Paragraph::new(log_lines).wrap(Wrap { trim: false });
        frame.render_widget(log_paragraph, log_inner);
    }

    fn build_info_header(&self, node_id: NodeId) -> Vec<Line<'static>> {
        let dim = Style::default().fg(Color::DarkGray);
        let val = Style::default().fg(Color::White);

        if node_id == self.client_id {
            vec![
                Line::from(vec![Span::styled(" host    ", dim), Span::styled(self.client_hostname.clone(), val)]),
                Line::from(vec![Span::styled(" id      ", dim), Span::styled(self.client_id.to_string(), val)]),
                Line::from(vec![Span::styled(" role    ", dim), Span::styled("client", val)]),
                Line::from(vec![Span::styled(" status  ", dim), Span::styled("online", Style::default().fg(Color::Green))]),
            ]
        } else if let Some(agent) = self.agents.get(&node_id) {
            let info = &agent.info;

            let status_color = match agent.status {
                NodeStatus::Online => Color::Green,
                NodeStatus::Degraded => Color::Yellow,
                NodeStatus::Offline => Color::Red,
            };

            let mut lines = vec![
                Line::from(vec![Span::styled(" host    ", dim), Span::styled(info.hostname.clone(), val)]),
                Line::from(vec![Span::styled(" id      ", dim), Span::styled(node_id.to_string(), val)]),
                Line::from(vec![Span::styled(" os      ", dim), Span::styled(format!("{}", info.os), val)]),
                Line::from(vec![Span::styled(" arch    ", dim), Span::styled(format!("{}", info.arch), val)]),
                Line::from(vec![Span::styled(" status  ", dim), Span::styled(format!("{}", agent.status), Style::default().fg(status_color))]),
                Line::from(vec![Span::styled(" cores   ", dim), Span::styled(format!("{}", info.capabilities.cpu_cores), val)]),
                Line::from(vec![Span::styled(" mem     ", dim), Span::styled(format!("{} MB", info.capabilities.ram_total_mb), val)]),
            ];

            if let Some(m) = &agent.metrics {
                let cpu_color = if m.cpu_usage_pct > 80.0 {
                    Color::Red
                } else if m.cpu_usage_pct > 50.0 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                let ram_color = if m.ram_usage_pct > 80.0 {
                    Color::Red
                } else if m.ram_usage_pct > 50.0 {
                    Color::Yellow
                } else {
                    Color::Green
                };

                let uptime = m.uptime_secs;
                let uptime_str = if uptime >= 86400 {
                    format!("{}d {}h", uptime / 86400, (uptime % 86400) / 3600)
                } else if uptime >= 3600 {
                    format!("{}h {}m", uptime / 3600, (uptime % 3600) / 60)
                } else {
                    format!("{}m {}s", uptime / 60, uptime % 60)
                };

                lines.push(Line::from(vec![Span::styled(" cpu     ", dim), Span::styled(format!("{:.1}%", m.cpu_usage_pct), Style::default().fg(cpu_color))]));
                lines.push(Line::from(vec![Span::styled(" ram     ", dim), Span::styled(format!("{:.1}%", m.ram_usage_pct), Style::default().fg(ram_color))]));
                lines.push(Line::from(vec![Span::styled(" uptime  ", dim), Span::styled(uptime_str, val)]));
            } else {
                lines.push(Line::from(Span::styled(" waiting for metrics...", dim)));
            }

            lines
        } else {
            vec![
                Line::from(Span::styled(" unknown node", dim)),
            ]
        }
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let view_label = match &self.active_view {
            ActiveView::Workspace => "WORKSPACE",
            ActiveView::Info { .. } => "INFO",
        };

        let view_detail = match &self.active_view {
            ActiveView::Workspace => String::new(),
            ActiveView::Info { node_id } => {
                if *node_id == self.client_id {
                    "client".to_string()
                } else {
                    self.agents
                        .get(node_id)
                        .map(|a| a.info.hostname.clone())
                        .unwrap_or_else(|| "?".into())
                }
            }
        };

        let online = self
            .agents
            .values()
            .filter(|a| a.status == NodeStatus::Online)
            .count();
        let total = self.agents.len();
        let tasks_active = self.task_manager.active_tasks().len();

        let mut spans = vec![
            Span::styled(
                format!(" {} ", view_label),
                Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
        ];

        if !view_detail.is_empty() {
            spans.push(Span::styled(
                format!(" {} ", view_detail),
                Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 40)),
            ));
        }

        spans.push(Span::styled(
            "  ",
            Style::default().bg(Color::Rgb(25, 25, 25)),
        ));

        // Nodes indicator
        let nodes_color = if online > 0 { Color::Green } else { Color::DarkGray };
        spans.push(Span::styled(
            format!(" {} ", online),
            Style::default().fg(Color::Black).bg(nodes_color).bold(),
        ));
        spans.push(Span::styled(
            format!("/{} nodes ", total),
            Style::default().fg(Color::DarkGray).bg(Color::Rgb(25, 25, 25)),
        ));

        // Tasks indicator
        if tasks_active > 0 {
            spans.push(Span::styled(
                format!(" {} ", tasks_active),
                Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
            ));
            spans.push(Span::styled(
                " active ",
                Style::default().fg(Color::DarkGray).bg(Color::Rgb(25, 25, 25)),
            ));
        }

        let line = Line::from(spans);
        let bar = Paragraph::new(line)
            .style(Style::default().bg(Color::Rgb(25, 25, 25)));
        frame.render_widget(bar, area);
    }
}

/// Render tasks in the info view with node assignment info.
fn render_info_tasks(
    frame: &mut Frame,
    area: Rect,
    tasks: &[crate::task_manager::TaskView],
    title: &str,
    agents: &HashMap<NodeId, AgentState>,
    client_id: NodeId,
    client_hostname: &str,
) {
    use ratatui::widgets::Padding;

    let active = tasks
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Assigned))
        .count();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
        .padding(Padding::horizontal(1))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(title, Style::default().fg(Color::Magenta).bold()),
            Span::styled(
                if active > 0 {
                    format!(" {} active ", active)
                } else {
                    format!(" {} ", tasks.len())
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let dim = Style::default().fg(Color::DarkGray);

    // Header row
    let mut lines: Vec<Line> = vec![Line::from(vec![
        Span::styled("STS ", dim),
        Span::styled(format!("{:<10}", "ID"), dim),
        Span::styled(format!("{:<12}", "TYPE"), dim),
        Span::styled(format!("{:<14}", "NODE"), dim),
        Span::styled("PROGRESS", dim),
    ])];

    if tasks.is_empty() {
        lines.push(Line::from(Span::styled(
            " No active tasks",
            Style::default().fg(Color::DarkGray),
        )));
        let p = Paragraph::new(lines);
        frame.render_widget(p, inner);
        return;
    }

    let visible = inner.height.saturating_sub(1) as usize; // -1 for header

    for t in tasks.iter().take(visible) {
        let short_id = &t.task_id.to_string()[..8];
        let (status_sym, status_color) = match t.status {
            TaskStatus::Pending => ("◦", Color::DarkGray),
            TaskStatus::Assigned => ("◉", Color::Blue),
            TaskStatus::Running => ("▶", Color::Yellow),
            TaskStatus::Completed => ("✓", Color::Green),
            TaskStatus::Failed => ("✗", Color::Red),
            TaskStatus::Cancelled => ("−", Color::DarkGray),
        };

        let node_label: &str = match t.assigned_to {
            Some(nid) if nid == client_id => client_hostname,
            Some(nid) => agents
                .get(&nid)
                .map(|a| a.info.hostname.as_str())
                .unwrap_or("--"),
            None => "--",
        };

        let progress = if t.progress > 0.0 {
            format!("{:.0}%", t.progress)
        } else {
            match t.status {
                TaskStatus::Completed => "done".to_string(),
                TaskStatus::Failed => "err".to_string(),
                _ => "--".to_string(),
            }
        };

        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", status_sym), Style::default().fg(status_color)),
            Span::styled(format!("{:<10}", short_id), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:<12}", format!("{}", t.task_type)), Style::default().fg(Color::White)),
            Span::styled(format!("{:<14}", node_label), Style::default().fg(Color::Cyan)),
            Span::styled(progress, Style::default().fg(Color::Yellow)),
        ]));
    }

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}

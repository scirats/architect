use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::Stream;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

use architect_core::proto::agent_service_server::{AgentService, AgentServiceServer};
use architect_core::proto::{AgentMsg, CoordinatorMsg};
use architect_core::protocol::{AgentMessage, ClientMessage};
use architect_core::types::{NodeId, NodeInfo};

// Re-export NetworkEvent so app.rs can keep using it unchanged
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    AgentConnected { id: NodeId, info: NodeInfo },
    AgentDisconnected { id: NodeId },
    Heartbeat { id: NodeId, metrics: architect_core::types::NodeMetrics },
    TaskResult { id: NodeId, result: architect_core::payload::TaskResult },
    TaskProgress { id: NodeId, task_id: uuid::Uuid, progress_pct: f32 },
    AgentError { id: NodeId, message: String },
    BufferedResults { id: NodeId, results: Vec<architect_core::payload::TaskResult> },
    Checkpoint { id: NodeId, task_id: uuid::Uuid, epoch: u32, step: u64, data: Vec<u8> },
    GradientShard { id: NodeId, task_id: uuid::Uuid, step: u64, shard_index: u32, data: Vec<f32> },
    StateSyncResponse { id: NodeId, running: Vec<uuid::Uuid>, completed: Vec<uuid::Uuid>, unknown: Vec<uuid::Uuid> },
    CacheReport { id: NodeId, datasets: Vec<String>, models: Vec<String> },
}

type AgentSender = mpsc::Sender<CoordinatorMsg>;

/// Handle used by app.rs to send messages to connected agents.
#[derive(Clone)]
pub struct NetworkManagerHandle {
    connections: Arc<RwLock<HashMap<NodeId, AgentSender>>>,
}

impl NetworkManagerHandle {
    pub async fn send_to(&self, node_id: NodeId, msg: ClientMessage) -> anyhow::Result<()> {
        let proto_msg = CoordinatorMsg::from(&msg);
        let conns = self.connections.read().await;
        if let Some(sender) = conns.get(&node_id) {
            sender
                .send(proto_msg)
                .await
                .map_err(|e| anyhow::anyhow!("Send failed: {}", e))?;
        } else {
            anyhow::bail!("Node {} not connected", node_id);
        }
        Ok(())
    }

    pub async fn broadcast(&self, msg: ClientMessage) -> anyhow::Result<()> {
        let proto_msg = CoordinatorMsg::from(&msg);
        let conns = self.connections.read().await;
        for sender in conns.values() {
            let _ = sender.send(proto_msg.clone()).await;
        }
        Ok(())
    }
}

struct GrpcAgentService {
    connections: Arc<RwLock<HashMap<NodeId, AgentSender>>>,
    /// Registry: NodeId → hostname. Rejects reconnection from a different host with the same ID.
    identity_registry: Arc<RwLock<HashMap<NodeId, String>>>,
    event_tx: mpsc::Sender<NetworkEvent>,
    cluster_token: String,
}

#[tonic::async_trait]
impl AgentService for GrpcAgentService {
    type SessionStream =
        Pin<Box<dyn Stream<Item = Result<CoordinatorMsg, Status>> + Send + 'static>>;

    async fn session(
        &self,
        request: Request<Streaming<AgentMsg>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        let mut inbound = request.into_inner();
        let connections = self.connections.clone();
        let identity_registry = self.identity_registry.clone();
        let event_tx = self.event_tx.clone();

        info!("New gRPC session initiated");

        // Create channel for sending messages back to this agent
        let (resp_tx, resp_rx) = mpsc::channel::<CoordinatorMsg>(64);

        // Return the response stream immediately to avoid deadlock.
        // The agent can't send Identity until we return response headers,
        // so we handle Identity inside the reader task.
        let cluster_token = self.cluster_token.clone();
        let connections_for_task = connections.clone();
        let registry_for_task = identity_registry.clone();
        let event_tx_reader = event_tx.clone();
        tokio::spawn(async move {
            // First message must be Identity
            let node_id;
            match inbound.next().await {
                Some(Ok(proto_msg)) => match AgentMessage::try_from(proto_msg) {
                    Ok(AgentMessage::Identity(info)) => {
                        // Validate auth token
                        if !cluster_token.is_empty() && info.auth_token != cluster_token {
                            warn!(
                                "Agent {} rejected: invalid auth token from {}",
                                info.id, info.hostname
                            );
                            return;
                        }

                        // Validate identity: same NodeId must come from same hostname
                        {
                            let mut registry = registry_for_task.write().await;
                            if let Some(prev_host) = registry.get(&info.id) {
                                if *prev_host != info.hostname {
                                    warn!(
                                        "Agent {} rejected: NodeId {} already registered to host '{}', got '{}'",
                                        info.id, info.id, prev_host, info.hostname
                                    );
                                    return;
                                }
                            } else {
                                registry.insert(info.id, info.hostname.clone());
                            }
                        }

                        node_id = info.id;
                        info!("gRPC agent authenticated: {} ({})", info.hostname, node_id);

                        // Register connection
                        {
                            let mut conns = connections_for_task.write().await;
                            conns.insert(node_id, resp_tx);
                        }

                        // Notify TUI
                        let _ = event_tx_reader
                            .send(NetworkEvent::AgentConnected {
                                id: node_id,
                                info,
                            })
                            .await;
                    }
                    Ok(_) => {
                        warn!("Agent sent non-Identity as first message, dropping");
                        return;
                    }
                    Err(e) => {
                        warn!("Failed to decode identity: {}", e);
                        return;
                    }
                },
                Some(Err(e)) => {
                    error!("Stream error before identity: {}", e);
                    return;
                }
                None => {
                    warn!("Stream closed before identity");
                    return;
                }
            }

            // Process remaining messages (with rate limiting)
            let mut msg_count: u32 = 0;
            let mut window_start = Instant::now();
            const MAX_MSG_PER_SEC: u32 = 100;

            while let Some(result) = inbound.next().await {
                // Rate limiting: reset every second, warn if exceeded
                let elapsed = window_start.elapsed();
                if elapsed.as_secs() >= 1 {
                    msg_count = 0;
                    window_start = Instant::now();
                }
                msg_count += 1;
                if msg_count > MAX_MSG_PER_SEC {
                    warn!("Rate limit exceeded for agent {}: {} msg/s", node_id, msg_count);
                    continue;
                }

                match result {
                    Ok(proto_msg) => {
                        match AgentMessage::try_from(proto_msg) {
                            Ok(msg) => {
                                let event = match msg {
                                    AgentMessage::Heartbeat(metrics) => {
                                        NetworkEvent::Heartbeat { id: node_id, metrics }
                                    }
                                    AgentMessage::Error { message } => {
                                        warn!("Agent {} error: {}", node_id, message);
                                        NetworkEvent::AgentError { id: node_id, message }
                                    }
                                    AgentMessage::TaskResult(result) => {
                                        info!("Task result from {}: task={}", node_id, result.task_id);
                                        NetworkEvent::TaskResult { id: node_id, result }
                                    }
                                    AgentMessage::TaskProgress { task_id, progress_pct } => {
                                        debug!("Task progress from {}: {}={:.1}%", node_id, task_id, progress_pct);
                                        NetworkEvent::TaskProgress { id: node_id, task_id, progress_pct }
                                    }
                                    AgentMessage::BufferedResults(results) => {
                                        info!("Buffered results from {}: {} results", node_id, results.len());
                                        NetworkEvent::BufferedResults { id: node_id, results }
                                    }
                                    AgentMessage::Checkpoint { task_id, epoch, step, data } => {
                                        info!("Checkpoint from {}: task={} epoch={} step={}", node_id, task_id, epoch, step);
                                        NetworkEvent::Checkpoint { id: node_id, task_id, epoch, step, data }
                                    }
                                    AgentMessage::GradientShard { task_id, step, shard_index, data } => {
                                        debug!("Gradient shard from {}: task={} step={} shard={}", node_id, task_id, step, shard_index);
                                        NetworkEvent::GradientShard { id: node_id, task_id, step, shard_index, data }
                                    }
                                    AgentMessage::StateSyncResponse { running, completed, unknown } => {
                                        info!("State sync response from {}: running={} completed={} unknown={}", node_id, running.len(), completed.len(), unknown.len());
                                        NetworkEvent::StateSyncResponse { id: node_id, running, completed, unknown }
                                    }
                                    AgentMessage::CacheReport { datasets, models } => {
                                        debug!("Cache report from {}: {} datasets, {} models", node_id, datasets.len(), models.len());
                                        NetworkEvent::CacheReport { id: node_id, datasets, models }
                                    }
                                    AgentMessage::Capabilities(_)
                                    | AgentMessage::Pong { .. }
                                    | AgentMessage::Identity(_) => continue,
                                };
                                if event_tx_reader.send(event).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!("Failed to convert agent message from {}: {}", node_id, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Stream error from {}: {}", node_id, e);
                        break;
                    }
                }
            }

            // Cleanup on disconnect
            {
                let mut conns = connections_for_task.write().await;
                conns.remove(&node_id);
            }
            let _ = event_tx_reader
                .send(NetworkEvent::AgentDisconnected { id: node_id })
                .await;
            info!("Agent disconnected: {}", node_id);
        });

        // Return response stream immediately — unblocks the agent to send Identity
        let stream = ReceiverStream::new(resp_rx).map(Ok);
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Start the gRPC server with TLS and return a handle for sending messages to agents.
pub async fn start_server(
    addr: SocketAddr,
    event_tx: mpsc::Sender<NetworkEvent>,
    cluster_token: String,
) -> anyhow::Result<NetworkManagerHandle> {
    let connections: Arc<RwLock<HashMap<NodeId, AgentSender>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let identity_registry: Arc<RwLock<HashMap<NodeId, String>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let handle = NetworkManagerHandle {
        connections: connections.clone(),
    };

    let service = GrpcAgentService {
        connections,
        identity_registry,
        event_tx,
        cluster_token,
    };

    // Generate / load TLS certs
    let cert_paths = architect_core::tls::ensure_certs()?;
    let identity = architect_core::tls::load_server_identity(&cert_paths)?;

    // mTLS: require client certificates signed by our CA
    let ca_pem = std::fs::read_to_string(&cert_paths.ca_cert)?;
    let ca_cert = tonic::transport::Certificate::from_pem(ca_pem);
    let tls_config = tonic::transport::ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(ca_cert);

    let server = tonic::transport::Server::builder()
        .tls_config(tls_config)?
        .add_service(AgentServiceServer::new(service))
        .serve(addr);

    info!("gRPC server (TLS) listening on {}", addr);

    tokio::spawn(async move {
        if let Err(e) = server.await {
            error!("gRPC server error: {}", e);
        }
    });

    Ok(handle)
}

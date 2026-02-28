use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::transport::Endpoint;
use tonic::Request;
use tracing::{error, info, warn};

use architect_core::proto::agent_service_client::AgentServiceClient;
use architect_core::proto::AgentMsg;
use architect_core::protocol::{AgentMessage, ClientMessage};
use architect_core::tls;

pub struct GrpcConnection {
    addr: SocketAddr,
    node_id: Option<String>,
}

impl GrpcConnection {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr, node_id: None }
    }

    pub fn with_node_id(mut self, node_id: &str) -> Self {
        self.node_id = Some(node_id.to_string());
        self
    }

    /// Connect to the coordinator via gRPC bidirectional streaming with TLS + optional mTLS.
    pub async fn connect(
        &self,
    ) -> Result<(mpsc::Sender<AgentMessage>, mpsc::Receiver<ClientMessage>)> {
        let ca_path = tls::certs_dir().join("ca.pem");
        let endpoint = if ca_path.exists() {
            let ca_cert = tls::load_ca_certificate(&ca_path)?;
            let mut tls_config = tonic::transport::ClientTlsConfig::new()
                .ca_certificate(ca_cert)
                .domain_name("localhost");

            // mTLS: if we have a client cert, use it
            if let Some(ref node_id) = self.node_id {
                match tls::load_client_identity(node_id) {
                    Ok(identity) => {
                        tls_config = tls_config.identity(identity);
                        info!("Using mTLS client certificate for node {}", node_id);
                    }
                    Err(e) => {
                        warn!("No client cert for mTLS ({}), connection will likely be rejected", e);
                    }
                }
            }

            Endpoint::from_shared(format!("https://{}", self.addr))?
                .connect_timeout(Duration::from_secs(5))
                .tls_config(tls_config)?
        } else {
            warn!("No CA cert found at {:?}, connecting without TLS", ca_path);
            Endpoint::from_shared(format!("http://{}", self.addr))?
                .connect_timeout(Duration::from_secs(5))
        };

        let channel = endpoint.connect().await?;
        let mut client = AgentServiceClient::new(channel);
        info!("gRPC channel established to {}", self.addr);

        // Outgoing: agent → coordinator
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<AgentMessage>(64);

        // Convert AgentMessage to proto AgentMsg
        let outbound_stream = ReceiverStream::new(outgoing_rx).map(|msg| AgentMsg::from(&msg));

        // Start bidirectional session with timeout
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            client.session(Request::new(outbound_stream)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("session handshake timed out"))??;

        let mut inbound = response.into_inner();
        info!("gRPC session active with coordinator at {}", self.addr);

        // Incoming: coordinator → agent
        let (incoming_tx, incoming_rx) = mpsc::channel::<ClientMessage>(64);

        // Reader task: convert proto CoordinatorMsg → ClientMessage
        tokio::spawn(async move {
            while let Some(result) = inbound.next().await {
                match result {
                    Ok(proto_msg) => match ClientMessage::try_from(proto_msg) {
                        Ok(msg) => {
                            if incoming_tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to decode coordinator message: {}", e);
                        }
                    },
                    Err(e) => {
                        error!("gRPC stream error: {}", e);
                        break;
                    }
                }
            }
            info!("gRPC connection to coordinator closed");
        });

        Ok((outgoing_tx, incoming_rx))
    }
}

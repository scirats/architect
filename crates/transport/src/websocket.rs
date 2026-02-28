use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info};

use architect_core::transport::{Transport, TransportType};
use architect_core::types::NodeId;

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<TcpStream>,
    Message,
>;

/// WebSocket transport for browser-friendly and firewall-traversing connections.
pub struct WebSocketTransport {
    sinks: Arc<Mutex<HashMap<NodeId, WsSink>>>,
}

impl WebSocketTransport {
    pub fn new() -> Self {
        Self {
            sinks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start accepting WebSocket connections.
    pub async fn listen(&self, addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        info!("WebSocket transport listening on {}", addr);

        let sinks = self.sinks.clone();

        tokio::spawn(async move {
            while let Ok((stream, peer_addr)) = listener.accept().await {
                let sinks = sinks.clone();
                tokio::spawn(async move {
                    match tokio_tungstenite::accept_async(stream).await {
                        Ok(ws_stream) => {
                            info!("WebSocket connected: {}", peer_addr);
                            let (_sink, mut rx) = ws_stream.split();

                            // Read messages
                            while let Some(msg) = rx.next().await {
                                match msg {
                                    Ok(Message::Binary(data)) => {
                                        debug!("WS received {} bytes from {}", data.len(), peer_addr);
                                    }
                                    Ok(Message::Close(_)) => break,
                                    Err(e) => {
                                        debug!("WS error from {}: {}", peer_addr, e);
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            debug!("WS handshake error from {}: {}", peer_addr, e);
                        }
                    }
                });
            }
        });

        Ok(())
    }

    /// Connect to a remote WebSocket server.
    pub async fn connect(&self, url: &str, node_id: NodeId) -> Result<()> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(url).await?;
        let (sink, _rx) = ws_stream.split();

        let mut sinks = self.sinks.lock().await;
        sinks.insert(node_id, sink);

        info!("WebSocket connected to {} for node {}", url, node_id);
        Ok(())
    }

    /// Register an already-established WebSocket sink.
    pub async fn register_sink(&self, node_id: NodeId, sink: WsSink) {
        let mut sinks = self.sinks.lock().await;
        sinks.insert(node_id, sink);
    }
}

impl Default for WebSocketTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn send(&self, node_id: NodeId, data: &[u8]) -> Result<()> {
        let mut sinks = self.sinks.lock().await;
        let sink = sinks
            .get_mut(&node_id)
            .ok_or_else(|| anyhow::anyhow!("No WebSocket connection for node {}", node_id))?;

        sink.send(Message::Binary(data.to_vec().into())).await?;

        debug!("WS sent {} bytes to {}", data.len(), node_id);
        Ok(())
    }

    async fn receive(&self, _node_id: NodeId) -> Result<Vec<u8>> {
        anyhow::bail!("Use listen loop for WebSocket receive")
    }

    async fn broadcast(&self, data: &[u8]) -> Result<()> {
        let mut sinks = self.sinks.lock().await;
        for (node_id, sink) in sinks.iter_mut() {
            if let Err(e) = sink.send(Message::Binary(data.to_vec().into())).await {
                debug!("WS broadcast error to {}: {}", node_id, e);
            }
        }
        Ok(())
    }

    async fn bandwidth_mbps(&self) -> f32 {
        100.0
    }

    async fn latency_ms(&self) -> f32 {
        15.0
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn transport_type(&self) -> TransportType {
        TransportType::WebSocket
    }
}

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use tokio::task::JoinHandle;
use tracing::info;

/// Serves agent binaries over HTTP for remote bootstrap.
/// Nodes can download the correct binary for their architecture.
pub struct AgentServer {
    addr: SocketAddr,
    binaries: HashMap<String, PathBuf>,
}

impl AgentServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            binaries: HashMap::new(),
        }
    }

    /// Register an agent binary for a specific architecture.
    pub fn add_binary(&mut self, arch: &str, path: PathBuf) {
        info!("Registered agent binary for {}: {:?}", arch, path);
        self.binaries.insert(arch.to_string(), path);
    }

    /// Start the HTTP server for distributing agent binaries.
    pub fn start(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("Agent binary server starting on {}", self.addr);

            let listener = match tokio::net::TcpListener::bind(self.addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Failed to bind agent server: {}", e);
                    return;
                }
            };

            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        let binaries = self.binaries.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_request(stream, addr, &binaries).await {
                                tracing::error!("Request error from {}: {}", addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("Accept error: {}", e);
                    }
                }
            }
        })
    }

    pub fn available_architectures(&self) -> Vec<&str> {
        self.binaries.keys().map(|s| s.as_str()).collect()
    }
}

/// Handle a simple HTTP request for agent binary download.
async fn handle_request(
    mut stream: tokio::net::TcpStream,
    addr: SocketAddr,
    binaries: &HashMap<String, PathBuf>,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse GET /agent/<arch>
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    info!("Agent server request from {}: {}", addr, path);

    if let Some(arch) = path.strip_prefix("/agent/") {
        if let Some(binary_path) = binaries.get(arch) {
            match tokio::fs::read(binary_path).await {
                Ok(data) => {
                    let header = format!(
                        "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                        data.len()
                    );
                    stream.write_all(header.as_bytes()).await?;
                    stream.write_all(&data).await?;
                }
                Err(e) => {
                    let body = format!("Binary not found: {}", e);
                    let header = format!(
                        "HTTP/1.0 404 Not Found\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(header.as_bytes()).await?;
                    stream.write_all(body.as_bytes()).await?;
                }
            }
        } else {
            let available: Vec<&str> = binaries.keys().map(|s| s.as_str()).collect();
            let body = format!("Unknown arch: {}. Available: {:?}", arch, available);
            let header = format!(
                "HTTP/1.0 404 Not Found\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await?;
            stream.write_all(body.as_bytes()).await?;
        }
    } else {
        let body = "Architect Agent Server. GET /agent/<arch> to download.";
        let header = format!(
            "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
    }

    Ok(())
}

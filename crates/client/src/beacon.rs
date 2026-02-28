use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{debug, error, info};

use architect_core::beacon::{self, CoordinatorBeacon};

/// Spawn a background task that broadcasts the coordinator beacon periodically.
pub async fn start_beacon_broadcast(
    grpc_port: u16,
    cluster_token: &str,
    hostname: String,
    client_id: String,
    interval_ms: u64,
    beacon_port: u16,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let beacon = CoordinatorBeacon {
        grpc_port,
        token_hash: beacon::hash_token(cluster_token),
        hostname,
        client_id,
    };

    let encoded = beacon::encode_beacon(&beacon)?;

    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.set_broadcast(true)?;

    let broadcast_addr: SocketAddr = format!("255.255.255.255:{}", beacon_port).parse()?;

    info!(
        "Beacon broadcaster started: grpc_port={}, interval={}ms, udp={}",
        grpc_port, interval_ms, broadcast_addr
    );

    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
        loop {
            interval.tick().await;
            match socket.send_to(&encoded, broadcast_addr).await {
                Ok(n) => debug!("Beacon sent ({} bytes)", n),
                Err(e) => error!("Beacon send failed: {}", e),
            }
        }
    });

    Ok(handle)
}

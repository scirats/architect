use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use architect_core::beacon::{self, CoordinatorBeacon, BEACON_MAX_SIZE};

/// Listen for coordinator beacons on UDP.
/// Returns the coordinator's gRPC address when a matching beacon is received.
pub async fn listen_for_beacon(
    cluster_token: &str,
    beacon_port: u16,
    timeout_duration: Duration,
) -> Option<(SocketAddr, CoordinatorBeacon)> {
    let expected_hash = beacon::hash_token(cluster_token);

    let bind_addr: SocketAddr = format!("0.0.0.0:{}", beacon_port).parse().ok()?;
    let socket = match UdpSocket::bind(bind_addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to bind beacon listener on {}: {}", bind_addr, e);
            return None;
        }
    };

    info!("Listening for coordinator beacon on UDP port {}", beacon_port);

    let mut buf = vec![0u8; BEACON_MAX_SIZE];
    let deadline = tokio::time::Instant::now() + timeout_duration;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            debug!("Beacon listen timeout reached");
            return None;
        }

        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, src_addr))) => {
                if let Some(b) = beacon::decode_beacon(&buf[..n]) {
                    if b.token_hash == expected_hash {
                        let grpc_addr: SocketAddr =
                            format!("{}:{}", src_addr.ip(), b.grpc_port).parse().ok()?;
                        info!(
                            "Discovered coordinator at {} (hostname={})",
                            grpc_addr, b.hostname
                        );
                        return Some((grpc_addr, b));
                    } else {
                        debug!("Beacon from {} rejected: token hash mismatch", src_addr);
                    }
                }
            }
            Ok(Err(e)) => {
                warn!("Beacon recv error: {}", e);
            }
            Err(_) => {
                return None;
            }
        }
    }
}

/// Browse mDNS for coordinator services.
/// Returns the first matching service address within the timeout.
pub async fn browse_mdns(timeout_duration: Duration) -> Option<SocketAddr> {
    use architect_discovery::mdns::MdnsDiscovery;
    use tokio::sync::mpsc;

    let mdns = MdnsDiscovery::new(0);
    let (tx, mut rx) = mpsc::channel(8);

    // Let the browse task run to completion so the mDNS daemon is properly shut down.
    // Aborting it would leak file descriptors (sockets not closed).
    tokio::spawn(async move {
        if let Err(e) = mdns.browse(tx, timeout_duration).await {
            warn!("mDNS browse error: {}", e);
        }
    });

    // Take the first discovered node (or timeout)
    let result = tokio::time::timeout(timeout_duration, rx.recv()).await;

    match result {
        Ok(Some(node)) => {
            let port = node.open_ports.first().copied().unwrap_or(9876);
            let addr: SocketAddr = format!("{}:{}", node.ip, port).parse().ok()?;
            info!("mDNS discovered coordinator at {}", addr);
            Some(addr)
        }
        _ => None,
    }
}

/// Try to discover the coordinator using beacon and mDNS concurrently.
/// Returns the first method that succeeds.
pub async fn discover_coordinator(
    cluster_token: &str,
    beacon_port: u16,
) -> Option<SocketAddr> {
    let listen_timeout = Duration::from_secs(15);

    let token = cluster_token.to_string();
    let beacon_fut = async {
        listen_for_beacon(&token, beacon_port, listen_timeout).await.map(|(addr, _)| addr)
    };

    let mdns_fut = browse_mdns(listen_timeout);

    // Race both methods: return whichever finds the coordinator first
    tokio::select! {
        Some(addr) = beacon_fut => Some(addr),
        Some(addr) = mdns_fut => Some(addr),
        else => None,
    }
}

use std::net::IpAddr;

use futures::StreamExt;
use tokio::sync::mpsc;
use tracing::info;

use architect_core::types::DeviceType;
use architect_discovery::{DiscoveredNode, NodeDiscovery};

/// Events emitted by the discovery system.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    DeviceFound(DiscoveredNode),
    ScanComplete { total_found: usize },
}

/// Result of a bootstrap attempt, forwarded to the TUI event loop.
#[derive(Debug, Clone)]
pub enum BootstrapEvent {
    Started { host: IpAddr },
    Progress { host: IpAddr, step: String },
    Success { host: IpAddr, method: String },
    Failed { host: IpAddr, error: String },
}

/// Run a network scan and emit events through the channel.
pub async fn run_scan(
    subnet: String,
    agent_port: u16,
    tx: mpsc::Sender<DiscoveryEvent>,
) {
    info!("Starting network scan on {}", subnet);
    let discovery = NodeDiscovery::new(subnet, agent_port);
    let mut stream = Box::pin(discovery.discover());
    let mut count = 0usize;

    while let Some(node) = stream.next().await {
        count += 1;
        if tx.send(DiscoveryEvent::DeviceFound(node)).await.is_err() {
            break;
        }
    }

    let _ = tx.send(DiscoveryEvent::ScanComplete { total_found: count }).await;
}

/// Format a discovered node for display.
pub fn format_device(node: &DiscoveredNode) -> String {
    let dtype = match node.device_type {
        DeviceType::Desktop => "desktop",
        DeviceType::Laptop => "laptop",
        DeviceType::Phone => "phone",
        DeviceType::SmartTV => "tv",
        DeviceType::RaspberryPi => "raspi",
        DeviceType::Unknown => "unknown",
    };
    let os = node.os_hint.as_deref().unwrap_or("-");
    let agent = if node.has_agent { "yes" } else { "no" };
    format!("{} {} os={} agent={}", node.ip, dtype, os, agent)
}

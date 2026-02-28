use std::collections::HashMap;
use std::net::IpAddr;

use async_stream::stream;
use futures::Stream;
use tracing::info;

use architect_core::types::DeviceType;

/// A node discovered via scanning or mDNS.
#[derive(Debug, Clone)]
pub struct DiscoveredNode {
    pub ip: IpAddr,
    pub hostname: Option<String>,
    pub open_ports: Vec<u16>,
    pub device_type: DeviceType,
    pub has_agent: bool,
    pub os_hint: Option<String>,
}

/// Orchestrates all discovery sources and merges results.
pub struct NodeDiscovery {
    subnet: String,
    agent_port: u16,
}

impl NodeDiscovery {
    pub fn new(subnet: String, agent_port: u16) -> Self {
        Self { subnet, agent_port }
    }

    /// Run all discovery methods and yield deduplicated results as a stream.
    pub fn discover(&self) -> impl Stream<Item = DiscoveredNode> + '_ {
        stream! {
            info!("Starting network discovery on subnet {}", self.subnet);

            // Network scan
            let scanner = super::scanner::NetworkScanner::new(
                self.subnet.clone(),
                self.agent_port,
            );

            let scan_results = scanner.scan().await;
            let mut seen: HashMap<IpAddr, DiscoveredNode> = HashMap::new();

            let fingerprinter = super::fingerprint::DeviceFingerprinter::new();

            for mut node in scan_results {
                if seen.contains_key(&node.ip) {
                    continue;
                }

                // Enrich with fingerprinting
                if let Some(enriched) = fingerprinter.probe(&node).await {
                    node.device_type = enriched.device_type;
                    node.os_hint = enriched.os_hint;
                    node.hostname = enriched.hostname.or(node.hostname);
                }

                seen.insert(node.ip, node.clone());
                yield node;
            }

            info!("Discovery complete: {} nodes found", seen.len());
        }
    }
}

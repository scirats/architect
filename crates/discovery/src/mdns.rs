use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use architect_core::types::DeviceType;

use super::discovery::DiscoveredNode;

/// mDNS service type for architect agents.
const SERVICE_TYPE: &str = "_architect._tcp.local.";

/// Discovers architect agents via mDNS on the local network.
pub struct MdnsDiscovery;

impl MdnsDiscovery {
    pub fn new(_agent_port: u16) -> Self {
        Self
    }

    /// Register this node as an mDNS service.
    pub fn register(&self, hostname: &str, port: u16) -> anyhow::Result<ServiceDaemon> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| anyhow::anyhow!("Failed to create mDNS daemon: {}", e))?;

        let service_name = format!("architect-{}", hostname);
        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &service_name,
            hostname,
            "",
            port,
            None,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create mDNS service info: {}", e))?;

        daemon
            .register(service)
            .map_err(|e| anyhow::anyhow!("Failed to register mDNS service: {}", e))?;

        info!("Registered mDNS service: {} on port {}", service_name, port);

        Ok(daemon)
    }

    /// Browse for architect agents via mDNS.
    /// Returns discovered nodes through the provided channel.
    pub async fn browse(
        &self,
        tx: mpsc::Sender<DiscoveredNode>,
        browse_duration: Duration,
    ) -> anyhow::Result<()> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| anyhow::anyhow!("Failed to create mDNS daemon: {}", e))?;

        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| anyhow::anyhow!("Failed to browse mDNS: {}", e))?;

        info!("Browsing mDNS for {} (timeout: {:?})", SERVICE_TYPE, browse_duration);

        let deadline = tokio::time::Instant::now() + browse_duration;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, tokio::task::spawn_blocking({
                let receiver = receiver.clone();
                move || receiver.recv_timeout(Duration::from_millis(500))
            }))
            .await
            {
                Ok(Ok(Ok(event))) => {
                    if let Some(node) = self.handle_event(event) {
                        if tx.send(node).await.is_err() {
                            break;
                        }
                    }
                }
                Ok(Ok(Err(_))) => {
                    // recv_timeout expired, continue loop
                    continue;
                }
                Ok(Err(e)) => {
                    error!("mDNS browse task error: {}", e);
                    break;
                }
                Err(_) => {
                    // Overall timeout reached
                    break;
                }
            }
        }

        if let Err(e) = daemon.stop_browse(SERVICE_TYPE) {
            warn!("Failed to stop mDNS browse: {:?}", e);
        }

        info!("mDNS browse completed");
        Ok(())
    }

    fn handle_event(&self, event: ServiceEvent) -> Option<DiscoveredNode> {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let addr = info.get_addresses().iter().next().copied()?;

                debug!(
                    "mDNS resolved: {} at {}:{}",
                    info.get_fullname(),
                    addr,
                    info.get_port()
                );

                Some(DiscoveredNode {
                    ip: addr,
                    hostname: Some(info.get_hostname().trim_end_matches('.').to_string()),
                    open_ports: vec![info.get_port()],
                    device_type: DeviceType::Unknown,
                    has_agent: true,
                    os_hint: None,
                })
            }
            ServiceEvent::SearchStarted(_) => {
                debug!("mDNS search started");
                None
            }
            ServiceEvent::ServiceFound(_, _) => None,
            ServiceEvent::ServiceRemoved(_, fullname) => {
                debug!("mDNS service removed: {}", fullname);
                None
            }
            ServiceEvent::SearchStopped(_) => None,
        }
    }
}

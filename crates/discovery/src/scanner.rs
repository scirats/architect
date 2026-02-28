use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, warn};

use architect_core::types::DeviceType;

use super::discovery::DiscoveredNode;

/// Well-known ports to scan for device identification.
const PROBE_PORTS: &[u16] = &[22, 5555, 5985, 9876];

/// Maximum concurrent connection attempts.
const MAX_CONCURRENT: usize = 128;

/// Timeout for each TCP connection probe.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// Scans a subnet for devices with open ports.
pub struct NetworkScanner {
    subnet: String,
    agent_port: u16,
}

impl NetworkScanner {
    pub fn new(subnet: String, agent_port: u16) -> Self {
        Self { subnet, agent_port }
    }

    /// Parse subnet string (e.g., "192.168.1.0/24") into a list of IPs.
    fn parse_subnet(&self) -> Vec<IpAddr> {
        let parts: Vec<&str> = self.subnet.split('/').collect();
        if parts.len() != 2 {
            warn!("Invalid subnet format: {}", self.subnet);
            return Vec::new();
        }

        let base: Ipv4Addr = match parts[0].parse() {
            Ok(ip) => ip,
            Err(_) => {
                warn!("Invalid IP in subnet: {}", parts[0]);
                return Vec::new();
            }
        };

        let prefix: u32 = match parts[1].parse() {
            Ok(p) if p <= 32 => p,
            _ => {
                warn!("Invalid prefix length: {}", parts[1]);
                return Vec::new();
            }
        };

        let base_u32 = u32::from(base);
        let host_bits = 32 - prefix;
        let num_hosts = 1u32 << host_bits;

        // Skip network (.0) and broadcast (.255) addresses
        (1..num_hosts.saturating_sub(1))
            .map(|i| {
                let ip_u32 = (base_u32 & !(num_hosts - 1)) | i;
                IpAddr::V4(Ipv4Addr::from(ip_u32))
            })
            .collect()
    }

    /// Scan all IPs in the subnet for open ports.
    pub async fn scan(&self) -> Vec<DiscoveredNode> {
        let ips = self.parse_subnet();
        if ips.is_empty() {
            return Vec::new();
        }

        debug!("Scanning {} IPs on subnet {}", ips.len(), self.subnet);

        let semaphore = std::sync::Arc::new(Semaphore::new(MAX_CONCURRENT));
        let agent_port = self.agent_port;

        let mut handles = Vec::new();

        for ip in ips {
            let sem = semaphore.clone();
            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.ok()?;
                scan_host(ip, agent_port).await
            });
            handles.push(handle);
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok(Some(node)) = handle.await {
                results.push(node);
            }
        }

        debug!("Scan complete: {} responsive hosts", results.len());
        results
    }
}

/// Probe a single host for open ports.
async fn scan_host(ip: IpAddr, agent_port: u16) -> Option<DiscoveredNode> {
    let mut open_ports = Vec::new();

    // Always check the agent port first
    let mut ports_to_check: Vec<u16> = vec![agent_port];
    for &p in PROBE_PORTS {
        if p != agent_port {
            ports_to_check.push(p);
        }
    }

    for port in &ports_to_check {
        if probe_port(ip, *port).await {
            open_ports.push(*port);
        }
    }

    if open_ports.is_empty() {
        return None;
    }

    let has_agent = open_ports.contains(&agent_port);
    let device_type = infer_device_type(&open_ports);

    Some(DiscoveredNode {
        ip,
        hostname: None,
        open_ports,
        device_type,
        has_agent,
        os_hint: None,
    })
}

/// Try to connect to a specific port with timeout.
async fn probe_port(ip: IpAddr, port: u16) -> bool {
    let addr = format!("{}:{}", ip, port);
    timeout(PROBE_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

/// Infer device type from open ports.
fn infer_device_type(ports: &[u16]) -> DeviceType {
    if ports.contains(&5555) {
        // ADB port -> likely Android device
        DeviceType::Phone
    } else if ports.contains(&5985) {
        // WinRM -> Windows desktop
        DeviceType::Desktop
    } else if ports.contains(&22) {
        // SSH -> could be anything, default to Desktop
        DeviceType::Desktop
    } else {
        DeviceType::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_subnet_24() {
        let scanner = NetworkScanner::new("192.168.1.0/24".into(), 9876);
        let ips = scanner.parse_subnet();
        assert_eq!(ips.len(), 254);
        assert_eq!(ips[0], IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(ips[253], IpAddr::V4(Ipv4Addr::new(192, 168, 1, 254)));
    }

    #[test]
    fn test_parse_subnet_invalid() {
        let scanner = NetworkScanner::new("invalid".into(), 9876);
        let ips = scanner.parse_subnet();
        assert!(ips.is_empty());
    }

    #[test]
    fn test_infer_device_type_adb() {
        assert_eq!(infer_device_type(&[5555, 22]), DeviceType::Phone);
    }

    #[test]
    fn test_infer_device_type_winrm() {
        assert_eq!(infer_device_type(&[5985]), DeviceType::Desktop);
    }

    #[test]
    fn test_infer_device_type_ssh() {
        assert_eq!(infer_device_type(&[22]), DeviceType::Desktop);
    }
}

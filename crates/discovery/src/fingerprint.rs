use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::debug;

use architect_core::types::DeviceType;

use super::discovery::DiscoveredNode;

/// Timeout for fingerprint probes.
const FINGERPRINT_TIMEOUT: Duration = Duration::from_secs(3);

/// Result of fingerprinting a discovered node.
pub struct FingerprintResult {
    pub device_type: DeviceType,
    pub os_hint: Option<String>,
    pub hostname: Option<String>,
}

/// Probes discovered nodes to identify OS and device type.
pub struct DeviceFingerprinter;

impl DeviceFingerprinter {
    pub fn new() -> Self {
        Self
    }

    /// Probe a discovered node using available services.
    pub async fn probe(&self, node: &DiscoveredNode) -> Option<FingerprintResult> {
        // Try SSH banner first (port 22)
        if node.open_ports.contains(&22) {
            if let Some(result) = self.probe_ssh(node).await {
                return Some(result);
            }
        }

        // Try HTTP probe on agent port
        if node.has_agent {
            if let Some(result) = self.probe_http(node).await {
                return Some(result);
            }
        }

        None
    }

    /// Read SSH banner to identify OS.
    async fn probe_ssh(&self, node: &DiscoveredNode) -> Option<FingerprintResult> {
        let addr = format!("{}:22", node.ip);
        let stream = timeout(FINGERPRINT_TIMEOUT, TcpStream::connect(&addr))
            .await
            .ok()?
            .ok()?;

        let mut stream = stream;
        let mut buf = vec![0u8; 256];
        let n = timeout(FINGERPRINT_TIMEOUT, stream.read(&mut buf))
            .await
            .ok()?
            .ok()?;

        let banner = String::from_utf8_lossy(&buf[..n]).to_string();
        debug!("SSH banner from {}: {}", node.ip, banner.trim());

        let os_hint = parse_ssh_banner(&banner);
        let device_type = infer_os_device_type(os_hint.as_deref());

        Some(FingerprintResult {
            device_type,
            os_hint,
            hostname: None,
        })
    }

    /// Probe HTTP endpoint to check for architect agent.
    async fn probe_http(&self, node: &DiscoveredNode) -> Option<FingerprintResult> {
        let port = node.open_ports.first()?;
        let addr = format!("{}:{}", node.ip, port);

        let mut stream = timeout(FINGERPRINT_TIMEOUT, TcpStream::connect(&addr))
            .await
            .ok()?
            .ok()?;

        let request = format!(
            "GET /health HTTP/1.0\r\nHost: {}\r\n\r\n",
            node.ip
        );

        timeout(FINGERPRINT_TIMEOUT, stream.write_all(request.as_bytes()))
            .await
            .ok()?
            .ok()?;

        let mut buf = vec![0u8; 1024];
        let n = timeout(FINGERPRINT_TIMEOUT, stream.read(&mut buf))
            .await
            .ok()?
            .ok()?;

        let response = String::from_utf8_lossy(&buf[..n]).to_string();
        debug!("HTTP response from {}: {}", node.ip, response.lines().next().unwrap_or(""));

        if response.contains("architect") {
            Some(FingerprintResult {
                device_type: node.device_type,
                os_hint: Some("architect-agent".into()),
                hostname: None,
            })
        } else {
            None
        }
    }
}

impl Default for DeviceFingerprinter {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse SSH banner to extract OS hint.
fn parse_ssh_banner(banner: &str) -> Option<String> {
    let lower = banner.to_lowercase();

    if lower.contains("ubuntu") {
        Some("Ubuntu Linux".into())
    } else if lower.contains("debian") {
        Some("Debian Linux".into())
    } else if lower.contains("raspbian") || lower.contains("raspberry") {
        Some("Raspberry Pi OS".into())
    } else if lower.contains("openssh") {
        // Generic OpenSSH — likely Linux or macOS
        if lower.contains("linux") {
            Some("Linux".into())
        } else {
            Some("Unix/Linux".into())
        }
    } else if lower.contains("dropbear") {
        Some("Embedded Linux".into())
    } else {
        None
    }
}

/// Infer device type from OS hint.
fn infer_os_device_type(os_hint: Option<&str>) -> DeviceType {
    match os_hint {
        Some(os) if os.contains("Raspberry") => DeviceType::RaspberryPi,
        Some(os) if os.contains("Embedded") => DeviceType::RaspberryPi,
        Some(os) if os.contains("Android") => DeviceType::Phone,
        _ => DeviceType::Desktop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssh_banner_ubuntu() {
        let banner = "SSH-2.0-OpenSSH_8.9p1 Ubuntu-3ubuntu0.1\r\n";
        assert_eq!(parse_ssh_banner(banner), Some("Ubuntu Linux".into()));
    }

    #[test]
    fn test_parse_ssh_banner_generic() {
        let banner = "SSH-2.0-OpenSSH_9.0\r\n";
        assert_eq!(parse_ssh_banner(banner), Some("Unix/Linux".into()));
    }

    #[test]
    fn test_parse_ssh_banner_dropbear() {
        let banner = "SSH-2.0-dropbear_2022.83\r\n";
        assert_eq!(parse_ssh_banner(banner), Some("Embedded Linux".into()));
    }

    #[test]
    fn test_infer_device_raspberry() {
        assert_eq!(infer_os_device_type(Some("Raspberry Pi OS")), DeviceType::RaspberryPi);
    }
}

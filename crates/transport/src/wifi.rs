use architect_core::transport::TransportType;

/// WiFi transport configuration with QoS support.
/// Selects TCP or UDP based on payload characteristics.
pub struct WiFiConfig {
    pub use_dscp: bool,
    pub prefer_udp_for_small: bool,
    pub small_payload_threshold: usize,
}

impl Default for WiFiConfig {
    fn default() -> Self {
        Self {
            use_dscp: true,
            prefer_udp_for_small: true,
            small_payload_threshold: 1400, // fits in single MTU
        }
    }
}

/// Determine the best WiFi transport variant based on available information.
pub fn detect_wifi_type() -> TransportType {
    // In practice, this would check the WiFi adapter's frequency band
    // For now, default to 5GHz
    TransportType::WiFi5GHz
}

/// Determine DSCP marking for QoS based on payload priority.
pub fn dscp_marking(priority: u8) -> u8 {
    match priority {
        0..=2 => 0x00,   // Best effort
        3..=5 => 0x22,   // AF21 - Low latency data
        6..=8 => 0x2E,   // EF - Expedited forwarding (real-time)
        _ => 0x00,
    }
}

/// Choose between TCP and UDP based on payload size.
pub fn select_protocol(payload_size: usize, config: &WiFiConfig) -> Protocol {
    if config.prefer_udp_for_small && payload_size <= config.small_payload_threshold {
        Protocol::Udp
    } else {
        Protocol::Tcp
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dscp_marking() {
        assert_eq!(dscp_marking(0), 0x00);
        assert_eq!(dscp_marking(5), 0x22);
        assert_eq!(dscp_marking(7), 0x2E);
    }

    #[test]
    fn test_select_protocol_small() {
        let config = WiFiConfig::default();
        assert_eq!(select_protocol(100, &config), Protocol::Udp);
    }

    #[test]
    fn test_select_protocol_large() {
        let config = WiFiConfig::default();
        assert_eq!(select_protocol(10000, &config), Protocol::Tcp);
    }
}

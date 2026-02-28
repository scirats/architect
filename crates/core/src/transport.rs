use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::types::NodeId;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TransportType {
    Ethernet,
    Powerline,
    WiFi5GHz,
    WiFi24GHz,
    WiFi6E,
    WiFiDirect,
    Bluetooth,
    UsbTethering,
    WebSocket,
    Quic,
}

impl TransportType {
    /// Higher number = higher priority for data transfer.
    pub fn priority(&self) -> u8 {
        match self {
            Self::Ethernet | Self::Powerline => 100,
            Self::WiFi6E => 90,
            Self::Quic => 85,
            Self::WiFi5GHz => 80,
            Self::WiFi24GHz => 60,
            Self::UsbTethering => 55,
            Self::WiFiDirect => 50,
            Self::WebSocket => 30,
            Self::Bluetooth => 20,
        }
    }

    /// Estimated bandwidth in Mbps for this transport type.
    pub fn bandwidth_estimate_mbps(&self) -> f32 {
        match self {
            Self::Ethernet | Self::Powerline => 1000.0,
            Self::WiFi6E => 2400.0,
            Self::Quic => 800.0,
            Self::WiFi5GHz => 400.0,
            Self::WiFi24GHz => 100.0,
            Self::UsbTethering => 480.0,
            Self::WiFiDirect => 250.0,
            Self::WebSocket => 50.0,
            Self::Bluetooth => 3.0,
        }
    }

    /// Whether this transport can handle large data transfers (>10MB).
    pub fn suitable_for_large_data(&self) -> bool {
        self.bandwidth_estimate_mbps() > 50.0
    }
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ethernet => write!(f, "Ethernet"),
            Self::Powerline => write!(f, "Powerline"),
            Self::WiFi5GHz => write!(f, "WiFi 5GHz"),
            Self::WiFi24GHz => write!(f, "WiFi 2.4GHz"),
            Self::WiFi6E => write!(f, "WiFi 6E"),
            Self::WiFiDirect => write!(f, "WiFi Direct"),
            Self::Bluetooth => write!(f, "Bluetooth"),
            Self::UsbTethering => write!(f, "USB Tethering"),
            Self::WebSocket => write!(f, "WebSocket"),
            Self::Quic => write!(f, "QUIC"),
        }
    }
}

/// Trait that all transport implementations must satisfy.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send data to a specific node.
    async fn send(&self, data: &[u8], target: NodeId) -> Result<(), CoreError>;

    /// Receive data from any node. Returns (data, sender_id).
    async fn receive(&self) -> Result<(Vec<u8>, NodeId), CoreError>;

    /// Broadcast data to all known nodes.
    async fn broadcast(&self, data: &[u8]) -> Result<(), CoreError>;

    /// Current measured bandwidth in Mbps.
    fn bandwidth_mbps(&self) -> f32;

    /// Current measured latency in milliseconds.
    fn latency_ms(&self) -> f32;

    /// Whether the transport is currently available.
    fn is_available(&self) -> bool;

    /// The type of this transport.
    fn transport_type(&self) -> TransportType;
}

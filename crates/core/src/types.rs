use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type NodeId = Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NodeRole {
    Client,
    Agent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OsKind {
    Linux,
    MacOS,
    Windows,
    Android,
    IOS,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchKind {
    X86_64,
    X86,
    Aarch64,
    Armv7,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceType {
    Desktop,
    Laptop,
    Phone,
    SmartTV,
    RaspberryPi,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vram_gb: f32,
    pub compute_capability: Option<String>,
    pub opencl_available: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BatteryInfo {
    pub level_pct: u8,
    pub charging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapabilities {
    pub cpu_cores: u32,
    pub cpu_freq_mhz: u32,
    pub ram_total_mb: u64,
    pub ram_available_mb: u64,
    pub gpu: Option<GpuInfo>,
    pub battery: Option<BatteryInfo>,
    pub compute_score: f32,
}

impl NodeCapabilities {
    /// Calculate a compute score based on hardware capabilities.
    /// Higher score = more capable node.
    pub fn calculate_score(&self) -> f32 {
        let cpu_score = self.cpu_cores as f32 * (self.cpu_freq_mhz as f32 / 1000.0);
        let ram_score = (self.ram_total_mb as f32 / 1024.0).min(64.0);
        let gpu_score = self
            .gpu
            .as_ref()
            .map(|g| g.vram_gb * 10.0)
            .unwrap_or(0.0);

        cpu_score + ram_score + gpu_score
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub role: NodeRole,
    pub hostname: String,
    pub ip: String,
    pub os: OsKind,
    pub arch: ArchKind,
    pub device_type: DeviceType,
    pub capabilities: NodeCapabilities,
    /// Authentication token for cluster membership (empty = no auth).
    #[serde(default)]
    pub auth_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub cpu_usage_pct: f32,
    pub ram_usage_pct: f32,
    pub gpu_usage_pct: Option<f32>,
    pub network_rx_mbps: f32,
    pub network_tx_mbps: f32,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Degraded,
    Offline,
}

// --- Display implementations ---

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeRole::Client => write!(f, "Client"),
            NodeRole::Agent => write!(f, "Agent"),
        }
    }
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeStatus::Online => write!(f, "Online"),
            NodeStatus::Degraded => write!(f, "Degraded"),
            NodeStatus::Offline => write!(f, "Offline"),
        }
    }
}

impl std::fmt::Display for OsKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OsKind::Linux => write!(f, "Linux"),
            OsKind::MacOS => write!(f, "macOS"),
            OsKind::Windows => write!(f, "Windows"),
            OsKind::Android => write!(f, "Android"),
            OsKind::IOS => write!(f, "iOS"),
            OsKind::Unknown => write!(f, "Unknown"),
        }
    }
}

impl std::fmt::Display for ArchKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchKind::X86_64 => write!(f, "x86_64"),
            ArchKind::X86 => write!(f, "x86"),
            ArchKind::Aarch64 => write!(f, "aarch64"),
            ArchKind::Armv7 => write!(f, "armv7"),
            ArchKind::Unknown => write!(f, "unknown"),
        }
    }
}

impl std::fmt::Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceType::Desktop => write!(f, "Desktop"),
            DeviceType::Laptop => write!(f, "Laptop"),
            DeviceType::Phone => write!(f, "Phone"),
            DeviceType::SmartTV => write!(f, "SmartTV"),
            DeviceType::RaspberryPi => write!(f, "RaspberryPi"),
            DeviceType::Unknown => write!(f, "Unknown"),
        }
    }
}

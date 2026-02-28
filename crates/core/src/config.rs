use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub listen_addr: SocketAddr,
    pub heartbeat_interval_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub tick_rate_ms: u64,
    pub frame_rate_ms: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:9876".parse().unwrap(),
            heartbeat_interval_ms: 2000,
            heartbeat_timeout_ms: 6000,
            tick_rate_ms: 250,
            frame_rate_ms: 16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub coordinator_addr: SocketAddr,
    pub heartbeat_interval_ms: u64,
    pub reconnect_delay_ms: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            coordinator_addr: "127.0.0.1:9876".parse().unwrap(),
            heartbeat_interval_ms: 2000,
            reconnect_delay_ms: 3000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorConfig {
    pub port: u16,
    pub scan_network_on_start: bool,
    pub rebalance_interval_secs: u64,
    /// Shared secret for agent authentication. Auto-generated on first run.
    #[serde(default = "generate_cluster_token")]
    pub cluster_token: String,
}

fn generate_cluster_token() -> String {
    Uuid::new_v4().to_string()
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            port: 9876,
            scan_network_on_start: false,
            rebalance_interval_secs: 2,
            cluster_token: generate_cluster_token(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    pub prefer_quic: bool,
    pub multipath_enabled: bool,
    pub gradient_compression: bool,
    pub gradient_top_k_percent: f32,
    pub bluetooth_for_control_only: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            prefer_quic: true,
            multipath_enabled: true,
            gradient_compression: true,
            gradient_top_k_percent: 1.0,
            bluetooth_for_control_only: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub algorithm: SchedulerAlgorithm,
    pub battery_limit_pct: u8,
    pub cpu_limit_pct: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SchedulerAlgorithm {
    RoundRobin,
    Weighted,
    Adaptive,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            algorithm: SchedulerAlgorithm::Adaptive,
            battery_limit_pct: 20,
            cpu_limit_pct: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlConfig {
    pub default_backend: MlBackend,
    pub gradient_sync: GradientSync,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MlBackend {
    Candle,
    Burn,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GradientSync {
    RingAllReduce,
    ParameterServer,
}

impl Default for MlConfig {
    fn default() -> Self {
        Self {
            default_backend: MlBackend::Candle,
            gradient_sync: GradientSync::RingAllReduce,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    pub beacon_enabled: bool,
    pub beacon_port: u16,
    pub beacon_interval_ms: u64,
    pub mdns_enabled: bool,
    pub network_scan_enabled: bool,
    pub scan_subnet: String,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            beacon_enabled: true,
            beacon_port: 9877,
            beacon_interval_ms: 3000,
            mdns_enabled: true,
            network_scan_enabled: false,
            scan_subnet: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    pub ssh_timeout_secs: u64,
    pub adb_enabled: bool,
    pub winrm_enabled: bool,
    pub serve_agent_http: bool,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            ssh_timeout_secs: 10,
            adb_enabled: true,
            winrm_enabled: true,
            serve_agent_http: true,
        }
    }
}

/// Limits for local task execution — throttles when resources are constrained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConfig {
    /// Max CPU usage (%) before rejecting new local tasks.
    pub cpu_limit_pct: f32,
    /// Max RAM usage (%) before rejecting new local tasks.
    pub ram_limit_pct: f32,
    /// Min battery level (%) to accept tasks when not charging.
    pub battery_min_pct: u8,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            cpu_limit_pct: 85.0,
            ram_limit_pct: 90.0,
            battery_min_pct: 20,
        }
    }
}

fn default_github_repo() -> String {
    "scirats/architect".to_string()
}

/// Top-level configuration aggregating all sub-configs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArchitectConfig {
    #[serde(default = "default_github_repo")]
    pub github_repo: String,
    #[serde(default)]
    pub coordinator: CoordinatorConfig,
    #[serde(default)]
    pub transport: TransportConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub ml: MlConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub bootstrap: BootstrapConfig,
    #[serde(default)]
    pub resources: ResourceConfig,
}

/// Returns the platform-specific config directory for architect.
///
/// - Linux/macOS: `~/.config/architect/`
/// - Windows: `%APPDATA%\architect\`
pub fn config_dir() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return std::path::PathBuf::from(appdata).join("architect");
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return std::path::PathBuf::from(xdg).join("architect");
        }
    }

    // Fallback: ~/.config/architect
    if let Some(home) = home_dir() {
        return home.join(".config").join("architect");
    }

    // Last resort
    std::path::PathBuf::from(".architect")
}

/// Returns the full path to the config file.
pub fn config_path() -> std::path::PathBuf {
    config_dir().join("config.toml")
}

/// Returns `~/.architect/` — the data directory for non-config files
/// (task journal, certs, logs, etc.).
pub fn data_dir() -> std::path::PathBuf {
    if let Some(home) = home_dir() {
        home.join(".architect")
    } else {
        std::path::PathBuf::from(".architect")
    }
}

/// Returns `~/.architect/logs/`.
pub fn log_dir() -> std::path::PathBuf {
    data_dir().join("logs")
}

/// Load configuration from the platform-specific path.
/// Creates the config file with defaults on first run.
pub fn load_or_create_config() -> ArchitectConfig {
    let path = config_path();

    if path.exists() {
        load_config(&path.to_string_lossy())
    } else {
        let config = ArchitectConfig::default();

        if let Err(e) = save_config(&config, &path.to_string_lossy()) {
            tracing::warn!("Could not create default config at {:?}: {}", path, e);
        } else {
            tracing::info!("Created default config at {:?}", path);
        }

        config
    }
}

/// Load configuration from a TOML file.
/// Falls back to defaults if the file doesn't exist.
pub fn load_config(path: &str) -> ArchitectConfig {
    match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(config) => {
                tracing::info!("Loaded config from {}", path);
                config
            }
            Err(e) => {
                tracing::warn!("Failed to parse config {}: {}, using defaults", path, e);
                ArchitectConfig::default()
            }
        },
        Err(_) => {
            tracing::debug!("Config file {} not found, using defaults", path);
            ArchitectConfig::default()
        }
    }
}

/// Save configuration to a TOML file.
/// Creates parent directories if they don't exist.
pub fn save_config(config: &ArchitectConfig, path: &str) -> anyhow::Result<()> {
    let path = std::path::Path::new(path);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let contents = toml::to_string_pretty(config)?;
    std::fs::write(path, contents)?;
    tracing::info!("Config saved to {:?}", path);
    Ok(())
}

/// Get the user's home directory.
fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE").ok().map(std::path::PathBuf::from)
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(std::path::PathBuf::from)
    }
}

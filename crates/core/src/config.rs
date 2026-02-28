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

fn deserialize_github_repo<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = serde::Deserialize::deserialize(deserializer)?;
    if s.is_empty() {
        Ok(default_github_repo())
    } else {
        Ok(s)
    }
}

/// Top-level configuration aggregating all sub-configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectConfig {
    #[serde(default = "default_github_repo", deserialize_with = "deserialize_github_repo")]
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

impl Default for ArchitectConfig {
    fn default() -> Self {
        Self {
            github_repo: default_github_repo(),
            coordinator: CoordinatorConfig::default(),
            transport: TransportConfig::default(),
            scheduler: SchedulerConfig::default(),
            ml: MlConfig::default(),
            discovery: DiscoveryConfig::default(),
            bootstrap: BootstrapConfig::default(),
            resources: ResourceConfig::default(),
        }
    }
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

/// Diagnostic result for a single config check.
pub struct DiagCheck {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// Run diagnostics on the config file and environment.
/// Returns a list of checks with pass/fail status.
pub fn doctor(config_path_override: Option<&str>) -> Vec<DiagCheck> {
    let mut checks = Vec::new();

    // 1. Config file exists
    let path = match config_path_override {
        Some(p) => std::path::PathBuf::from(p),
        None => config_path(),
    };
    let path_str = path.to_string_lossy().to_string();
    checks.push(DiagCheck {
        name: "config file",
        ok: path.exists(),
        detail: if path.exists() {
            format!("found at {}", path_str)
        } else {
            format!("not found at {} (run the client once to generate it)", path_str)
        },
    });

    if !path.exists() {
        return checks;
    }

    // 2. Config file is valid TOML
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            checks.push(DiagCheck {
                name: "config readable",
                ok: false,
                detail: format!("cannot read: {}", e),
            });
            return checks;
        }
    };

    let config: ArchitectConfig = match toml::from_str(&contents) {
        Ok(c) => {
            checks.push(DiagCheck {
                name: "config parseable",
                ok: true,
                detail: "valid TOML".into(),
            });
            c
        }
        Err(e) => {
            checks.push(DiagCheck {
                name: "config parseable",
                ok: false,
                detail: format!("parse error: {}", e),
            });
            return checks;
        }
    };

    // 3. github_repo
    checks.push(DiagCheck {
        name: "github_repo",
        ok: !config.github_repo.is_empty() && config.github_repo.contains('/'),
        detail: if config.github_repo.is_empty() {
            "empty — set to \"owner/repo\"".into()
        } else if !config.github_repo.contains('/') {
            format!("\"{}\" — must be in owner/repo format", config.github_repo)
        } else {
            config.github_repo.clone()
        },
    });

    // 4. cluster_token
    checks.push(DiagCheck {
        name: "cluster_token",
        ok: !config.coordinator.cluster_token.is_empty(),
        detail: if config.coordinator.cluster_token.is_empty() {
            "empty — agents won't authenticate".into()
        } else {
            format!("{}...{}", &config.coordinator.cluster_token[..8],
                &config.coordinator.cluster_token[config.coordinator.cluster_token.len().saturating_sub(4)..])
        },
    });

    // 5. coordinator port
    checks.push(DiagCheck {
        name: "coordinator.port",
        ok: config.coordinator.port > 0,
        detail: format!("{}", config.coordinator.port),
    });

    // 6. beacon config
    checks.push(DiagCheck {
        name: "discovery.beacon",
        ok: true,
        detail: if config.discovery.beacon_enabled {
            format!("enabled on port {} (interval {}ms)", config.discovery.beacon_port, config.discovery.beacon_interval_ms)
        } else {
            "disabled — agents won't auto-discover this coordinator".into()
        },
    });

    // 7. mDNS
    checks.push(DiagCheck {
        name: "discovery.mdns",
        ok: true,
        detail: if config.discovery.mdns_enabled { "enabled".into() } else { "disabled".into() },
    });

    // 8. resource limits
    let cpu_ok = config.resources.cpu_limit_pct > 0.0 && config.resources.cpu_limit_pct <= 100.0;
    let ram_ok = config.resources.ram_limit_pct > 0.0 && config.resources.ram_limit_pct <= 100.0;
    checks.push(DiagCheck {
        name: "resources.cpu_limit",
        ok: cpu_ok,
        detail: if cpu_ok {
            format!("{}%", config.resources.cpu_limit_pct)
        } else {
            format!("{}% — must be between 0 and 100", config.resources.cpu_limit_pct)
        },
    });
    checks.push(DiagCheck {
        name: "resources.ram_limit",
        ok: ram_ok,
        detail: if ram_ok {
            format!("{}%", config.resources.ram_limit_pct)
        } else {
            format!("{}% — must be between 0 and 100", config.resources.ram_limit_pct)
        },
    });

    // 9. TLS certs
    let certs_dir = crate::tls::certs_dir();
    let ca_cert = certs_dir.join("ca-cert.pem");
    let ca_key = certs_dir.join("ca-key.pem");
    checks.push(DiagCheck {
        name: "tls.ca_cert",
        ok: ca_cert.exists(),
        detail: if ca_cert.exists() {
            format!("found at {}", ca_cert.display())
        } else {
            format!("not found at {} (run :bless or generate certs)", ca_cert.display())
        },
    });
    checks.push(DiagCheck {
        name: "tls.ca_key",
        ok: ca_key.exists(),
        detail: if ca_key.exists() {
            format!("found at {}", ca_key.display())
        } else {
            format!("not found at {}", ca_key.display())
        },
    });

    // 10. Log directory writable
    let log = log_dir();
    checks.push(DiagCheck {
        name: "log_dir",
        ok: log.exists(),
        detail: if log.exists() {
            format!("{}", log.display())
        } else {
            format!("{} — does not exist", log.display())
        },
    });

    checks
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

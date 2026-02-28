use architect_core::types::{
    ArchKind, DeviceType, NodeCapabilities, NodeId, NodeInfo, NodeMetrics, NodeRole, OsKind,
};
use sysinfo::System;
use uuid::Uuid;

pub struct MetricsCollector {
    system: System,
    node_id: NodeId,
    tasks_completed: u64,
    tasks_failed: u64,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let node_id = load_or_create_node_id();
        let mut system = System::new_all();
        system.refresh_all();
        Self {
            system,
            node_id,
            tasks_completed: 0,
            tasks_failed: 0,
        }
    }

    pub fn collect_info(&self) -> NodeInfo {
        let hostname = System::host_name().unwrap_or_else(|| "unknown".into());
        let os = detect_os();
        let arch = detect_arch();
        let device_type = detect_device_type();

        let cpu_freq_mhz = self
            .system
            .cpus()
            .first()
            .map(|cpu| cpu.frequency() as u32)
            .unwrap_or(0);

        let gpu = crate::gpu::detect_gpu();
        let battery = crate::battery::detect_battery();

        let mut capabilities = NodeCapabilities {
            cpu_cores: self.system.cpus().len() as u32,
            cpu_freq_mhz,
            ram_total_mb: self.system.total_memory() / 1024 / 1024,
            ram_available_mb: self.system.available_memory() / 1024 / 1024,
            gpu,
            battery,
            compute_score: 0.0,
        };
        capabilities.compute_score = capabilities.calculate_score();

        NodeInfo {
            id: self.node_id,
            role: NodeRole::Agent,
            hostname,
            ip: String::new(), // filled by coordinator on connection
            os,
            arch,
            device_type,
            capabilities,
            auth_token: String::new(), // set by main before sending
        }
    }

    pub fn collect_metrics(&mut self) -> NodeMetrics {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();

        let cpu_usage = self.system.global_cpu_usage();
        let ram_total = self.system.total_memory() as f32;
        let ram_used = self.system.used_memory() as f32;
        let ram_usage = if ram_total > 0.0 {
            (ram_used / ram_total) * 100.0
        } else {
            0.0
        };

        NodeMetrics {
            cpu_usage_pct: cpu_usage,
            ram_usage_pct: ram_usage,
            gpu_usage_pct: crate::gpu::monitor_gpu_usage(),
            network_rx_mbps: 0.0,
            network_tx_mbps: 0.0,
            tasks_completed: self.tasks_completed,
            tasks_failed: self.tasks_failed,
            uptime_secs: System::uptime(),
        }
    }
}

fn detect_os() -> OsKind {
    if cfg!(target_os = "linux") {
        OsKind::Linux
    } else if cfg!(target_os = "macos") {
        OsKind::MacOS
    } else if cfg!(target_os = "windows") {
        OsKind::Windows
    } else if cfg!(target_os = "android") {
        OsKind::Android
    } else if cfg!(target_os = "ios") {
        OsKind::IOS
    } else {
        OsKind::Unknown
    }
}

fn detect_arch() -> ArchKind {
    if cfg!(target_arch = "x86_64") {
        ArchKind::X86_64
    } else if cfg!(target_arch = "aarch64") {
        ArchKind::Aarch64
    } else if cfg!(target_arch = "x86") {
        ArchKind::X86
    } else if cfg!(target_arch = "arm") {
        ArchKind::Armv7
    } else {
        ArchKind::Unknown
    }
}

fn detect_device_type() -> DeviceType {
    if cfg!(target_os = "android") {
        return DeviceType::Phone;
    }

    // Heuristic: battery present → laptop, otherwise desktop
    if crate::battery::detect_battery().is_some() {
        DeviceType::Laptop
    } else {
        DeviceType::Desktop
    }
}

/// Load the persistent node ID from `~/.architect/node_id`, or generate and save a new one.
fn load_or_create_node_id() -> NodeId {
    let path = architect_core::config::data_dir().join("node_id");
    if let Ok(contents) = std::fs::read_to_string(&path) {
        if let Ok(id) = contents.trim().parse::<Uuid>() {
            return id;
        }
    }
    let id = Uuid::new_v4();
    let dir = architect_core::config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, id.to_string());
    id
}

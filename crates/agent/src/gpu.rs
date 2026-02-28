use architect_core::types::GpuInfo;
use tracing::debug;

/// Attempt to detect any available GPU.
pub fn detect_gpu() -> Option<GpuInfo> {
    // Try platform-specific detection
    #[cfg(target_os = "macos")]
    if let Some(gpu) = detect_macos_gpu() {
        return Some(gpu);
    }

    #[cfg(target_os = "linux")]
    if let Some(gpu) = detect_linux_gpu() {
        return Some(gpu);
    }

    debug!("No GPU detected");
    None
}

/// Monitor current GPU usage percentage (if available).
pub fn monitor_gpu_usage() -> Option<f32> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f32>()
        .ok()
}

#[cfg(target_os = "macos")]
fn detect_macos_gpu() -> Option<GpuInfo> {
    use std::process::Command;

    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Parse JSON output to extract GPU info
    let text = String::from_utf8_lossy(&output.stdout);

    // Simple heuristic: look for chipset model name
    let name = extract_json_value(&text, "sppci_model")
        .unwrap_or_else(|| "Apple GPU".to_string());

    // macOS Metal GPUs share system RAM
    let vram_gb = extract_json_value(&text, "spdisplays_vram_shared")
        .and_then(|v| v.trim_end_matches(" GB").parse::<f32>().ok())
        .unwrap_or(0.0);

    debug!("Detected macOS GPU: {} ({:.1} GB)", name, vram_gb);

    Some(GpuInfo {
        name,
        vram_gb,
        compute_capability: None,
        opencl_available: true, // macOS has OpenCL
    })
}

#[cfg(target_os = "linux")]
fn detect_linux_gpu() -> Option<GpuInfo> {
    // Check for NVIDIA GPUs via /proc
    if let Ok(entries) = std::fs::read_dir("/proc/driver/nvidia/gpus") {
        for entry in entries.flatten() {
            if let Ok(info) = std::fs::read_to_string(entry.path().join("information")) {
                let name = info
                    .lines()
                    .find(|l| l.starts_with("Model:"))
                    .map(|l| l.trim_start_matches("Model:").trim().to_string())
                    .unwrap_or_else(|| "NVIDIA GPU".to_string());

                debug!("Detected Linux NVIDIA GPU: {}", name);
                return Some(GpuInfo {
                    name,
                    vram_gb: 0.0, // Would need NVML for accurate value
                    compute_capability: None,
                    opencl_available: true,
                });
            }
        }
    }

    // Check for AMD GPUs via /sys/class/drm
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let path = entry.path().join("device/vendor");
            if let Ok(vendor) = std::fs::read_to_string(&path) {
                if vendor.trim() == "0x1002" {
                    // AMD vendor ID
                    let name = "AMD GPU".to_string();
                    debug!("Detected Linux AMD GPU");
                    return Some(GpuInfo {
                        name,
                        vram_gb: 0.0,
                        compute_capability: None,
                        opencl_available: true,
                    });
                }
            }
        }
    }

    None
}

/// Simple JSON value extractor (avoids adding serde_json dep to agent).
#[cfg(target_os = "macos")]
fn extract_json_value(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\" : \"", key);
    let start = json.find(&pattern)? + pattern.len();
    let end = json[start..].find('"')? + start;
    Some(json[start..end].to_string())
}

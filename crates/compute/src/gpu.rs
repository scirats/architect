use std::process::Command;
use std::time::Instant;

use tracing::{debug, info, warn};

use crate::backend::{ComputeBackend, ComputeBackendType, ComputeResult};

/// Check if CUDA is available on this system by probing the NVIDIA driver.
pub fn is_cuda_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/proc/driver/nvidia").exists() {
            debug!("CUDA detected via /proc/driver/nvidia");
            return true;
        }
    }

    if Command::new("nvidia-smi")
        .arg("--query-gpu=name")
        .arg("--format=csv,noheader")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        debug!("CUDA detected via nvidia-smi");
        return true;
    }

    false
}

/// Check if Metal compute is available (macOS only).
pub fn is_metal_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("system_profiler")
            .args(["SPDisplaysDataType"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.contains("Metal") {
                debug!("Metal GPU support detected");
                return true;
            }
        }
    }

    false
}

/// Query current GPU utilization percentage via nvidia-smi.
pub fn query_gpu_usage() -> Option<f32> {
    let output = Command::new("nvidia-smi")
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

/// GPU compute backend (CUDA/Metal).
/// Computation still uses CPU — actual GPU kernels require native SDK integration.
pub struct GpuCompute {
    detected: ComputeBackendType,
}

impl GpuCompute {
    pub fn new() -> Self {
        let detected = if is_cuda_available() {
            info!("GPU compute: CUDA device detected (kernels use CPU fallback)");
            ComputeBackendType::Cuda
        } else if is_metal_available() {
            info!("GPU compute: Metal device detected (kernels use CPU fallback)");
            ComputeBackendType::OpenCl
        } else {
            warn!("GPU compute: no GPU detected, using CPU fallback");
            ComputeBackendType::Cpu
        };
        Self { detected }
    }
}

impl Default for GpuCompute {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputeBackend for GpuCompute {
    fn matmul(&self, a: &[f32], b: &[f32], m: usize, n: usize, k: usize) -> ComputeResult {
        let start = Instant::now();

        let mut c = vec![0.0f32; m * n];
        for i in 0..m {
            for j in 0..n {
                let mut sum = 0.0;
                for p in 0..k {
                    sum += a[i * k + p] * b[p * n + j];
                }
                c[i * n + j] = sum;
            }
        }

        ComputeResult {
            data: c,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    fn vector_add(&self, a: &[f32], b: &[f32]) -> ComputeResult {
        let start = Instant::now();
        let data: Vec<f32> = a.iter().zip(b.iter()).map(|(x, y)| x + y).collect();

        ComputeResult {
            data,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    fn process_shard(&self, data: &[f32], scale: f32) -> ComputeResult {
        let start = Instant::now();
        let result: Vec<f32> = data.iter().map(|x| x * scale).collect();

        ComputeResult {
            data: result,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    fn backend_type(&self) -> ComputeBackendType {
        self.detected
    }
}

use tracing::info;

/// Result of a compute operation.
#[derive(Debug, Clone)]
pub struct ComputeResult {
    pub data: Vec<f32>,
    pub duration_ms: u64,
}

/// Available compute backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeBackendType {
    Cpu,
    Cuda,
    OpenCl,
    Wgpu,
}

/// Trait for compute backends.
pub trait ComputeBackend: Send + Sync {
    /// Matrix multiplication: C = A * B
    fn matmul(&self, a: &[f32], b: &[f32], m: usize, n: usize, k: usize) -> ComputeResult;

    /// Element-wise vector addition: C = A + B
    fn vector_add(&self, a: &[f32], b: &[f32]) -> ComputeResult;

    /// Dot product of two vectors.
    fn dot_product(&self, a: &[f32], b: &[f32]) -> f32;

    /// Process a data shard (generic parallel map).
    fn process_shard(&self, data: &[f32], scale: f32) -> ComputeResult;

    /// Name of this backend.
    fn backend_type(&self) -> ComputeBackendType;
}

/// Auto-detect the best available compute backend.
pub fn auto_detect() -> Box<dyn ComputeBackend> {
    #[cfg(feature = "cuda")]
    {
        if super::gpu::is_cuda_available() {
            info!("Using CUDA compute backend");
            return Box::new(super::gpu::GpuCompute::new());
        }
    }

    #[cfg(feature = "wgpu")]
    {
        if let Some(gpu) = super::wgpu_backend::WgpuCompute::new() {
            info!("Using wgpu compute backend");
            return Box::new(gpu);
        }
    }

    #[cfg(feature = "cpu")]
    {
        info!("Using CPU (Rayon) compute backend");
        Box::new(super::cpu::CpuCompute::new())
    }

    #[cfg(not(feature = "cpu"))]
    {
        panic!("No compute backend available. Enable 'cpu', 'cuda', or 'wgpu' feature.");
    }
}

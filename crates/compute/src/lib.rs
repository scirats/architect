pub mod backend;
pub mod cpu;

#[cfg(feature = "cuda")]
pub mod gpu;

#[cfg(feature = "wgpu")]
pub mod wgpu_backend;

pub use backend::{ComputeBackend, ComputeResult};

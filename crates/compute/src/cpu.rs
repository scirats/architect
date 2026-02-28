use std::time::Instant;

use rayon::prelude::*;
use tracing::debug;

use crate::backend::{ComputeBackend, ComputeBackendType, ComputeResult};

/// CPU compute backend using Rayon for parallelism.
pub struct CpuCompute {
    num_threads: usize,
}

impl CpuCompute {
    pub fn new() -> Self {
        let num_threads = rayon::current_num_threads();
        debug!("CpuCompute initialized with {} threads", num_threads);
        Self { num_threads }
    }

    pub fn num_threads(&self) -> usize {
        self.num_threads
    }
}

impl Default for CpuCompute {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputeBackend for CpuCompute {
    fn matmul(&self, a: &[f32], b: &[f32], m: usize, n: usize, k: usize) -> ComputeResult {
        assert_eq!(a.len(), m * k, "Matrix A dimensions mismatch");
        assert_eq!(b.len(), k * n, "Matrix B dimensions mismatch");

        let start = Instant::now();

        let mut c = vec![0.0f32; m * n];

        // Parallel over rows of C
        c.par_chunks_mut(n).enumerate().for_each(|(i, row)| {
            for j in 0..n {
                let mut sum = 0.0f32;
                for p in 0..k {
                    sum += a[i * k + p] * b[p * n + j];
                }
                row[j] = sum;
            }
        });

        let duration_ms = start.elapsed().as_millis() as u64;
        debug!("matmul {}x{}x{} completed in {}ms", m, k, n, duration_ms);

        ComputeResult {
            data: c,
            duration_ms,
        }
    }

    fn vector_add(&self, a: &[f32], b: &[f32]) -> ComputeResult {
        assert_eq!(a.len(), b.len(), "Vector dimensions mismatch");

        let start = Instant::now();

        let data: Vec<f32> = a
            .par_iter()
            .zip(b.par_iter())
            .map(|(x, y)| x + y)
            .collect();

        let duration_ms = start.elapsed().as_millis() as u64;
        debug!("vector_add len={} completed in {}ms", a.len(), duration_ms);

        ComputeResult { data, duration_ms }
    }

    fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len(), "Vector dimensions mismatch");

        a.par_iter()
            .zip(b.par_iter())
            .map(|(x, y)| x * y)
            .sum()
    }

    fn process_shard(&self, data: &[f32], scale: f32) -> ComputeResult {
        let start = Instant::now();

        let result: Vec<f32> = data
            .par_iter()
            .map(|x| x * scale)
            .collect();

        let duration_ms = start.elapsed().as_millis() as u64;
        debug!("process_shard len={} scale={} completed in {}ms", data.len(), scale, duration_ms);

        ComputeResult {
            data: result,
            duration_ms,
        }
    }

    fn backend_type(&self) -> ComputeBackendType {
        ComputeBackendType::Cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matmul_identity() {
        let cpu = CpuCompute::new();

        // 2x2 identity * 2x2 matrix
        let a = vec![1.0, 0.0, 0.0, 1.0]; // identity
        let b = vec![5.0, 6.0, 7.0, 8.0];

        let result = cpu.matmul(&a, &b, 2, 2, 2);
        assert_eq!(result.data, vec![5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn test_matmul_simple() {
        let cpu = CpuCompute::new();

        let a = vec![1.0, 2.0, 3.0, 4.0]; // 2x2
        let b = vec![5.0, 6.0, 7.0, 8.0]; // 2x2

        let result = cpu.matmul(&a, &b, 2, 2, 2);
        // [1*5+2*7, 1*6+2*8] = [19, 22]
        // [3*5+4*7, 3*6+4*8] = [43, 50]
        assert_eq!(result.data, vec![19.0, 22.0, 43.0, 50.0]);
    }

    #[test]
    fn test_vector_add() {
        let cpu = CpuCompute::new();

        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];

        let result = cpu.vector_add(&a, &b);
        assert_eq!(result.data, vec![5.0, 7.0, 9.0]);
    }

    #[test]
    fn test_dot_product() {
        let cpu = CpuCompute::new();

        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];

        let result = cpu.dot_product(&a, &b);
        assert!((result - 32.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_process_shard() {
        let cpu = CpuCompute::new();

        let data = vec![1.0, 2.0, 3.0, 4.0];
        let result = cpu.process_shard(&data, 2.0);
        assert_eq!(result.data, vec![2.0, 4.0, 6.0, 8.0]);
    }
}

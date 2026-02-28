use half::f16;
use serde::{Deserialize, Serialize};

/// Sparse gradient representation for top-k compression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseGradient {
    pub indices: Vec<u32>,
    pub values: Vec<f32>,
    pub original_len: usize,
}

/// Gradient compression utilities for reducing network traffic.
pub struct GradientCompressor;

impl GradientCompressor {
    /// Quantize f32 gradients to f16, reducing size by 50%.
    pub fn quantize_f16(gradients: &[f32]) -> Vec<u8> {
        let mut result = Vec::with_capacity(gradients.len() * 2);
        for &val in gradients {
            let half = f16::from_f32(val);
            result.extend_from_slice(&half.to_le_bytes());
        }
        result
    }

    /// Dequantize f16 bytes back to f32.
    pub fn dequantize_f16(data: &[u8]) -> Vec<f32> {
        let mut result = Vec::with_capacity(data.len() / 2);
        for chunk in data.chunks_exact(2) {
            let bytes = [chunk[0], chunk[1]];
            let half = f16::from_le_bytes(bytes);
            result.push(half.to_f32());
        }
        result
    }

    /// Top-K sparsification: only keep the top k% most significant gradients.
    /// Reduces volume by (100-k)%, with minimal accuracy loss for k >= 0.1%.
    pub fn top_k_sparse(gradients: &[f32], k_percent: f32) -> SparseGradient {
        let k = ((gradients.len() as f32 * k_percent / 100.0).ceil() as usize).max(1);

        // Find the k largest values by absolute magnitude
        let mut indexed: Vec<(usize, f32)> = gradients
            .iter()
            .enumerate()
            .map(|(i, &v)| (i, v))
            .collect();

        indexed.sort_unstable_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        indexed.truncate(k);
        indexed.sort_unstable_by_key(|&(i, _)| i);

        let indices: Vec<u32> = indexed.iter().map(|&(i, _)| i as u32).collect();
        let values: Vec<f32> = indexed.iter().map(|&(_, v)| v).collect();

        SparseGradient {
            indices,
            values,
            original_len: gradients.len(),
        }
    }

    /// Decompress a sparse gradient back to a full dense vector.
    pub fn decompress(sparse: &SparseGradient) -> Vec<f32> {
        let mut result = vec![0.0f32; sparse.original_len];
        for (&idx, &val) in sparse.indices.iter().zip(sparse.values.iter()) {
            if (idx as usize) < result.len() {
                result[idx as usize] = val;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_f16_roundtrip() {
        let original = vec![1.0, -2.5, 0.0, std::f32::consts::PI, -0.001];
        let compressed = GradientCompressor::quantize_f16(&original);
        let decompressed = GradientCompressor::dequantize_f16(&compressed);

        assert_eq!(decompressed.len(), original.len());
        for (a, b) in original.iter().zip(decompressed.iter()) {
            assert!((a - b).abs() < 0.01, "f16 roundtrip: {} vs {}", a, b);
        }
    }

    #[test]
    fn test_f16_size_reduction() {
        let gradients = vec![1.0f32; 1000];
        let compressed = GradientCompressor::quantize_f16(&gradients);
        assert_eq!(compressed.len(), 2000); // 2 bytes per f16 vs 4 per f32
    }

    #[test]
    fn test_top_k_sparse() {
        let gradients = vec![0.1, 0.5, 0.01, 0.9, 0.02, 0.8, 0.001];
        let sparse = GradientCompressor::top_k_sparse(&gradients, 50.0);

        // 50% of 7 = 3.5 -> ceil = 4
        assert_eq!(sparse.values.len(), 4);
        assert_eq!(sparse.original_len, 7);

        // Should contain the 4 largest by magnitude: 0.9, 0.8, 0.5, 0.1
        assert!(sparse.values.contains(&0.9));
        assert!(sparse.values.contains(&0.8));
        assert!(sparse.values.contains(&0.5));
    }

    #[test]
    fn test_sparse_decompress_roundtrip() {
        let original = vec![0.0, 1.0, 0.0, 3.0, 0.0, 5.0];
        let sparse = GradientCompressor::top_k_sparse(&original, 50.0);
        let decompressed = GradientCompressor::decompress(&sparse);

        assert_eq!(decompressed.len(), original.len());
        // The top 3 values (5.0, 3.0, 1.0) should be preserved
        assert_eq!(decompressed[5], 5.0);
        assert_eq!(decompressed[3], 3.0);
        assert_eq!(decompressed[1], 1.0);
    }
}

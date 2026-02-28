use tracing::{debug, info};

use architect_compute::{ComputeBackend, ComputeResult};

/// A shard of a model that can be loaded on a single node.
/// Represents a subset of layers for pipeline parallelism.
pub struct ModelShard {
    shard_id: u32,
    layer_start: u32,
    layer_end: u32,
    weights: Vec<Vec<f32>>,
}

impl ModelShard {
    /// Create a new model shard for a layer range.
    pub fn new(shard_id: u32, layer_start: u32, layer_end: u32) -> Self {
        info!(
            "Creating model shard {}: layers {}-{}",
            shard_id, layer_start, layer_end
        );

        Self {
            shard_id,
            layer_start,
            layer_end,
            weights: Vec::new(),
        }
    }

    /// Load weights for this shard's layers.
    pub fn load_weights(&mut self, weights: Vec<Vec<f32>>) {
        let expected = (self.layer_end - self.layer_start) as usize;
        assert!(
            weights.len() <= expected,
            "Too many weight layers for shard"
        );
        debug!(
            "Shard {}: loaded {} layer weights",
            self.shard_id,
            weights.len()
        );
        self.weights = weights;
    }

    /// Forward pass through this shard's layers.
    /// Takes input activations and returns output activations.
    pub fn forward(&self, input: &[f32], backend: &dyn ComputeBackend) -> ComputeResult {
        let mut current = input.to_vec();

        for (i, layer_weights) in self.weights.iter().enumerate() {
            debug!(
                "Shard {} forward: layer {} (size={})",
                self.shard_id,
                self.layer_start as usize + i,
                layer_weights.len()
            );

            // Simple linear transformation: output = input * weights
            // In practice this would be a full transformer layer
            let dim = (layer_weights.len() as f64).sqrt() as usize;
            if dim * dim == layer_weights.len() && current.len() == dim {
                let result = backend.matmul(&current, layer_weights, 1, dim, dim);
                current = result.data;
            } else {
                // Fallback: element-wise scale
                let result = backend.process_shard(&current, 1.0);
                current = result.data;
            }
        }

        ComputeResult {
            data: current,
            duration_ms: 0,
        }
    }

    pub fn shard_id(&self) -> u32 {
        self.shard_id
    }

    pub fn layer_range(&self) -> (u32, u32) {
        (self.layer_start, self.layer_end)
    }

    pub fn num_layers(&self) -> u32 {
        self.layer_end - self.layer_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use architect_compute::backend::auto_detect;

    #[test]
    fn test_shard_creation() {
        let shard = ModelShard::new(0, 0, 12);
        assert_eq!(shard.shard_id(), 0);
        assert_eq!(shard.layer_range(), (0, 12));
        assert_eq!(shard.num_layers(), 12);
    }

    #[test]
    fn test_shard_forward_passthrough() {
        let shard = ModelShard::new(0, 0, 2);
        let backend = auto_detect();

        // No weights loaded — should pass through
        let input = vec![1.0, 2.0, 3.0];
        let result = shard.forward(&input, backend.as_ref());
        assert_eq!(result.data, input);
    }
}

use std::collections::HashMap;

use tracing::info;

use architect_core::types::NodeId;

use super::model_shard::ModelShard;

/// Configuration for distributed inference.
pub struct InferenceConfig {
    pub total_layers: u32,
    pub pipeline_stages: u32,
}

/// Pipeline stage assignment.
#[derive(Debug, Clone)]
pub struct StageAssignment {
    pub node_id: NodeId,
    pub shard_id: u32,
    pub layer_start: u32,
    pub layer_end: u32,
}

/// Orchestrates distributed inference across multiple nodes.
/// Uses pipeline parallelism: each node handles a subset of model layers.
pub struct DistributedInference {
    config: InferenceConfig,
    assignments: Vec<StageAssignment>,
}

impl DistributedInference {
    pub fn new(config: InferenceConfig) -> Self {
        Self {
            config,
            assignments: Vec::new(),
        }
    }

    /// Plan how to distribute model layers across available nodes.
    pub fn plan(&mut self, nodes: &[NodeId]) -> Vec<StageAssignment> {
        let num_nodes = nodes.len().min(self.config.pipeline_stages as usize);
        if num_nodes == 0 {
            return Vec::new();
        }

        let layers_per_stage = self.config.total_layers / num_nodes as u32;
        let mut remainder = self.config.total_layers % num_nodes as u32;

        let mut assignments = Vec::new();
        let mut current_layer = 0u32;

        for (i, &node_id) in nodes.iter().take(num_nodes).enumerate() {
            let mut stage_layers = layers_per_stage;
            if remainder > 0 {
                stage_layers += 1;
                remainder -= 1;
            }

            let assignment = StageAssignment {
                node_id,
                shard_id: i as u32,
                layer_start: current_layer,
                layer_end: current_layer + stage_layers,
            };

            info!(
                "Pipeline stage {}: node {} handles layers {}-{}",
                i, node_id, assignment.layer_start, assignment.layer_end
            );

            current_layer += stage_layers;
            assignments.push(assignment);
        }

        self.assignments = assignments.clone();
        assignments
    }

    /// Create model shards based on the planned assignments.
    pub fn create_shards(&self) -> HashMap<NodeId, ModelShard> {
        self.assignments
            .iter()
            .map(|a| {
                let shard = ModelShard::new(a.shard_id, a.layer_start, a.layer_end);
                (a.node_id, shard)
            })
            .collect()
    }

    pub fn pipeline_stages(&self) -> usize {
        self.assignments.len()
    }

    pub fn assignments(&self) -> &[StageAssignment] {
        &self.assignments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_plan_distributes_layers() {
        let config = InferenceConfig {
            total_layers: 24,
            pipeline_stages: 4,
        };

        let mut di = DistributedInference::new(config);
        let nodes: Vec<NodeId> = (0..4).map(|_| Uuid::new_v4()).collect();

        let assignments = di.plan(&nodes);

        assert_eq!(assignments.len(), 4);
        assert_eq!(assignments[0].layer_start, 0);
        assert_eq!(assignments[0].layer_end, 6);
        assert_eq!(assignments[3].layer_end, 24);
    }

    #[test]
    fn test_plan_handles_uneven_division() {
        let config = InferenceConfig {
            total_layers: 10,
            pipeline_stages: 3,
        };

        let mut di = DistributedInference::new(config);
        let nodes: Vec<NodeId> = (0..3).map(|_| Uuid::new_v4()).collect();

        let assignments = di.plan(&nodes);

        // 10 / 3 = 3 remainder 1
        // First node gets 4, rest get 3
        let total: u32 = assignments.iter().map(|a| a.layer_end - a.layer_start).sum();
        assert_eq!(total, 10);
    }

    #[test]
    fn test_create_shards() {
        let config = InferenceConfig {
            total_layers: 12,
            pipeline_stages: 2,
        };

        let mut di = DistributedInference::new(config);
        let nodes: Vec<NodeId> = (0..2).map(|_| Uuid::new_v4()).collect();

        di.plan(&nodes);
        let shards = di.create_shards();

        assert_eq!(shards.len(), 2);
        for (node_id, shard) in &shards {
            assert!(nodes.contains(node_id));
            assert!(shard.num_layers() > 0);
        }
    }
}

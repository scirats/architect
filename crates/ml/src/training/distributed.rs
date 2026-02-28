use std::collections::HashMap;

use tracing::info;

use architect_core::types::NodeId;

/// Training configuration.
pub struct TrainingConfig {
    pub batch_size: usize,
    pub learning_rate: f32,
    pub epochs: u32,
    pub gradient_accumulation_steps: u32,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            batch_size: 32,
            learning_rate: 1e-4,
            epochs: 1,
            gradient_accumulation_steps: 1,
        }
    }
}

/// Training state per node.
#[derive(Debug, Clone)]
pub struct NodeTrainingState {
    pub node_id: NodeId,
    pub data_shard_index: usize,
    pub current_epoch: u32,
    pub current_step: u64,
    pub loss: f32,
}

/// Orchestrates distributed training across multiple nodes.
pub struct DistributedTrainer {
    config: TrainingConfig,
    node_states: HashMap<NodeId, NodeTrainingState>,
}

impl DistributedTrainer {
    pub fn new(config: TrainingConfig) -> Self {
        Self {
            config,
            node_states: HashMap::new(),
        }
    }

    /// Assign data shards to nodes for data-parallel training.
    pub fn assign_data_shards(&mut self, nodes: &[NodeId], total_samples: usize) {
        let samples_per_node = total_samples / nodes.len().max(1);

        for (i, &node_id) in nodes.iter().enumerate() {
            let state = NodeTrainingState {
                node_id,
                data_shard_index: i,
                current_epoch: 0,
                current_step: 0,
                loss: f32::MAX,
            };

            info!(
                "Node {} assigned data shard {} ({} samples)",
                node_id, i, samples_per_node
            );

            self.node_states.insert(node_id, state);
        }
    }

    /// Record training progress from a node.
    pub fn update_progress(&mut self, node_id: NodeId, epoch: u32, step: u64, loss: f32) {
        if let Some(state) = self.node_states.get_mut(&node_id) {
            state.current_epoch = epoch;
            state.current_step = step;
            state.loss = loss;
        }
    }

    /// Get the average loss across all nodes.
    pub fn average_loss(&self) -> f32 {
        if self.node_states.is_empty() {
            return 0.0;
        }

        let total: f32 = self
            .node_states
            .values()
            .filter(|s| s.loss < f32::MAX)
            .map(|s| s.loss)
            .sum();

        let count = self
            .node_states
            .values()
            .filter(|s| s.loss < f32::MAX)
            .count();

        if count > 0 {
            total / count as f32
        } else {
            0.0
        }
    }

    /// Check if all nodes have completed the target epochs.
    pub fn is_complete(&self) -> bool {
        self.node_states
            .values()
            .all(|s| s.current_epoch >= self.config.epochs)
    }

    pub fn config(&self) -> &TrainingConfig {
        &self.config
    }

    pub fn node_count(&self) -> usize {
        self.node_states.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_assign_data_shards() {
        let mut trainer = DistributedTrainer::new(TrainingConfig::default());
        let nodes: Vec<NodeId> = (0..3).map(|_| Uuid::new_v4()).collect();

        trainer.assign_data_shards(&nodes, 900);
        assert_eq!(trainer.node_count(), 3);
    }

    #[test]
    fn test_average_loss() {
        let mut trainer = DistributedTrainer::new(TrainingConfig::default());
        let nodes: Vec<NodeId> = (0..2).map(|_| Uuid::new_v4()).collect();

        trainer.assign_data_shards(&nodes, 100);
        trainer.update_progress(nodes[0], 0, 10, 2.0);
        trainer.update_progress(nodes[1], 0, 10, 4.0);

        let avg = trainer.average_loss();
        assert!((avg - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_is_complete() {
        let config = TrainingConfig {
            epochs: 2,
            ..Default::default()
        };
        let mut trainer = DistributedTrainer::new(config);
        let nodes: Vec<NodeId> = (0..2).map(|_| Uuid::new_v4()).collect();

        trainer.assign_data_shards(&nodes, 100);
        assert!(!trainer.is_complete());

        trainer.update_progress(nodes[0], 2, 100, 0.5);
        trainer.update_progress(nodes[1], 2, 100, 0.6);
        assert!(trainer.is_complete());
    }
}

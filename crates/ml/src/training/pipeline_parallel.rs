use std::collections::VecDeque;

use tracing::debug;

use architect_core::types::NodeId;

/// A microbatch moving through the pipeline.
#[derive(Debug, Clone)]
pub struct MicroBatch {
    pub batch_id: u32,
    pub current_stage: u32,
    pub activations: Vec<f32>,
}

/// Pipeline stage for forward/backward pass.
#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub stage_id: u32,
    pub node_id: NodeId,
    pub forward_queue: VecDeque<MicroBatch>,
    pub backward_queue: VecDeque<MicroBatch>,
}

/// Orchestrates pipeline-parallel execution.
/// Splits a batch into microbatches and pipelines them through stages.
pub struct PipelineSchedule {
    stages: Vec<PipelineStage>,
    num_microbatches: u32,
}

impl PipelineSchedule {
    pub fn new(node_ids: &[NodeId], num_microbatches: u32) -> Self {
        let stages = node_ids
            .iter()
            .enumerate()
            .map(|(i, &node_id)| PipelineStage {
                stage_id: i as u32,
                node_id,
                forward_queue: VecDeque::new(),
                backward_queue: VecDeque::new(),
            })
            .collect();

        Self {
            stages,
            num_microbatches,
        }
    }

    /// Generate the 1F1B (one forward, one backward) schedule.
    /// Returns a sequence of (stage_id, action) pairs.
    pub fn generate_1f1b_schedule(&self) -> Vec<PipelineAction> {
        let num_stages = self.stages.len() as u32;
        let mut schedule = Vec::new();

        // Warmup: fill the pipeline with forward passes
        for mb in 0..num_stages.min(self.num_microbatches) {
            for stage in 0..num_stages {
                if mb >= stage {
                    schedule.push(PipelineAction {
                        stage_id: stage,
                        microbatch_id: mb - stage,
                        is_forward: true,
                    });
                }
            }
        }

        // Steady state: 1F1B
        for mb in num_stages..self.num_microbatches {
            for stage in 0..num_stages {
                let fwd_mb = mb.saturating_sub(stage);
                if fwd_mb < self.num_microbatches {
                    schedule.push(PipelineAction {
                        stage_id: stage,
                        microbatch_id: fwd_mb,
                        is_forward: true,
                    });
                }

                // Backward for earlier microbatch
                if let Some(bwd_mb) = mb.checked_sub(stage).and_then(|v| v.checked_sub(num_stages)) {
                    schedule.push(PipelineAction {
                        stage_id: stage,
                        microbatch_id: bwd_mb,
                        is_forward: false,
                    });
                }
            }
        }

        // Cooldown: drain backward passes
        for mb in 0..num_stages {
            for stage in (0..num_stages).rev() {
                schedule.push(PipelineAction {
                    stage_id: stage,
                    microbatch_id: self.num_microbatches - 1 - mb,
                    is_forward: false,
                });
            }
        }

        debug!(
            "Generated 1F1B schedule: {} actions for {} stages, {} microbatches",
            schedule.len(),
            num_stages,
            self.num_microbatches
        );

        schedule
    }

    pub fn num_stages(&self) -> usize {
        self.stages.len()
    }
}

/// A single action in the pipeline schedule.
#[derive(Debug, Clone)]
pub struct PipelineAction {
    pub stage_id: u32,
    pub microbatch_id: u32,
    pub is_forward: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_pipeline_schedule_creation() {
        let nodes: Vec<NodeId> = (0..4).map(|_| Uuid::new_v4()).collect();
        let schedule = PipelineSchedule::new(&nodes, 8);

        assert_eq!(schedule.num_stages(), 4);
    }

    #[test]
    fn test_1f1b_schedule_not_empty() {
        let nodes: Vec<NodeId> = (0..3).map(|_| Uuid::new_v4()).collect();
        let schedule = PipelineSchedule::new(&nodes, 6);

        let actions = schedule.generate_1f1b_schedule();
        assert!(!actions.is_empty());

        // Should have both forward and backward actions
        assert!(actions.iter().any(|a| a.is_forward));
        assert!(actions.iter().any(|a| !a.is_forward));
    }
}

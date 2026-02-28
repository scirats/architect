use architect_core::payload::AITask;
use architect_core::types::NodeId;
use tracing::debug;

use crate::{NodeState, Scheduler};

/// Scheduler that assigns tasks proportional to compute_score.
pub struct WeightedScheduler;

impl WeightedScheduler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WeightedScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for WeightedScheduler {
    fn select_node(&mut self, _task: &AITask, nodes: &[NodeState]) -> Option<NodeId> {
        if nodes.is_empty() {
            return None;
        }

        let best = nodes
            .iter()
            .max_by(|a, b| {
                let score_a = a.capabilities.compute_score;
                let score_b = b.capabilities.compute_score;
                score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
            })?;

        debug!(
            "Weighted scheduler selected node {} (score={})",
            best.id, best.capabilities.compute_score
        );

        Some(best.id)
    }

    fn name(&self) -> &str {
        "weighted"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_common::*;

    #[test]
    fn test_weighted_selects_highest_score() {
        let mut sched = WeightedScheduler::new();
        let mut nodes = make_test_nodes(3);

        // Give different compute scores
        nodes[0].capabilities.compute_score = 10.0;
        nodes[1].capabilities.compute_score = 50.0;
        nodes[2].capabilities.compute_score = 30.0;

        let task = make_test_task();
        let selected = sched.select_node(&task, &nodes).unwrap();

        assert_eq!(selected, nodes[1].id);
    }
}

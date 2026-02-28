use architect_core::payload::AITask;
use architect_core::types::NodeId;

use crate::{NodeState, Scheduler};

/// Simple round-robin scheduler.
pub struct RoundRobinScheduler {
    last_index: usize,
}

impl RoundRobinScheduler {
    pub fn new() -> Self {
        Self { last_index: 0 }
    }
}

impl Default for RoundRobinScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for RoundRobinScheduler {
    fn select_node(&mut self, _task: &AITask, nodes: &[NodeState]) -> Option<NodeId> {
        if nodes.is_empty() {
            return None;
        }

        self.last_index = (self.last_index + 1) % nodes.len();
        Some(nodes[self.last_index].id)
    }

    fn name(&self) -> &str {
        "round-robin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_common::*;

    #[test]
    fn test_round_robin_cycles() {
        let mut sched = RoundRobinScheduler::new();
        let nodes = make_test_nodes(3);
        let task = make_test_task();

        let first = sched.select_node(&task, &nodes).unwrap();
        let second = sched.select_node(&task, &nodes).unwrap();
        let third = sched.select_node(&task, &nodes).unwrap();
        let fourth = sched.select_node(&task, &nodes).unwrap();

        // Should cycle through nodes
        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_eq!(first, fourth); // wraps around
    }

    #[test]
    fn test_round_robin_empty() {
        let mut sched = RoundRobinScheduler::new();
        let task = make_test_task();
        assert!(sched.select_node(&task, &[]).is_none());
    }
}

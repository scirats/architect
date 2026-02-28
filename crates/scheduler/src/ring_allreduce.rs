use architect_core::types::NodeId;
use tracing::debug;

use crate::NodeState;

/// Ring AllReduce plan for distributed gradient aggregation.
///
/// Implements scatter-reduce + all-gather in a ring topology.
/// Each node sends chunks to the next node in the ring.
#[derive(Debug, Clone)]
pub struct RingAllReducePlan {
    /// Ordered ring of node IDs.
    pub ring: Vec<NodeId>,
    /// Number of chunks to split data into (= number of nodes).
    pub num_chunks: usize,
}

/// Phase in the ring allreduce algorithm.
#[derive(Debug, Clone)]
pub struct RingStep {
    pub sender: NodeId,
    pub receiver: NodeId,
    pub chunk_index: usize,
    pub phase: RingPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RingPhase {
    ScatterReduce,
    AllGather,
}

impl RingAllReducePlan {
    /// Create a ring allreduce plan from available nodes.
    /// Nodes are sorted by ID for deterministic ring ordering.
    pub fn new(nodes: &[NodeState]) -> Option<Self> {
        if nodes.len() < 2 {
            return None;
        }

        let mut ring: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
        ring.sort();

        let num_chunks = ring.len();

        debug!("Created ring allreduce plan with {} nodes", num_chunks);

        Some(Self { ring, num_chunks })
    }

    /// Generate scatter-reduce steps.
    /// In scatter-reduce, each node sends one chunk to the next node.
    /// After N-1 iterations, each node has the reduced result for one chunk.
    pub fn scatter_reduce_steps(&self) -> Vec<RingStep> {
        let n = self.ring.len();
        let mut steps = Vec::new();

        for iteration in 0..n - 1 {
            for (i, &sender) in self.ring.iter().enumerate() {
                let receiver = self.ring[(i + 1) % n];
                let chunk_index = (i + n - iteration) % n;

                steps.push(RingStep {
                    sender,
                    receiver,
                    chunk_index,
                    phase: RingPhase::ScatterReduce,
                });
            }
        }

        steps
    }

    /// Generate all-gather steps.
    /// After scatter-reduce, each node broadcasts its reduced chunk around the ring.
    pub fn all_gather_steps(&self) -> Vec<RingStep> {
        let n = self.ring.len();
        let mut steps = Vec::new();

        for iteration in 0..n - 1 {
            for (i, &sender) in self.ring.iter().enumerate() {
                let receiver = self.ring[(i + 1) % n];
                let chunk_index = (i + n - iteration + 1) % n;

                steps.push(RingStep {
                    sender,
                    receiver,
                    chunk_index,
                    phase: RingPhase::AllGather,
                });
            }
        }

        steps
    }

    /// Total number of communication steps.
    pub fn total_steps(&self) -> usize {
        let n = self.ring.len();
        // scatter-reduce: (n-1) rounds * n transfers
        // all-gather: (n-1) rounds * n transfers
        2 * n * (n - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_common::*;

    #[test]
    fn test_ring_plan_creation() {
        let nodes = make_test_nodes(4);
        let plan = RingAllReducePlan::new(&nodes).unwrap();

        assert_eq!(plan.ring.len(), 4);
        assert_eq!(plan.num_chunks, 4);
    }

    #[test]
    fn test_ring_plan_needs_two_nodes() {
        let nodes = make_test_nodes(1);
        assert!(RingAllReducePlan::new(&nodes).is_none());
    }

    #[test]
    fn test_scatter_reduce_steps_count() {
        let nodes = make_test_nodes(4);
        let plan = RingAllReducePlan::new(&nodes).unwrap();
        let steps = plan.scatter_reduce_steps();

        // (n-1) rounds * n nodes = 3 * 4 = 12
        assert_eq!(steps.len(), 12);
        assert!(steps.iter().all(|s| s.phase == RingPhase::ScatterReduce));
    }

    #[test]
    fn test_all_gather_steps_count() {
        let nodes = make_test_nodes(4);
        let plan = RingAllReducePlan::new(&nodes).unwrap();
        let steps = plan.all_gather_steps();

        assert_eq!(steps.len(), 12);
        assert!(steps.iter().all(|s| s.phase == RingPhase::AllGather));
    }

    #[test]
    fn test_total_steps() {
        let nodes = make_test_nodes(4);
        let plan = RingAllReducePlan::new(&nodes).unwrap();

        // 2 * 4 * 3 = 24
        assert_eq!(plan.total_steps(), 24);
    }
}

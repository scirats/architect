pub mod adaptive;
pub mod data_locality;
pub mod ring_allreduce;
pub mod round_robin;
pub mod weighted;

#[cfg(test)]
pub(crate) mod tests_common;

use architect_core::payload::AITask;
use architect_core::types::{NodeId, NodeMetrics, NodeCapabilities};

/// Snapshot of a node's state for scheduling decisions.
#[derive(Debug, Clone)]
pub struct NodeState {
    pub id: NodeId,
    pub capabilities: NodeCapabilities,
    pub metrics: Option<NodeMetrics>,
    pub current_tasks: u32,
    pub has_cached_data: bool,
    /// List of dataset/model paths cached on this node.
    pub cached_datasets: Vec<String>,
}

/// Trait for task scheduling algorithms.
pub trait Scheduler: Send + Sync {
    /// Select the best node for a given task.
    fn select_node(&mut self, task: &AITask, nodes: &[NodeState]) -> Option<NodeId>;

    /// Name of the scheduling algorithm.
    fn name(&self) -> &str;
}

pub use adaptive::AdaptiveScheduler;
pub use data_locality::DataLocalityScheduler;
pub use round_robin::RoundRobinScheduler;
pub use weighted::WeightedScheduler;

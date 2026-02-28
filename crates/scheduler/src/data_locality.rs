use architect_core::payload::{AIPayload, AITask};
use architect_core::types::NodeId;
use tracing::debug;

use crate::{NodeState, Scheduler};

/// Scheduler that prefers nodes with cached data to minimize transfers.
/// Uses exact and prefix matching on dataset/model paths.
/// Falls back to the node with the highest compute score when no data is cached.
pub struct DataLocalityScheduler;

impl DataLocalityScheduler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DataLocalityScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the primary data key (dataset or model path) from a task payload.
fn extract_data_key(task: &AITask) -> Option<String> {
    match &task.payload {
        AIPayload::TrainRequest(c) => {
            if !c.dataset_path.is_empty() { Some(c.dataset_path.clone()) } else { None }
        }
        AIPayload::FinetuneRequest(c) => {
            if !c.dataset_path.is_empty() { Some(c.dataset_path.clone()) }
            else if !c.base_model.is_empty() { Some(c.base_model.clone()) }
            else { None }
        }
        AIPayload::PreprocessRequest(c) => {
            if !c.input_path.is_empty() { Some(c.input_path.clone()) } else { None }
        }
        AIPayload::EvalRequest(c) => {
            if !c.model_path.is_empty() { Some(c.model_path.clone()) }
            else if !c.dataset_path.is_empty() { Some(c.dataset_path.clone()) }
            else { None }
        }
        AIPayload::RagRequest(c) => {
            if !c.index_path.is_empty() { Some(c.index_path.clone()) } else { None }
        }
        _ => None,
    }
}

/// Compute a locality score: 50 for exact match, 25 for prefix match, 0 otherwise.
fn locality_score(node: &NodeState, data_key: &str) -> f32 {
    // Exact match
    if node.cached_datasets.iter().any(|d| d == data_key) {
        return 50.0;
    }
    // Prefix match (same directory)
    let prefix = data_key.rsplit_once('/').map(|(p, _)| p).unwrap_or(data_key);
    if node.cached_datasets.iter().any(|d| d.starts_with(prefix)) {
        return 25.0;
    }
    0.0
}

impl Scheduler for DataLocalityScheduler {
    fn select_node(&mut self, task: &AITask, nodes: &[NodeState]) -> Option<NodeId> {
        if nodes.is_empty() {
            return None;
        }

        let data_key = extract_data_key(task);

        // Score each node: locality_bonus + compute_score
        let best = nodes
            .iter()
            .max_by(|a, b| {
                let a_locality = data_key.as_ref().map(|k| locality_score(a, k)).unwrap_or(0.0);
                let b_locality = data_key.as_ref().map(|k| locality_score(b, k)).unwrap_or(0.0);
                let a_score = a.capabilities.compute_score + a_locality;
                let b_score = b.capabilities.compute_score + b_locality;
                a_score
                    .partial_cmp(&b_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?;

        let bonus = data_key.as_ref().map(|k| locality_score(best, k)).unwrap_or(0.0);
        if bonus > 0.0 {
            debug!("Data locality: selected node {} (locality_bonus={:.0})", best.id, bonus);
        } else {
            debug!("Data locality: selected node {} (compute_score={:.1})", best.id, best.capabilities.compute_score);
        }
        Some(best.id)
    }

    fn name(&self) -> &str {
        "data-locality"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_common::*;
    use architect_core::payload::{AIPayload, TrainConfig, TaskType};

    fn make_train_task(dataset: &str) -> AITask {
        use uuid::Uuid;
        AITask {
            task_id: Uuid::new_v4(),
            task_type: TaskType::Train,
            payload: AIPayload::TrainRequest(TrainConfig {
                dataset_path: dataset.to_string(),
                ..Default::default()
            }),
            priority: 5,
            created_at: 0,
            timeout_ms: 300_000,
            depends_on: Vec::new(),
            quota: None,
        }
    }

    #[test]
    fn test_prefers_cached_node() {
        let mut sched = DataLocalityScheduler::new();
        let mut nodes = make_test_nodes(3);

        nodes[0].has_cached_data = false;
        nodes[0].capabilities.compute_score = 100.0;

        nodes[1].has_cached_data = true;
        nodes[1].capabilities.compute_score = 50.0;
        nodes[1].cached_datasets = vec!["/data/train.csv".to_string()];

        nodes[2].has_cached_data = false;
        nodes[2].capabilities.compute_score = 80.0;

        let task = make_train_task("/data/train.csv");
        let selected = sched.select_node(&task, &nodes).unwrap();

        // Should pick node 1 (exact cache match, 50 + 50 = 100) over node 0 (100 + 0 = 100)
        // When tied, max_by picks the last one — but 50+50 = 100 ties with 100+0 = 100
        // Actually node 0 and 1 tie at 100. Let's adjust: node 1 should win with lower base
        // but locality bonus makes it 50+50=100 vs 100+0=100. Adjust scores:
        assert!(
            selected == nodes[1].id || selected == nodes[0].id,
            "Should select either node with score 100"
        );

        // Better test: lower node 0 score so node 1 clearly wins with bonus
        nodes[0].capabilities.compute_score = 80.0;
        let selected = sched.select_node(&task, &nodes).unwrap();
        assert_eq!(selected, nodes[1].id); // 50 + 50 = 100 > 80
    }

    #[test]
    fn test_prefix_match() {
        let mut sched = DataLocalityScheduler::new();
        let mut nodes = make_test_nodes(2);

        nodes[0].capabilities.compute_score = 30.0;
        nodes[1].capabilities.compute_score = 30.0;
        nodes[1].cached_datasets = vec!["/data/other.csv".to_string()];

        let task = make_train_task("/data/train.csv");
        let selected = sched.select_node(&task, &nodes).unwrap();

        // Node 1 has prefix match (/data/) => 30 + 25 = 55 > 30
        assert_eq!(selected, nodes[1].id);
    }

    #[test]
    fn test_fallback_to_highest_score() {
        let mut sched = DataLocalityScheduler::new();
        let mut nodes = make_test_nodes(2);

        // No cached data
        nodes[0].capabilities.compute_score = 30.0;
        nodes[1].capabilities.compute_score = 70.0;

        let task = make_test_task();
        let selected = sched.select_node(&task, &nodes).unwrap();

        assert_eq!(selected, nodes[1].id);
    }
}

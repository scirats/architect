use architect_core::payload::{AIPayload, AITask, ControlMessage, TaskType};
use architect_core::types::NodeCapabilities;
use uuid::Uuid;

use crate::NodeState;

pub fn make_test_nodes(count: usize) -> Vec<NodeState> {
    (0..count)
        .map(|_| NodeState {
            id: Uuid::new_v4(),
            capabilities: NodeCapabilities {
                cpu_cores: 4,
                cpu_freq_mhz: 2400,
                ram_total_mb: 8192,
                ram_available_mb: 4096,
                gpu: None,
                battery: None,
                compute_score: 20.0,
            },
            metrics: None,
            current_tasks: 0,
            has_cached_data: false,
            cached_datasets: Vec::new(),
        })
        .collect()
}

pub fn make_test_task() -> AITask {
    AITask {
        task_id: Uuid::new_v4(),
        task_type: TaskType::Compute,
        payload: AIPayload::Control(ControlMessage::Resume),
        priority: 5,
        created_at: 0,
        timeout_ms: 300_000,
        depends_on: Vec::new(),
        quota: None,
    }
}

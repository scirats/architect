use serde::{Deserialize, Serialize};

use crate::payload::{AITask, TaskResult};
use crate::types::{NodeCapabilities, NodeInfo, NodeMetrics};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Identify,
    RequestMetrics,
    RequestCapabilities,
    SubmitTask(AITask),
    CancelTask { task_id: uuid::Uuid },
    Shutdown,
    Purge,
    Ping { seq: u64 },
    UpdateToken { new_token: String },
    UpdateAvailable { version: String, download_url: String },
    ApplyGradients { task_id: uuid::Uuid, step: u64, aggregated: Vec<f32> },
    StateSync { assigned_tasks: Vec<uuid::Uuid> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessage {
    Identity(NodeInfo),
    Heartbeat(NodeMetrics),
    Capabilities(NodeCapabilities),
    TaskResult(TaskResult),
    TaskProgress { task_id: uuid::Uuid, progress_pct: f32 },
    Pong { seq: u64 },
    Error { message: String },
    /// Buffered results from a previous session, re-sent on reconnect.
    BufferedResults(Vec<TaskResult>),
    Checkpoint { task_id: uuid::Uuid, epoch: u32, step: u64, data: Vec<u8> },
    GradientShard { task_id: uuid::Uuid, step: u64, shard_index: u32, data: Vec<f32> },
    StateSyncResponse { running: Vec<uuid::Uuid>, completed: Vec<uuid::Uuid>, unknown: Vec<uuid::Uuid> },
    CacheReport { datasets: Vec<String>, models: Vec<String> },
}

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::NodeId;

/// Configuration for inference requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    pub max_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            max_tokens: 256,
            temperature: 0.7,
            top_p: 0.9,
        }
    }
}

/// Cluster-wide configuration that can be updated at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub rebalance_interval_secs: u64,
    pub gradient_compression: bool,
    pub gradient_top_k_percent: f32,
    pub battery_limit_pct: u8,
    pub cpu_limit_pct: u8,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            rebalance_interval_secs: 2,
            gradient_compression: true,
            gradient_top_k_percent: 1.0,
            battery_limit_pct: 20,
            cpu_limit_pct: 90,
        }
    }
}

/// Control messages for cluster management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    Shutdown,
    Pause,
    Resume,
    UpdateConfig(ClusterConfig),
}

/// Checkpoint data for resuming training.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckpointData {
    pub epoch: u32,
    pub step: u64,
    pub state: Vec<u8>,
}

/// Resource quota for limiting task resource usage.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceQuota {
    pub max_cpu_pct: Option<f32>,
    pub max_ram_mb: Option<u64>,
}

/// Configuration for training requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainConfig {
    pub dataset_path: String,
    pub epochs: u32,
    pub learning_rate: f32,
    pub batch_size: u32,
    pub output_path: String,
    #[serde(default)]
    pub resume_from: Option<CheckpointData>,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            dataset_path: String::new(),
            epochs: 3,
            learning_rate: 0.001,
            batch_size: 32,
            output_path: "output".into(),
            resume_from: None,
        }
    }
}

/// Configuration for fine-tuning requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinetuneConfig {
    pub base_model: String,
    pub dataset_path: String,
    pub epochs: u32,
    pub learning_rate: f32,
    pub batch_size: u32,
    pub output_path: String,
    pub adapter: String,
    #[serde(default)]
    pub resume_from: Option<CheckpointData>,
}

impl Default for FinetuneConfig {
    fn default() -> Self {
        Self {
            base_model: String::new(),
            dataset_path: String::new(),
            epochs: 3,
            learning_rate: 0.0001,
            batch_size: 16,
            output_path: "output".into(),
            adapter: "lora".into(),
            resume_from: None,
        }
    }
}

/// Configuration for preprocessing requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprocessConfig {
    pub input_path: String,
    pub output_path: String,
    pub format: String,
}

/// Configuration for evaluation requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalConfig {
    pub model_path: String,
    pub dataset_path: String,
    pub metrics: Vec<String>,
    pub output_path: Option<String>,
}

/// Configuration for RAG requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagConfig {
    pub query: String,
    pub index_path: String,
    pub model_path: Option<String>,
    pub top_k: u32,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            query: String::new(),
            index_path: String::new(),
            model_path: None,
            top_k: 5,
        }
    }
}

/// Configuration for embedding requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedConfig {
    pub input: String,
    pub model_path: Option<String>,
    pub index_path: Option<String>,
    pub operation: String,
}

/// Type of task for display purposes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskType {
    Train,
    Finetune,
    Inference,
    Preprocess,
    Evaluate,
    Rag,
    Embed,
    Compute,
    Plugin,
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Train => write!(f, "train"),
            Self::Finetune => write!(f, "finetune"),
            Self::Inference => write!(f, "infer"),
            Self::Preprocess => write!(f, "preprocess"),
            Self::Evaluate => write!(f, "eval"),
            Self::Rag => write!(f, "rag"),
            Self::Embed => write!(f, "embed"),
            Self::Compute => write!(f, "compute"),
            Self::Plugin => write!(f, "plugin"),
        }
    }
}

/// Payload types that travel between nodes for AI/ML workloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AIPayload {
    Gradients {
        data: Vec<f32>,
        model_id: String,
    },
    ModelWeights {
        data: Vec<u8>,
        layer_range: (u32, u32),
    },
    InferenceRequest {
        prompt: Vec<u8>,
        config: InferenceConfig,
    },
    InferenceResponse {
        tokens: Vec<u32>,
        latency_ms: f32,
    },
    DataShard {
        data: Vec<u8>,
        shard_id: u32,
        total_shards: u32,
    },
    TrainRequest(TrainConfig),
    FinetuneRequest(FinetuneConfig),
    PreprocessRequest(PreprocessConfig),
    EvalRequest(EvalConfig),
    RagRequest(RagConfig),
    EmbedRequest(EmbedConfig),
    Control(ControlMessage),
    Plugin { name: String, config: String },
}

/// A task to be executed by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AITask {
    pub task_id: Uuid,
    pub task_type: TaskType,
    pub payload: AIPayload,
    pub priority: u8,
    pub created_at: u64,
    /// Timeout in milliseconds (0 = no timeout). Default: 300_000 (5 min).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Task IDs that must complete before this task can be dispatched.
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    /// Resource quota for this task.
    #[serde(default)]
    pub quota: Option<ResourceQuota>,
}

fn default_timeout_ms() -> u64 {
    300_000
}

/// Result of a completed task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: Uuid,
    pub node_id: NodeId,
    pub success: bool,
    pub payload: Option<AIPayload>,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// Status of a task in the system.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Assigned,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Assigned => write!(f, "Assigned"),
            Self::Running => write!(f, "Running"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed => write!(f, "Failed"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

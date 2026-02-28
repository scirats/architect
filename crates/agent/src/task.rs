use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use uuid::Uuid;

use architect_core::payload::{AIPayload, AITask, TaskResult};
use architect_core::protocol::AgentMessage;
use architect_core::types::NodeId;

/// Maximum buffered results (prevents unbounded memory growth if disconnected for long).
const MAX_BUFFERED_RESULTS: usize = 1000;

/// In-memory buffer for results that could not be sent (connection lost).
/// Shared across reconnections so results survive disconnects.
#[derive(Clone, Default)]
pub struct ResultBuffer {
    inner: Arc<Mutex<Vec<TaskResult>>>,
}

impl ResultBuffer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn push(&self, result: TaskResult) {
        let mut buf = self.inner.lock().unwrap();
        if buf.len() >= MAX_BUFFERED_RESULTS {
            warn!("Result buffer full ({} items), dropping oldest", MAX_BUFFERED_RESULTS);
            buf.remove(0);
        }
        buf.push(result);
    }

    /// Drain all buffered results, returning them.
    pub fn drain(&self) -> Vec<TaskResult> {
        let mut buf = self.inner.lock().unwrap();
        std::mem::take(&mut *buf)
    }
}


/// Executes tasks received from the coordinator, supporting concurrent execution.
pub struct TaskExecutor {
    node_id: NodeId,
    tx: mpsc::Sender<AgentMessage>,
    running_tasks: HashMap<Uuid, JoinHandle<()>>,
    max_concurrent: usize,
    pub result_buffer: ResultBuffer,
}

impl TaskExecutor {
    /// Create a TaskExecutor with an external result buffer (persists across reconnections).
    pub fn new_with_buffer(
        node_id: NodeId,
        tx: mpsc::Sender<AgentMessage>,
        buffer: ResultBuffer,
    ) -> Self {
        let cpu_cores = num_cpus();
        let max_concurrent = (cpu_cores / 2).max(1);
        Self {
            node_id,
            tx,
            running_tasks: HashMap::new(),
            max_concurrent,
            result_buffer: buffer,
        }
    }

    /// Execute a task. Spawns it in the background if capacity allows.
    pub async fn execute(&mut self, task: AITask) {
        self.cleanup_completed();

        if self.running_tasks.len() >= self.max_concurrent {
            warn!(
                "At capacity ({}/{}), rejecting task {}",
                self.running_tasks.len(),
                self.max_concurrent,
                task.task_id
            );
            let _ = self
                .tx
                .send(AgentMessage::Error {
                    message: format!(
                        "At capacity: {}/{} tasks running",
                        self.running_tasks.len(),
                        self.max_concurrent
                    ),
                })
                .await;
            return;
        }

        let task_id = task.task_id;
        let node_id = self.node_id;
        let tx = self.tx.clone();
        let buffer = self.result_buffer.clone();

        info!(
            "Spawning task {} ({}/{})",
            task_id,
            self.running_tasks.len() + 1,
            self.max_concurrent
        );

        let handle = tokio::spawn(async move {
            // Panic-safe: if run_task panics, we catch it and send a failure result
            let result = std::panic::AssertUnwindSafe(run_task(node_id, tx.clone(), task, buffer.clone()));
            if futures::FutureExt::catch_unwind(result).await.is_err() {
                warn!("Task {} panicked during execution", task_id);
                let fail_result = TaskResult {
                    task_id,
                    node_id,
                    success: false,
                    payload: None,
                    duration_ms: 0,
                    error: Some("Task panicked during execution".into()),
                };
                if tx.send(AgentMessage::TaskResult(fail_result.clone())).await.is_err() {
                    buffer.push(fail_result);
                }
            }
        });

        self.running_tasks.insert(task_id, handle);
    }

    /// Cancel a running task by aborting its handle.
    pub fn cancel(&mut self, task_id: Uuid) {
        if let Some(handle) = self.running_tasks.remove(&task_id) {
            info!("Cancelling task {}", task_id);
            handle.abort();
        }
    }

    /// Wait for all running tasks to complete, up to `timeout`.
    /// Aborts remaining tasks after timeout.
    pub async fn drain(&mut self, timeout: std::time::Duration) {
        if self.running_tasks.is_empty() {
            return;
        }
        info!("Draining {} running tasks (timeout {:?})", self.running_tasks.len(), timeout);
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.running_tasks.is_empty() {
            if tokio::time::Instant::now() >= deadline {
                warn!("Drain timeout: aborting {} remaining tasks", self.running_tasks.len());
                for (id, handle) in self.running_tasks.drain() {
                    info!("Aborting task {} on shutdown", id);
                    handle.abort();
                }
                break;
            }
            self.cleanup_completed();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        info!("Task drain complete");
    }

    /// Check if a task is currently running.
    pub fn is_running(&self, task_id: &Uuid) -> bool {
        self.running_tasks.contains_key(task_id)
    }


    /// Remove completed tasks from the map.
    pub fn cleanup_completed(&mut self) {
        self.running_tasks.retain(|id, handle| {
            if handle.is_finished() {
                info!("Task {} finished, cleaning up", id);
                false
            } else {
                true
            }
        });
    }
}

/// Run a single task to completion, reporting progress and result.
/// If the connection is lost, the result is buffered for re-delivery.
/// Applies task timeout and catches panics.
async fn run_task(
    node_id: NodeId,
    tx: mpsc::Sender<AgentMessage>,
    task: AITask,
    buffer: ResultBuffer,
) {
    let task_id = task.task_id;
    let timeout_ms = task.timeout_ms;
    let started_at = Instant::now();

    // Pre-execution quota check: verify sufficient RAM is available
    if let Some(ref quota) = task.quota {
        if let Some(max_ram_mb) = quota.max_ram_mb {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            let available_mb = sys.available_memory() / (1024 * 1024);
            if available_mb < max_ram_mb {
                warn!(
                    "Task {} rejected: insufficient RAM ({} MB available, {} MB quota)",
                    task_id, available_mb, max_ram_mb
                );
                let fail_result = TaskResult {
                    task_id,
                    node_id,
                    success: false,
                    payload: None,
                    duration_ms: 0,
                    error: Some(format!(
                        "Insufficient RAM: {} MB available, {} MB required by quota",
                        available_mb, max_ram_mb
                    )),
                };
                if tx
                    .send(AgentMessage::TaskResult(fail_result.clone()))
                    .await
                    .is_err()
                {
                    buffer.push(fail_result);
                }
                return;
            }
        }
    }

    // Send initial progress
    let _ = tx
        .send(AgentMessage::TaskProgress {
            task_id,
            progress_pct: 0.0,
        })
        .await;

    // Spawn quota monitor if RAM or CPU limits are set
    let monitor_handle = if task.quota.as_ref().is_some_and(|q| q.max_ram_mb.is_some() || q.max_cpu_pct.is_some()) {
        let quota = task.quota.clone().unwrap();
        let monitor_tx = tx.clone();
        let monitor_task_id = task_id;
        Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let mut sys = sysinfo::System::new();
                sys.refresh_memory();
                let used_mb = (sys.total_memory() - sys.available_memory()) / (1024 * 1024);
                if let Some(max_ram) = quota.max_ram_mb {
                    if used_mb > max_ram {
                        warn!(
                            "Task {} exceeding memory quota: ~{} MB used > {} MB limit",
                            monitor_task_id, used_mb, max_ram
                        );
                        let _ = monitor_tx.send(AgentMessage::Error {
                            message: format!(
                                "Task {} exceeded memory quota ({} MB > {} MB)",
                                monitor_task_id, used_mb, max_ram
                            ),
                        }).await;
                    }
                }
                if let Some(max_cpu) = quota.max_cpu_pct {
                    sys.refresh_cpu_all();
                    let cpu_pct = sys.global_cpu_usage();
                    if cpu_pct > max_cpu {
                        warn!(
                            "Task {} CPU usage {:.1}% exceeds quota {:.1}%",
                            monitor_task_id, cpu_pct, max_cpu
                        );
                    }
                }
            }
        }))
    } else {
        None
    };

    // Run with timeout
    let result = if timeout_ms > 0 {
        match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            run_payload(task_id, &task.payload, &tx),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => {
                warn!("Task {} timed out after {}ms", task_id, timeout_ms);
                Err(format!("Task timed out after {}ms", timeout_ms))
            }
        }
    } else {
        run_payload(task_id, &task.payload, &tx).await
    };
    let duration_ms = started_at.elapsed().as_millis() as u64;

    // Stop quota monitor
    if let Some(handle) = monitor_handle {
        handle.abort();
    }

    let task_result = match result {
        Ok(response) => TaskResult {
            task_id,
            node_id,
            success: true,
            payload: response,
            duration_ms,
            error: None,
        },
        Err(e) => TaskResult {
            task_id,
            node_id,
            success: false,
            payload: None,
            duration_ms,
            error: Some(e),
        },
    };

    // Try to send result; buffer it if the connection is lost
    if tx
        .send(AgentMessage::TaskResult(task_result.clone()))
        .await
        .is_err()
    {
        info!(
            "Connection lost, buffering result for task {}",
            task_id
        );
        buffer.push(task_result);
    }
}

/// Report progress back to the coordinator.
async fn report_progress(tx: &mpsc::Sender<AgentMessage>, task_id: Uuid, pct: f32) {
    let _ = tx
        .send(AgentMessage::TaskProgress {
            task_id,
            progress_pct: pct,
        })
        .await;
}

async fn run_payload(
    task_id: Uuid,
    payload: &AIPayload,
    tx: &mpsc::Sender<AgentMessage>,
) -> Result<Option<AIPayload>, String> {
    match payload {
        AIPayload::Control(ctrl) => {
            info!("Processing control message: {:?}", ctrl);
            Ok(None)
        }

        AIPayload::InferenceRequest { prompt, config } => {
            info!(
                "Task {}: inference (prompt {} bytes, max_tokens={})",
                task_id,
                prompt.len(),
                config.max_tokens
            );
            report_progress(tx, task_id, 10.0).await;

            let num_tokens = config.max_tokens.min(128) as usize;
            let step_ms = 20;
            let mut tokens = Vec::with_capacity(num_tokens);

            for i in 0..num_tokens {
                let token =
                    ((prompt.len() as u32).wrapping_mul(31).wrapping_add(i as u32)) % 50257;
                tokens.push(token);

                if i % 16 == 0 {
                    let pct = 10.0 + (i as f32 / num_tokens as f32) * 85.0;
                    report_progress(tx, task_id, pct).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(step_ms)).await;
            }

            report_progress(tx, task_id, 100.0).await;
            let latency_ms = (num_tokens * step_ms as usize) as f32;
            info!(
                "Task {}: generated {} tokens in {:.0}ms",
                task_id,
                tokens.len(),
                latency_ms
            );

            Ok(Some(AIPayload::InferenceResponse {
                tokens,
                latency_ms,
            }))
        }

        AIPayload::Gradients { data, model_id } => {
            info!(
                "Task {}: processing gradients (model={}, {} values)",
                task_id,
                model_id,
                data.len()
            );
            report_progress(tx, task_id, 20.0).await;

            let scaled: Vec<f32> = data.iter().map(|g| g * 0.1).collect();
            report_progress(tx, task_id, 80.0).await;

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            report_progress(tx, task_id, 100.0).await;

            info!(
                "Task {}: gradients processed ({} values)",
                task_id,
                scaled.len()
            );
            Ok(Some(AIPayload::Gradients {
                data: scaled,
                model_id: model_id.clone(),
            }))
        }

        AIPayload::ModelWeights { data, layer_range } => {
            info!(
                "Task {}: received model weights ({} bytes, layers {:?})",
                task_id,
                data.len(),
                layer_range
            );
            report_progress(tx, task_id, 100.0).await;
            Ok(None)
        }

        AIPayload::DataShard {
            data,
            shard_id,
            total_shards,
        } => {
            info!(
                "Task {}: processing data shard {}/{} ({} bytes)",
                task_id, shard_id, total_shards,
                data.len()
            );
            report_progress(tx, task_id, 50.0).await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            report_progress(tx, task_id, 100.0).await;
            Ok(None)
        }

        AIPayload::InferenceResponse { .. } => {
            Err("Agent should not receive InferenceResponse".into())
        }

        AIPayload::TrainRequest(config) => {
            info!(
                "Task {}: training (dataset={}, epochs={}, lr={}, batch={})",
                task_id, config.dataset_path, config.epochs, config.learning_rate, config.batch_size
            );

            if config.dataset_path.is_empty() {
                return Err("Training requires --dataset path".into());
            }

            let total_epochs = config.epochs.max(1);
            let steps_per_epoch = 10u32;
            let step_delay_ms = 200u64;

            // Resume from checkpoint if available
            let start_epoch = config.resume_from.as_ref().map(|c| c.epoch).unwrap_or(0);
            let start_step = config.resume_from.as_ref().map(|c| c.step as u32).unwrap_or(0);
            if start_epoch > 0 || start_step > 0 {
                info!("Task {}: resuming from epoch {} step {}", task_id, start_epoch, start_step);
            }

            for epoch in start_epoch..total_epochs {
                let first_step = if epoch == start_epoch { start_step } else { 0 };
                for step in first_step..steps_per_epoch {
                    let global_step = epoch * steps_per_epoch + step;
                    let total_steps = total_epochs * steps_per_epoch;
                    let pct = (global_step as f32 / total_steps as f32) * 100.0;
                    report_progress(tx, task_id, pct).await;
                    tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms)).await;

                    // Emit gradient shard after each step (simulated gradients)
                    let gradient_data: Vec<f32> = (0..64).map(|i| {
                        ((global_step as f32) * 0.01 + i as f32 * 0.001).sin()
                    }).collect();
                    let _ = tx.send(AgentMessage::GradientShard {
                        task_id,
                        step: global_step as u64,
                        shard_index: 0,
                        data: gradient_data,
                    }).await;

                    // Emit checkpoint every 5 steps
                    if global_step > 0 && global_step.is_multiple_of(5) {
                        let state = bincode::serialize(&(epoch, step, pct)).unwrap_or_default();
                        let _ = tx.send(AgentMessage::Checkpoint {
                            task_id,
                            epoch,
                            step: global_step as u64,
                            data: state,
                        }).await;
                    }
                }
                info!(
                    "Task {}: epoch {}/{} complete",
                    task_id,
                    epoch + 1,
                    total_epochs
                );
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Task {}: training complete ({} epochs, output={})",
                task_id, total_epochs, config.output_path
            );
            Ok(None)
        }

        AIPayload::FinetuneRequest(config) => {
            info!(
                "Task {}: fine-tuning (base={}, dataset={}, adapter={})",
                task_id, config.base_model, config.dataset_path, config.adapter
            );

            if config.base_model.is_empty() {
                return Err("Fine-tuning requires --base model".into());
            }
            if config.dataset_path.is_empty() {
                return Err("Fine-tuning requires --dataset path".into());
            }

            report_progress(tx, task_id, 5.0).await;
            info!("Task {}: loading base model {}", task_id, config.base_model);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let total_epochs = config.epochs.max(1);
            let steps_per_epoch = 8u32;
            let step_delay_ms = 250u64;

            // Resume from checkpoint if available
            let start_epoch = config.resume_from.as_ref().map(|c| c.epoch).unwrap_or(0);
            let start_step = config.resume_from.as_ref().map(|c| c.step as u32).unwrap_or(0);
            if start_epoch > 0 || start_step > 0 {
                info!("Task {}: resuming finetune from epoch {} step {}", task_id, start_epoch, start_step);
            }

            for epoch in start_epoch..total_epochs {
                let base_pct = 10.0;
                let first_step = if epoch == start_epoch { start_step } else { 0 };
                for step in first_step..steps_per_epoch {
                    let global_step = epoch * steps_per_epoch + step;
                    let total_steps = total_epochs * steps_per_epoch;
                    let pct =
                        base_pct + (global_step as f32 / total_steps as f32) * (100.0 - base_pct);
                    report_progress(tx, task_id, pct).await;
                    tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms)).await;

                    // Emit checkpoint every 4 steps
                    if global_step > 0 && global_step.is_multiple_of(4) {
                        let state = bincode::serialize(&(epoch, step, pct)).unwrap_or_default();
                        let _ = tx.send(AgentMessage::Checkpoint {
                            task_id,
                            epoch,
                            step: global_step as u64,
                            data: state,
                        }).await;
                    }
                }
                info!(
                    "Task {}: finetune epoch {}/{} complete",
                    task_id,
                    epoch + 1,
                    total_epochs
                );
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Task {}: fine-tuning complete (adapter={}, output={})",
                task_id, config.adapter, config.output_path
            );
            Ok(None)
        }

        AIPayload::PreprocessRequest(config) => {
            info!(
                "Task {}: preprocessing (input={}, format={})",
                task_id, config.input_path, config.format
            );

            if config.input_path.is_empty() {
                return Err("Preprocessing requires --input path".into());
            }

            let stages = ["validating", "tokenizing", "encoding", "writing"];
            for (i, stage) in stages.iter().enumerate() {
                let pct = ((i + 1) as f32 / stages.len() as f32) * 100.0;
                info!("Task {}: {} data", task_id, stage);
                report_progress(tx, task_id, pct).await;
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Task {}: preprocessing complete (output={})",
                task_id, config.output_path
            );
            Ok(None)
        }

        AIPayload::EvalRequest(config) => {
            info!(
                "Task {}: evaluation (model={}, dataset={}, metrics={:?})",
                task_id, config.model_path, config.dataset_path, config.metrics
            );

            if config.model_path.is_empty() {
                return Err("Evaluation requires --model path".into());
            }
            if config.dataset_path.is_empty() {
                return Err("Evaluation requires --dataset path".into());
            }

            report_progress(tx, task_id, 10.0).await;
            info!("Task {}: loading model", task_id);
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;

            let total_metrics = config.metrics.len().max(1);
            for (i, metric) in config.metrics.iter().enumerate() {
                let pct = 20.0 + (i as f32 / total_metrics as f32) * 75.0;
                info!("Task {}: computing metric '{}'", task_id, metric);
                report_progress(tx, task_id, pct).await;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Task {}: evaluation complete ({} metrics computed)",
                task_id,
                config.metrics.len()
            );
            Ok(None)
        }

        AIPayload::RagRequest(config) => {
            info!(
                "Task {}: RAG query (query='{}', index={}, top_k={})",
                task_id, config.query, config.index_path, config.top_k
            );

            if config.query.is_empty() {
                return Err("RAG requires a query string".into());
            }
            if config.index_path.is_empty() {
                return Err("RAG requires --index path".into());
            }

            info!("Task {}: retrieving top-{} documents", task_id, config.top_k);
            report_progress(tx, task_id, 20.0).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            info!("Task {}: generating response from context", task_id);
            report_progress(tx, task_id, 60.0).await;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            report_progress(tx, task_id, 100.0).await;
            info!("Task {}: RAG complete", task_id);

            let response_tokens: Vec<u32> = (0..32)
                .map(|i| {
                    (config.query.len() as u32)
                        .wrapping_mul(7)
                        .wrapping_add(i)
                        % 50257
                })
                .collect();

            Ok(Some(AIPayload::InferenceResponse {
                tokens: response_tokens,
                latency_ms: 800.0,
            }))
        }

        AIPayload::EmbedRequest(config) => {
            info!(
                "Task {}: embedding (input='{}', op={})",
                task_id,
                &config.input[..config.input.len().min(64)],
                config.operation
            );

            if config.input.is_empty() {
                return Err("Embedding requires input text".into());
            }

            report_progress(tx, task_id, 30.0).await;
            info!("Task {}: tokenizing input", task_id);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            report_progress(tx, task_id, 70.0).await;
            info!("Task {}: computing embeddings", task_id);
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            let dim = 768;
            let embeddings: Vec<f32> = (0..dim)
                .map(|i| {
                    let seed = config.input.len() as f32 * 0.01 + i as f32 * 0.001;
                    (seed * std::f32::consts::E).sin() * 0.1
                })
                .collect();

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Task {}: embedding complete (dim={}, op={})",
                task_id, dim, config.operation
            );

            Ok(Some(AIPayload::Gradients {
                data: embeddings,
                model_id: config.model_path.clone().unwrap_or_else(|| "default".into()),
            }))
        }

        AIPayload::Plugin { name, config } => {
            info!("Task {}: running plugin '{}' ", task_id, name);
            report_progress(tx, task_id, 10.0).await;

            let manifests = architect_core::plugin::discover_plugins();
            let manifest = match manifests.get(name.as_str()) {
                Some(m) => m,
                None => return Err(format!("plugin '{}' not found", name)),
            };

            report_progress(tx, task_id, 20.0).await;

            let output = tokio::process::Command::new(&manifest.command)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();

            let mut child = match output {
                Ok(c) => c,
                Err(e) => return Err(format!("failed to spawn plugin '{}': {}", name, e)),
            };

            // Write config to stdin
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(config.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }

            // Wait with timeout
            let timeout = std::time::Duration::from_millis(manifest.timeout_ms);
            let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

            match result {
                Ok(Ok(output)) => {
                    // Parse stderr for progress lines
                    let stderr_str = String::from_utf8_lossy(&output.stderr);
                    for line in stderr_str.lines() {
                        if let Some(pct_str) = line.strip_prefix("PROGRESS:") {
                            if let Ok(pct) = pct_str.trim().parse::<f32>() {
                                report_progress(tx, task_id, 20.0 + pct * 70.0).await;
                            }
                        }
                    }

                    report_progress(tx, task_id, 100.0).await;

                    if output.status.success() {
                        info!("Task {}: plugin '{}' completed successfully", task_id, name);
                        Ok(None)
                    } else {
                        let err = stderr_str.lines().last().unwrap_or("unknown error");
                        Err(format!("plugin '{}' exited with error: {}", name, err))
                    }
                }
                Ok(Err(e)) => Err(format!("plugin '{}' IO error: {}", name, e)),
                Err(_) => Err(format!("plugin '{}' timed out after {}ms", name, manifest.timeout_ms)),
            }
        }
    }
}

/// Get the number of CPU cores (logical).
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

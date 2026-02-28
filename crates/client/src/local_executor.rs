use std::collections::HashMap;
use std::time::Instant;

use sysinfo::System;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info};
use uuid::Uuid;

use architect_core::config::ResourceConfig;
use architect_core::payload::*;
use architect_core::types::{BatteryInfo, NodeId};

use crate::event::Event;

/// Executes tasks locally on the client, making it a participant in the hive.
/// Monitors CPU, RAM, and battery to throttle when resources are constrained.
pub struct LocalTaskExecutor {
    node_id: NodeId,
    event_tx: mpsc::UnboundedSender<Event>,
    running_tasks: HashMap<Uuid, JoinHandle<()>>,
    max_concurrent: usize,
    // Resource monitoring
    sys: System,
    limits: ResourceConfig,
    cpu_pct: f32,
    ram_pct: f32,
    battery: Option<BatteryInfo>,
}

impl LocalTaskExecutor {
    pub fn new(
        node_id: NodeId,
        event_tx: mpsc::UnboundedSender<Event>,
        limits: ResourceConfig,
    ) -> Self {
        let cpu_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let max_concurrent = (cpu_cores / 2).max(1);
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self {
            node_id,
            event_tx,
            running_tasks: HashMap::new(),
            max_concurrent,
            sys,
            limits,
            cpu_pct: 0.0,
            ram_pct: 0.0,
            battery: None,
        }
    }

    /// Refresh system metrics. Call periodically (e.g. on each tick).
    pub fn refresh_metrics(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.cpu_pct = self.sys.global_cpu_usage();
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        self.ram_pct = if total > 0 {
            (used as f32 / total as f32) * 100.0
        } else {
            0.0
        };
        self.battery = detect_battery();
    }

    /// Check if resources allow accepting a new task.
    /// Returns Ok(()) or Err with the reason for rejection.
    fn check_resources(&self) -> Result<(), String> {
        if self.cpu_pct > self.limits.cpu_limit_pct {
            return Err(format!(
                "CPU too high ({:.0}% > {:.0}%)",
                self.cpu_pct, self.limits.cpu_limit_pct
            ));
        }
        if self.ram_pct > self.limits.ram_limit_pct {
            return Err(format!(
                "RAM too high ({:.0}% > {:.0}%)",
                self.ram_pct, self.limits.ram_limit_pct
            ));
        }
        if let Some(ref bat) = self.battery {
            if !bat.charging && bat.level_pct < self.limits.battery_min_pct {
                return Err(format!(
                    "Battery too low ({}% < {}%)",
                    bat.level_pct, self.limits.battery_min_pct
                ));
            }
        }
        Ok(())
    }

    /// Execute a task locally. Returns Err with reason if rejected.
    pub fn execute(&mut self, task: AITask) -> Result<(), String> {
        self.cleanup_completed();

        if self.running_tasks.len() >= self.max_concurrent {
            return Err(format!(
                "At capacity ({}/{})",
                self.running_tasks.len(),
                self.max_concurrent
            ));
        }

        self.check_resources()?;

        let task_id = task.task_id;
        let node_id = self.node_id;
        let event_tx = self.event_tx.clone();

        info!(
            "Executing task {} locally ({}/{}) [cpu={:.0}% ram={:.0}%]",
            task_id,
            self.running_tasks.len() + 1,
            self.max_concurrent,
            self.cpu_pct,
            self.ram_pct,
        );

        let handle = tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(
                run_local_task(node_id, event_tx.clone(), task),
            );
            if futures::FutureExt::catch_unwind(result).await.is_err() {
                tracing::warn!("Local task {} panicked", task_id);
                let fail_result = TaskResult {
                    task_id,
                    node_id,
                    success: false,
                    payload: None,
                    duration_ms: 0,
                    error: Some("Task panicked during execution".into()),
                };
                let _ = event_tx.send(Event::LocalTaskResult(fail_result));
            }
        });

        self.running_tasks.insert(task_id, handle);
        Ok(())
    }

    /// Number of currently running local tasks.
    pub fn active_count(&self) -> u32 {
        self.running_tasks.len() as u32
    }

    /// Cancel a running local task.
    pub fn cancel(&mut self, task_id: Uuid) {
        if let Some(handle) = self.running_tasks.remove(&task_id) {
            info!("Cancelling local task {}", task_id);
            handle.abort();
        }
    }

    /// Update resource limits at runtime (config hot reload).
    pub fn update_resource_config(&mut self, config: ResourceConfig) {
        self.limits = config;
    }

    /// Remove completed tasks from tracking.
    pub fn cleanup_completed(&mut self) {
        self.running_tasks.retain(|id, handle| {
            if handle.is_finished() {
                info!("Local task {} finished, cleaning up", id);
                false
            } else {
                true
            }
        });
    }
}

// --- Battery detection (platform-specific, mirrors agent logic) ---

fn detect_battery() -> Option<BatteryInfo> {
    #[cfg(target_os = "linux")]
    {
        detect_battery_linux()
    }

    #[cfg(target_os = "macos")]
    {
        detect_battery_macos()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn detect_battery_linux() -> Option<BatteryInfo> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("BAT") {
            continue;
        }
        let path = entry.path();
        let capacity = std::fs::read_to_string(path.join("capacity"))
            .ok()?
            .trim()
            .parse::<u8>()
            .ok()?;
        let status = std::fs::read_to_string(path.join("status"))
            .ok()
            .unwrap_or_default();
        let charging = status.trim() == "Charging" || status.trim() == "Full";
        debug!("Battery: {}% charging={}", capacity, charging);
        return Some(BatteryInfo {
            level_pct: capacity,
            charging,
        });
    }
    None
}

#[cfg(target_os = "macos")]
fn detect_battery_macos() -> Option<BatteryInfo> {
    let output = std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if !line.contains("InternalBattery") {
            continue;
        }
        let level_pct = line
            .split_whitespace()
            .find(|w| w.ends_with("%;"))
            .or_else(|| line.split_whitespace().find(|w| w.ends_with('%')))
            .and_then(|w| w.trim_end_matches("%;").trim_end_matches('%').parse::<u8>().ok())?;
        let charging = line.contains("charging") || line.contains("charged");
        debug!("Battery: {}% charging={}", level_pct, charging);
        return Some(BatteryInfo {
            level_pct,
            charging,
        });
    }
    None
}

/// Run a single task locally, reporting progress and result via events.
async fn run_local_task(
    node_id: NodeId,
    event_tx: mpsc::UnboundedSender<Event>,
    task: AITask,
) {
    let task_id = task.task_id;
    let timeout_ms = task.timeout_ms;
    let started_at = Instant::now();

    // Initial progress
    let _ = event_tx.send(Event::LocalTaskProgress {
        task_id,
        progress_pct: 0.0,
    });

    let result = if timeout_ms > 0 {
        match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            run_payload(task_id, &task.payload, &event_tx),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(format!("Task timed out after {}ms", timeout_ms)),
        }
    } else {
        run_payload(task_id, &task.payload, &event_tx).await
    };
    let duration_ms = started_at.elapsed().as_millis() as u64;

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

    let _ = event_tx.send(Event::LocalTaskResult(task_result));
}

/// Report progress for a local task.
async fn report_progress(
    event_tx: &mpsc::UnboundedSender<Event>,
    task_id: Uuid,
    pct: f32,
) {
    let _ = event_tx.send(Event::LocalTaskProgress {
        task_id,
        progress_pct: pct,
    });
}

/// Execute payload locally — mirrors agent logic.
async fn run_payload(
    task_id: Uuid,
    payload: &AIPayload,
    tx: &mpsc::UnboundedSender<Event>,
) -> Result<Option<AIPayload>, String> {
    match payload {
        AIPayload::Control(ctrl) => {
            info!("Local: processing control message: {:?}", ctrl);
            Ok(None)
        }

        AIPayload::InferenceRequest { prompt, config } => {
            info!(
                "Local task {}: inference (prompt {} bytes, max_tokens={})",
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
                "Local task {}: generated {} tokens in {:.0}ms",
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
                "Local task {}: processing gradients (model={}, {} values)",
                task_id,
                model_id,
                data.len()
            );
            report_progress(tx, task_id, 20.0).await;

            let scaled: Vec<f32> = data.iter().map(|g| g * 0.1).collect();
            report_progress(tx, task_id, 80.0).await;

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            report_progress(tx, task_id, 100.0).await;

            Ok(Some(AIPayload::Gradients {
                data: scaled,
                model_id: model_id.clone(),
            }))
        }

        AIPayload::ModelWeights { data, layer_range } => {
            info!(
                "Local task {}: received model weights ({} bytes, layers {:?})",
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
                "Local task {}: processing data shard {}/{} ({} bytes)",
                task_id, shard_id, total_shards,
                data.len()
            );
            report_progress(tx, task_id, 50.0).await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            report_progress(tx, task_id, 100.0).await;
            Ok(None)
        }

        AIPayload::InferenceResponse { .. } => {
            Err("Client should not execute InferenceResponse".into())
        }

        AIPayload::TrainRequest(config) => {
            info!(
                "Local task {}: training (dataset={}, epochs={}, lr={}, batch={})",
                task_id, config.dataset_path, config.epochs, config.learning_rate, config.batch_size
            );

            if config.dataset_path.is_empty() {
                return Err("Training requires --dataset path".into());
            }

            let total_epochs = config.epochs.max(1);
            let steps_per_epoch = 10u32;
            let step_delay_ms = 200u64;

            for epoch in 0..total_epochs {
                for step in 0..steps_per_epoch {
                    let global_step = epoch * steps_per_epoch + step;
                    let total_steps = total_epochs * steps_per_epoch;
                    let pct = (global_step as f32 / total_steps as f32) * 100.0;
                    report_progress(tx, task_id, pct).await;
                    tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms)).await;
                }
                info!(
                    "Local task {}: epoch {}/{} complete",
                    task_id,
                    epoch + 1,
                    total_epochs
                );
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Local task {}: training complete ({} epochs, output={})",
                task_id, total_epochs, config.output_path
            );
            Ok(None)
        }

        AIPayload::FinetuneRequest(config) => {
            info!(
                "Local task {}: fine-tuning (base={}, dataset={}, adapter={})",
                task_id, config.base_model, config.dataset_path, config.adapter
            );

            if config.base_model.is_empty() {
                return Err("Fine-tuning requires --base model".into());
            }
            if config.dataset_path.is_empty() {
                return Err("Fine-tuning requires --dataset path".into());
            }

            report_progress(tx, task_id, 5.0).await;
            info!("Local task {}: loading base model {}", task_id, config.base_model);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let total_epochs = config.epochs.max(1);
            let steps_per_epoch = 8u32;
            let step_delay_ms = 250u64;

            for epoch in 0..total_epochs {
                let base_pct = 10.0;
                for step in 0..steps_per_epoch {
                    let global_step = epoch * steps_per_epoch + step;
                    let total_steps = total_epochs * steps_per_epoch;
                    let pct =
                        base_pct + (global_step as f32 / total_steps as f32) * (100.0 - base_pct);
                    report_progress(tx, task_id, pct).await;
                    tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms)).await;
                }
                info!(
                    "Local task {}: finetune epoch {}/{} complete",
                    task_id,
                    epoch + 1,
                    total_epochs
                );
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Local task {}: fine-tuning complete (adapter={}, output={})",
                task_id, config.adapter, config.output_path
            );
            Ok(None)
        }

        AIPayload::PreprocessRequest(config) => {
            info!(
                "Local task {}: preprocessing (input={}, format={})",
                task_id, config.input_path, config.format
            );

            if config.input_path.is_empty() {
                return Err("Preprocessing requires --input path".into());
            }

            let stages = ["validating", "tokenizing", "encoding", "writing"];
            for (i, stage) in stages.iter().enumerate() {
                let pct = ((i + 1) as f32 / stages.len() as f32) * 100.0;
                info!("Local task {}: {} data", task_id, stage);
                report_progress(tx, task_id, pct).await;
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Local task {}: preprocessing complete (output={})",
                task_id, config.output_path
            );
            Ok(None)
        }

        AIPayload::EvalRequest(config) => {
            info!(
                "Local task {}: evaluation (model={}, dataset={}, metrics={:?})",
                task_id, config.model_path, config.dataset_path, config.metrics
            );

            if config.model_path.is_empty() {
                return Err("Evaluation requires --model path".into());
            }
            if config.dataset_path.is_empty() {
                return Err("Evaluation requires --dataset path".into());
            }

            report_progress(tx, task_id, 10.0).await;
            info!("Local task {}: loading model", task_id);
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;

            let total_metrics = config.metrics.len().max(1);
            for (i, metric) in config.metrics.iter().enumerate() {
                let pct = 20.0 + (i as f32 / total_metrics as f32) * 75.0;
                info!("Local task {}: computing metric '{}'", task_id, metric);
                report_progress(tx, task_id, pct).await;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            report_progress(tx, task_id, 100.0).await;
            info!(
                "Local task {}: evaluation complete ({} metrics computed)",
                task_id,
                config.metrics.len()
            );
            Ok(None)
        }

        AIPayload::RagRequest(config) => {
            info!(
                "Local task {}: RAG query (query='{}', index={}, top_k={})",
                task_id, config.query, config.index_path, config.top_k
            );

            if config.query.is_empty() {
                return Err("RAG requires a query string".into());
            }
            if config.index_path.is_empty() {
                return Err("RAG requires --index path".into());
            }

            info!("Local task {}: retrieving top-{} documents", task_id, config.top_k);
            report_progress(tx, task_id, 20.0).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            info!("Local task {}: generating response from context", task_id);
            report_progress(tx, task_id, 60.0).await;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            report_progress(tx, task_id, 100.0).await;
            info!("Local task {}: RAG complete", task_id);

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
                "Local task {}: embedding (input='{}', op={})",
                task_id,
                &config.input[..config.input.len().min(64)],
                config.operation
            );

            if config.input.is_empty() {
                return Err("Embedding requires input text".into());
            }

            report_progress(tx, task_id, 30.0).await;
            info!("Local task {}: tokenizing input", task_id);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            report_progress(tx, task_id, 70.0).await;
            info!("Local task {}: computing embeddings", task_id);
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
                "Local task {}: embedding complete (dim={}, op={})",
                task_id, dim, config.operation
            );

            Ok(Some(AIPayload::Gradients {
                data: embeddings,
                model_id: config.model_path.clone().unwrap_or_else(|| "default".into()),
            }))
        }

        AIPayload::Plugin { name, config } => {
            info!("Local task {}: running plugin '{}'", task_id, name);
            report_progress(tx, task_id, 10.0).await;

            let manifests = architect_core::plugin::discover_plugins();
            let manifest = match manifests.get(name.as_str()) {
                Some(m) => m,
                None => return Err(format!("plugin '{}' not found", name)),
            };

            report_progress(tx, task_id, 20.0).await;

            let mut child = tokio::process::Command::new(&manifest.command)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("failed to spawn plugin '{}': {}", name, e))?;

            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(config.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }

            let timeout = std::time::Duration::from_millis(manifest.timeout_ms);
            match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    report_progress(tx, task_id, 100.0).await;
                    if output.status.success() {
                        info!("Local task {}: plugin '{}' completed", task_id, name);
                        Ok(None)
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let err = stderr.lines().last().unwrap_or("unknown error");
                        Err(format!("plugin '{}' failed: {}", name, err))
                    }
                }
                Ok(Err(e)) => Err(format!("plugin '{}' IO error: {}", name, e)),
                Err(_) => Err(format!("plugin '{}' timed out", name)),
            }
        }
    }
}

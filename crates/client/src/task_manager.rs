use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use architect_core::payload::{AIPayload, AITask, CheckpointData, TaskResult, TaskStatus, TaskType};
use architect_core::types::NodeId;

/// View of a task for display in the TUI.
#[derive(Debug, Clone)]
pub struct TaskView {
    pub task_id: Uuid,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub assigned_to: Option<NodeId>,
    pub progress: f32,
}

/// Maximum number of terminal (completed/failed/cancelled) tasks to keep in memory.
const MAX_TERMINAL_TASKS: usize = 500;

/// Manages task lifecycle: submission, assignment, completion.
pub struct TaskManager {
    tasks: HashMap<Uuid, TaskEntry>,
    /// Set of task IDs that have been completed — used for idempotency.
    completed_ids: std::collections::HashSet<Uuid>,
}

struct TaskEntry {
    task: AITask,
    status: TaskStatus,
    assigned_to: Option<NodeId>,
    result: Option<TaskResult>,
    progress: f32,
    progress_msg: Option<String>,
    retry_count: u32,
    max_retries: u32,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            completed_ids: std::collections::HashSet::new(),
        }
    }

    /// Submit a new task. Returns the task ID.
    pub fn submit(&mut self, task: AITask) -> Uuid {
        let task_id = task.task_id;
        info!("Task submitted: {}", task_id);

        self.tasks.insert(
            task_id,
            TaskEntry {
                task,
                status: TaskStatus::Pending,
                assigned_to: None,
                result: None,
                progress: 0.0,
                progress_msg: None,
                retry_count: 0,
                max_retries: 3,
            },
        );

        task_id
    }

    /// Assign a task to a node.
    pub fn assign(&mut self, task_id: Uuid, node_id: NodeId) {
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            entry.status = TaskStatus::Assigned;
            entry.assigned_to = Some(node_id);
            info!("Task {} assigned to {}", task_id, node_id);
        }
    }

    /// Mark a task as running with optional progress.
    pub fn update_progress(&mut self, task_id: Uuid, pct: f32, msg: Option<String>) {
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            entry.status = TaskStatus::Running;
            entry.progress = pct;
            entry.progress_msg = msg;
        }
    }

    /// Complete a task with a result. Idempotent: ignores duplicate completions.
    pub fn complete(&mut self, task_id: Uuid, result: TaskResult) {
        // Idempotency: skip if already completed
        if self.completed_ids.contains(&task_id) {
            info!("Task {} already completed, ignoring duplicate result", task_id);
            return;
        }
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            if matches!(entry.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled) {
                info!("Task {} already terminal ({}), ignoring", task_id, entry.status);
                return;
            }
            entry.status = if result.success {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            };
            entry.progress = if result.success { 100.0 } else { entry.progress };
            entry.result = Some(result);
            self.completed_ids.insert(task_id);
            info!("Task {} completed", task_id);
        }
    }

    /// Cancel a task.
    pub fn cancel(&mut self, task_id: Uuid) {
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            entry.status = TaskStatus::Cancelled;
            info!("Task {} cancelled", task_id);
        }
    }

    /// Re-queue a task for reassignment. Returns true if re-queued, false if max retries exceeded.
    /// Idempotent: if already Pending, returns true without incrementing retry count.
    pub fn requeue(&mut self, task_id: Uuid) -> bool {
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            // Already pending — avoid double-count from AgentDisconnected + tick timeout
            if entry.status == TaskStatus::Pending {
                return true;
            }

            entry.retry_count += 1;
            if entry.retry_count > entry.max_retries {
                entry.status = TaskStatus::Failed;
                entry.result = Some(TaskResult {
                    task_id,
                    node_id: entry.assigned_to.unwrap_or(Uuid::nil()),
                    success: false,
                    payload: None,
                    duration_ms: 0,
                    error: Some(format!("Max retries ({}) exceeded", entry.max_retries)),
                });
                info!("Task {} failed: max retries exceeded", task_id);
                return false;
            }

            entry.status = TaskStatus::Pending;
            entry.assigned_to = None;
            entry.progress = 0.0;
            entry.progress_msg = None;
            info!(
                "Task {} re-queued (retry {}/{})",
                task_id, entry.retry_count, entry.max_retries
            );
            true
        } else {
            false
        }
    }

    /// Inject checkpoint data into a task's payload for resume-from-checkpoint on requeue.
    pub fn inject_checkpoint(&mut self, task_id: Uuid, checkpoint: CheckpointData) {
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            match &mut entry.task.payload {
                AIPayload::TrainRequest(config) => {
                    config.resume_from = Some(checkpoint);
                }
                AIPayload::FinetuneRequest(config) => {
                    config.resume_from = Some(checkpoint);
                }
                _ => {}
            }
        }
    }

    /// Revert an assigned task back to pending (e.g. when local execution is throttled).
    pub fn revert_to_pending(&mut self, task_id: Uuid) {
        if let Some(entry) = self.tasks.get_mut(&task_id) {
            if entry.status == TaskStatus::Assigned {
                entry.status = TaskStatus::Pending;
                entry.assigned_to = None;
            }
        }
    }

    /// Get all active task IDs assigned to a given node.
    pub fn tasks_assigned_to(&self, node_id: NodeId) -> Vec<Uuid> {
        self.tasks
            .iter()
            .filter(|(_, e)| {
                e.assigned_to == Some(node_id)
                    && matches!(
                        e.status,
                        TaskStatus::Assigned | TaskStatus::Running
                    )
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Get pending tasks that are ready for dispatch (all dependencies satisfied).
    /// Sorted by priority descending (highest priority first).
    pub fn pending_tasks(&self) -> Vec<AITask> {
        let mut tasks: Vec<&TaskEntry> = self
            .tasks
            .values()
            .filter(|e| e.status == TaskStatus::Pending)
            .filter(|e| self.dependencies_satisfied(&e.task))
            .collect();

        // Sort by priority descending
        tasks.sort_by(|a, b| b.task.priority.cmp(&a.task.priority));

        tasks.iter().map(|e| e.task.clone()).collect()
    }

    /// Check if all dependencies of a task are completed.
    fn dependencies_satisfied(&self, task: &AITask) -> bool {
        task.depends_on.iter().all(|dep_id| {
            self.completed_ids.contains(dep_id)
        })
    }

    /// Find the lowest-priority task assigned to a given node (for work stealing).
    pub fn lowest_priority_task_on(&self, node_id: NodeId) -> Option<Uuid> {
        self.tasks
            .iter()
            .filter(|(_, e)| {
                e.assigned_to == Some(node_id)
                    && matches!(e.status, TaskStatus::Assigned | TaskStatus::Running)
            })
            .min_by_key(|(_, e)| e.task.priority)
            .map(|(id, _)| *id)
    }

    /// Find a task by ID prefix (first N chars match).
    pub fn find_by_prefix(&self, prefix: &str) -> Option<Uuid> {
        let prefix = prefix.to_lowercase();
        self.tasks
            .keys()
            .find(|id| id.to_string().starts_with(&prefix))
            .copied()
    }

    /// Get all tasks as views, sorted by creation time (newest first).
    pub fn all_tasks(&self) -> Vec<TaskView> {
        let mut views: Vec<TaskView> = self.tasks.values().map(|e| self.entry_to_view(e)).collect();
        views.sort_by(|a, b| b.task_id.cmp(&a.task_id));
        views
    }

    /// Get active tasks (pending, assigned, running).
    pub fn active_tasks(&self) -> Vec<TaskView> {
        self.tasks
            .values()
            .filter(|e| {
                matches!(
                    e.status,
                    TaskStatus::Pending | TaskStatus::Assigned | TaskStatus::Running
                )
            })
            .map(|e| self.entry_to_view(e))
            .collect()
    }

    /// Remove oldest terminal tasks if we exceed MAX_TERMINAL_TASKS.
    pub fn prune_terminal(&mut self) {
        let terminal_count = self
            .tasks
            .values()
            .filter(|e| {
                matches!(
                    e.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                )
            })
            .count();

        if terminal_count <= MAX_TERMINAL_TASKS {
            return;
        }

        // Collect terminal task IDs sorted by created_at (oldest first)
        let mut terminal: Vec<(Uuid, u64)> = self
            .tasks
            .iter()
            .filter(|(_, e)| {
                matches!(
                    e.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                )
            })
            .map(|(id, e)| (*id, e.task.created_at))
            .collect();
        terminal.sort_by_key(|(_, t)| *t);

        let to_remove = terminal_count - MAX_TERMINAL_TASKS;
        for (id, _) in terminal.into_iter().take(to_remove) {
            self.tasks.remove(&id);
        }
        info!("Pruned {} terminal tasks", to_remove);
    }

    fn entry_to_view(&self, e: &TaskEntry) -> TaskView {
        TaskView {
            task_id: e.task.task_id,
            task_type: e.task.task_type,
            status: e.status,
            assigned_to: e.assigned_to,
            progress: e.progress,
        }
    }

    // --- Persistence ---

    /// Persist all non-terminal tasks to the SQLite database.
    pub fn save_to_db(&self, conn: &rusqlite::Connection) -> anyhow::Result<()> {
        let tx = conn.unchecked_transaction()?;
        for e in self.tasks.values() {
            if matches!(e.status, TaskStatus::Pending | TaskStatus::Assigned | TaskStatus::Running) {
                let payload = bincode::serialize(&e.task.payload).unwrap_or_default();
                architect_core::db::upsert_task(
                    &tx,
                    &architect_core::db::TaskRow {
                        id: &e.task.task_id.to_string(),
                        task_type: &format!("{}", e.task.task_type),
                        payload: &payload,
                        priority: e.task.priority,
                        created_at: e.task.created_at,
                        timeout_ms: e.task.timeout_ms,
                        status: &format!("{}", e.status),
                        assigned_to: e.assigned_to.as_ref().map(|id| id.to_string()).as_deref(),
                        retry_count: e.retry_count,
                        max_retries: e.max_retries,
                        progress: e.progress,
                        error: None,
                    },
                )?;
            }
        }
        // Clean up old terminal tasks (older than 24h)
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(86400);
        architect_core::db::delete_old_terminal_tasks(&tx, cutoff)?;
        tx.commit()?;
        Ok(())
    }

    /// Load persisted tasks from SQLite, treating all as Pending for reassignment.
    pub fn load_from_db(&mut self, conn: &rusqlite::Connection) -> anyhow::Result<usize> {
        let rows = architect_core::db::load_active_tasks(conn)?;
        let count = rows.len();

        for row in rows {
            let task_id = match Uuid::parse_str(&row.id) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let task_type = match row.task_type.as_str() {
                "train" => TaskType::Train,
                "finetune" => TaskType::Finetune,
                "infer" => TaskType::Inference,
                "preprocess" => TaskType::Preprocess,
                "eval" => TaskType::Evaluate,
                "rag" => TaskType::Rag,
                "embed" => TaskType::Embed,
                _ => TaskType::Compute,
            };
            let payload: architect_core::payload::AIPayload = match bincode::deserialize(&row.payload) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let task = AITask {
                task_id,
                task_type,
                payload,
                priority: row.priority,
                created_at: row.created_at,
                timeout_ms: row.timeout_ms,
                depends_on: Vec::new(),
                quota: None,
            };

            self.tasks.insert(
                task_id,
                TaskEntry {
                    task,
                    status: TaskStatus::Pending,
                    assigned_to: None,
                    result: None,
                    progress: 0.0,
                    progress_msg: None,
                    retry_count: row.retry_count,
                    max_retries: row.max_retries,
                },
            );
        }

        if count > 0 {
            info!("Loaded {} tasks from database", count);
        }
        Ok(count)
    }

    /// Legacy: load from JSON file (fallback for migration from pre-SQLite versions).
    pub fn load_from_file(&mut self, path: &std::path::Path) -> anyhow::Result<usize> {
        if !path.exists() {
            return Ok(0);
        }
        let json = std::fs::read_to_string(path)?;
        let persisted: Vec<PersistedTask> = serde_json::from_str(&json)?;
        let count = persisted.len();

        for p in persisted {
            self.tasks.insert(
                p.task.task_id,
                TaskEntry {
                    task: p.task,
                    status: TaskStatus::Pending,
                    assigned_to: None,
                    result: None,
                    progress: 0.0,
                    progress_msg: None,
                    retry_count: p.retry_count,
                    max_retries: p.max_retries,
                },
            );
        }

        let _ = std::fs::remove_file(path);
        info!("Loaded {} tasks from journal", count);
        Ok(count)
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable snapshot of a task for journal persistence.
#[derive(Serialize, Deserialize)]
struct PersistedTask {
    task: AITask,
    retry_count: u32,
    max_retries: u32,
}

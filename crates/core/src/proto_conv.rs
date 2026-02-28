//! Conversions between Rust domain types and generated protobuf types.

use uuid::Uuid;

use crate::payload::*;
use crate::proto::*;
use crate::protocol::{AgentMessage, ClientMessage};
use crate::types::*;

// --- Helper: serialize AIPayload to bytes via bincode ---

fn encode_payload(payload: &AIPayload) -> Vec<u8> {
    bincode::serialize(payload).unwrap_or_default()
}

fn decode_payload(data: &[u8]) -> Option<AIPayload> {
    bincode::deserialize(data).ok()
}

// --- NodeInfo ↔ NodeInfoProto ---

impl From<&NodeInfo> for NodeInfoProto {
    fn from(info: &NodeInfo) -> Self {
        NodeInfoProto {
            id: info.id.to_string(),
            role: format!("{}", info.role),
            hostname: info.hostname.clone(),
            ip: info.ip.clone(),
            os: format!("{:?}", info.os),
            arch: format!("{:?}", info.arch),
            device_type: format!("{:?}", info.device_type),
            capabilities: Some(NodeCapabilitiesProto::from(&info.capabilities)),
            auth_token: info.auth_token.clone(),
        }
    }
}

impl TryFrom<&NodeInfoProto> for NodeInfo {
    type Error = String;

    fn try_from(proto: &NodeInfoProto) -> Result<Self, String> {
        let id = Uuid::parse_str(&proto.id).map_err(|e| format!("Invalid UUID: {}", e))?;
        let role = match proto.role.as_str() {
            "Client" => NodeRole::Client,
            _ => NodeRole::Agent,
        };
        let os = match proto.os.as_str() {
            "Linux" => OsKind::Linux,
            "MacOS" => OsKind::MacOS,
            "Windows" => OsKind::Windows,
            "Android" => OsKind::Android,
            _ => OsKind::Unknown,
        };
        let arch = match proto.arch.as_str() {
            "X86_64" => ArchKind::X86_64,
            "X86" => ArchKind::X86,
            "Aarch64" => ArchKind::Aarch64,
            "Armv7" => ArchKind::Armv7,
            _ => ArchKind::Unknown,
        };
        let device_type = match proto.device_type.as_str() {
            "Desktop" => DeviceType::Desktop,
            "Laptop" => DeviceType::Laptop,
            "Phone" => DeviceType::Phone,
            "SmartTV" => DeviceType::SmartTV,
            "RaspberryPi" => DeviceType::RaspberryPi,
            _ => DeviceType::Unknown,
        };
        let capabilities = proto
            .capabilities
            .as_ref()
            .map(NodeCapabilities::from)
            .unwrap_or_else(|| NodeCapabilities {
                cpu_cores: 0,
                cpu_freq_mhz: 0,
                ram_total_mb: 0,
                ram_available_mb: 0,
                gpu: None,
                battery: None,
                compute_score: 0.0,
            });

        Ok(NodeInfo {
            id,
            role,
            hostname: proto.hostname.clone(),
            ip: proto.ip.clone(),
            os,
            arch,
            device_type,
            capabilities,
            auth_token: proto.auth_token.clone(),
        })
    }
}

// --- NodeCapabilities ↔ NodeCapabilitiesProto ---

impl From<&NodeCapabilities> for NodeCapabilitiesProto {
    fn from(c: &NodeCapabilities) -> Self {
        NodeCapabilitiesProto {
            cpu_cores: c.cpu_cores,
            cpu_freq_mhz: c.cpu_freq_mhz,
            ram_total_mb: c.ram_total_mb,
            ram_available_mb: c.ram_available_mb,
            gpu: c.gpu.as_ref().map(|g| GpuInfoProto {
                name: g.name.clone(),
                vram_gb: g.vram_gb,
                compute_capability: g.compute_capability.clone().unwrap_or_default(),
                opencl_available: g.opencl_available,
            }),
            battery: c.battery.map(|b| BatteryInfoProto {
                level_pct: b.level_pct as u32,
                charging: b.charging,
            }),
            compute_score: c.compute_score,
        }
    }
}

impl From<&NodeCapabilitiesProto> for NodeCapabilities {
    fn from(p: &NodeCapabilitiesProto) -> Self {
        NodeCapabilities {
            cpu_cores: p.cpu_cores,
            cpu_freq_mhz: p.cpu_freq_mhz,
            ram_total_mb: p.ram_total_mb,
            ram_available_mb: p.ram_available_mb,
            gpu: p.gpu.as_ref().map(|g| GpuInfo {
                name: g.name.clone(),
                vram_gb: g.vram_gb,
                compute_capability: if g.compute_capability.is_empty() {
                    None
                } else {
                    Some(g.compute_capability.clone())
                },
                opencl_available: g.opencl_available,
            }),
            battery: p.battery.as_ref().map(|b| BatteryInfo {
                level_pct: b.level_pct as u8,
                charging: b.charging,
            }),
            compute_score: p.compute_score,
        }
    }
}

// --- NodeMetrics ↔ NodeMetricsProto ---

impl From<&NodeMetrics> for NodeMetricsProto {
    fn from(m: &NodeMetrics) -> Self {
        NodeMetricsProto {
            cpu_usage_pct: m.cpu_usage_pct,
            ram_usage_pct: m.ram_usage_pct,
            gpu_usage_pct: m.gpu_usage_pct.unwrap_or(0.0),
            has_gpu_usage: m.gpu_usage_pct.is_some(),
            network_rx_mbps: m.network_rx_mbps,
            network_tx_mbps: m.network_tx_mbps,
            tasks_completed: m.tasks_completed,
            tasks_failed: m.tasks_failed,
            uptime_secs: m.uptime_secs,
        }
    }
}

impl From<&NodeMetricsProto> for NodeMetrics {
    fn from(p: &NodeMetricsProto) -> Self {
        NodeMetrics {
            cpu_usage_pct: p.cpu_usage_pct,
            ram_usage_pct: p.ram_usage_pct,
            gpu_usage_pct: if p.has_gpu_usage {
                Some(p.gpu_usage_pct)
            } else {
                None
            },
            network_rx_mbps: p.network_rx_mbps,
            network_tx_mbps: p.network_tx_mbps,
            tasks_completed: p.tasks_completed,
            tasks_failed: p.tasks_failed,
            uptime_secs: p.uptime_secs,
        }
    }
}

// --- TaskResult ↔ TaskResultProto ---

impl From<&TaskResult> for TaskResultProto {
    fn from(r: &TaskResult) -> Self {
        TaskResultProto {
            task_id: r.task_id.to_string(),
            node_id: r.node_id.to_string(),
            success: r.success,
            payload: r.payload.as_ref().map(encode_payload).unwrap_or_default(),
            has_payload: r.payload.is_some(),
            duration_ms: r.duration_ms,
            error: r.error.clone().unwrap_or_default(),
            has_error: r.error.is_some(),
        }
    }
}

impl TryFrom<&TaskResultProto> for TaskResult {
    type Error = String;

    fn try_from(p: &TaskResultProto) -> Result<Self, String> {
        Ok(TaskResult {
            task_id: Uuid::parse_str(&p.task_id).map_err(|e| e.to_string())?,
            node_id: Uuid::parse_str(&p.node_id).map_err(|e| e.to_string())?,
            success: p.success,
            payload: if p.has_payload {
                decode_payload(&p.payload)
            } else {
                None
            },
            duration_ms: p.duration_ms,
            error: if p.has_error {
                Some(p.error.clone())
            } else {
                None
            },
        })
    }
}

// --- AITask ↔ AITaskProto ---

impl From<&AITask> for AiTaskProto {
    fn from(t: &AITask) -> Self {
        AiTaskProto {
            task_id: t.task_id.to_string(),
            task_type: format!("{}", t.task_type),
            payload: encode_payload(&t.payload),
            priority: t.priority as u32,
            created_at: t.created_at,
            timeout_ms: t.timeout_ms,
            depends_on: t.depends_on.iter().map(|id| id.to_string()).collect(),
            quota: t.quota.as_ref().map(|q| ResourceQuotaProto {
                max_cpu_pct: q.max_cpu_pct.unwrap_or(0.0),
                max_ram_mb: q.max_ram_mb.unwrap_or(0),
                has_cpu: q.max_cpu_pct.is_some(),
                has_ram: q.max_ram_mb.is_some(),
            }),
        }
    }
}

impl TryFrom<&AiTaskProto> for AITask {
    type Error = String;

    fn try_from(p: &AiTaskProto) -> Result<Self, String> {
        let task_type = match p.task_type.as_str() {
            "train" => TaskType::Train,
            "finetune" => TaskType::Finetune,
            "infer" => TaskType::Inference,
            "preprocess" => TaskType::Preprocess,
            "eval" => TaskType::Evaluate,
            "rag" => TaskType::Rag,
            "embed" => TaskType::Embed,
            "plugin" => TaskType::Plugin,
            _ => TaskType::Compute,
        };
        let payload = decode_payload(&p.payload).ok_or("Failed to decode payload")?;
        let depends_on: Vec<Uuid> = p
            .depends_on
            .iter()
            .filter_map(|s| Uuid::parse_str(s).ok())
            .collect();
        let quota = p.quota.as_ref().and_then(|q| {
            let cpu = if q.has_cpu { Some(q.max_cpu_pct) } else { None };
            let ram = if q.has_ram { Some(q.max_ram_mb) } else { None };
            if cpu.is_some() || ram.is_some() {
                Some(ResourceQuota { max_cpu_pct: cpu, max_ram_mb: ram })
            } else {
                None
            }
        });
        Ok(AITask {
            task_id: Uuid::parse_str(&p.task_id).map_err(|e| e.to_string())?,
            task_type,
            payload,
            priority: p.priority as u8,
            created_at: p.created_at,
            timeout_ms: if p.timeout_ms == 0 { 300_000 } else { p.timeout_ms },
            depends_on,
            quota,
        })
    }
}

// --- AgentMessage → AgentMsg ---

impl From<&AgentMessage> for AgentMsg {
    fn from(msg: &AgentMessage) -> Self {
        let payload = match msg {
            AgentMessage::Identity(info) => {
                agent_msg::Payload::Identity(NodeInfoProto::from(info))
            }
            AgentMessage::Heartbeat(metrics) => {
                agent_msg::Payload::Heartbeat(NodeMetricsProto::from(metrics))
            }
            AgentMessage::Capabilities(caps) => {
                agent_msg::Payload::Capabilities(NodeCapabilitiesProto::from(caps))
            }
            AgentMessage::TaskResult(result) => {
                agent_msg::Payload::TaskResult(TaskResultProto::from(result))
            }
            AgentMessage::TaskProgress {
                task_id,
                progress_pct,
            } => agent_msg::Payload::TaskProgress(TaskProgressProto {
                task_id: task_id.to_string(),
                progress_pct: *progress_pct,
            }),
            AgentMessage::Pong { seq } => {
                agent_msg::Payload::Pong(PongProto { seq: *seq })
            }
            AgentMessage::Error { message } => agent_msg::Payload::Error(ErrorProto {
                message: message.clone(),
            }),
            AgentMessage::BufferedResults(results) => {
                agent_msg::Payload::BufferedResults(BufferedResultsProto {
                    results: results.iter().map(TaskResultProto::from).collect(),
                })
            }
            AgentMessage::Checkpoint { task_id, epoch, step, data } => {
                agent_msg::Payload::Checkpoint(CheckpointProto {
                    task_id: task_id.to_string(),
                    epoch: *epoch,
                    step: *step,
                    data: data.clone(),
                })
            }
            AgentMessage::GradientShard { task_id, step, shard_index, data } => {
                agent_msg::Payload::GradientShard(GradientShardProto {
                    task_id: task_id.to_string(),
                    step: *step,
                    shard_index: *shard_index,
                    data: data.clone(),
                })
            }
            AgentMessage::StateSyncResponse { running, completed, unknown } => {
                agent_msg::Payload::StateSyncResponse(StateSyncResponseProto {
                    running: running.iter().map(|id| id.to_string()).collect(),
                    completed: completed.iter().map(|id| id.to_string()).collect(),
                    unknown: unknown.iter().map(|id| id.to_string()).collect(),
                })
            }
            AgentMessage::CacheReport { datasets, models } => {
                agent_msg::Payload::CacheReport(CacheReportProto {
                    datasets: datasets.clone(),
                    models: models.clone(),
                })
            }
        };
        AgentMsg {
            payload: Some(payload),
        }
    }
}

impl TryFrom<AgentMsg> for AgentMessage {
    type Error = String;

    fn try_from(msg: AgentMsg) -> Result<Self, String> {
        let payload = msg.payload.ok_or("Empty agent message")?;
        match payload {
            agent_msg::Payload::Identity(info) => {
                Ok(AgentMessage::Identity(NodeInfo::try_from(&info)?))
            }
            agent_msg::Payload::Heartbeat(metrics) => {
                Ok(AgentMessage::Heartbeat(NodeMetrics::from(&metrics)))
            }
            agent_msg::Payload::Capabilities(caps) => {
                Ok(AgentMessage::Capabilities(NodeCapabilities::from(&caps)))
            }
            agent_msg::Payload::TaskResult(tr) => {
                Ok(AgentMessage::TaskResult(TaskResult::try_from(&tr)?))
            }
            agent_msg::Payload::TaskProgress(tp) => Ok(AgentMessage::TaskProgress {
                task_id: Uuid::parse_str(&tp.task_id).map_err(|e| e.to_string())?,
                progress_pct: tp.progress_pct,
            }),
            agent_msg::Payload::Pong(p) => Ok(AgentMessage::Pong { seq: p.seq }),
            agent_msg::Payload::Error(e) => Ok(AgentMessage::Error {
                message: e.message,
            }),
            agent_msg::Payload::BufferedResults(br) => {
                let results: Result<Vec<TaskResult>, String> = br
                    .results
                    .iter()
                    .map(TaskResult::try_from)
                    .collect();
                Ok(AgentMessage::BufferedResults(results?))
            }
            agent_msg::Payload::Checkpoint(cp) => Ok(AgentMessage::Checkpoint {
                task_id: Uuid::parse_str(&cp.task_id).map_err(|e| e.to_string())?,
                epoch: cp.epoch,
                step: cp.step,
                data: cp.data,
            }),
            agent_msg::Payload::GradientShard(gs) => Ok(AgentMessage::GradientShard {
                task_id: Uuid::parse_str(&gs.task_id).map_err(|e| e.to_string())?,
                step: gs.step,
                shard_index: gs.shard_index,
                data: gs.data,
            }),
            agent_msg::Payload::StateSyncResponse(ssr) => {
                let running = ssr.running.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
                let completed = ssr.completed.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
                let unknown = ssr.unknown.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
                Ok(AgentMessage::StateSyncResponse { running, completed, unknown })
            }
            agent_msg::Payload::CacheReport(cr) => Ok(AgentMessage::CacheReport {
                datasets: cr.datasets,
                models: cr.models,
            }),
        }
    }
}

// --- ClientMessage → CoordinatorMsg ---

impl From<&ClientMessage> for CoordinatorMsg {
    fn from(msg: &ClientMessage) -> Self {
        let payload = match msg {
            ClientMessage::Identify => coordinator_msg::Payload::Identify(Empty {}),
            ClientMessage::RequestMetrics => {
                coordinator_msg::Payload::RequestMetrics(Empty {})
            }
            ClientMessage::RequestCapabilities => {
                coordinator_msg::Payload::RequestCapabilities(Empty {})
            }
            ClientMessage::SubmitTask(task) => {
                coordinator_msg::Payload::SubmitTask(AiTaskProto::from(task))
            }
            ClientMessage::CancelTask { task_id } => {
                coordinator_msg::Payload::CancelTask(CancelTaskProto {
                    task_id: task_id.to_string(),
                })
            }
            ClientMessage::Shutdown => coordinator_msg::Payload::Shutdown(Empty {}),
            ClientMessage::Purge => coordinator_msg::Payload::Purge(Empty {}),
            ClientMessage::Ping { seq } => {
                coordinator_msg::Payload::Ping(PingProto { seq: *seq })
            }
            ClientMessage::UpdateToken { new_token } => {
                coordinator_msg::Payload::UpdateToken(UpdateTokenProto {
                    new_token: new_token.clone(),
                })
            }
            ClientMessage::UpdateAvailable { version, download_url } => {
                coordinator_msg::Payload::UpdateAvailable(UpdateAvailableProto {
                    version: version.clone(),
                    download_url: download_url.clone(),
                })
            }
            ClientMessage::ApplyGradients { task_id, step, aggregated } => {
                coordinator_msg::Payload::ApplyGradients(ApplyGradientsProto {
                    task_id: task_id.to_string(),
                    step: *step,
                    data: aggregated.clone(),
                })
            }
            ClientMessage::StateSync { assigned_tasks } => {
                coordinator_msg::Payload::StateSync(StateSyncProto {
                    assigned_tasks: assigned_tasks.iter().map(|id| id.to_string()).collect(),
                })
            }
        };
        CoordinatorMsg {
            payload: Some(payload),
        }
    }
}

impl TryFrom<CoordinatorMsg> for ClientMessage {
    type Error = String;

    fn try_from(msg: CoordinatorMsg) -> Result<Self, String> {
        let payload = msg.payload.ok_or("Empty coordinator message")?;
        match payload {
            coordinator_msg::Payload::Identify(_) => Ok(ClientMessage::Identify),
            coordinator_msg::Payload::RequestMetrics(_) => Ok(ClientMessage::RequestMetrics),
            coordinator_msg::Payload::RequestCapabilities(_) => {
                Ok(ClientMessage::RequestCapabilities)
            }
            coordinator_msg::Payload::SubmitTask(task) => {
                Ok(ClientMessage::SubmitTask(AITask::try_from(&task)?))
            }
            coordinator_msg::Payload::CancelTask(ct) => Ok(ClientMessage::CancelTask {
                task_id: Uuid::parse_str(&ct.task_id).map_err(|e| e.to_string())?,
            }),
            coordinator_msg::Payload::Shutdown(_) => Ok(ClientMessage::Shutdown),
            coordinator_msg::Payload::Purge(_) => Ok(ClientMessage::Purge),
            coordinator_msg::Payload::Ping(p) => Ok(ClientMessage::Ping { seq: p.seq }),
            coordinator_msg::Payload::UpdateToken(t) => Ok(ClientMessage::UpdateToken {
                new_token: t.new_token,
            }),
            coordinator_msg::Payload::UpdateAvailable(u) => Ok(ClientMessage::UpdateAvailable {
                version: u.version,
                download_url: u.download_url,
            }),
            coordinator_msg::Payload::ApplyGradients(ag) => Ok(ClientMessage::ApplyGradients {
                task_id: Uuid::parse_str(&ag.task_id).map_err(|e| e.to_string())?,
                step: ag.step,
                aggregated: ag.data,
            }),
            coordinator_msg::Payload::StateSync(ss) => {
                let tasks = ss.assigned_tasks.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
                Ok(ClientMessage::StateSync { assigned_tasks: tasks })
            }
        }
    }
}

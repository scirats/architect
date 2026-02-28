use architect_core::payload::AITask;
use architect_core::types::NodeId;
use tracing::debug;

use crate::{NodeState, Scheduler};

/// Weights for the adaptive scoring formula.
const W_CPU: f32 = 0.3;
const W_GPU: f32 = 0.5;
const W_RAM: f32 = 0.1;
const W_TASKS: f32 = 0.1;

/// Battery penalty threshold (percentage).
const BATTERY_LOW_THRESHOLD: u8 = 30;

/// Penalty multiplier for low battery.
const BATTERY_PENALTY: f32 = 0.5;

/// Adaptive scheduler that scores nodes based on real-time metrics.
///
/// Score formula:
///   score = cpu_free * 0.3 + gpu_free * 0.5 + ram_free * 0.1 + (1/tasks) * 0.1
///   - Battery < 30% applies 0.5x penalty
pub struct AdaptiveScheduler;

impl AdaptiveScheduler {
    pub fn new() -> Self {
        Self
    }

    fn score_node(node: &NodeState) -> f32 {
        let (cpu_free, gpu_free, ram_free) = match &node.metrics {
            Some(m) => {
                let cpu = 100.0 - m.cpu_usage_pct;
                let gpu = 100.0 - m.gpu_usage_pct.unwrap_or(0.0);
                let ram = 100.0 - m.ram_usage_pct;
                (cpu, gpu, ram)
            }
            None => {
                // No metrics yet — assume partially loaded
                (50.0, 50.0, 50.0)
            }
        };

        let task_factor = if node.current_tasks == 0 {
            100.0
        } else {
            100.0 / (node.current_tasks as f32 + 1.0)
        };

        let mut score = cpu_free * W_CPU + gpu_free * W_GPU + ram_free * W_RAM + task_factor * W_TASKS;

        // Battery penalty
        if let Some(battery) = &node.capabilities.battery {
            if !battery.charging && battery.level_pct < BATTERY_LOW_THRESHOLD {
                score *= BATTERY_PENALTY;
            }
        }

        score
    }
}

impl Default for AdaptiveScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for AdaptiveScheduler {
    fn select_node(&mut self, task: &AITask, nodes: &[NodeState]) -> Option<NodeId> {
        if nodes.is_empty() {
            return None;
        }

        // Priority factor: higher priority tasks get better nodes
        let priority_factor = 1.0 + task.priority as f32 * 0.1;

        let mut best_id = nodes[0].id;
        let mut best_score = f32::NEG_INFINITY;

        for node in nodes {
            let score = Self::score_node(node) * priority_factor;
            debug!("Adaptive score for {} (prio={}): {:.2}", node.id, task.priority, score);

            if score > best_score {
                best_score = score;
                best_id = node.id;
            }
        }

        debug!("Adaptive scheduler selected node {} (score={:.2})", best_id, best_score);
        Some(best_id)
    }

    fn name(&self) -> &str {
        "adaptive"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_common::*;
    use architect_core::types::{BatteryInfo, NodeMetrics};

    #[test]
    fn test_adaptive_prefers_idle_node() {
        let mut sched = AdaptiveScheduler::new();
        let mut nodes = make_test_nodes(2);

        // Node 0: heavily loaded
        nodes[0].metrics = Some(NodeMetrics {
            cpu_usage_pct: 90.0,
            ram_usage_pct: 85.0,
            gpu_usage_pct: Some(80.0),
            network_rx_mbps: 0.0,
            network_tx_mbps: 0.0,
            uptime_secs: 1000,
            tasks_completed: 10,
            tasks_failed: 0,
        });
        nodes[0].current_tasks = 5;

        // Node 1: idle
        nodes[1].metrics = Some(NodeMetrics {
            cpu_usage_pct: 5.0,
            ram_usage_pct: 20.0,
            gpu_usage_pct: Some(0.0),
            network_rx_mbps: 0.0,
            network_tx_mbps: 0.0,
            uptime_secs: 1000,
            tasks_completed: 2,
            tasks_failed: 0,
        });
        nodes[1].current_tasks = 0;

        let task = make_test_task();
        let selected = sched.select_node(&task, &nodes).unwrap();
        assert_eq!(selected, nodes[1].id);
    }

    #[test]
    fn test_adaptive_battery_penalty() {
        let mut sched = AdaptiveScheduler::new();
        let mut nodes = make_test_nodes(2);

        // Both idle, but node 0 has low battery
        nodes[0].capabilities.battery = Some(BatteryInfo {
            level_pct: 15,
            charging: false,
        });

        let task = make_test_task();
        let selected = sched.select_node(&task, &nodes).unwrap();
        assert_eq!(selected, nodes[1].id);
    }
}

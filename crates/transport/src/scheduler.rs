use std::sync::Arc;

use tracing::debug;

use architect_core::payload::AIPayload;
use architect_core::transport::{Transport, TransportType};
use architect_core::types::NodeId;

/// Health status of a transport channel.
#[derive(Debug, Clone)]
pub struct ChannelHealth {
    pub transport_type: TransportType,
    pub bandwidth_mbps: f32,
    pub latency_ms: f32,
    pub packet_loss_pct: f32,
    pub available: bool,
}

impl ChannelHealth {
    pub fn is_degraded(&self) -> bool {
        self.packet_loss_pct > 5.0 || !self.available
    }
}

/// Smart transport scheduler that routes payloads through the best available transport.
pub struct TransportScheduler {
    transports: Vec<Arc<dyn Transport>>,
}

impl TransportScheduler {
    pub fn new() -> Self {
        Self {
            transports: Vec::new(),
        }
    }

    /// Add a transport backend.
    pub fn add_transport(&mut self, transport: Arc<dyn Transport>) {
        self.transports.push(transport);
    }

    /// Select the best transport for a given payload and target node.
    pub fn route(&self, payload: &AIPayload, _target: NodeId) -> Option<&Arc<dyn Transport>> {
        let is_large = Self::payload_is_large(payload);
        let is_control = matches!(payload, AIPayload::Control(_));

        // Filter available transports
        let mut candidates: Vec<&Arc<dyn Transport>> = self
            .transports
            .iter()
            .filter(|t| t.is_available())
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // For large payloads, filter out transports not suitable for large data
        if is_large {
            let large_candidates: Vec<_> = candidates
                .iter()
                .filter(|t| t.transport_type().suitable_for_large_data())
                .copied()
                .collect();

            if !large_candidates.is_empty() {
                candidates = large_candidates;
            }
        }

        // For control messages, prefer lowest latency
        if is_control {
            candidates.sort_by(|a, b| {
                a.latency_ms()
                    .partial_cmp(&b.latency_ms())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        } else {
            // For data, prefer highest bandwidth, then lowest latency
            candidates.sort_by(|a, b| {
                let score_a = a.bandwidth_mbps() - a.latency_ms() * 0.1;
                let score_b = b.bandwidth_mbps() - b.latency_ms() * 0.1;
                score_b
                    .partial_cmp(&score_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let selected = candidates.first().copied();
        if let Some(t) = selected {
            debug!(
                "Routed payload via {} (bw={:.0}Mbps, lat={:.1}ms)",
                t.transport_type(),
                t.bandwidth_mbps(),
                t.latency_ms()
            );
        }

        selected
    }

    /// Estimate payload size to determine transport suitability.
    fn payload_is_large(payload: &AIPayload) -> bool {
        match payload {
            AIPayload::Gradients { data, .. } => data.len() * 4 > 1024 * 1024, // > 1MB
            AIPayload::ModelWeights { data, .. } => data.len() > 1024 * 1024,
            AIPayload::DataShard { data, .. } => data.len() > 1024 * 1024,
            AIPayload::InferenceRequest { prompt, .. } => prompt.len() > 1024 * 1024,
            _ => false,
        }
    }

    /// Get all available transports.
    pub fn available_transports(&self) -> Vec<TransportType> {
        self.transports
            .iter()
            .filter(|t| t.is_available())
            .map(|t| t.transport_type())
            .collect()
    }
}

impl Default for TransportScheduler {
    fn default() -> Self {
        Self::new()
    }
}

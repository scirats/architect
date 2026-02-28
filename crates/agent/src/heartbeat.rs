use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error};

use architect_core::protocol::AgentMessage;

use crate::metrics::MetricsCollector;

pub async fn heartbeat_loop(
    tx: mpsc::Sender<AgentMessage>,
    collector: Arc<Mutex<MetricsCollector>>,
    interval_ms: u64,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
    loop {
        interval.tick().await;
        let metrics = {
            let mut c = collector.lock().unwrap();
            c.collect_metrics()
        };
        debug!("Sending heartbeat: cpu={:.1}%, ram={:.1}%", metrics.cpu_usage_pct, metrics.ram_usage_pct);
        if tx.send(AgentMessage::Heartbeat(metrics)).await.is_err() {
            error!("Heartbeat channel closed");
            break;
        }
    }
}

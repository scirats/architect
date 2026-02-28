use tokio::sync::mpsc;
use tracing::{info, warn};

use architect_bootstrap::orchestrator::{BootstrapOrchestrator, BootstrapTarget};
use architect_discovery::DiscoveredNode;

use crate::event::Event;
use crate::network_discovery::BootstrapEvent;

/// Deploy an agent to a discovered node in the background.
pub fn spawn_deploy(
    node: DiscoveredNode,
    ssh_user: Option<String>,
    ssh_pass: Option<String>,
    github_repo: String,
    cluster_token: String,
    coordinator_addr: String,
    event_tx: mpsc::UnboundedSender<Event>,
) {
    tokio::spawn(async move {
        let _ = event_tx.send(Event::Bootstrap(BootstrapEvent::Started {
            host: node.ip,
        }));

        let mut orchestrator = BootstrapOrchestrator::new();
        orchestrator.set_github_repo(github_repo);
        orchestrator.set_cluster_token(cluster_token);
        orchestrator.set_coordinator_addr(coordinator_addr);

        // Progress callback → sends events to TUI
        let host = node.ip;
        let tx = event_tx.clone();
        orchestrator.set_on_progress(Box::new(move |step: &str| {
            let _ = tx.send(Event::Bootstrap(BootstrapEvent::Progress {
                host,
                step: step.to_string(),
            }));
        }));

        let target = BootstrapTarget {
            host: node.ip,
            device_type: node.device_type,
            ssh_user,
            ssh_pass,
            winrm_credentials: None,
        };

        let result = orchestrator.bootstrap_target(&target).await;

        if result.success {
            info!("Agent seeded to {} via {:?}", node.ip, result.method);
            let _ = event_tx.send(Event::Bootstrap(BootstrapEvent::Success {
                host: node.ip,
                method: format!("{:?}", result.method),
            }));
        } else {
            let err = result.error.unwrap_or_else(|| "unknown error".into());
            warn!("Seed failed for {}: {}", node.ip, err);
            let _ = event_tx.send(Event::Bootstrap(BootstrapEvent::Failed {
                host: node.ip,
                error: err,
            }));
        }
    });
}

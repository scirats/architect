use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent};
use futures::StreamExt;
use tokio::sync::mpsc;

use architect_core::payload::TaskResult;

use crate::grpc::server::NetworkEvent;
use crate::network_discovery::{BootstrapEvent, DiscoveryEvent};

#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,
    Network(NetworkEvent),
    Discovery(DiscoveryEvent),
    Bootstrap(BootstrapEvent),
    /// Progress from a locally-executing task.
    LocalTaskProgress { task_id: uuid::Uuid, progress_pct: f32 },
    /// Result from a locally-executing task.
    LocalTaskResult(TaskResult),
    /// Task send to agent failed — revert to pending for reassignment.
    TaskSendFailed { task_id: uuid::Uuid, node_id: uuid::Uuid },
    /// Activity log entry from background tasks (build, deploy, etc.)
    Activity { message: String, level: ActivityLevel },
}

/// Re-export for convenience in spawned tasks.
pub use crate::widgets::activity_log::ActivityLevel;

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<Event>,
    _task: tokio::task::JoinHandle<()>,
    /// Sender for injecting discovery/bootstrap events from outside the event loop.
    pub inject_tx: mpsc::UnboundedSender<Event>,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64, network_rx: mpsc::Receiver<NetworkEvent>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let inject_tx = tx.clone();

        let task = tokio::spawn(Self::event_loop(tx, tick_rate_ms, network_rx));

        Self {
            rx,
            _task: task,
            inject_tx,
        }
    }

    async fn event_loop(
        tx: mpsc::UnboundedSender<Event>,
        tick_rate_ms: u64,
        mut network_rx: mpsc::Receiver<NetworkEvent>,
    ) {
        let mut reader = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(tick_rate_ms));

        loop {
            tokio::select! {
                maybe_event = reader.next() => {
                    if let Some(Ok(evt)) = maybe_event {
                        match evt {
                            CrosstermEvent::Key(key) => {
                                let _ = tx.send(Event::Key(key));
                            }
                            CrosstermEvent::Resize(w, h) => {
                                let _ = tx.send(Event::Resize(w, h));
                            }
                            _ => {}
                        }
                    }
                }
                _ = tick.tick() => {
                    let _ = tx.send(Event::Tick);
                }
                Some(net_event) = network_rx.recv() => {
                    let _ = tx.send(Event::Network(net_event));
                }
            }
        }
    }

    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

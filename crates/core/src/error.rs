use thiserror::Error;

use crate::types::NodeId;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    #[error("Connection lost to node {node_id}")]
    ConnectionLost { node_id: NodeId },

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Timeout after {0:?}")]
    Timeout(std::time::Duration),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Task error: {0}")]
    Task(String),

    #[error("Scheduler error: {0}")]
    Scheduler(String),

    #[error("Discovery error: {0}")]
    Discovery(String),

    #[error("Compute error: {0}")]
    Compute(String),

    #[error("ML error: {0}")]
    Ml(String),

    #[error("Bootstrap error: {0}")]
    Bootstrap(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

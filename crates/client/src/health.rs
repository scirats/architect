use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Snapshot of cluster health, updated by App on each tick.
#[derive(Debug, Clone, Default)]
pub struct HealthSnapshot {
    pub agents_online: usize,
    pub agents_total: usize,
    pub tasks_active: usize,
    pub tasks_pending: usize,
    pub tasks_completed_total: u64,
    pub tasks_failed_total: u64,
    pub uptime_secs: u64,
}

pub async fn start_health_server(
    addr: SocketAddr,
    snapshot: Arc<Mutex<HealthSnapshot>>,
) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Health server failed to bind to {}: {}", addr, e);
            return;
        }
    };
    info!("Health server listening on {}", addr);

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                error!("Health server accept error: {}", e);
                continue;
            }
        };

        let snap = snapshot.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = service_fn(move |req: Request<hyper::body::Incoming>| {
                let snap = snap.clone();
                async move { handle_request(req, snap) }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await
            {
                if !e.to_string().contains("connection closed") {
                    error!("Health server connection error: {}", e);
                }
            }
        });
    }
}

fn handle_request(
    req: Request<hyper::body::Incoming>,
    snapshot: Arc<Mutex<HealthSnapshot>>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let path = req.uri().path();
    let snap = snapshot.lock().unwrap().clone();

    match path {
        "/health" => {
            let body = serde_json::json!({
                "status": "ok",
                "agents_online": snap.agents_online,
                "agents_total": snap.agents_total,
                "tasks_active": snap.tasks_active,
                "tasks_pending": snap.tasks_pending,
                "uptime_secs": snap.uptime_secs,
            });
            let resp = Response::builder()
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(body.to_string())))
                .unwrap();
            Ok(resp)
        }
        "/metrics" => {
            let body = format!(
                "# HELP architect_agents_total Total number of known agents\n\
                 # TYPE architect_agents_total gauge\n\
                 architect_agents_total {}\n\
                 # HELP architect_agents_online Number of online agents\n\
                 # TYPE architect_agents_online gauge\n\
                 architect_agents_online {}\n\
                 # HELP architect_tasks_active Number of active tasks\n\
                 # TYPE architect_tasks_active gauge\n\
                 architect_tasks_active {}\n\
                 # HELP architect_tasks_pending Number of pending tasks\n\
                 # TYPE architect_tasks_pending gauge\n\
                 architect_tasks_pending {}\n\
                 # HELP architect_tasks_completed_total Total completed tasks\n\
                 # TYPE architect_tasks_completed_total counter\n\
                 architect_tasks_completed_total {}\n\
                 # HELP architect_tasks_failed_total Total failed tasks\n\
                 # TYPE architect_tasks_failed_total counter\n\
                 architect_tasks_failed_total {}\n",
                snap.agents_total,
                snap.agents_online,
                snap.tasks_active,
                snap.tasks_pending,
                snap.tasks_completed_total,
                snap.tasks_failed_total,
            );
            let resp = Response::builder()
                .header("Content-Type", "text/plain; version=0.0.4")
                .body(Full::new(Bytes::from(body)))
                .unwrap();
            Ok(resp)
        }
        path if path.starts_with("/agent/") => {
            let label = &path["/agent/".len()..];
            // Validate label against known targets
            let valid = crate::cross_compile::TARGETS.iter().any(|(_, l)| *l == label);
            if !valid {
                let resp = Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::from("Unknown target")))
                    .unwrap();
                return Ok(resp);
            }
            let binary_path = crate::cross_compile::builds_dir().join(label).join("architect-agent");
            match std::fs::read(&binary_path) {
                Ok(data) => {
                    let resp = Response::builder()
                        .header("Content-Type", "application/octet-stream")
                        .header("Content-Disposition", "attachment; filename=\"architect-agent\"")
                        .body(Full::new(Bytes::from(data)))
                        .unwrap();
                    Ok(resp)
                }
                Err(_) => {
                    let resp = Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Full::new(Bytes::from("Binary not built for this target")))
                        .unwrap();
                    Ok(resp)
                }
            }
        }
        _ => {
            let resp = Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Not Found")))
                .unwrap();
            Ok(resp)
        }
    }
}

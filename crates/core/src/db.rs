use rusqlite::{Connection, params};
use tracing::info;

use crate::config::data_dir;

/// Open (or create) the SQLite database at `~/.architect/architect.db`.
pub fn open_db() -> anyhow::Result<Connection> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let db_path = dir.join("architect.db");

    let conn = Connection::open(&db_path)?;

    // WAL mode for concurrent reads
    conn.pragma_update(None, "journal_mode", "WAL")?;

    init_tables(&conn)?;

    // File permissions: 0o600 (owner-only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&db_path, perms);
    }

    info!("SQLite database opened at {:?}", db_path);
    Ok(conn)
}

pub fn init_tables(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            task_type TEXT NOT NULL,
            payload BLOB NOT NULL,
            priority INTEGER NOT NULL DEFAULT 5,
            created_at INTEGER NOT NULL,
            timeout_ms INTEGER NOT NULL DEFAULT 300000,
            status TEXT NOT NULL DEFAULT 'Pending',
            assigned_to TEXT,
            retry_count INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3,
            progress REAL NOT NULL DEFAULT 0.0,
            error TEXT
        );

        CREATE TABLE IF NOT EXISTS metrics_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            node_id TEXT NOT NULL,
            ts INTEGER NOT NULL,
            cpu REAL NOT NULL,
            ram REAL NOT NULL,
            gpu REAL,
            tasks_completed INTEGER NOT NULL DEFAULT 0,
            tasks_failed INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_metrics_node_ts ON metrics_history(node_id, ts);

        CREATE TABLE IF NOT EXISTS identity_registry (
            node_id TEXT PRIMARY KEY,
            hostname TEXT NOT NULL,
            first_seen INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts INTEGER NOT NULL,
            actor TEXT NOT NULL,
            action TEXT NOT NULL,
            resource TEXT NOT NULL DEFAULT '',
            detail TEXT NOT NULL DEFAULT ''
        );

        CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(ts);

        CREATE TABLE IF NOT EXISTS checkpoints (
            task_id TEXT NOT NULL,
            epoch INTEGER NOT NULL,
            step INTEGER NOT NULL,
            data BLOB NOT NULL,
            ts INTEGER NOT NULL,
            PRIMARY KEY (task_id, epoch, step)
        );

        CREATE TABLE IF NOT EXISTS leader_lock (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            leader_id TEXT NOT NULL,
            acquired_at INTEGER NOT NULL,
            lease_expires INTEGER NOT NULL
        );
        ",
    )?;
    Ok(())
}

/// Insert an audit log entry.
pub fn audit(
    conn: &Connection,
    actor: &str,
    action: &str,
    resource: &str,
    detail: &str,
) -> anyhow::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    conn.execute(
        "INSERT INTO audit_log (ts, actor, action, resource, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![ts as i64, actor, action, resource, detail],
    )?;
    Ok(())
}

/// Parameters for upserting a task row.
pub struct TaskRow<'a> {
    pub id: &'a str,
    pub task_type: &'a str,
    pub payload: &'a [u8],
    pub priority: u8,
    pub created_at: u64,
    pub timeout_ms: u64,
    pub status: &'a str,
    pub assigned_to: Option<&'a str>,
    pub retry_count: u32,
    pub max_retries: u32,
    pub progress: f32,
    pub error: Option<&'a str>,
}

/// Save a task to the database (UPSERT).
pub fn upsert_task(conn: &Connection, row: &TaskRow<'_>) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO tasks (id, task_type, payload, priority, created_at, timeout_ms, status, assigned_to, retry_count, max_retries, progress, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET
           status = excluded.status,
           assigned_to = excluded.assigned_to,
           retry_count = excluded.retry_count,
           progress = excluded.progress,
           error = excluded.error",
        params![
            row.id,
            row.task_type,
            row.payload,
            row.priority as i32,
            row.created_at as i64,
            row.timeout_ms as i64,
            row.status,
            row.assigned_to,
            row.retry_count as i32,
            row.max_retries as i32,
            row.progress as f64,
            row.error,
        ],
    )?;
    Ok(())
}

/// A task row loaded from the database.
pub struct ActiveTaskRow {
    pub id: String,
    pub task_type: String,
    pub payload: Vec<u8>,
    pub priority: u8,
    pub created_at: u64,
    pub timeout_ms: u64,
    pub retry_count: u32,
    pub max_retries: u32,
}

/// Load non-terminal tasks from the database.
pub fn load_active_tasks(conn: &Connection) -> anyhow::Result<Vec<ActiveTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, task_type, payload, priority, created_at, timeout_ms, retry_count, max_retries
         FROM tasks WHERE status IN ('Pending', 'Assigned', 'Running')",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ActiveTaskRow {
            id: row.get(0)?,
            task_type: row.get(1)?,
            payload: row.get(2)?,
            priority: row.get::<_, i32>(3)? as u8,
            created_at: row.get::<_, i64>(4)? as u64,
            timeout_ms: row.get::<_, i64>(5)? as u64,
            retry_count: row.get::<_, i32>(6)? as u32,
            max_retries: row.get::<_, i32>(7)? as u32,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Delete terminal tasks older than `before_ts` (epoch seconds).
pub fn delete_old_terminal_tasks(conn: &Connection, before_ts: u64) -> anyhow::Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM tasks WHERE status IN ('Completed', 'Failed', 'Cancelled') AND created_at < ?1",
        params![before_ts as i64],
    )?;
    Ok(deleted)
}

/// Parameters for inserting a metrics snapshot.
pub struct MetricsRow<'a> {
    pub node_id: &'a str,
    pub ts: u64,
    pub cpu: f32,
    pub ram: f32,
    pub gpu: Option<f32>,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
}

/// Insert a metrics snapshot.
pub fn insert_metrics(conn: &Connection, row: &MetricsRow<'_>) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO metrics_history (node_id, ts, cpu, ram, gpu, tasks_completed, tasks_failed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.node_id,
            row.ts as i64,
            row.cpu as f64,
            row.ram as f64,
            row.gpu.map(|g| g as f64),
            row.tasks_completed as i64,
            row.tasks_failed as i64,
        ],
    )?;
    Ok(())
}

/// Prune metrics older than `before_ts`.
pub fn prune_old_metrics(conn: &Connection, before_ts: u64) -> anyhow::Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM metrics_history WHERE ts < ?1",
        params![before_ts as i64],
    )?;
    Ok(deleted)
}

/// Upsert identity registry entry.
pub fn upsert_identity(conn: &Connection, node_id: &str, hostname: &str) -> anyhow::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    conn.execute(
        "INSERT INTO identity_registry (node_id, hostname, first_seen)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(node_id) DO UPDATE SET hostname = excluded.hostname",
        params![node_id, hostname, ts as i64],
    )?;
    Ok(())
}

/// Load all known identities.
pub fn load_identities(conn: &Connection) -> anyhow::Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT node_id, hostname FROM identity_registry")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

// --- Checkpoints ---

pub struct CheckpointRow {
    pub task_id: String,
    pub epoch: u32,
    pub step: u64,
    pub data: Vec<u8>,
    pub ts: u64,
}

/// Save a training checkpoint.
pub fn save_checkpoint(
    conn: &Connection,
    task_id: &str,
    epoch: u32,
    step: u64,
    data: &[u8],
) -> anyhow::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    conn.execute(
        "INSERT OR REPLACE INTO checkpoints (task_id, epoch, step, data, ts)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, epoch as i32, step as i64, data, ts as i64],
    )?;
    Ok(())
}

/// Load the latest checkpoint for a task (highest epoch, then highest step).
pub fn load_latest_checkpoint(conn: &Connection, task_id: &str) -> anyhow::Result<Option<CheckpointRow>> {
    let mut stmt = conn.prepare(
        "SELECT task_id, epoch, step, data, ts FROM checkpoints
         WHERE task_id = ?1 ORDER BY epoch DESC, step DESC LIMIT 1",
    )?;
    let mut rows = stmt.query_map(params![task_id], |row| {
        Ok(CheckpointRow {
            task_id: row.get(0)?,
            epoch: row.get::<_, i32>(1)? as u32,
            step: row.get::<_, i64>(2)? as u64,
            data: row.get(3)?,
            ts: row.get::<_, i64>(4)? as u64,
        })
    })?;
    match rows.next() {
        Some(Ok(r)) => Ok(Some(r)),
        _ => Ok(None),
    }
}

/// Delete all checkpoints for a task.
pub fn delete_checkpoints(conn: &Connection, task_id: &str) -> anyhow::Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM checkpoints WHERE task_id = ?1",
        params![task_id],
    )?;
    Ok(deleted)
}

// --- Leader Election ---

/// Try to acquire the leader lease. Returns true if this instance became leader.
pub fn try_acquire_lease(conn: &Connection, leader_id: &str, lease_secs: u64) -> anyhow::Result<bool> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expires = now + lease_secs as i64;

    // Try insert first (no leader yet)
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO leader_lock (id, leader_id, acquired_at, lease_expires)
         VALUES (1, ?1, ?2, ?3)",
        params![leader_id, now, expires],
    )?;
    if inserted > 0 {
        return Ok(true);
    }

    // Update if lease expired or we are the current leader
    let updated = conn.execute(
        "UPDATE leader_lock SET leader_id = ?1, acquired_at = ?2, lease_expires = ?3
         WHERE id = 1 AND (leader_id = ?1 OR lease_expires < ?2)",
        params![leader_id, now, expires],
    )?;
    Ok(updated > 0)
}

/// Renew the leader lease. Returns true if renewal succeeded (we are still leader).
pub fn renew_lease(conn: &Connection, leader_id: &str, lease_secs: u64) -> anyhow::Result<bool> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expires = now + lease_secs as i64;

    let updated = conn.execute(
        "UPDATE leader_lock SET acquired_at = ?1, lease_expires = ?2
         WHERE id = 1 AND leader_id = ?3",
        params![now, expires, leader_id],
    )?;
    Ok(updated > 0)
}

/// Get the current leader ID and expiry, if any.
pub fn current_leader(conn: &Connection) -> anyhow::Result<Option<(String, u64)>> {
    let mut stmt = conn.prepare(
        "SELECT leader_id, lease_expires FROM leader_lock WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
    })?;
    match rows.next() {
        Some(Ok(r)) => Ok(Some(r)),
        _ => Ok(None),
    }
}

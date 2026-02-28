use std::path::PathBuf;
use anyhow::Result;
use tracing::{info, warn};

pub struct LeaderElection {
    db_path: PathBuf,
    my_id: String,
    is_leader: bool,
    lease_secs: u64,
}

impl LeaderElection {
    pub fn new(db_path: PathBuf, my_id: String) -> Self {
        Self {
            db_path,
            my_id,
            is_leader: false,
            lease_secs: 30,
        }
    }

    /// Attempt to acquire the leader lease. Returns true if we are now the leader.
    pub fn try_acquire(&mut self) -> Result<bool> {
        let conn = rusqlite::Connection::open(&self.db_path)?;
        architect_core::db::init_tables(&conn)?;
        let acquired = architect_core::db::try_acquire_lease(&conn, &self.my_id, self.lease_secs)?;
        self.is_leader = acquired;
        if acquired {
            info!("Leader lease acquired");
        }
        Ok(acquired)
    }

    /// Renew the leader lease. Returns false if we lost the lease.
    pub fn renew(&mut self) -> Result<bool> {
        let conn = rusqlite::Connection::open(&self.db_path)?;
        let renewed = architect_core::db::renew_lease(&conn, &self.my_id, self.lease_secs)?;
        if !renewed && self.is_leader {
            warn!("Lost leader lease");
        }
        self.is_leader = renewed;
        Ok(renewed)
    }

    pub fn is_leader(&self) -> bool {
        self.is_leader
    }
}

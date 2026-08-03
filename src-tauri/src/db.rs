mod accounts;
mod model_integrity;
mod request_logs;
mod schema;
mod settings;
mod types;
mod usage;

#[cfg(test)]
mod tests;

pub(crate) use accounts::normalize_models;
#[allow(unused_imports)]
pub(crate) use request_logs::{
    ChannelMonitorAccountSnapshot, ChannelMonitorEventSnapshot, RequestLogStart, RequestLogUsage,
};
#[allow(unused_imports)]
pub use request_logs::{RequestLog, RequestLogOverview, RequestLogPage, RequestLogQuery};
#[allow(unused_imports)]
pub use types::{Account, AccountUpdate, NewAccount, UpsertAction, UsageTotals};

use arc_swap::ArcSwap;
use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Db {
    conn: Mutex<Connection>,
    async_conn: Option<tokio_rusqlite::Connection>,
    active_accounts: ArcSwap<Vec<Account>>,
}

impl Db {
    pub fn new(path: &Path) -> SqlResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        schema::initialize(&conn)?;
        model_integrity::initialize(&conn)?;
        request_logs::initialize(&conn)?;

        // File-backed databases use a dedicated rusqlite worker. In-memory test
        // databases cannot share state across two SQLite connections, so their
        // async helpers deliberately fall back to the primary connection.
        let async_conn = if path == Path::new(":memory:") {
            None
        } else {
            let writer = Connection::open(path)?;
            writer.busy_timeout(std::time::Duration::from_secs(5))?;
            writer.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
            Some(tokio_rusqlite::Connection::from(writer))
        };

        let db = Self {
            conn: Mutex::new(conn),
            async_conn,
            active_accounts: ArcSwap::from_pointee(Vec::new()),
        };
        db.refresh_active_accounts()?;
        Ok(db)
    }

    /// Async connection handle for use in async contexts (proxy hot path).
    pub(super) fn async_conn(&self) -> Option<&tokio_rusqlite::Connection> {
        self.async_conn.as_ref()
    }

    /// Immutable routing snapshot. Reads are lock-free and never touch SQLite.
    pub fn active_accounts(&self) -> Arc<Vec<Account>> {
        self.active_accounts.load_full()
    }

    pub(super) fn refresh_active_accounts(&self) -> SqlResult<()> {
        let accounts = {
            let conn = self
                .conn
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            accounts::query_active_accounts(&conn)?
        };
        self.active_accounts.store(Arc::new(accounts));
        Ok(())
    }
}

mod accounts;
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
pub use types::{Account, NewAccount, UpsertAction, UsageTotals};

use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug)]
pub struct Db {
    conn: Mutex<Connection>,
    async_conn: tokio_rusqlite::Connection,
}

impl Db {
    pub fn new(path: &Path) -> SqlResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        schema::initialize(&conn)?;
        request_logs::initialize(&conn)?;

        // Open a second connection for async access (WAL mode allows concurrent readers/writers)
        // Use a dedicated thread to avoid blocking issues in various runtime contexts
        let path_for_async = path.to_owned();
        let async_conn = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .expect("创建 tokio runtime 失败")
                .block_on(tokio_rusqlite::Connection::open(&path_for_async))
        })
        .join()
        .expect("线程 panicked")?;
        // Set busy_timeout on the async connection
        let _ = std::thread::spawn({
            let async_conn = async_conn.clone();
            move || {
                tokio::runtime::Runtime::new()
                    .expect("创建 tokio runtime 失败")
                    .block_on(async_conn.call(|c| {
                        c.busy_timeout(std::time::Duration::from_secs(5))?;
                        Ok::<_, rusqlite::Error>(())
                    }))
            }
        })
        .join();

        Ok(Self {
            conn: Mutex::new(conn),
            async_conn,
        })
    }

    /// Async connection handle for use in async contexts (proxy hot path).
    pub fn async_conn(&self) -> &tokio_rusqlite::Connection {
        &self.async_conn
    }
}

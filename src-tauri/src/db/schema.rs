use rusqlite::{Connection, Result as SqlResult};

pub(super) fn initialize(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            api_key TEXT NOT NULL DEFAULT '',
            base_url TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
         );
         CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
         );",
    )?;

    let columns = [
        ("account_type", "TEXT NOT NULL DEFAULT 'api_key'"),
        ("access_token", "TEXT NOT NULL DEFAULT ''"),
        ("refresh_token", "TEXT NOT NULL DEFAULT ''"),
        ("id_token", "TEXT NOT NULL DEFAULT ''"),
        ("client_id", "TEXT NOT NULL DEFAULT ''"),
        ("chatgpt_account_id", "TEXT NOT NULL DEFAULT ''"),
        ("chatgpt_user_id", "TEXT NOT NULL DEFAULT ''"),
        ("email", "TEXT NOT NULL DEFAULT ''"),
        ("plan_type", "TEXT NOT NULL DEFAULT ''"),
        ("expires_at", "INTEGER"),
        ("priority", "INTEGER NOT NULL DEFAULT 1"),
        ("models", "TEXT NOT NULL DEFAULT ''"),
        ("weight", "INTEGER NOT NULL DEFAULT 1"),
        ("concurrency", "INTEGER NOT NULL DEFAULT 10"),
        ("last_error", "TEXT NOT NULL DEFAULT ''"),
        ("last_used_at", "TEXT"),
        ("request_count", "INTEGER NOT NULL DEFAULT 0"),
        ("total_tokens", "INTEGER NOT NULL DEFAULT 0"),
        ("input_tokens", "INTEGER NOT NULL DEFAULT 0"),
        ("output_tokens", "INTEGER NOT NULL DEFAULT 0"),
        ("cached_tokens", "INTEGER NOT NULL DEFAULT 0"),
        ("cache_write_tokens", "INTEGER NOT NULL DEFAULT 0"),
        ("reasoning_tokens", "INTEGER NOT NULL DEFAULT 0"),
        ("unpriced_tokens", "INTEGER NOT NULL DEFAULT 0"),
        ("total_cost", "REAL NOT NULL DEFAULT 0.0"),
        ("updated_at", "TEXT NOT NULL DEFAULT '1970-01-01 00:00:00'"),
        ("deleted_at", "TEXT"),
    ];
    for (name, definition) in columns {
        if !column_exists(conn, "accounts", name)? {
            conn.execute(
                &format!("ALTER TABLE accounts ADD COLUMN {name} {definition}"),
                [],
            )?;
        }
    }
    conn.execute_batch(
        "UPDATE accounts
            SET account_type = CASE WHEN api_key <> '' THEN 'api_key' ELSE 'oauth' END
          WHERE account_type = '';
         UPDATE accounts SET updated_at = created_at
           WHERE updated_at = '1970-01-01 00:00:00';
         UPDATE accounts SET models = '' WHERE models IS NULL;
         UPDATE accounts SET weight = 1
          WHERE weight IS NULL OR weight < 1 OR weight > 1000;
         UPDATE accounts SET concurrency = 10
          WHERE concurrency IS NULL OR concurrency < 1 OR concurrency > 1000;
         UPDATE accounts SET unpriced_tokens = total_tokens
          WHERE total_tokens > 0
            AND input_tokens = 0
            AND output_tokens = 0
            AND cached_tokens = 0
            AND cache_write_tokens = 0
            AND unpriced_tokens = 0;
         CREATE INDEX IF NOT EXISTS idx_accounts_status ON accounts(status);
         CREATE INDEX IF NOT EXISTS idx_accounts_identity
            ON accounts(chatgpt_account_id, email);",
    )
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> SqlResult<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use crate::pricing::UsageBreakdown;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub account_type: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    #[serde(skip_serializing)]
    pub access_token: String,
    #[serde(skip_serializing)]
    pub refresh_token: String,
    pub refreshable: bool,
    #[serde(skip_serializing)]
    pub id_token: String,
    #[serde(skip_serializing)]
    pub client_id: String,
    pub credential_masked: String,
    pub base_url: String,
    pub chatgpt_account_id: String,
    pub chatgpt_user_id: String,
    pub email: String,
    pub plan_type: String,
    pub expires_at: Option<i64>,
    pub priority: i64,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default = "default_weight")]
    pub weight: i64,
    pub concurrency: i64,
    pub status: String,
    pub last_error: String,
    pub last_used_at: Option<String>,
    pub request_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewAccount {
    pub name: String,
    pub account_type: String,
    pub api_key: String,
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub client_id: String,
    pub base_url: String,
    pub chatgpt_account_id: String,
    pub chatgpt_user_id: String,
    pub email: String,
    pub plan_type: String,
    pub expires_at: Option<i64>,
    pub priority: Option<i64>,
    /// `None` keeps the existing value during upsert; an empty list allows every model.
    pub models: Option<Vec<String>>,
    pub weight: Option<i64>,
    pub concurrency: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertAction {
    Created,
    Updated,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageTotals {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub unpriced_tokens: i64,
    pub total_cost: f64,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn new(path: &Path) -> SqlResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
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
            if !column_exists(&conn, "accounts", name)? {
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
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_setting(&self, key: &str) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
    }

    pub fn get_or_create_setting(&self, key: &str, default_value: &str) -> SqlResult<String> {
        let conn = self.conn.lock().unwrap();
        if let Some(value) = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?
        {
            return Ok(value);
        }
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, default_value],
        )?;
        Ok(default_value.to_string())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn list_accounts(&self) -> SqlResult<Vec<Account>> {
        self.query_accounts(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency
               FROM accounts WHERE deleted_at IS NULL
              ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END, priority, created_at DESC",
            [],
        )
    }

    pub fn get_active_accounts(&self) -> SqlResult<Vec<Account>> {
        self.query_accounts(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency
               FROM accounts WHERE status = 'active' AND deleted_at IS NULL ORDER BY priority, created_at",
            [],
        )
    }

    pub fn get_account(&self, id: &str) -> SqlResult<Option<Account>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency
               FROM accounts WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            account_from_row,
        )
        .optional()
    }

    fn query_accounts<P>(&self, sql: &str, params: P) -> SqlResult<Vec<Account>>
    where
        P: rusqlite::Params,
    {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, account_from_row)?;
        rows.collect()
    }

    pub fn upsert_account(&self, account: &NewAccount) -> SqlResult<(Account, UpsertAction)> {
        let weight = validate_weight(account.weight)?;
        let concurrency = validate_concurrency(account.concurrency)?;
        let conn = self.conn.lock().unwrap();
        let existing_id = find_existing_id(&conn, account)?;
        let models = account.models.as_deref().map(models_to_storage);
        let base_url = normalize_base_url(&account.base_url);
        let action = if let Some(id) = existing_id {
            conn.execute(
                "UPDATE accounts SET
                    name = CASE WHEN ?2 <> '' THEN ?2 ELSE name END,
                    account_type = ?3,
                    api_key = CASE WHEN ?4 <> '' THEN ?4 ELSE api_key END,
                    access_token = CASE WHEN ?5 <> '' THEN ?5 ELSE access_token END,
                    refresh_token = CASE WHEN ?6 <> '' THEN ?6 ELSE refresh_token END,
                    id_token = CASE WHEN ?7 <> '' THEN ?7 ELSE id_token END,
                    client_id = CASE WHEN ?8 <> '' THEN ?8 ELSE client_id END,
                    base_url = CASE WHEN ?9 <> '' THEN ?9 ELSE base_url END,
                    chatgpt_account_id = CASE WHEN ?10 <> '' THEN ?10 ELSE chatgpt_account_id END,
                    chatgpt_user_id = CASE WHEN ?11 <> '' THEN ?11 ELSE chatgpt_user_id END,
                    email = CASE WHEN ?12 <> '' THEN ?12 ELSE email END,
                    plan_type = CASE WHEN ?13 <> '' THEN ?13 ELSE plan_type END,
                    expires_at = COALESCE(?14, expires_at),
                    priority = COALESCE(?15, priority),
                    models = COALESCE(?16, models),
                    weight = COALESCE(?17, weight),
                    concurrency = COALESCE(?18, concurrency),
                    status = 'active', last_error = '', updated_at = datetime('now', 'localtime')
                  WHERE id = ?1",
                rusqlite::params![
                    id,
                    account.name,
                    account.account_type,
                    account.api_key,
                    account.access_token,
                    account.refresh_token,
                    account.id_token,
                    account.client_id,
                    base_url,
                    account.chatgpt_account_id,
                    account.chatgpt_user_id,
                    account.email,
                    account.plan_type,
                    account.expires_at,
                    account.priority,
                    models,
                    weight,
                    concurrency,
                ],
            )?;
            UpsertAction::Updated
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            let name = if account.name.trim().is_empty() {
                default_account_name(account)
            } else {
                account.name.trim().to_string()
            };
            conn.execute(
                "INSERT INTO accounts (
                    id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, concurrency, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                            datetime('now', 'localtime'))",
                rusqlite::params![
                    id,
                    name,
                    account.account_type,
                    account.api_key,
                    account.access_token,
                    account.refresh_token,
                    account.id_token,
                    account.client_id,
                    base_url,
                    account.chatgpt_account_id,
                    account.chatgpt_user_id,
                    account.email,
                    account.plan_type,
                    account.expires_at,
                    account.priority.unwrap_or(1),
                    models.unwrap_or_default(),
                    weight.unwrap_or(1),
                    concurrency.unwrap_or(10),
                ],
            )?;
            UpsertAction::Created
        };

        let id = find_existing_id(&conn, account)?.expect("account must exist after upsert");
        let saved = conn.query_row(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency
               FROM accounts WHERE id = ?1",
            [id],
            account_from_row,
        )?;
        Ok((saved, action))
    }

    pub fn update_oauth_tokens(&self, id: &str, account: &NewAccount) -> SqlResult<Account> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE accounts SET
                access_token = ?2,
                refresh_token = CASE WHEN ?3 <> '' THEN ?3 ELSE refresh_token END,
                id_token = CASE WHEN ?4 <> '' THEN ?4 ELSE id_token END,
                client_id = CASE WHEN ?5 <> '' THEN ?5 ELSE client_id END,
                chatgpt_account_id = CASE WHEN ?6 <> '' THEN ?6 ELSE chatgpt_account_id END,
                chatgpt_user_id = CASE WHEN ?7 <> '' THEN ?7 ELSE chatgpt_user_id END,
                email = CASE WHEN ?8 <> '' THEN ?8 ELSE email END,
                plan_type = CASE WHEN ?9 <> '' THEN ?9 ELSE plan_type END,
                expires_at = ?10, status = 'active', last_error = '',
                updated_at = datetime('now', 'localtime')
              WHERE id = ?1",
            rusqlite::params![
                id,
                account.access_token,
                account.refresh_token,
                account.id_token,
                account.client_id,
                account.chatgpt_account_id,
                account.chatgpt_user_id,
                account.email,
                account.plan_type,
                account.expires_at,
            ],
        )?;
        drop(conn);
        self.get_account(id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn delete_account(&self, id: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET deleted_at = datetime('now', 'localtime'), status = 'disabled', updated_at = datetime('now', 'localtime') WHERE id = ?1 AND deleted_at IS NULL",
            [id],
        )? > 0)
    }

    pub fn list_trashed_accounts(&self) -> SqlResult<Vec<Account>> {
        self.query_accounts(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency
               FROM accounts WHERE deleted_at IS NOT NULL
              ORDER BY deleted_at DESC",
            [],
        )
    }

    pub fn restore_account(&self, id: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET deleted_at = NULL, status = 'active', last_error = '', updated_at = datetime('now', 'localtime') WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id],
        )? > 0)
    }

    pub fn purge_account(&self, id: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM accounts WHERE id = ?1 AND deleted_at IS NOT NULL", [id])? > 0)
    }

    pub fn purge_all_trashed(&self) -> SqlResult<u64> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM accounts WHERE deleted_at IS NOT NULL", [])
            .map(|n| n as u64)
    }

    pub fn set_status(&self, id: &str, status: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET status = ?1, updated_at = datetime('now', 'localtime') WHERE id = ?2",
            rusqlite::params![status, id],
        )? > 0)
    }

    pub fn set_priority(&self, id: &str, priority: i64) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET priority = ?1, updated_at = datetime('now', 'localtime') WHERE id = ?2",
            rusqlite::params![priority, id],
        )? > 0)
    }

    pub fn set_concurrency(&self, id: &str, concurrency: i64) -> SqlResult<bool> {
        validate_concurrency(Some(concurrency))?;
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET concurrency = ?1, updated_at = datetime('now', 'localtime') WHERE id = ?2",
            rusqlite::params![concurrency, id],
        )? > 0)
    }

    pub fn mark_used(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE accounts SET request_count = request_count + 1,
                    last_used_at = datetime('now', 'localtime'), last_error = '' WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn record_usage(
        &self,
        id: &str,
        usage: &UsageBreakdown,
        cost: f64,
        unpriced_tokens: i64,
    ) -> SqlResult<()> {
        let total_tokens = usage.total_tokens.max(0);
        let input_tokens = usage.input_tokens.max(0);
        let output_tokens = usage.output_tokens.max(0);
        let cached_tokens = usage.cached_tokens.max(0).min(input_tokens);
        let cache_write_tokens = usage
            .cache_write_tokens
            .max(0)
            .min(input_tokens.saturating_sub(cached_tokens));
        let reasoning_tokens = usage.reasoning_tokens.max(0).min(output_tokens);
        let unpriced_tokens = unpriced_tokens.max(0).min(total_tokens);
        let cost = if cost.is_finite() && cost > 0.0 {
            cost
        } else {
            0.0
        };
        if total_tokens <= 0 && cost <= 0.0 {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE accounts SET
                total_tokens = total_tokens + ?2,
                input_tokens = input_tokens + ?3,
                output_tokens = output_tokens + ?4,
                cached_tokens = cached_tokens + ?5,
                cache_write_tokens = cache_write_tokens + ?6,
                reasoning_tokens = reasoning_tokens + ?7,
                unpriced_tokens = unpriced_tokens + ?8,
                total_cost = total_cost + ?9
              WHERE id = ?1",
            rusqlite::params![
                id,
                total_tokens,
                input_tokens,
                output_tokens,
                cached_tokens,
                cache_write_tokens,
                reasoning_tokens,
                unpriced_tokens,
                cost,
            ],
        )?;
        Ok(())
    }

    pub fn set_error(&self, id: &str, error: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE accounts SET last_error = ?2, updated_at = datetime('now', 'localtime') WHERE id = ?1",
            rusqlite::params![id, truncate(error, 500)],
        )?;
        Ok(())
    }

    pub fn reset_request_counts(&self) -> SqlResult<u64> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "UPDATE accounts SET
                request_count = 0,
                total_tokens = 0,
                input_tokens = 0,
                output_tokens = 0,
                cached_tokens = 0,
                cache_write_tokens = 0,
                reasoning_tokens = 0,
                unpriced_tokens = 0,
                total_cost = 0.0,
                last_used_at = NULL
              WHERE request_count > 0
                 OR total_tokens > 0
                 OR input_tokens > 0
                 OR output_tokens > 0
                 OR cached_tokens > 0
                 OR cache_write_tokens > 0
                 OR reasoning_tokens > 0
                 OR unpriced_tokens > 0
                 OR total_cost > 0",
            [],
        )?;
        Ok(affected as u64)
    }

    pub fn total_request_count(&self) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(SUM(request_count), 0) FROM accounts",
            [],
            |row| row.get(0),
        )
    }

    pub fn total_tokens(&self) -> SqlResult<i64> {
        Ok(self.usage_totals()?.total_tokens)
    }

    pub fn total_cost(&self) -> SqlResult<f64> {
        Ok(self.usage_totals()?.total_cost)
    }

    pub fn usage_totals(&self) -> SqlResult<UsageTotals> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cached_tokens), 0),
                COALESCE(SUM(cache_write_tokens), 0),
                COALESCE(SUM(reasoning_tokens), 0),
                COALESCE(SUM(unpriced_tokens), 0),
                COALESCE(SUM(total_cost), 0.0)
               FROM accounts
              WHERE deleted_at IS NULL",
            [],
            |row| {
                Ok(UsageTotals {
                    total_tokens: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cached_tokens: row.get(3)?,
                    cache_write_tokens: row.get(4)?,
                    reasoning_tokens: row.get(5)?,
                    unpriced_tokens: row.get(6)?,
                    total_cost: row.get(7)?,
                })
            },
        )
    }

    pub fn export_data(&self) -> SqlResult<Value> {
        let accounts = self.list_accounts()?;
        let data = accounts
            .into_iter()
            .map(|account| {
                let credentials = if account.account_type == "oauth" {
                    json!({
                        "access_token": account.access_token,
                        "refresh_token": account.refresh_token,
                        "id_token": account.id_token,
                        "client_id": account.client_id,
                        "chatgpt_account_id": account.chatgpt_account_id,
                        "chatgpt_user_id": account.chatgpt_user_id,
                        "email": account.email,
                        "plan_type": account.plan_type,
                        "expires_at": account.expires_at,
                    })
                } else {
                    json!({
                        "api_key": account.api_key,
                        "base_url": account.base_url,
                    })
                };
                json!({
                    "name": account.name,
                    "platform": "openai",
                    "type": if account.account_type == "api_key" { "apikey" } else { "oauth" },
                    "priority": account.priority,
                    "models": account.models,
                    "weight": account.weight,
                    "concurrency": account.concurrency,
                    "credentials": credentials,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "type": "sub2api-data",
            "version": 1,
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "proxies": [],
            "accounts": data,
        }))
    }
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

fn account_from_row(row: &rusqlite::Row<'_>) -> SqlResult<Account> {
    let api_key: String = row.get(3)?;
    let access_token: String = row.get(4)?;
    let account_type: String = row.get(2)?;
    let models = row.get::<_, Option<String>>(15)?.unwrap_or_default();
    let weight = row
        .get::<_, Option<i64>>(16)?
        .filter(|weight| (1..=1000).contains(weight))
        .unwrap_or_else(default_weight);
    let raw = if account_type == "oauth" {
        &access_token
    } else {
        &api_key
    };
    let credential_masked = mask_secret(raw);
    Ok(Account {
        id: row.get(0)?,
        name: row.get(1)?,
        account_type,
        api_key,
        access_token,
        refresh_token: row.get(5)?,
        refreshable: !row.get::<_, String>(5)?.is_empty(),
        id_token: row.get(6)?,
        client_id: row.get(7)?,
        credential_masked,
        base_url: row.get(8)?,
        chatgpt_account_id: row.get(9)?,
        chatgpt_user_id: row.get(10)?,
        email: row.get(11)?,
        plan_type: row.get(12)?,
        expires_at: row.get(13)?,
        priority: row.get(14)?,
        models: models_from_storage(&models),
        weight,
        concurrency: row
            .get::<_, Option<i64>>(23)?
            .filter(|concurrency| (1..=1000).contains(concurrency))
            .unwrap_or(10),
        status: row.get(17)?,
        last_error: row.get(18)?,
        last_used_at: row.get(19)?,
        request_count: row.get(20)?,
        created_at: row.get(21)?,
        updated_at: row.get(22)?,
    })
}

pub(crate) fn normalize_models<I, S>(models: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    models
        .into_iter()
        .filter_map(|model| {
            let model = model.as_ref().trim();
            if model.is_empty() || !seen.insert(model.to_string()) {
                None
            } else {
                Some(model.to_string())
            }
        })
        .collect()
}

fn models_to_storage(models: &[String]) -> String {
    let models = normalize_models(models);
    if models.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&models).expect("serializing strings cannot fail")
    }
}

fn models_from_storage(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    if let Ok(models) = serde_json::from_str::<Vec<String>>(value) {
        return normalize_models(models);
    }
    normalize_models(value.split(|character| character == ',' || character == '\n'))
}

const fn default_weight() -> i64 {
    1
}

fn validate_weight(weight: Option<i64>) -> SqlResult<Option<i64>> {
    if weight.is_some_and(|weight| !(1..=1000).contains(&weight)) {
        return Err(rusqlite::Error::InvalidParameterName(
            "weight must be between 1 and 1000".to_string(),
        ));
    }
    Ok(weight)
}

fn validate_concurrency(concurrency: Option<i64>) -> SqlResult<Option<i64>> {
    if concurrency.is_some_and(|concurrency| !(1..=1000).contains(&concurrency)) {
        return Err(rusqlite::Error::InvalidParameterName(
            "concurrency must be between 1 and 1000".to_string(),
        ));
    }
    Ok(concurrency)
}

fn normalize_base_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn find_existing_id(conn: &Connection, account: &NewAccount) -> SqlResult<Option<String>> {
    if !account.refresh_token.is_empty() {
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM accounts WHERE refresh_token = ?1 LIMIT 1",
                [&account.refresh_token],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(Some(id));
        }
    }
    if !account.chatgpt_user_id.is_empty() && !account.chatgpt_account_id.is_empty() {
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM accounts
                  WHERE chatgpt_user_id = ?1 AND chatgpt_account_id = ?2
                  LIMIT 1",
                rusqlite::params![account.chatgpt_user_id, account.chatgpt_account_id],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(Some(id));
        }
    } else if !account.chatgpt_user_id.is_empty() {
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM accounts WHERE chatgpt_user_id = ?1 LIMIT 1",
                [&account.chatgpt_user_id],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(Some(id));
        }
    } else if !account.chatgpt_account_id.is_empty() {
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM accounts WHERE chatgpt_account_id = ?1 LIMIT 1",
                [&account.chatgpt_account_id],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(Some(id));
        }
    }
    if !account.api_key.is_empty() {
        let base_url = normalize_base_url(&account.base_url);
        return conn
            .query_row(
                "SELECT id FROM accounts
                  WHERE api_key = ?1 AND RTRIM(TRIM(base_url), '/') = ?2
                  LIMIT 1",
                rusqlite::params![account.api_key, base_url],
                |row| row.get(0),
            )
            .optional();
    }
    if !account.email.is_empty() {
        let existing = if account.chatgpt_account_id.is_empty() {
            conn.query_row(
                "SELECT id FROM accounts WHERE email = ?1 AND account_type = ?2 LIMIT 1",
                rusqlite::params![account.email, account.account_type],
                |row| row.get(0),
            )
            .optional()?
        } else {
            conn.query_row(
                "SELECT id FROM accounts
                  WHERE email = ?1 AND chatgpt_account_id = ?2 AND account_type = ?3
                  LIMIT 1",
                rusqlite::params![
                    account.email,
                    account.chatgpt_account_id,
                    account.account_type
                ],
                |row| row.get(0),
            )
            .optional()?
        };
        if existing.is_some() {
            return Ok(existing);
        }
    }
    if !account.access_token.is_empty() {
        return conn
            .query_row(
                "SELECT id FROM accounts WHERE access_token = ?1 LIMIT 1",
                [&account.access_token],
                |row| row.get(0),
            )
            .optional();
    }
    Ok(None)
}

fn default_account_name(account: &NewAccount) -> String {
    if !account.email.is_empty() {
        return account.email.clone();
    }
    if account.account_type == "oauth" {
        "OpenAI OAuth".to_string()
    } else {
        "OpenAI 中转站".to_string()
    }
}

fn mask_secret(secret: &str) -> String {
    let chars = secret.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return "-".to_string();
    }
    if chars.len() <= 10 {
        return "****".to_string();
    }
    format!(
        "{}****{}",
        chars[..5].iter().collect::<String>(),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_updates_account_priority() {
        let db = Db::new(Path::new(":memory:")).unwrap();
        let (account, action) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "sk-test-priority".to_string(),
                priority: Some(7),
                ..NewAccount::default()
            })
            .unwrap();
        assert_eq!(action, UpsertAction::Created);
        assert_eq!(account.priority, 7);

        assert!(db.set_priority(&account.id, 2).unwrap());
        assert_eq!(db.get_account(&account.id).unwrap().unwrap().priority, 2);
    }

    #[test]
    fn stores_exports_and_preserves_routing_configuration() {
        let db = Db::new(Path::new(":memory:")).unwrap();
        let (account, action) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "sk-test-routing".to_string(),
                models: Some(vec![
                    " gpt-5 ".to_string(),
                    "gpt-5".to_string(),
                    "gpt-5-mini".to_string(),
                ]),
                weight: Some(4),
                concurrency: Some(12),
                ..NewAccount::default()
            })
            .unwrap();
        assert_eq!(action, UpsertAction::Created);
        assert_eq!(account.models, ["gpt-5", "gpt-5-mini"]);
        assert_eq!(account.weight, 4);
        assert_eq!(account.concurrency, 12);

        let (preserved, action) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "sk-test-routing".to_string(),
                ..NewAccount::default()
            })
            .unwrap();
        assert_eq!(action, UpsertAction::Updated);
        assert_eq!(preserved.models, account.models);
        assert_eq!(preserved.weight, 4);
        assert_eq!(preserved.concurrency, 12);

        let (updated, _) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "sk-test-routing".to_string(),
                models: Some(Vec::new()),
                weight: Some(2),
                ..NewAccount::default()
            })
            .unwrap();
        assert!(updated.models.is_empty());
        assert_eq!(updated.weight, 2);

        let exported = db.export_data().unwrap();
        assert_eq!(exported.pointer("/accounts/0/models"), Some(&json!([])));
        assert_eq!(exported.pointer("/accounts/0/weight"), Some(&json!(2)));
        assert_eq!(
            exported.pointer("/accounts/0/concurrency"),
            Some(&json!(12))
        );
        assert_eq!(exported.pointer("/accounts/0/type"), Some(&json!("apikey")));
        assert_eq!(
            exported.pointer("/accounts/0/credentials/api_key"),
            Some(&json!("sk-test-routing"))
        );
    }

    #[test]
    fn api_key_identity_includes_base_url_and_weight_is_bounded() {
        let db = Db::new(Path::new(":memory:")).unwrap();
        let (first, _) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "shared-key".to_string(),
                base_url: "https://relay-a.example/v1/".to_string(),
                weight: Some(1),
                ..NewAccount::default()
            })
            .unwrap();
        let (second, action) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "shared-key".to_string(),
                base_url: "https://relay-b.example/v1".to_string(),
                weight: Some(1000),
                ..NewAccount::default()
            })
            .unwrap();
        assert_eq!(action, UpsertAction::Created);
        assert_ne!(first.id, second.id);

        let (same_first, action) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "shared-key".to_string(),
                base_url: "https://relay-a.example/v1".to_string(),
                ..NewAccount::default()
            })
            .unwrap();
        assert_eq!(action, UpsertAction::Updated);
        assert_eq!(same_first.id, first.id);

        let error = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "invalid-weight".to_string(),
                weight: Some(1001),
                ..NewAccount::default()
            })
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("weight must be between 1 and 1000"));
    }

    #[test]
    fn oauth_identity_does_not_collapse_users_in_the_same_chatgpt_account() {
        let db = Db::new(Path::new(":memory:")).unwrap();
        let shared_account_id = "shared-workspace";
        let (first, _) = db
            .upsert_account(&NewAccount {
                account_type: "oauth".to_string(),
                refresh_token: "refresh-first".to_string(),
                chatgpt_account_id: shared_account_id.to_string(),
                chatgpt_user_id: "user-first".to_string(),
                email: "first@example.com".to_string(),
                ..NewAccount::default()
            })
            .unwrap();
        let (second, action) = db
            .upsert_account(&NewAccount {
                account_type: "oauth".to_string(),
                refresh_token: "refresh-second".to_string(),
                chatgpt_account_id: shared_account_id.to_string(),
                chatgpt_user_id: "user-second".to_string(),
                email: "second@example.com".to_string(),
                ..NewAccount::default()
            })
            .unwrap();

        assert_eq!(action, UpsertAction::Created);
        assert_ne!(first.id, second.id);
        assert_eq!(db.list_accounts().unwrap().len(), 2);

        let (same_first, action) = db
            .upsert_account(&NewAccount {
                name: "first updated".to_string(),
                account_type: "oauth".to_string(),
                refresh_token: "rotated-refresh-first".to_string(),
                chatgpt_account_id: shared_account_id.to_string(),
                chatgpt_user_id: "user-first".to_string(),
                email: "first@example.com".to_string(),
                ..NewAccount::default()
            })
            .unwrap();
        assert_eq!(action, UpsertAction::Updated);
        assert_eq!(same_first.id, first.id);
        assert_eq!(db.list_accounts().unwrap().len(), 2);
    }

    #[test]
    fn migrates_legacy_account_table_with_routing_defaults() {
        let path = std::env::temp_dir().join(format!(
            "sub2api-db-migration-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE accounts (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL DEFAULT '',
                    api_key TEXT NOT NULL DEFAULT '',
                    base_url TEXT NOT NULL DEFAULT '',
                    status TEXT NOT NULL DEFAULT 'active',
                    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
                 );
                 INSERT INTO accounts (id, name, api_key)
                 VALUES ('legacy', 'Legacy API Key', 'sk-legacy');",
            )
            .unwrap();
        }

        let db = Db::new(&path).unwrap();
        let account = db.get_account("legacy").unwrap().unwrap();
        assert!(account.models.is_empty());
        assert_eq!(account.weight, 1);
        assert_eq!(account.api_key, "sk-legacy");
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}

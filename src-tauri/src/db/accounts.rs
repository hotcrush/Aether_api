use super::types::{default_weight, Account, AccountUpdate, NewAccount, UpsertAction};
use super::Db;
use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Standalone query used by `Db::refresh_active_accounts` to populate the ArcSwap cache.
pub(super) fn query_active_accounts(conn: &Connection) -> SqlResult<Vec<Account>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                expires_at, priority, models, weight, status, last_error, last_used_at,
                request_count, created_at, updated_at, concurrency, rate_multiplier,
                auto_sync_rate_multiplier
           FROM accounts WHERE status = 'active' AND deleted_at IS NULL ORDER BY priority, created_at",
    )?;
    let rows = stmt.query_map([], account_from_row)?;
    rows.collect()
}

impl Db {
    pub fn list_accounts(&self) -> SqlResult<Vec<Account>> {
        self.query_accounts(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency, rate_multiplier,
                    auto_sync_rate_multiplier
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
                    request_count, created_at, updated_at, concurrency, rate_multiplier,
                    auto_sync_rate_multiplier
               FROM accounts WHERE status = 'active' AND deleted_at IS NULL ORDER BY priority, created_at",
            [],
        )
    }

    /// Async version for proxy hot path.
    pub async fn get_active_accounts_async(&self) -> SqlResult<Vec<Account>> {
        let Some(async_conn) = self.async_conn() else {
            return self.get_active_accounts();
        };
        async_conn
            .call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                            client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                            expires_at, priority, models, weight, status, last_error, last_used_at,
                            request_count, created_at, updated_at, concurrency, rate_multiplier,
                            auto_sync_rate_multiplier
                       FROM accounts WHERE status = 'active' AND deleted_at IS NULL ORDER BY priority, created_at",
                )?;
                let rows = stmt.query_map([], account_from_row)?;
                rows.collect()
            })
            .await
            .map_err(|e| match e {
                tokio_rusqlite::Error::Error(e) => e,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })
    }

    pub fn get_account(&self, id: &str) -> SqlResult<Option<Account>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency, rate_multiplier,
                    auto_sync_rate_multiplier
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
        let rate_multiplier = validate_rate_multiplier(account.rate_multiplier)?;
        let auto_sync_rate_multiplier = account.auto_sync_rate_multiplier;
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
                    rate_multiplier = COALESCE(?19, rate_multiplier),
                    auto_sync_rate_multiplier = COALESCE(?20, auto_sync_rate_multiplier),
                    status = 'active', deleted_at = NULL, last_error = '',
                    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
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
                    rate_multiplier,
                    auto_sync_rate_multiplier,
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
                    expires_at, priority, models, weight, concurrency, rate_multiplier,
                    auto_sync_rate_multiplier, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                            strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
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
                    rate_multiplier.unwrap_or(1.0),
                    auto_sync_rate_multiplier.unwrap_or(false),
                ],
            )?;
            UpsertAction::Created
        };

        let id = find_existing_id(&conn, account)?.expect("account must exist after upsert");
        let saved = conn.query_row(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency, rate_multiplier,
                    auto_sync_rate_multiplier
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
                updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
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
            "UPDATE accounts SET deleted_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), status = 'disabled', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1 AND deleted_at IS NULL",
            [id],
        )? > 0)
    }

    pub fn list_trashed_accounts(&self) -> SqlResult<Vec<Account>> {
        self.query_accounts(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency, rate_multiplier,
                    auto_sync_rate_multiplier
               FROM accounts WHERE deleted_at IS NOT NULL
              ORDER BY deleted_at DESC",
            [],
        )
    }

    pub fn restore_account(&self, id: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET deleted_at = NULL, status = 'active', last_error = '', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id],
        )? > 0)
    }

    pub fn purge_account(&self, id: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "DELETE FROM accounts WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id],
        )? > 0)
    }

    pub fn purge_all_trashed(&self) -> SqlResult<u64> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM accounts WHERE deleted_at IS NOT NULL", [])
            .map(|count| count as u64)
    }

    pub fn set_status(&self, id: &str, status: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET status = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?2",
            rusqlite::params![status, id],
        )? > 0)
    }

    pub fn update_account(&self, id: &str, update: &AccountUpdate) -> SqlResult<Option<Account>> {
        validate_weight(Some(update.weight))?;
        validate_concurrency(Some(update.concurrency))?;
        validate_rate_multiplier(Some(update.rate_multiplier))?;
        let conn = self.conn.lock().unwrap();
        let current = conn
            .query_row(
                "SELECT account_type, api_key, base_url, models, rate_multiplier,
                        auto_sync_rate_multiplier
                   FROM accounts WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, f64>(4)?,
                        row.get::<_, bool>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            account_type,
            current_key,
            current_base_url,
            current_models,
            current_multiplier,
            auto_sync,
        )) = current
        else {
            return Ok(None);
        };

        let relay = account_type == "api_key";
        let api_key = if relay {
            update
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&current_key)
                .to_string()
        } else {
            current_key
        };
        let base_url = if relay {
            normalize_base_url(&update.base_url)
        } else {
            current_base_url
        };
        let models = if relay {
            models_to_storage(&update.models)
        } else {
            current_models
        };
        if relay {
            let duplicate = conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM accounts
                     WHERE id <> ?1 AND deleted_at IS NULL AND account_type = 'api_key'
                       AND api_key = ?2 AND RTRIM(TRIM(base_url), '/') = ?3
                 )",
                rusqlite::params![id, api_key, base_url],
                |row| row.get::<_, bool>(0),
            )?;
            if duplicate {
                return Err(rusqlite::Error::InvalidParameterName(
                    "相同 API Key 和 Base URL 的中转站已存在".to_string(),
                ));
            }
        }
        let rate_multiplier = if auto_sync {
            current_multiplier
        } else {
            update.rate_multiplier
        };
        conn.execute(
            "UPDATE accounts SET
                name = ?2, api_key = ?3, base_url = ?4, models = ?5,
                priority = ?6, weight = ?7, concurrency = ?8, rate_multiplier = ?9,
                last_error = '', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
              WHERE id = ?1 AND deleted_at IS NULL",
            rusqlite::params![
                id,
                update.name.trim(),
                api_key,
                base_url,
                models,
                update.priority,
                update.weight,
                update.concurrency,
                rate_multiplier,
            ],
        )?;
        drop(conn);
        self.get_account(id)
    }

    pub fn set_priority(&self, id: &str, priority: i64) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET priority = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?2",
            rusqlite::params![priority, id],
        )? > 0)
    }

    pub fn set_concurrency(&self, id: &str, concurrency: i64) -> SqlResult<bool> {
        validate_concurrency(Some(concurrency))?;
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET concurrency = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?2",
            rusqlite::params![concurrency, id],
        )? > 0)
    }

    pub fn set_rate_multiplier(&self, id: &str, multiplier: f64) -> SqlResult<bool> {
        validate_rate_multiplier(Some(multiplier))?;
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET rate_multiplier = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
              WHERE id = ?2 AND deleted_at IS NULL AND auto_sync_rate_multiplier = 0",
            rusqlite::params![multiplier, id],
        )? > 0)
    }

    pub fn set_auto_sync_rate_multiplier(&self, id: &str, enabled: bool) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET auto_sync_rate_multiplier = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
              WHERE id = ?2 AND deleted_at IS NULL AND account_type = 'api_key' AND base_url <> ''",
            rusqlite::params![enabled, id],
        )? > 0)
    }

    /// Stores an upstream-probed multiplier only while the managed sync flag remains enabled.
    pub fn set_rate_multiplier_from_sync(&self, id: &str, multiplier: f64) -> SqlResult<bool> {
        validate_rate_multiplier(Some(multiplier))?;
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE accounts SET rate_multiplier = ?1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
              WHERE id = ?2 AND deleted_at IS NULL AND auto_sync_rate_multiplier = 1",
            rusqlite::params![multiplier, id],
        )? > 0)
    }

    pub fn list_rate_sync_accounts(&self) -> SqlResult<Vec<Account>> {
        self.query_accounts(
            "SELECT id, name, account_type, api_key, access_token, refresh_token, id_token,
                    client_id, base_url, chatgpt_account_id, chatgpt_user_id, email, plan_type,
                    expires_at, priority, models, weight, status, last_error, last_used_at,
                    request_count, created_at, updated_at, concurrency, rate_multiplier,
                    auto_sync_rate_multiplier
               FROM accounts
              WHERE status = 'active' AND deleted_at IS NULL
                AND account_type = 'api_key' AND api_key <> '' AND base_url <> ''
                AND auto_sync_rate_multiplier = 1
              ORDER BY priority, created_at",
            [],
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
                    "rate_multiplier": account.rate_multiplier,
                    "extra": if account.auto_sync_rate_multiplier {
                        json!({"upstream_billing_rate_sync_enabled": true})
                    } else {
                        json!({})
                    },
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
        rate_multiplier: row
            .get::<_, Option<f64>>(24)?
            .filter(|multiplier| multiplier.is_finite() && (0.0..=100.0).contains(multiplier))
            .unwrap_or(1.0),
        auto_sync_rate_multiplier: row.get::<_, Option<bool>>(25)?.unwrap_or(false),
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

fn validate_rate_multiplier(multiplier: Option<f64>) -> SqlResult<Option<f64>> {
    if multiplier
        .is_some_and(|multiplier| !multiplier.is_finite() || !(0.0..=100.0).contains(&multiplier))
    {
        return Err(rusqlite::Error::InvalidParameterName(
            "rate_multiplier must be between 0 and 100".to_string(),
        ));
    }
    Ok(multiplier)
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

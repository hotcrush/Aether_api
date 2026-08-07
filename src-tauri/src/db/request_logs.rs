use super::Db;
use crate::pricing::UsageBreakdown;
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, Result as SqlResult,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const REQUEST_LOG_RETENTION_DAYS: i64 = 30;
const REQUEST_LOG_MAX_ROWS: i64 = 20_000;
const REQUEST_LOG_DEFAULT_LIMIT: i64 = 100;
const REQUEST_LOG_MAX_LIMIT: i64 = 500;
const SLOW_TTFB_MS: i64 = 6_000;
const SLOW_DURATION_MS: i64 = 20_000;
const REDACTED_CREDENTIAL_MESSAGE: &str = "上游错误包含敏感凭据，内容已脱敏";
const REDACTED_TOKEN_MESSAGE: &str = "上游错误包含认证令牌，内容已脱敏";

#[derive(Debug, Clone, Serialize)]
pub struct RequestLog {
    pub id: i64,
    pub request_id: String,
    pub attempt_index: i64,
    pub account_id: Option<String>,
    pub account_name: String,
    pub account_type: String,
    pub source: String,
    pub method: String,
    pub path: String,
    pub endpoint_family: String,
    pub model: String,
    pub status: String,
    pub http_status: Option<i64>,
    pub streaming: bool,
    pub ttfb_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub unpriced_tokens: i64,
    pub estimated_cost: f64,
    pub message: String,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RequestLogQuery {
    pub status: Option<String>,
    pub account_id: Option<String>,
    pub source: Option<String>,
    pub search: Option<String>,
    pub before_id: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestLogPage {
    pub items: Vec<RequestLog>,
    pub has_more: bool,
    pub next_before_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestLogOverview {
    pub total_requests: i64,
    pub total_attempts: i64,
    pub success_attempts: i64,
    pub error_attempts: i64,
    pub retry_attempts: i64,
    pub pending_attempts: i64,
    pub average_ttfb_ms: Option<f64>,
    pub average_duration_ms: Option<f64>,
    pub total_tokens: i64,
    pub estimated_cost: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestLogStart {
    pub request_id: String,
    pub attempt_index: i64,
    pub account_id: Option<String>,
    pub account_name: String,
    pub account_type: String,
    pub source: String,
    pub method: String,
    pub path: String,
    pub endpoint_family: String,
    pub model: String,
    pub streaming: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RequestLogUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub unpriced_tokens: i64,
    pub estimated_cost: f64,
}

impl RequestLogUsage {
    pub(crate) fn from_breakdown(
        usage: &UsageBreakdown,
        estimated_cost: f64,
        unpriced_tokens: i64,
    ) -> Self {
        let usage = usage.clone().normalize();
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_tokens: usage.cached_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            total_tokens: usage.total_tokens,
            unpriced_tokens: unpriced_tokens.max(0).min(usage.total_tokens),
            estimated_cost: finite_cost(estimated_cost),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ChannelMonitorEventSnapshot {
    pub id: i64,
    pub request_id: String,
    pub attempt_index: i64,
    pub status: String,
    pub http_status: Option<i64>,
    pub ttfb_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub endpoint_family: String,
    pub model: String,
    pub source: String,
    pub message: String,
    pub estimated_cost: Option<f64>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ChannelMonitorAccountSnapshot {
    pub account_id: String,
    pub name: String,
    pub account_type: String,
    pub account_status: String,
    pub latest_status: Option<String>,
    pub latest_checked_at: Option<String>,
    pub latest_ttfb_ms: Option<i64>,
    pub concurrency: i64,
    pub available_24h: i64,
    pub available_7d: i64,
    pub availability_24h: Option<f64>,
    pub availability_7d: Option<f64>,
    pub avg_ttfb_24h_ms: Option<f64>,
    pub avg_ttfb_7d_ms: Option<f64>,
    pub attempts_24h: i64,
    pub attempts_7d: i64,
    pub failed_24h: i64,
    pub failed_7d: i64,
    pub estimated_cost_24h: Option<f64>,
    pub estimated_cost_7d: Option<f64>,
    pub timeline: Vec<ChannelMonitorEventSnapshot>,
}

pub(super) fn initialize(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS request_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            request_id TEXT NOT NULL,
            attempt_index INTEGER NOT NULL DEFAULT 0,
            account_id TEXT,
            account_name TEXT NOT NULL DEFAULT '',
            account_type TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT 'proxy',
            method TEXT NOT NULL DEFAULT '',
            path TEXT NOT NULL DEFAULT '',
            endpoint_family TEXT NOT NULL DEFAULT 'other',
            model TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',
            http_status INTEGER,
            streaming INTEGER NOT NULL DEFAULT 0,
            ttfb_ms INTEGER,
            duration_ms INTEGER,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            unpriced_tokens INTEGER NOT NULL DEFAULT 0,
            estimated_cost REAL NOT NULL DEFAULT 0.0,
            message TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            completed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_request_logs_created_at
            ON request_logs(created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_request_logs_status
            ON request_logs(status, id DESC);
         CREATE INDEX IF NOT EXISTS idx_request_logs_account
            ON request_logs(account_id, id DESC);
         CREATE INDEX IF NOT EXISTS idx_request_logs_account_created_at
            ON request_logs(account_id, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_request_logs_request
            ON request_logs(request_id, attempt_index);
         UPDATE request_logs
            SET status = 'cancelled',
                message = CASE WHEN message = '' THEN '应用上次退出前请求未完成' ELSE message END,
                completed_at = COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
          WHERE status = 'pending';",
    )?;
    prune_request_logs(conn)
}

fn prune_request_logs(conn: &Connection) -> SqlResult<()> {
    conn.execute(
        "DELETE FROM request_logs
          WHERE created_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)",
        [format!("-{REQUEST_LOG_RETENTION_DAYS} days")],
    )?;
    conn.execute(
        "DELETE FROM request_logs
          WHERE id NOT IN (
              SELECT id FROM request_logs ORDER BY id DESC LIMIT ?1
          )",
        [REQUEST_LOG_MAX_ROWS],
    )?;
    Ok(())
}

impl Db {
    pub(crate) fn insert_request_log(&self, start: &RequestLogStart) -> SqlResult<i64> {
        let account_id = normalized_filter(start.account_id.as_deref());
        let request_id = limited_text(&start.request_id, 128);
        let account_name = limited_text(&start.account_name, 200);
        let account_type = limited_text(&start.account_type, 32);
        let source = limited_text(&start.source, 32);
        let method = limited_text(&start.method, 16).to_ascii_uppercase();
        let path = limited_text(start.path.split('?').next().unwrap_or_default(), 500);
        let endpoint_family = limited_text(&start.endpoint_family, 64);
        let model = limited_text(&start.model, 200);
        let conn = lock_connection(self);
        conn.execute(
            "INSERT INTO request_logs (
                request_id, attempt_index, account_id, account_name, account_type,
                source, method, path, endpoint_family, model, streaming
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                request_id,
                start.attempt_index.max(0),
                account_id,
                account_name,
                account_type,
                source,
                method,
                path,
                endpoint_family,
                model,
                i64::from(start.streaming),
            ],
        )?;
        let id = conn.last_insert_rowid();
        if id % 100 == 0 {
            prune_request_logs(&conn)?;
        }
        Ok(id)
    }

    pub(crate) fn mark_request_log_response(
        &self,
        id: i64,
        http_status: u16,
        ttfb_ms: i64,
    ) -> SqlResult<()> {
        let conn = lock_connection(self);
        conn.execute(
            "UPDATE request_logs
                SET http_status = ?1, ttfb_ms = COALESCE(ttfb_ms, ?2)
              WHERE id = ?3 AND status = 'pending'",
            params![i64::from(http_status), ttfb_ms.max(0), id],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete_request_log(
        &self,
        id: i64,
        status: &str,
        http_status: Option<i64>,
        ttfb_ms: Option<i64>,
        duration_ms: i64,
        usage: RequestLogUsage,
        message: &str,
    ) -> SqlResult<()> {
        let status = normalize_status(status);
        let message = sanitize_log_message(message);
        let conn = lock_connection(self);
        conn.execute(
            "UPDATE request_logs SET
                status = ?1,
                http_status = COALESCE(?2, http_status),
                ttfb_ms = COALESCE(?3, ttfb_ms),
                duration_ms = ?4,
                input_tokens = ?5,
                output_tokens = ?6,
                cached_tokens = ?7,
                cache_write_tokens = ?8,
                reasoning_tokens = ?9,
                total_tokens = ?10,
                unpriced_tokens = ?11,
                estimated_cost = ?12,
                message = ?13,
                completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE id = ?14 AND status = 'pending'",
            params![
                status,
                http_status.map(|value| value.max(0)),
                ttfb_ms.map(|value| value.max(0)),
                duration_ms.max(0),
                usage.input_tokens.max(0),
                usage.output_tokens.max(0),
                usage.cached_tokens.max(0),
                usage.cache_write_tokens.max(0),
                usage.reasoning_tokens.max(0),
                usage.total_tokens.max(0),
                usage.unpriced_tokens.max(0),
                finite_cost(usage.estimated_cost),
                message,
                id,
            ],
        )?;
        Ok(())
    }

    pub fn list_request_logs(&self, query: RequestLogQuery) -> SqlResult<RequestLogPage> {
        let limit = query
            .limit
            .unwrap_or(REQUEST_LOG_DEFAULT_LIMIT)
            .clamp(1, REQUEST_LOG_MAX_LIMIT);
        let mut clauses = Vec::new();
        let mut values = Vec::<SqlValue>::new();

        if let Some(status) = normalized_filter(query.status.as_deref()) {
            clauses.push("status = ?".to_string());
            values.push(SqlValue::Text(status));
        }
        if let Some(account_id) = normalized_filter(query.account_id.as_deref()) {
            clauses.push("account_id = ?".to_string());
            values.push(SqlValue::Text(account_id));
        }
        if let Some(source) = normalized_filter(query.source.as_deref()) {
            clauses.push("source = ?".to_string());
            values.push(SqlValue::Text(source));
        }
        if let Some(before_id) = query.before_id.filter(|id| *id > 0) {
            clauses.push("id < ?".to_string());
            values.push(SqlValue::Integer(before_id));
        }
        if let Some(search) = normalized_filter(query.search.as_deref()) {
            let pattern = format!("%{}%", escape_like(&search));
            clauses.push(
                "(request_id LIKE ? ESCAPE '\\' OR account_name LIKE ? ESCAPE '\\'
                  OR model LIKE ? ESCAPE '\\' OR path LIKE ? ESCAPE '\\'
                  OR message LIKE ? ESCAPE '\\')"
                    .to_string(),
            );
            for _ in 0..5 {
                values.push(SqlValue::Text(pattern.clone()));
            }
        }

        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        values.push(SqlValue::Integer(limit + 1));
        let sql = format!(
            "SELECT id, request_id, attempt_index, account_id, account_name, account_type,
                    source, method, path, endpoint_family, model, status, http_status,
                    streaming, ttfb_ms, duration_ms, input_tokens, output_tokens,
                    cached_tokens, cache_write_tokens, reasoning_tokens, total_tokens,
                    unpriced_tokens, estimated_cost, message, created_at, completed_at
               FROM request_logs{where_sql}
              ORDER BY id DESC LIMIT ?"
        );

        let conn = lock_connection(self);
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), request_log_from_row)?;
        let mut items = rows.collect::<SqlResult<Vec<_>>>()?;
        let has_more = items.len() > limit as usize;
        if has_more {
            items.truncate(limit as usize);
        }
        let next_before_id = has_more.then(|| items.last().map(|item| item.id)).flatten();
        Ok(RequestLogPage {
            items,
            has_more,
            next_before_id,
        })
    }

    pub fn request_log_overview(&self) -> SqlResult<RequestLogOverview> {
        let conn = lock_connection(self);
        conn.query_row(
            "SELECT
                COUNT(DISTINCT request_id),
                COUNT(*),
                COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status IN ('error', 'cancelled') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'retry' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), 0),
                AVG(ttfb_ms),
                AVG(duration_ms),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(estimated_cost), 0.0)
               FROM request_logs",
            [],
            |row| {
                Ok(RequestLogOverview {
                    total_requests: row.get(0)?,
                    total_attempts: row.get(1)?,
                    success_attempts: row.get(2)?,
                    error_attempts: row.get(3)?,
                    retry_attempts: row.get(4)?,
                    pending_attempts: row.get(5)?,
                    average_ttfb_ms: row.get(6)?,
                    average_duration_ms: row.get(7)?,
                    total_tokens: row.get(8)?,
                    estimated_cost: row.get(9)?,
                })
            },
        )
    }

    pub fn today_estimated_cost(&self) -> SqlResult<f64> {
        let conn = lock_connection(self);
        conn.query_row(
            "SELECT COALESCE(SUM(estimated_cost), 0.0)
               FROM request_logs
              WHERE date(created_at, 'localtime') = date('now', 'localtime')",
            [],
            |row| row.get(0),
        )
    }

    pub fn account_estimated_cost_since(
        &self,
        account_id: &str,
        started_at: i64,
    ) -> SqlResult<(i64, f64)> {
        let conn = lock_connection(self);
        conn.query_row(
            "SELECT COUNT(DISTINCT request_id), COALESCE(SUM(estimated_cost), 0.0)
               FROM request_logs
              WHERE account_id = ?1
                AND status = 'success'
                AND created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', ?2, 'unixepoch')",
            params![account_id, started_at.max(0)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    }

    pub fn clear_request_logs(&self) -> SqlResult<u64> {
        let conn = lock_connection(self);
        Ok(conn.execute("DELETE FROM request_logs", [])? as u64)
    }

    pub(crate) fn channel_monitor_snapshot(&self) -> SqlResult<Vec<ChannelMonitorAccountSnapshot>> {
        let conn = lock_connection(self);
        let mut statement = conn.prepare(
            "WITH metrics AS (
                SELECT account_id,
                    SUM(CASE WHEN created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day')
                              AND status IN ('success', 'operational', 'degraded') THEN 1 ELSE 0 END) AS available_24h,
                    SUM(CASE WHEN created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day') THEN 1 ELSE 0 END) AS attempts_24h,
                    SUM(CASE WHEN created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day')
                              AND status IN ('retry', 'error', 'failed') THEN 1 ELSE 0 END) AS failed_24h,
                    AVG(CASE WHEN created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day') THEN ttfb_ms END) AS avg_ttfb_24h,
                    SUM(CASE WHEN created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day') THEN estimated_cost ELSE 0.0 END) AS cost_24h,
                    SUM(CASE WHEN status IN ('success', 'operational', 'degraded') THEN 1 ELSE 0 END) AS available_7d,
                    COUNT(*) AS attempts_7d,
                    SUM(CASE WHEN status IN ('retry', 'error', 'failed') THEN 1 ELSE 0 END) AS failed_7d,
                    AVG(ttfb_ms) AS avg_ttfb_7d,
                    SUM(estimated_cost) AS cost_7d
                  FROM request_logs
                 WHERE account_id IS NOT NULL
                   AND created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-7 days')
                   AND status IN ('success', 'retry', 'error', 'operational', 'degraded', 'failed')
                 GROUP BY account_id
             )
             SELECT a.id, a.name, a.account_type, a.status, a.concurrency,
                    COALESCE(m.available_24h, 0), COALESCE(m.attempts_24h, 0),
                    COALESCE(m.failed_24h, 0), m.avg_ttfb_24h, COALESCE(m.cost_24h, 0.0),
                    COALESCE(m.available_7d, 0), COALESCE(m.attempts_7d, 0),
                    COALESCE(m.failed_7d, 0), m.avg_ttfb_7d, COALESCE(m.cost_7d, 0.0)
               FROM accounts a
               LEFT JOIN metrics m ON m.account_id = a.id
              WHERE a.deleted_at IS NULL
              ORDER BY CASE a.status WHEN 'active' THEN 0 ELSE 1 END, a.priority, a.created_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let available_24h = row.get(5)?;
            let attempts_24h = row.get(6)?;
            let failed_24h = row.get(7)?;
            let cost_24h: f64 = row.get(9)?;
            let available_7d = row.get(10)?;
            let attempts_7d = row.get(11)?;
            let failed_7d = row.get(12)?;
            let cost_7d: f64 = row.get(14)?;
            Ok(ChannelMonitorAccountSnapshot {
                account_id: row.get(0)?,
                name: row.get(1)?,
                account_type: row.get(2)?,
                account_status: row.get(3)?,
                latest_status: None,
                latest_checked_at: None,
                latest_ttfb_ms: None,
                concurrency: row.get(4)?,
                available_24h,
                available_7d,
                availability_24h: availability(available_24h, attempts_24h),
                availability_7d: availability(available_7d, attempts_7d),
                avg_ttfb_24h_ms: row.get(8)?,
                avg_ttfb_7d_ms: row.get(13)?,
                attempts_24h,
                attempts_7d,
                failed_24h,
                failed_7d,
                estimated_cost_24h: (attempts_24h > 0).then_some(cost_24h),
                estimated_cost_7d: (attempts_7d > 0).then_some(cost_7d),
                timeline: Vec::new(),
            })
        })?;
        let mut snapshots = rows.collect::<SqlResult<Vec<_>>>()?;
        drop(statement);

        let account_indexes = snapshots
            .iter()
            .enumerate()
            .map(|(index, snapshot)| (snapshot.account_id.clone(), index))
            .collect::<HashMap<_, _>>();
        let mut timeline_statement = conn.prepare(
            "WITH ranked AS (
                SELECT id, account_id, request_id, attempt_index, status, http_status,
                       ttfb_ms, duration_ms, endpoint_family, model, source, message,
                       estimated_cost, created_at, COALESCE(completed_at, created_at) AS checked_at,
                       ROW_NUMBER() OVER (PARTITION BY account_id ORDER BY id DESC) AS rank
                  FROM request_logs
                 WHERE account_id IS NOT NULL
                   AND status IN ('success', 'retry', 'error', 'operational', 'degraded', 'failed')
             )
             SELECT id, account_id, request_id, attempt_index, status, http_status,
                    ttfb_ms, duration_ms, endpoint_family, model, source, message,
                    estimated_cost, created_at, checked_at
               FROM ranked
              WHERE rank <= 30
              ORDER BY account_id, id DESC",
        )?;
        let rows = timeline_statement.query_map([], |row| {
            let raw_status: String = row.get(4)?;
            let ttfb_ms = row.get(6)?;
            let duration_ms = row.get(7)?;
            let source: String = row.get(10)?;
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(14)?,
                ChannelMonitorEventSnapshot {
                    id: row.get(0)?,
                    request_id: row.get(2)?,
                    attempt_index: row.get(3)?,
                    status: monitor_status(&raw_status, ttfb_ms, duration_ms).to_string(),
                    http_status: row.get(5)?,
                    ttfb_ms,
                    duration_ms,
                    endpoint_family: row.get(8)?,
                    model: row.get(9)?,
                    source: monitor_source(&source).to_string(),
                    message: row.get(11)?,
                    estimated_cost: Some(row.get(12)?),
                    created_at: row.get(13)?,
                },
            ))
        })?;
        for row in rows {
            let (account_id, checked_at, event) = row?;
            let Some(index) = account_indexes.get(&account_id).copied() else {
                continue;
            };
            let snapshot = &mut snapshots[index];
            if snapshot.latest_status.is_none() {
                snapshot.latest_status = Some(event.status.clone());
                snapshot.latest_checked_at = Some(checked_at);
                snapshot.latest_ttfb_ms = event.ttfb_ms;
            }
            snapshot.timeline.push(event);
        }
        Ok(snapshots)
    }
}

fn lock_connection(db: &Db) -> std::sync::MutexGuard<'_, Connection> {
    db.conn
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn request_log_from_row(row: &rusqlite::Row<'_>) -> SqlResult<RequestLog> {
    Ok(RequestLog {
        id: row.get(0)?,
        request_id: row.get(1)?,
        attempt_index: row.get(2)?,
        account_id: row.get(3)?,
        account_name: row.get(4)?,
        account_type: row.get(5)?,
        source: row.get(6)?,
        method: row.get(7)?,
        path: row.get(8)?,
        endpoint_family: row.get(9)?,
        model: row.get(10)?,
        status: row.get(11)?,
        http_status: row.get(12)?,
        streaming: row.get::<_, i64>(13)? != 0,
        ttfb_ms: row.get(14)?,
        duration_ms: row.get(15)?,
        input_tokens: row.get(16)?,
        output_tokens: row.get(17)?,
        cached_tokens: row.get(18)?,
        cache_write_tokens: row.get(19)?,
        reasoning_tokens: row.get(20)?,
        total_tokens: row.get(21)?,
        unpriced_tokens: row.get(22)?,
        estimated_cost: row.get(23)?,
        message: row.get(24)?,
        created_at: row.get(25)?,
        completed_at: row.get(26)?,
    })
}

fn normalize_status(status: &str) -> &'static str {
    match status {
        "success" => "success",
        "retry" => "retry",
        "cancelled" => "cancelled",
        _ => "error",
    }
}

fn monitor_status(status: &str, ttfb_ms: Option<i64>, duration_ms: Option<i64>) -> &'static str {
    match status {
        "success" | "operational"
            if ttfb_ms.is_some_and(|value| value >= SLOW_TTFB_MS)
                || duration_ms.is_some_and(|value| value >= SLOW_DURATION_MS) =>
        {
            "degraded"
        }
        "success" | "operational" => "operational",
        "degraded" => "degraded",
        "retry" | "error" | "failed" => "error",
        _ => "error",
    }
}

fn monitor_source(source: &str) -> &'static str {
    if source == "probe" {
        "probe"
    } else {
        "traffic"
    }
}

fn availability(available: i64, attempts: i64) -> Option<f64> {
    (attempts > 0).then(|| available as f64 / attempts as f64 * 100.0)
}

fn normalized_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("all"))
        .map(|value| limited_text(value, 200))
}

fn limited_text(value: &str, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn finite_cost(cost: f64) -> f64 {
    if cost.is_finite() && cost > 0.0 {
        cost
    } else {
        0.0
    }
}

fn sanitize_log_message(message: &str) -> String {
    let normalized = message.replace(['\r', '\n', '\t'], " ");
    let lower = normalized.to_ascii_lowercase();
    if [
        "access_token",
        "refresh_token",
        "api_key",
        "authorization:",
        "authorization=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return REDACTED_CREDENTIAL_MESSAGE.to_string();
    }
    if message_contains_sensitive_url(&normalized) {
        return REDACTED_CREDENTIAL_MESSAGE.to_string();
    }
    if lower.contains("bearer ") || (lower.contains("eyj") && normalized.matches('.').count() >= 2)
    {
        return REDACTED_TOKEN_MESSAGE.to_string();
    }
    redact_sk_tokens(&normalized).chars().take(500).collect()
}

fn message_contains_sensitive_url(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(relative_start) = next_url_scheme(&lower[cursor..]) {
        let start = cursor + relative_start;
        let end = message[start..]
            .find(char::is_whitespace)
            .map(|relative_end| start + relative_end)
            .unwrap_or(message.len());
        let candidate = message[start..end].trim_end_matches(|character: char| {
            matches!(
                character,
                ')' | ']' | '}' | '>' | '"' | '\'' | ',' | ';' | '.'
            )
        });
        if let Ok(url) = reqwest::Url::parse(candidate) {
            if !url.username().is_empty() || url.password().is_some() {
                return true;
            }
            if url
                .query_pairs()
                .any(|(key, _)| sensitive_query_key(key.as_ref()))
            {
                return true;
            }
        }
        cursor = end.max(start + 1);
    }
    false
}

fn next_url_scheme(value: &str) -> Option<usize> {
    match (value.find("http://"), value.find("https://")) {
        (Some(http), Some(https)) => Some(http.min(https)),
        (Some(http), None) => Some(http),
        (None, Some(https)) => Some(https),
        (None, None) => None,
    }
}

fn sensitive_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "password",
        "code",
        "session",
        "cookie",
        "signature",
        "credential",
    ]
    .iter()
    .any(|marker| key.contains(marker))
}

fn redact_sk_tokens(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let mut output = String::new();
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find("sk-") {
        let start = cursor + relative;
        output.push_str(&value[cursor..start]);
        output.push_str("sk-***");
        let mut end = start + 3;
        while end < value.len() {
            let character = value[end..].chars().next().unwrap();
            if character.is_whitespace() || matches!(character, '"' | '\'' | ',' | '}' | ']') {
                break;
            }
            end += character.len_utf8();
        }
        cursor = end;
    }
    output.push_str(&value[cursor..]);
    output
}

#[cfg(test)]
mod sanitization_tests {
    use super::*;

    #[test]
    fn redacts_url_userinfo_and_sensitive_query_values() {
        for message in [
            "error sending request for url (https://user:password@relay.example/v1)",
            "error sending request for url https://relay.example/v1?session=private-value",
            "error sending request for url https://relay.example/v1?access%5Ftoken=private-value",
        ] {
            assert_eq!(sanitize_log_message(message), REDACTED_CREDENTIAL_MESSAGE);
        }
    }

    #[test]
    fn preserves_urls_without_credentials() {
        let message = "error sending request for url https://relay.example/v1/models?model=gpt-5";
        assert_eq!(sanitize_log_message(message), message);
    }
}

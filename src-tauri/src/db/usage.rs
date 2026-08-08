use super::types::UsageTotals;
use super::Db;
use crate::pricing::UsageBreakdown;
use rusqlite::Result as SqlResult;

impl Db {
    pub fn mark_used(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE accounts SET request_count = request_count + 1,
                    last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), last_error = '' WHERE id = ?1",
            [id],
        )?;
        conn.execute(
            "UPDATE global_usage SET total_requests = total_requests + 1 WHERE id = 1",
            [],
        )?;
        Ok(())
    }

    /// Async version for proxy hot path.
    pub async fn mark_used_async(&self, id: &str) -> SqlResult<()> {
        let Some(async_conn) = self.async_conn() else {
            return self.mark_used(id);
        };
        let id = id.to_owned();
        async_conn
            .call(move |conn| {
                conn.execute(
                    "UPDATE accounts SET request_count = request_count + 1,
                            last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), last_error = '' WHERE id = ?1",
                    [&id],
                )?;
                conn.execute(
                    "UPDATE global_usage SET total_requests = total_requests + 1 WHERE id = 1",
                    [],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| match e {
                tokio_rusqlite::Error::Error(e) => e,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })
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
        let cost = quantize_cost(cost);
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
        conn.execute(
            "UPDATE global_usage SET
                total_tokens = total_tokens + ?1,
                input_tokens = input_tokens + ?2,
                output_tokens = output_tokens + ?3,
                cached_tokens = cached_tokens + ?4,
                cache_write_tokens = cache_write_tokens + ?5,
                reasoning_tokens = reasoning_tokens + ?6,
                unpriced_tokens = unpriced_tokens + ?7,
                total_cost = total_cost + ?8
              WHERE id = 1",
            rusqlite::params![
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

    /// Async version for proxy hot path.
    pub async fn record_usage_async(
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
        let cost = quantize_cost(cost);
        if total_tokens <= 0 && cost <= 0.0 {
            return Ok(());
        }
        let Some(async_conn) = self.async_conn() else {
            return self.record_usage(id, usage, cost, unpriced_tokens);
        };
        let id = id.to_owned();
        async_conn
            .call(move |conn| {
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
                conn.execute(
                    "UPDATE global_usage SET
                        total_tokens = total_tokens + ?1,
                        input_tokens = input_tokens + ?2,
                        output_tokens = output_tokens + ?3,
                        cached_tokens = cached_tokens + ?4,
                        cache_write_tokens = cache_write_tokens + ?5,
                        reasoning_tokens = reasoning_tokens + ?6,
                        unpriced_tokens = unpriced_tokens + ?7,
                        total_cost = total_cost + ?8
                      WHERE id = 1",
                    rusqlite::params![
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
            })
            .await
            .map_err(|e| match e {
                tokio_rusqlite::Error::Error(e) => e,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })
    }

    pub fn set_error(&self, id: &str, error: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE accounts SET last_error = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
            rusqlite::params![id, truncate(error, 500)],
        )?;
        Ok(())
    }

    /// Async version for proxy hot path.
    pub async fn set_error_async(&self, id: &str, error: &str) -> SqlResult<()> {
        let Some(async_conn) = self.async_conn() else {
            return self.set_error(id, error);
        };
        let id = id.to_owned();
        let error = truncate(error, 500);
        async_conn
            .call(move |conn| {
                conn.execute(
                    "UPDATE accounts SET last_error = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
                    rusqlite::params![id, error],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| match e {
                tokio_rusqlite::Error::Error(e) => e,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })
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
        conn.execute(
            "UPDATE global_usage SET
                total_tokens = 0,
                input_tokens = 0,
                output_tokens = 0,
                cached_tokens = 0,
                cache_write_tokens = 0,
                reasoning_tokens = 0,
                unpriced_tokens = 0,
                total_cost = 0.0,
                total_requests = 0
              WHERE id = 1",
            [],
        )?;
        Ok(affected as u64)
    }

    pub fn total_request_count(&self) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT total_requests FROM global_usage WHERE id = 1",
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
            "SELECT total_tokens, input_tokens, output_tokens, cached_tokens,
                    cache_write_tokens, reasoning_tokens, unpriced_tokens, total_cost
               FROM global_usage WHERE id = 1",
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
}

fn quantize_cost(cost: f64) -> f64 {
    if !cost.is_finite() || cost <= 0.0 {
        return 0.0;
    }
    (cost * 100_000_000.0).round() / 100_000_000.0
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

use super::Db;
use crate::model_integrity::{ModelIntegrityCheck, ModelIntegrityResult};
use rusqlite::{params, Connection, Result as SqlResult};

const RESULT_RETENTION_PER_ACCOUNT: i64 = 30;

pub(super) fn initialize(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS model_integrity_results (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            declared INTEGER,
            observed_models TEXT NOT NULL DEFAULT '[]',
            risk TEXT NOT NULL,
            score INTEGER NOT NULL,
            summary TEXT NOT NULL DEFAULT '',
            checks TEXT NOT NULL DEFAULT '[]',
            probe_count INTEGER NOT NULL DEFAULT 0,
            successful_probes INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            duration_ms INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_model_integrity_account
            ON model_integrity_results(account_id, id DESC);",
    )
}

impl Db {
    pub(crate) fn insert_model_integrity_result(
        &self,
        result: &ModelIntegrityResult,
    ) -> SqlResult<ModelIntegrityResult> {
        let observed_models = serde_json::to_string(&result.observed_models)
            .expect("serializing model names cannot fail");
        let checks = serde_json::to_string(&result.checks)
            .expect("serializing integrity checks cannot fail");
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "INSERT INTO model_integrity_results (
                account_id, requested_model, declared, observed_models, risk, score,
                summary, checks, probe_count, successful_probes, total_tokens,
                reasoning_tokens, duration_ms, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                result.account_id,
                result.requested_model,
                result.declared.map(i64::from),
                observed_models,
                result.risk,
                i64::from(result.score),
                result.summary,
                checks,
                i64::from(result.probe_count),
                i64::from(result.successful_probes),
                result.total_tokens.max(0),
                result.reasoning_tokens.max(0),
                result.duration_ms.max(0),
                result.created_at,
            ],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "DELETE FROM model_integrity_results
              WHERE account_id = ?1
                AND id NOT IN (
                    SELECT id FROM model_integrity_results
                     WHERE account_id = ?1
                     ORDER BY id DESC LIMIT ?2
                )",
            params![result.account_id, RESULT_RETENTION_PER_ACCOUNT],
        )?;
        let mut stored = result.clone();
        stored.id = id;
        Ok(stored)
    }

    pub(crate) fn latest_model_integrity_results(&self) -> SqlResult<Vec<ModelIntegrityResult>> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut statement = conn.prepare(
            "SELECT r.id, r.account_id, r.requested_model, r.declared,
                    r.observed_models, r.risk, r.score, r.summary, r.checks,
                    r.probe_count, r.successful_probes, r.total_tokens,
                    r.reasoning_tokens, r.duration_ms, r.created_at
               FROM model_integrity_results r
              WHERE r.id = (
                    SELECT MAX(candidate.id)
                      FROM model_integrity_results candidate
                     WHERE candidate.account_id = r.account_id
              )
              ORDER BY r.id DESC",
        )?;
        let rows = statement.query_map([], model_integrity_result_from_row)?;
        rows.collect()
    }

    pub(crate) fn list_model_integrity_results(
        &self,
        account_id: &str,
        limit: i64,
    ) -> SqlResult<Vec<ModelIntegrityResult>> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut statement = conn.prepare(
            "SELECT id, account_id, requested_model, declared, observed_models,
                    risk, score, summary, checks, probe_count, successful_probes,
                    total_tokens, reasoning_tokens, duration_ms, created_at
               FROM model_integrity_results
              WHERE account_id = ?1
              ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![account_id, limit.clamp(1, RESULT_RETENTION_PER_ACCOUNT)],
            model_integrity_result_from_row,
        )?;
        rows.collect()
    }
}

fn model_integrity_result_from_row(row: &rusqlite::Row<'_>) -> SqlResult<ModelIntegrityResult> {
    let observed_models = row.get::<_, String>(4)?;
    let checks = row.get::<_, String>(8)?;
    Ok(ModelIntegrityResult {
        id: row.get(0)?,
        account_id: row.get(1)?,
        requested_model: row.get(2)?,
        declared: row.get::<_, Option<i64>>(3)?.map(|value| value != 0),
        observed_models: serde_json::from_str(&observed_models).unwrap_or_default(),
        risk: row.get(5)?,
        score: row.get::<_, i64>(6)?.clamp(0, 100) as u8,
        summary: row.get(7)?,
        checks: serde_json::from_str::<Vec<ModelIntegrityCheck>>(&checks).unwrap_or_default(),
        probe_count: row.get::<_, i64>(9)?.clamp(0, u8::MAX as i64) as u8,
        successful_probes: row.get::<_, i64>(10)?.clamp(0, u8::MAX as i64) as u8,
        total_tokens: row.get(11)?,
        reasoning_tokens: row.get(12)?,
        duration_ms: row.get(13)?,
        created_at: row.get(14)?,
    })
}

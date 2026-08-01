use super::Db;
use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use serde_json::{Map, Value};

impl Db {
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

    /// Replace selected entries inside a JSON-object setting in one SQLite transaction.
    ///
    /// The quota cache is intentionally stored as one setting for backwards
    /// compatibility with existing installations, but callers must update only
    /// their account entry so concurrent proxy/UI writes do not replace the
    /// entire map.
    pub fn set_json_setting_entries(
        &self,
        key: &str,
        entries: &[(String, String)],
    ) -> SqlResult<()> {
        let mut conn = self.conn.lock().unwrap();
        update_json_setting_entries(&mut conn, key, entries, false)
    }

    /// Deep-merge selected entries inside a JSON-object setting in one transaction.
    pub fn merge_json_setting_entries(
        &self,
        key: &str,
        entries: &[(String, String)],
    ) -> SqlResult<()> {
        let mut conn = self.conn.lock().unwrap();
        update_json_setting_entries(&mut conn, key, entries, true)
    }

    /// Async version used by the proxy response path.
    pub async fn set_json_setting_entries_async(
        &self,
        key: &str,
        entries: Vec<(String, String)>,
    ) -> SqlResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let Some(async_conn) = self.async_conn() else {
            return self.set_json_setting_entries(key, &entries);
        };
        let key = key.to_owned();
        async_conn
            .call(move |conn| update_json_setting_entries(conn, &key, &entries, false))
            .await
            .map_err(|error| match error {
                tokio_rusqlite::Error::Error(error) => error,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })
    }

    /// Async deep-merge version used by partial quota-header snapshots.
    pub async fn merge_json_setting_entries_async(
        &self,
        key: &str,
        entries: Vec<(String, String)>,
    ) -> SqlResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let Some(async_conn) = self.async_conn() else {
            return self.merge_json_setting_entries(key, &entries);
        };
        let key = key.to_owned();
        async_conn
            .call(move |conn| update_json_setting_entries(conn, &key, &entries, true))
            .await
            .map_err(|error| match error {
                tokio_rusqlite::Error::Error(error) => error,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })
    }
}

fn update_json_setting_entries(
    conn: &mut Connection,
    key: &str,
    entries: &[(String, String)],
    deep_merge: bool,
) -> SqlResult<()> {
    let tx = conn.transaction()?;
    let current = tx
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    let mut root = current
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));

    let Some(root_object) = root.as_object_mut() else {
        return Ok(());
    };
    for (entry_key, raw_entry) in entries {
        let Ok(mut incoming) = serde_json::from_str::<Value>(raw_entry) else {
            continue;
        };
        if let Some(existing) = root_object.get(entry_key) {
            let existing_at = json_cached_at(existing);
            let incoming_at = json_cached_at(&incoming);
            if incoming_at > 0 && existing_at > incoming_at {
                continue;
            }
        }
        if deep_merge {
            if let Some(existing) = root_object.get_mut(entry_key) {
                merge_json_value(existing, incoming);
                continue;
            }
        }
        // Move the parsed value into the map. `incoming` is mutable only so the
        // deep-merge branch can avoid an unnecessary clone in the common case.
        root_object.insert(entry_key.clone(), std::mem::take(&mut incoming));
    }

    let encoded = serde_json::to_string(&root)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, encoded],
    )?;
    tx.commit()
}

fn json_cached_at(value: &Value) -> i64 {
    value
        .get("cached_at")
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn merge_json_value(target: &mut Value, incoming: Value) {
    match incoming {
        Value::Object(incoming_object) => {
            if let Some(target_object) = target.as_object_mut() {
                for (key, value) in incoming_object {
                    if let Some(existing) = target_object.get_mut(&key) {
                        merge_json_value(existing, value);
                    } else {
                        target_object.insert(key, value);
                    }
                }
            } else {
                *target = Value::Object(incoming_object);
            }
        }
        value => *target = value,
    }
}

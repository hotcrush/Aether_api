use super::Db;
use rusqlite::{OptionalExtension, Result as SqlResult};

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
}

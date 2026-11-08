use anyhow::{Context as _, Result};
use rusqlite::Connection;

pub(crate) fn get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", (key,), |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

pub(crate) fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )
    .context("set setting")?;
    Ok(())
}

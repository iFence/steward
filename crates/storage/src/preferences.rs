use std::collections::BTreeMap;

use anyhow::{bail, Context as _, Result};
use rusqlite::Connection;

const MAX_PREFERENCE_JSON_BYTES: usize = 64 * 1024;

pub(crate) fn list(
    conn: &Connection,
    plugin_id: &str,
) -> Result<BTreeMap<String, serde_json::Value>> {
    validate_plugin_id(plugin_id)?;
    let mut statement = conn
        .prepare(
            "SELECT key, value_json FROM plugin_preferences
             WHERE plugin_id = ?1 ORDER BY key",
        )
        .context("prepare plugin preference query")?;
    let rows = statement
        .query_map((plugin_id,), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("query plugin preferences")?;
    let mut preferences = BTreeMap::new();
    for row in rows {
        let (key, value) = row.context("read plugin preference row")?;
        if let Ok(value) = serde_json::from_str(&value) {
            preferences.insert(key, value);
        }
    }
    Ok(preferences)
}

pub(crate) fn set(
    conn: &Connection,
    plugin_id: &str,
    key: &str,
    value: &serde_json::Value,
    updated_at: i64,
) -> Result<()> {
    validate_plugin_id(plugin_id)?;
    if key.is_empty() || key.len() > 64 {
        bail!("plugin preference key must be 1-64 bytes");
    }
    let value = serde_json::to_string(value).context("serialize plugin preference")?;
    if value.len() > MAX_PREFERENCE_JSON_BYTES {
        bail!(
            "plugin preference value exceeds {} KiB",
            MAX_PREFERENCE_JSON_BYTES / 1024
        );
    }
    conn.execute(
        "INSERT INTO plugin_preferences (plugin_id, key, value_json, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(plugin_id, key) DO UPDATE SET
           value_json = excluded.value_json,
           updated_at = excluded.updated_at",
        (plugin_id, key, value, updated_at),
    )
    .context("upsert plugin preference")?;
    Ok(())
}

fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    if plugin_id.trim().is_empty() || plugin_id.len() > 256 {
        bail!("plugin ID must be 1-256 bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_validation_is_bounded() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE plugin_preferences (
                plugin_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(plugin_id, key)
            );",
        )
        .unwrap();
        assert!(set(&conn, "", "theme", &serde_json::json!("dark"), 1).is_err());
        assert!(set(&conn, "com.example.test", "", &serde_json::json!("dark"), 1).is_err());
        let oversized = "x".repeat(MAX_PREFERENCE_JSON_BYTES);
        assert!(set(
            &conn,
            "com.example.test",
            "oversized",
            &serde_json::json!(oversized),
            1
        )
        .is_err());
    }
}

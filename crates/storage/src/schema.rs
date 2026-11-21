use anyhow::{bail, Context as _, Result};
use rusqlite::Connection;

pub(crate) const CURRENT_SCHEMA_VERSION: i64 = 3;

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("read schema version")?;
    if current > CURRENT_SCHEMA_VERSION {
        bail!(
            "database schema version {current} is newer than supported version {CURRENT_SCHEMA_VERSION}"
        );
    }

    let tx = conn
        .unchecked_transaction()
        .context("begin schema migration")?;
    if current < 1 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS apps (
                path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                last_seen INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS usage (
                path TEXT PRIMARY KEY,
                count INTEGER NOT NULL DEFAULT 0,
                last_used INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS clipboard_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL,
                copied_at INTEGER NOT NULL
            );
            -- File search was removed; drop snapshots left by older builds.
            DROP TABLE IF EXISTS settings_blob;
            DELETE FROM settings WHERE key = 'file_index';",
        )
        .context("apply schema migration 1")?;
        tx.pragma_update(None, "user_version", 1)
            .context("record schema migration 1")?;
    }
    if current < 2 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS capability_grants (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                plugin_id TEXT NOT NULL,
                publisher_fingerprint TEXT NOT NULL,
                version_range TEXT NOT NULL,
                capability TEXT NOT NULL,
                scope TEXT NOT NULL,
                granted_at INTEGER NOT NULL,
                revoked_at INTEGER
            );
            CREATE UNIQUE INDEX IF NOT EXISTS capability_grants_active
                ON capability_grants (
                    plugin_id,
                    publisher_fingerprint,
                    version_range,
                    capability,
                    scope
                )
                WHERE revoked_at IS NULL;
            CREATE INDEX IF NOT EXISTS capability_grants_plugin
                ON capability_grants(plugin_id, revoked_at);
            CREATE TABLE IF NOT EXISTS capability_audit (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                plugin_id TEXT NOT NULL,
                publisher_fingerprint TEXT NOT NULL,
                capability TEXT NOT NULL,
                scope TEXT NOT NULL,
                outcome TEXT NOT NULL,
                occurred_at INTEGER NOT NULL,
                detail TEXT
            );
            CREATE INDEX IF NOT EXISTS capability_audit_plugin
                ON capability_audit(plugin_id, occurred_at DESC);",
        )
        .context("apply schema migration 2")?;
        tx.pragma_update(None, "user_version", 2)
            .context("record schema migration 2")?;
    }
    if current < 3 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS plugin_preferences (
                plugin_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(plugin_id, key)
            );
            CREATE INDEX IF NOT EXISTS plugin_preferences_plugin
                ON plugin_preferences(plugin_id);",
        )
        .context("apply schema migration 3")?;
        tx.pragma_update(None, "user_version", 3)
            .context("record schema migration 3")?;
    }
    tx.commit().context("commit schema migration")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_idempotent_and_records_the_latest_version() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_schema_is_reported_instead_of_being_overwritten() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
            .unwrap();
        let error = migrate(&conn).unwrap_err().to_string();
        assert!(error.contains("newer than supported"));
    }
}

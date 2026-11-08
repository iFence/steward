use anyhow::{Context as _, Result};
use rusqlite::Connection;

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
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
    .context("run schema migrations")
}

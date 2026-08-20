//! SQLite wrappers and configuration file access.
//!
//! M1 stores two things in a single SQLite database: an application index
//! cache (so cold start can skip full file scanning until it changes) and per-
//! application usage frequency used to rank launcher results.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rusqlite::Connection;
use steward_core_engine::AppEntry;

/// The on-disk database file name inside the data directory.
const DB_FILE: &str = "steward.db";

/// Thin wrapper over a SQLite connection with a stable application schema.
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Open (creating if needed) the database at the OS data directory, e.g.
    /// `%APPDATA%\Steward\steward.db` on Windows.
    pub fn open() -> Result<Self> {
        let data_dir = dirs::data_dir()
            .context("no OS data directory available")?
            .join("Steward");
        Self::open_at(&data_dir)
    }

    /// Open (creating if needed) the database under an explicit directory. Used
    /// by callers that need a stable test path.
    pub fn open_at(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).context("create data directory")?;
        let conn = Connection::open(data_dir.join(DB_FILE)).context("open sqlite database")?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("enable WAL")?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    /// Open an in-memory database — used by tests.
    #[cfg(test)]
    fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory sqlite database")?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS apps (
                    path TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    last_seen INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS usage (
                    path TEXT PRIMARY KEY,
                    count INTEGER NOT NULL DEFAULT 0,
                    last_used INTEGER NOT NULL
                );",
            )
            .context("run schema migrations")
    }

    /// Record that `path` was launched: bump the count and update last-used.
    pub fn upsert_usage(&self, path: &Path) -> Result<()> {
        let key = path_key(path);
        let now = unix_seconds();
        self.conn
            .execute(
                "INSERT INTO usage(path, count, last_used) VALUES (?1, 1, ?2)
                 ON CONFLICT(path) DO UPDATE SET
                    count = count + 1,
                    last_used = excluded.last_used",
                (key, now),
            )
            .context("upsert usage")?;
        Ok(())
    }

    /// Usage count for `path`, or 0 if never launched.
    pub fn frequency(&self, path: &Path) -> u32 {
        let key = path_key(path);
        self.conn
            .query_row("SELECT count FROM usage WHERE path = ?1", (key,), |row| {
                row.get::<_, u32>(0)
            })
            .unwrap_or(0)
    }

    /// Convenience: resolve a usage count from a path string (matching the
    /// signature expected by `core_engine::Engine::query`).
    pub fn frequency_str(&self, path: &str) -> u32 {
        self.frequency(Path::new(path))
    }

    /// All cached applications from the last successful scan, for acceleration
    /// of cold start.
    pub fn cached_apps(&self) -> Result<Vec<AppEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, name FROM apps")
            .context("prepare cached apps query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AppEntry {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    name: row.get::<_, String>(1)?,
                })
            })
            .context("query cached apps")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect cached apps")
    }

    /// Replace the set of scanned apps. Simple approach for M1: clear and
    /// re-insert on each full scan.
    pub fn mark_seen(&mut self, apps: &[AppEntry]) -> Result<()> {
        let now = unix_seconds();
        self.conn
            .execute("DELETE FROM apps", [])
            .context("clear apps cache")?;
        let tx = self.conn.transaction().context("begin scan transaction")?;
        {
            let mut stmt = tx
                .prepare("INSERT INTO apps(path, name, last_seen) VALUES (?1, ?2, ?3)")
                .context("prepare app insert")?;
            for app in apps {
                stmt.execute((path_key(&app.path), &app.name, now))
                    .context("insert app cache")?;
            }
        }
        tx.commit().context("commit scan transaction")?;
        Ok(())
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str) -> AppEntry {
        AppEntry {
            name: name.into(),
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn migrate_and_usage_flow() {
        let storage = Storage::open_in_memory().unwrap();
        let p = Path::new("C:/calc.exe");
        assert_eq!(storage.frequency(p), 0);

        storage.upsert_usage(p).unwrap();
        storage.upsert_usage(p).unwrap();
        assert_eq!(storage.frequency(p), 2);
        // A different path is independent.
        assert_eq!(storage.frequency(Path::new("C:/term.exe")), 0);
    }

    #[test]
    fn mark_seen_then_cached_apps_roundtrips() {
        let mut storage = Storage::open_in_memory().unwrap();
        let apps = vec![
            entry("Calculator", "C:/calc.exe"),
            entry("Terminal", "C:/term.exe"),
        ];
        storage.mark_seen(&apps).unwrap();
        let cached = storage.cached_apps().unwrap();
        assert_eq!(cached.len(), 2);
        assert!(cached.iter().any(|a| a.name == "Calculator"));
    }
}

//! SQLite wrappers and configuration file access.
//!
//! M1 stores two things in a single SQLite database: an application index
//! cache (so cold start can skip full file scanning until it changes) and per-
//! application usage frequency used to rank launcher results.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use rusqlite::Connection;
use steward_core_engine::AppEntry;

/// The on-disk database file name inside the data directory.
const DB_FILE: &str = "steward.db";
/// How long a completed app scan is trusted before the next boot re-scans in
/// the background. Cold start reads the cache within this window and never
/// blocks on the scanner.
const SCAN_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Settings-table key storing the UNIX timestamp of the last full scan.
const LAST_SCAN_SETTING: &str = "last_scan";

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
                );
                CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );",
            )
            .context("run schema migrations")
    }

    /// Read a persisted application setting, or `None` when it was never set.
    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", (key,), |row| {
                row.get::<_, String>(0)
            })
            .ok()
    }

    /// Persist an application setting (insert or replace).
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO settings(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            .context("set setting")?;
        Ok(())
    }

    /// Whether the cached app index can be trusted without a re-scan: the
    /// cache is fresh for [`SCAN_CACHE_TTL`] after the last successful scan.
    pub fn is_cache_fresh(&self) -> bool {
        self.last_scan()
            .is_some_and(|t| t.elapsed().map(|age| age < SCAN_CACHE_TTL).unwrap_or(false))
    }

    /// Timestamp of the last completed scan, if any.
    fn last_scan(&self) -> Option<SystemTime> {
        self.get_setting(LAST_SCAN_SETTING)
            .and_then(|value| value.parse::<u64>().ok())
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
    }

    /// Record that a full scan completed just now.
    pub fn touch_scan(&self) -> Result<()> {
        self.set_setting(LAST_SCAN_SETTING, &unix_seconds().to_string())
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

    /// Merge the result of a full scan into the cache: upsert seen apps and
    /// drop entries that existed in a previous scan but were not seen this
    /// time. Unlike the previous clear-and-reinsert approach this avoids
    /// rewriting unchanged rows on every boot, and it records the scan time
    /// so subsequent cold starts can read the cache without re-scanning.
    pub fn mark_seen(&mut self, apps: &[AppEntry]) -> Result<()> {
        let now = unix_seconds();
        let tx = self.conn.transaction().context("begin scan transaction")?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO apps(path, name, last_seen) VALUES (?1, ?2, ?3)
                     ON CONFLICT(path) DO UPDATE SET name = excluded.name, last_seen = excluded.last_seen",
                )
                .context("prepare app upsert")?;
            let mut seen: HashSet<String> = HashSet::with_capacity(apps.len());
            for app in apps {
                let key = path_key(&app.path);
                stmt.execute((&key, &app.name, now))
                    .context("upsert app cache")?;
                seen.insert(key);
            }
            // Remove cached entries that were not present in this scan
            // (deleting by path instead of by timestamp so scans in the same
            // second still prune correctly).
            let existing = {
                let mut all = tx
                    .prepare("SELECT path FROM apps")
                    .context("prepare cached paths query")?;
                let paths = all
                    .query_map([], |row| row.get::<_, String>(0))
                    .context("query cached paths")?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .context("collect cached paths")?;
                paths
            };
            let mut remove = tx
                .prepare("DELETE FROM apps WHERE path = ?1")
                .context("prepare stale app delete")?;
            for path in existing {
                if !seen.contains(&path) {
                    remove.execute([&path]).context("delete stale cached app")?;
                }
            }
        }
        tx.commit().context("commit scan transaction")?;
        self.touch_scan()?;
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
        assert!(!storage.is_cache_fresh());
        storage.mark_seen(&apps).unwrap();
        assert!(storage.is_cache_fresh());
        let cached = storage.cached_apps().unwrap();
        assert_eq!(cached.len(), 2);
        assert!(cached.iter().any(|a| a.name == "Calculator"));
    }

    #[test]
    fn mark_seen_is_incremental_and_removes_missing_apps() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage
            .mark_seen(&[
                entry("Calculator", "C:/calc.exe"),
                entry("Terminal", "C:/term.exe"),
            ])
            .unwrap();
        // Second scan keeps the still-present app, adds a new one, and drops
        // the app that disappeared.
        storage
            .mark_seen(&[
                entry("Calculator", "C:/calc.exe"),
                entry("Paint", "C:/paint.exe"),
            ])
            .unwrap();
        let cached = storage.cached_apps().unwrap();
        assert_eq!(cached.len(), 2);
        assert!(cached.iter().any(|a| a.name == "Calculator"));
        assert!(cached.iter().any(|a| a.name == "Paint"));
        assert!(!cached.iter().any(|a| a.name == "Terminal"));
    }

    #[test]
    fn settings_roundtrip_and_overwrite() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(storage.get_setting("theme_color"), None);

        storage.set_setting("theme_color", "#89b4fa").unwrap();
        assert_eq!(
            storage.get_setting("theme_color").as_deref(),
            Some("#89b4fa")
        );

        // Setting the same key again replaces the value.
        storage.set_setting("theme_color", "#a6e3a1").unwrap();
        assert_eq!(
            storage.get_setting("theme_color").as_deref(),
            Some("#a6e3a1")
        );
    }
}

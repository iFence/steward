//! Plugin metadata cache (SQLite): incremental scanning / indexing.
//!
//! M2 stores parsed manifests in `plugins.db` so cold start reads the cache
//! directly and never walks the plugin directories (no full file I/O scan on
//! the summon path). A background reconcile (`scan`) re-parses a plugin only
//! when its manifest version changed or it is new, and drops entries whose
//! directory disappeared.

pub mod manifest;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rusqlite::Connection;

pub use manifest::{
    Isolation, ManifestError, Permission, PluginCommand, PluginManifest, Trigger, TriggerType,
};

/// On-disk database file name inside the Steward data directory.
const DB_FILE: &str = "plugins.db";
/// Plugin installation root (one directory per plugin).
const PLUGINS_DIR: &str = "plugins";

/// A plugin as cached by the registry: its validated manifest plus where it
/// lives on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMeta {
    pub manifest: PluginManifest,
    /// Plugin directory (contains `plugin.json` and `dist/index.js`).
    pub dir: PathBuf,
    /// Absolute bundle entry path (`dir/dist/index.js`).
    pub entry: PathBuf,
    /// Inline SVG icon for launcher rows, if the manifest declares one.
    pub icon: Option<String>,
    /// UNIX timestamp of the last time this row was (re)written.
    pub scanned_at: i64,
}

/// Result of one reconcile pass over the plugin root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Number of plugins present after the scan.
    pub plugins: usize,
    /// Plugins whose manifest appeared for the first time.
    pub inserted: usize,
    /// Plugins whose version changed (or whose fields were rewritten).
    pub updated: usize,
    /// Plugins whose manifest was parsed but needed no cache write.
    pub unchanged: usize,
    /// Cached plugins whose directory disappeared.
    pub removed: usize,
    /// Human-readable failures (plugin dir + reason), one per entry.
    pub failed: Vec<String>,
}

/// Thin wrapper over a SQLite connection holding the plugin metadata cache.
pub struct Registry {
    conn: Connection,
    data_dir: PathBuf,
    root: PathBuf,
}

impl Registry {
    /// Open (creating if needed) the registry database and plugin root under
    /// the OS data directory, e.g. `%APPDATA%\Steward\plugins` on Windows.
    pub fn open() -> Result<Self> {
        let data_dir = dirs::data_dir()
            .context("no OS data directory available")?
            .join("Steward");
        Self::open_at(&data_dir)
    }

    /// Open (creating if needed) the registry database and plugin root under
    /// an explicit app data directory. Used by callers with a stable test
    /// path or a custom install location.
    pub fn open_at(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).context("create data directory")?;
        let root = data_dir.join(PLUGINS_DIR);
        Self::open_with_root(data_dir, &root)
    }

    /// Open the registry database under `data_dir` but scan plugins from an
    /// explicit plugin root (e.g. `STEWARD_PLUGINS_DIR` pointing at a repo's
    /// `packages/plugins` during development).
    pub fn open_with_root(data_dir: &Path, root: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).context("create data directory")?;
        std::fs::create_dir_all(root).context("create plugins directory")?;
        let conn = Connection::open(data_dir.join(DB_FILE)).context("open plugin database")?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("enable WAL")?;
        let registry = Self {
            conn,
            data_dir: data_dir.to_path_buf(),
            root: root.to_path_buf(),
        };
        registry.migrate()?;
        Ok(registry)
    }

    /// Open an in-memory database with a temp-dir plugin root — used by tests.
    #[cfg(test)]
    fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory plugin database")?;
        let root = std::env::temp_dir().join(format!(
            "steward-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).context("create temp plugin root")?;
        let registry = Self {
            conn,
            data_dir: std::env::temp_dir(),
            root,
        };
        registry.migrate()?;
        Ok(registry)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS plugins (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    dir TEXT NOT NULL,
                    entry TEXT NOT NULL,
                    icon TEXT,
                    fs_roots TEXT NOT NULL DEFAULT '[]',
                    isolation TEXT NOT NULL,
                    permissions TEXT NOT NULL,
                    commands TEXT NOT NULL,
                    scanned_at INTEGER NOT NULL
                );",
            )
            .context("run plugin schema migrations")?;
        // Databases created before the icon column existed: add it in place so
        // the cold-start cache keeps serving without a full rescan.
        let has_icon = self
            .conn
            .prepare("PRAGMA table_info(plugins)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "icon");
        if !has_icon {
            self.conn
                .execute("ALTER TABLE plugins ADD COLUMN icon TEXT", [])
                .context("add icon column to plugin cache")?;
        }
        let has_fs_roots = self
            .conn
            .prepare("PRAGMA table_info(plugins)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "fs_roots");
        if !has_fs_roots {
            self.conn
                .execute(
                    "ALTER TABLE plugins ADD COLUMN fs_roots TEXT NOT NULL DEFAULT '[]'",
                    [],
                )
                .context("add fs_roots column to plugin cache")?;
        }
        Ok(())
    }

    /// The plugin installation root this registry manages.
    pub fn plugins_root(&self) -> &Path {
        &self.root
    }

    /// The data directory holding the registry database.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// All cached plugins, read straight from SQLite. This is the cold-start
    /// path: no filesystem walk, no manifest parsing.
    pub fn cached_plugins(&self) -> Result<Vec<PluginMeta>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, version, dir, entry, icon, fs_roots, isolation, permissions, commands, scanned_at
                 FROM plugins",
            )
            .context("prepare cached plugins query")?;
        let rows = stmt
            .query_map([], |row| {
                let manifest = PluginManifest {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    version: row.get(2)?,
                    icon: row.get(5)?,
                    fs_roots: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                    isolation: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
                    permissions: serde_json::from_str(&row.get::<_, String>(8)?)
                        .unwrap_or_default(),
                    commands: serde_json::from_str(&row.get::<_, String>(9)?).unwrap_or_default(),
                };
                let icon = manifest.icon.clone();
                Ok(PluginMeta {
                    manifest,
                    dir: PathBuf::from(row.get::<_, String>(3)?),
                    entry: PathBuf::from(row.get::<_, String>(4)?),
                    icon,
                    scanned_at: row.get(10)?,
                })
            })
            .context("query cached plugins")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect cached plugins")
    }

    /// Reconcile the cache against the configured plugin root: parse every
    /// `plugin.json` one directory deep, upsert rows whose version changed,
    /// and drop rows whose directory disappeared.
    pub fn scan(&mut self) -> Result<ScanReport> {
        let root = self.root.clone();
        self.scan_at(&root)
    }

    /// Reconcile the cache against an explicit plugin root (tests).
    pub fn scan_at(&mut self, root: &Path) -> Result<ScanReport> {
        if !root.is_dir() {
            return Ok(ScanReport::default());
        }
        let cached = self.cached_plugins().context("read cache before scan")?;
        let mut cached_by_id = cached
            .into_iter()
            .map(|meta| (meta.manifest.id.clone(), meta))
            .collect::<HashMap<_, _>>();

        let mut report = ScanReport::default();
        let mut seen_dirs = Vec::new();
        for entry in std::fs::read_dir(root).context("read plugin root")? {
            let dir = entry.context("read plugin directory entry")?.path();
            if !dir.is_dir() {
                continue;
            }
            let manifest = match manifest::load_manifest(&dir) {
                Ok(manifest) => manifest,
                Err(error) => {
                    report.failed.push(format!("{}: {error:#}", dir.display()));
                    continue;
                }
            };
            seen_dirs.push(dir.clone());
            let entry_path = manifest::entry_path(&dir);
            match cached_by_id.get(&manifest.id) {
                Some(cached) if cached.manifest.version == manifest.version => {
                    report.unchanged += 1;
                }
                Some(_) => {
                    self.upsert(&dir, &entry_path, &manifest, &mut cached_by_id)
                        .context("upsert changed plugin")?;
                    report.updated += 1;
                }
                None => {
                    self.upsert(&dir, &entry_path, &manifest, &mut cached_by_id)
                        .context("upsert new plugin")?;
                    report.inserted += 1;
                }
            }
        }

        // Drop cached rows whose directory is gone (or whose plugin.json
        // disappeared or became invalid).
        for (id, meta) in &cached_by_id {
            if !meta.dir.is_dir() || !seen_dirs.contains(&meta.dir) {
                self.conn
                    .execute("DELETE FROM plugins WHERE id = ?1", (id,))
                    .context("delete stale plugin")?;
                report.removed += 1;
            }
        }
        report.plugins = self
            .cached_plugins()
            .context("count plugins after scan")?
            .len();
        Ok(report)
    }

    fn upsert(
        &mut self,
        dir: &Path,
        entry: &Path,
        manifest: &PluginManifest,
        cached: &mut HashMap<String, PluginMeta>,
    ) -> Result<()> {
        let now = unix_seconds();
        self.conn
            .execute(
                "INSERT INTO plugins (id, name, version, dir, entry, icon, fs_roots, isolation, permissions, commands, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    version = excluded.version,
                    dir = excluded.dir,
                    entry = excluded.entry,
                    icon = excluded.icon,
                    fs_roots = excluded.fs_roots,
                    isolation = excluded.isolation,
                    permissions = excluded.permissions,
                    commands = excluded.commands,
                    scanned_at = excluded.scanned_at",
                (
                    &manifest.id,
                    &manifest.name,
                    &manifest.version,
                    &dir.to_string_lossy(),
                    &entry.to_string_lossy(),
                    &manifest.icon,
                    &serde_json::to_string(&manifest.fs_roots).unwrap_or_default(),
                    &serde_json::to_string(&manifest.isolation).unwrap_or_default(),
                    &serde_json::to_string(&manifest.permissions).unwrap_or_default(),
                    &serde_json::to_string(&manifest.commands).unwrap_or_default(),
                    now,
                ),
            )
            .context("upsert plugin metadata")?;
        cached.insert(
            manifest.id.clone(),
            PluginMeta {
                manifest: manifest.clone(),
                dir: dir.to_path_buf(),
                entry: entry.to_path_buf(),
                icon: manifest.icon.clone(),
                scanned_at: now,
            },
        );
        Ok(())
    }
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

    fn write_plugin(root: &Path, id: &str, version: &str, trigger: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = format!(
            r#"{{
                "id": "{id}",
                "name": "Test {id}",
                "version": "{version}",
                "commands": [
                    {{
                        "name": "cmd",
                        "title": "Command",
                        "trigger": {{ "type": "prefix", "value": "{trigger}" }}
                    }}
                ],
                "permissions": [],
                "isolation": "shared-pool"
            }}"#
        );
        std::fs::write(dir.join("plugin.json"), manifest).unwrap();
        let dist = dir.join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.js"), "// bundle").unwrap();
    }

    #[test]
    fn scan_then_cached_roundtrip() {
        let mut registry = Registry::open_in_memory().unwrap();
        let root = registry.plugins_root().to_path_buf();
        write_plugin(&root, "com.test.alpha", "1.0.0", "alpha ");
        write_plugin(&root, "com.test.beta", "1.0.0", "beta ");

        let report = registry.scan().unwrap();
        assert_eq!(report.inserted, 2);
        assert_eq!(report.plugins, 2);
        assert_eq!(report.unchanged, 0);

        let cached = registry.cached_plugins().unwrap();
        assert_eq!(cached.len(), 2);
        let alpha = cached
            .iter()
            .find(|m| m.manifest.id == "com.test.alpha")
            .unwrap();
        assert_eq!(alpha.manifest.name, "Test com.test.alpha");
        assert_eq!(alpha.entry, root.join("com.test.alpha/dist/index.js"));
        assert!(alpha.dir.is_dir());
    }

    #[test]
    fn scan_is_incremental_on_version_change() {
        let mut registry = Registry::open_in_memory().unwrap();
        let root = registry.plugins_root().to_path_buf();
        write_plugin(&root, "com.test.alpha", "1.0.0", "alpha ");

        registry.scan().unwrap();
        let report = registry.scan().unwrap();
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.updated, 0);

        write_plugin(&root, "com.test.alpha", "1.1.0", "alpha ");
        let report = registry.scan().unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(report.unchanged, 0);
        let cached = registry.cached_plugins().unwrap();
        assert_eq!(cached[0].manifest.version, "1.1.0");
    }

    #[test]
    fn removed_plugin_directory_is_pruned() {
        let mut registry = Registry::open_in_memory().unwrap();
        let root = registry.plugins_root().to_path_buf();
        write_plugin(&root, "com.test.alpha", "1.0.0", "alpha ");
        write_plugin(&root, "com.test.beta", "1.0.0", "beta ");
        registry.scan().unwrap();

        std::fs::remove_dir_all(root.join("com.test.beta")).unwrap();
        let report = registry.scan().unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.plugins, 1);
    }

    #[test]
    fn cold_start_reads_cache_without_scan() {
        let data_dir = std::env::temp_dir().join(format!(
            "steward-registry-db-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut registry = Registry::open_at(&data_dir).unwrap();
        write_plugin(registry.plugins_root(), "com.test.alpha", "1.0.0", "alpha ");
        registry.scan().unwrap();
        drop(registry);

        // A fresh registry on the same data dir serves the cache directly.
        {
            let registry = Registry::open_at(&data_dir).unwrap();
            let cached = registry.cached_plugins().unwrap();
            assert_eq!(cached.len(), 1);
            assert_eq!(cached[0].manifest.id, "com.test.alpha");
        }
        std::fs::remove_dir_all(&data_dir).unwrap();
    }

    #[test]
    fn manifest_icon_is_cached_and_survives_reopen() {
        let data_dir = std::env::temp_dir().join(format!(
            "steward-registry-icon-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut registry = Registry::open_at(&data_dir).unwrap();
        let plugin_dir = registry.plugins_root().join("com.example.icon");
        std::fs::create_dir_all(plugin_dir.join("dist")).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{
                "id": "com.example.icon",
                "name": "Icon",
                "version": "1.0.0",
                "icon": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"></svg>",
                "commands": [
                    { "name": "icon", "title": "Icon", "trigger": { "type": "command" } }
                ]
            }"#,
        )
        .unwrap();
        std::fs::write(plugin_dir.join("dist").join("index.js"), "// bundle").unwrap();

        registry.scan().unwrap();
        let cached = registry.cached_plugins().unwrap();
        assert_eq!(cached.len(), 1);
        assert!(cached[0].icon.as_deref().unwrap().contains("<svg"));
        drop(registry);

        // A fresh registry on the same data dir serves the icon from cache.
        let registry = Registry::open_at(&data_dir).unwrap();
        let cached = registry.cached_plugins().unwrap();
        assert!(cached[0].icon.is_some(), "icon must survive cold start");
        drop(registry);
        std::fs::remove_dir_all(&data_dir).unwrap();
    }

    #[test]
    fn existing_database_without_icon_column_is_migrated() {
        let data_dir = std::env::temp_dir().join(format!(
            "steward-registry-migrate-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&data_dir).unwrap();
        // Simulate a cache written by a pre-icon build.
        {
            let conn = Connection::open(data_dir.join(DB_FILE)).unwrap();
            conn.execute_batch(
                "CREATE TABLE plugins (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    dir TEXT NOT NULL,
                    entry TEXT NOT NULL,
                    isolation TEXT NOT NULL,
                    permissions TEXT NOT NULL,
                    commands TEXT NOT NULL,
                    scanned_at INTEGER NOT NULL
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO plugins (id, name, version, dir, entry, isolation, permissions, commands, scanned_at)
                 VALUES ('com.example.old', 'Old', '1.0.0', 'dir', 'entry', 'shared-pool', '[]', '[]', 0)",
                [],
            )
            .unwrap();
        }

        let registry = Registry::open_at(&data_dir).unwrap();
        let cached = registry.cached_plugins().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].manifest.id, "com.example.old");
        assert!(cached[0].icon.is_none());
        drop(registry);
        std::fs::remove_dir_all(&data_dir).unwrap();
    }

    #[test]
    fn invalid_manifest_is_reported_and_not_cached() {
        let mut registry = Registry::open_in_memory().unwrap();
        let root = registry.plugins_root().to_path_buf();
        // Unsupported permission -> validation failure at scan time.
        write_plugin(&root, "com.test.alpha", "1.0.0", "alpha ");
        let bad = root.join("com.test.bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(
            bad.join("plugin.json"),
            r#"{
                "id": "com.test.bad",
                "name": "Bad",
                "version": "1.0.0",
            "commands": [
                { "name": "cmd", "title": "Command", "trigger": { "type": "prefix", "value": "x" } }
            ],
            "permissions": ["clipboard.erase"],
            "isolation": "shared-pool"
        }"#,
        )
        .unwrap();

        let report = registry.scan().unwrap();
        assert_eq!(report.inserted, 1);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].contains("com.test.bad"));
        let cached = registry.cached_plugins().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].manifest.id, "com.test.alpha");
    }
}

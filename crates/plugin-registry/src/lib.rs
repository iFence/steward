//! Plugin routing metadata cache (SQLite).
//!
//! Cold start reads cached route data without walking plugin directories or
//! loading any NSH VM. The cache schema is disposable and versioned separately
//! from the on-disk manifest schema.

pub mod manifest;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rusqlite::Connection;

pub use manifest::{
    ManifestError, PluginCommand, PluginManifest, Trigger, TriggerType, MANIFEST_FILE,
    PACKAGE_FILE, SCHEMA_VERSION,
};

const DB_FILE: &str = "plugins.db";
const PLUGINS_DIR: &str = "plugins";
const CACHE_SCHEMA_VERSION: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMeta {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    pub package: PathBuf,
    pub icon: Option<String>,
    pub scanned_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub plugins: usize,
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub failed: Vec<String>,
}

pub struct Registry {
    conn: Connection,
    data_dir: PathBuf,
    root: PathBuf,
}

impl Registry {
    pub fn open() -> Result<Self> {
        let data_dir = dirs::data_dir()
            .context("no OS data directory available")?
            .join("Steward");
        Self::open_at(&data_dir)
    }

    pub fn open_at(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join(PLUGINS_DIR);
        Self::open_with_root(data_dir, &root)
    }

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

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version != CACHE_SCHEMA_VERSION {
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS plugins;
                 CREATE TABLE plugins (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    dir TEXT NOT NULL,
                    package TEXT NOT NULL,
                    icon TEXT,
                    commands TEXT NOT NULL,
                    scanned_at INTEGER NOT NULL
                 );",
            )?;
            self.conn
                .pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
        }
        Ok(())
    }

    pub fn plugins_root(&self) -> &Path {
        &self.root
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn cached_plugins(&self) -> Result<Vec<PluginMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, version, dir, package, icon, commands, scanned_at FROM plugins",
        )?;
        let rows = stmt.query_map([], |row| {
            let manifest = PluginManifest {
                schema_version: SCHEMA_VERSION,
                id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                icon: row.get(5)?,
                package: PACKAGE_FILE.to_string(),
                commands: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            };
            let icon = manifest.icon.clone();
            Ok(PluginMeta {
                manifest,
                dir: PathBuf::from(row.get::<_, String>(3)?),
                package: PathBuf::from(row.get::<_, String>(4)?),
                icon,
                scanned_at: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect cached plugins")
    }

    pub fn scan(&mut self) -> Result<ScanReport> {
        let root = self.root.clone();
        self.scan_at(&root)
    }

    pub fn scan_at(&mut self, root: &Path) -> Result<ScanReport> {
        if !root.is_dir() {
            return Ok(ScanReport::default());
        }
        let cached = self.cached_plugins()?;
        let mut cached_by_id = cached
            .into_iter()
            .map(|meta| (meta.manifest.id.clone(), meta))
            .collect::<HashMap<_, _>>();
        let mut report = ScanReport::default();
        let mut seen = Vec::new();

        for entry in std::fs::read_dir(root).context("read plugin root")? {
            let dir = entry?.path();
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
            let package = manifest::package_path(&dir);
            if !package.is_file() || !manifest::is_safe_package_path(Path::new(&manifest.package)) {
                report.failed.push(format!(
                    "{}: missing or unsafe package {}",
                    dir.display(),
                    package.display()
                ));
                continue;
            }
            seen.push(dir.clone());
            match cached_by_id.get(&manifest.id) {
                Some(old) if old.manifest.version == manifest.version && old.package == package => {
                    report.unchanged += 1;
                }
                Some(_) => {
                    self.upsert(&dir, &package, &manifest, &mut cached_by_id)?;
                    report.updated += 1;
                }
                None => {
                    self.upsert(&dir, &package, &manifest, &mut cached_by_id)?;
                    report.inserted += 1;
                }
            }
        }

        for (id, meta) in &cached_by_id {
            if !seen.contains(&meta.dir) {
                self.conn
                    .execute("DELETE FROM plugins WHERE id = ?1", (id,))?;
                report.removed += 1;
            }
        }
        report.plugins = self.cached_plugins()?.len();
        Ok(report)
    }

    fn upsert(
        &mut self,
        dir: &Path,
        package: &Path,
        manifest: &PluginManifest,
        cached: &mut HashMap<String, PluginMeta>,
    ) -> Result<()> {
        let now = unix_seconds();
        self.conn.execute(
            "INSERT INTO plugins (id,name,version,dir,package,icon,commands,scanned_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET
               name=excluded.name, version=excluded.version, dir=excluded.dir,
               package=excluded.package, icon=excluded.icon,
               commands=excluded.commands, scanned_at=excluded.scanned_at",
            (
                &manifest.id,
                &manifest.name,
                &manifest.version,
                &dir.to_string_lossy(),
                &package.to_string_lossy(),
                &manifest.icon,
                &serde_json::to_string(&manifest.commands).unwrap_or_default(),
                now,
            ),
        )?;
        cached.insert(
            manifest.id.clone(),
            PluginMeta {
                manifest: manifest.clone(),
                dir: dir.to_path_buf(),
                package: package.to_path_buf(),
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

    fn write_plugin(root: &Path, id: &str, version: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"{{
                  "schemaVersion": 2,
                  "id": "{id}",
                  "name": "Plugin {id}",
                  "version": "{version}",
                  "package": "plugin.nshpkg",
                  "commands": [{{
                    "name": "run",
                    "title": "Run",
                    "trigger": {{ "type": "command" }}
                  }}]
                }}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join(PACKAGE_FILE), b"package").unwrap();
    }

    fn temp_registry() -> (PathBuf, Registry) {
        let dir = std::env::temp_dir().join(format!(
            "steward-plugin-registry-v2-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let registry = Registry::open_at(&dir).unwrap();
        (dir, registry)
    }

    #[test]
    fn scan_cache_and_incremental_update() {
        let (dir, mut registry) = temp_registry();
        write_plugin(registry.plugins_root(), "com.test.one", "1.0.0");
        let first = registry.scan().unwrap();
        assert_eq!(first.inserted, 1);
        let cached = registry.cached_plugins().unwrap();
        assert_eq!(cached[0].manifest.id, "com.test.one");
        assert_eq!(cached[0].package, cached[0].dir.join(PACKAGE_FILE));

        write_plugin(registry.plugins_root(), "com.test.one", "1.1.0");
        assert_eq!(registry.scan().unwrap().updated, 1);
        assert_eq!(
            registry.cached_plugins().unwrap()[0].manifest.version,
            "1.1.0"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_package_is_not_cached() {
        let (dir, mut registry) = temp_registry();
        let plugin = registry.plugins_root().join("com.test.missing");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join(MANIFEST_FILE),
            r#"{
              "schemaVersion": 2,
              "id": "com.test.missing",
              "name": "Missing",
              "version": "1.0.0",
              "package": "plugin.nshpkg",
              "commands": [{"name":"run","title":"Run","trigger":{"type":"command"}}]
            }"#,
        )
        .unwrap();
        let report = registry.scan().unwrap();
        assert_eq!(report.plugins, 0);
        assert_eq!(report.failed.len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }
}

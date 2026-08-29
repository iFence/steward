//! Per-plugin local key/value storage (M3).
//!
//! Each plugin gets its own JSON file under the OS data directory
//! (`<data_dir>/Steward/plugin-storage/<plugin_id>.json`), so state survives
//! runtime restarts and is isolated between plugins. It is deliberately
//! permission-free (the plugin can only touch its own file) and behaves like
//! a small `localStorage`: `get`/`set`/`remove`/`clear` on string keys.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};

/// Sub-directory (under the Steward data directory) holding per-plugin KV
/// files.
const STORAGE_DIR: &str = "plugin-storage";

/// One plugin's key/value map, backed by a single JSON file.
pub struct PluginStorage {
    map: HashMap<String, String>,
    path: PathBuf,
}

impl PluginStorage {
    /// Create (or load) storage for `plugin_id`. The file is created lazily
    /// on the first write.
    pub fn load(plugin_id: &str) -> Result<Self> {
        // An explicit override wins (used by headless/sandboxed runs and the
        // host), so tests never depend on the OS data directory being writable.
        if let Ok(dir) = std::env::var("STEWARD_DATA_DIR") {
            return Self::load_at(PathBuf::from(dir).as_path(), plugin_id);
        }
        // Prefer the OS data directory; fall back to the temp dir when it is
        // unavailable (e.g. sandboxed tests) so every isolate can always open
        // its backing file.
        if let Some(base) = dirs::data_dir() {
            if let Ok(storage) = Self::load_at(&base.join("Steward"), plugin_id) {
                return Ok(storage);
            }
        }
        let base = std::env::temp_dir().join("steward-app");
        Self::load_at(&base, plugin_id)
    }

    /// Create storage under an explicit data directory (tests / custom install).
    pub fn load_at(data_dir: &Path, plugin_id: &str) -> Result<Self> {
        let dir = data_dir.join(STORAGE_DIR);
        std::fs::create_dir_all(&dir).context("create plugin storage directory")?;
        let safe_id = plugin_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let path = dir.join(format!("{safe_id}.json"));
        let map = if path.is_file() {
            let text = std::fs::read_to_string(&path).context("read plugin storage")?;
            serde_json::from_str::<HashMap<String, String>>(&text).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self { map, path })
    }

    /// Read one key (`None` when unset).
    pub fn get(&self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }

    /// Set one key and persist.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        self.map.insert(key.to_string(), value.to_string());
        self.save()
    }

    /// Remove one key and persist.
    pub fn remove(&mut self, key: &str) -> Result<()> {
        self.map.remove(key);
        self.save()
    }

    /// Remove every key and persist.
    pub fn clear(&mut self) -> Result<()> {
        self.map.clear();
        self.save()
    }

    fn save(&self) -> Result<()> {
        let text = serde_json::to_string(&self.map).context("serialize plugin storage")?;
        std::fs::write(&self.path, text).context("write plugin storage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_round_trips_and_persists() {
        let data_dir = std::env::temp_dir().join(format!(
            "steward-plugin-storage-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut storage = PluginStorage::load_at(&data_dir, "com.example.pinned").unwrap();
        assert_eq!(storage.get("a"), None);
        storage.set("a", "1").unwrap();
        storage.set("b", "hello").unwrap();
        assert_eq!(storage.get("a").as_deref(), Some("1"));

        // Reopen on the same directory: the file persisted.
        let reopened = PluginStorage::load_at(&data_dir, "com.example.pinned").unwrap();
        assert_eq!(reopened.get("b").as_deref(), Some("hello"));

        storage.remove("a").unwrap();
        assert_eq!(storage.get("a"), None);
        storage.clear().unwrap();
        assert_eq!(reopened.get("b").as_deref(), Some("hello"));
        std::fs::remove_dir_all(&data_dir).unwrap();
    }
}

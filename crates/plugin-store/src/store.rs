use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use semver::Version;
use steward_plugin_registry::manifest::load_manifest;
use steward_plugin_registry::Registry;

use crate::archive::{write_distribution, PreparedPackage};
use crate::{
    atomic_write, remove_directory_under, unique_suffix, unix_seconds, HistoryVersion,
    InstalledPlugin, InstalledRecord, PackageInspection, PluginOperation, PluginOperationKind,
    PluginStore, StoreState, MAX_HISTORY_VERSIONS, STORE_SCHEMA_VERSION,
};

pub(crate) fn install(
    store: &PluginStore,
    prepared: PreparedPackage,
    require_update: bool,
) -> Result<PluginOperation> {
    let inspection = store.inspect_prepared(&prepared)?;
    ensure_installable(&inspection)?;
    let plugin_id = inspection.app_id.clone();
    ensure_plugin_id(&plugin_id)?;

    let active = store.plugins_root().join(&plugin_id);
    if require_update && !active.exists() {
        anyhow::bail!("plugin '{plugin_id}' is not installed; use install instead of update");
    }
    let previous = if active.exists() {
        if !active.is_dir() {
            anyhow::bail!(
                "installed plugin path is not a directory: {}",
                active.display()
            );
        }
        let previous_inspection = store.inspect(&active)?;
        let previous_version = Version::parse(&previous_inspection.version)
            .context("parse installed plugin version")?;
        let incoming_version =
            Version::parse(&inspection.version).context("parse incoming plugin version")?;
        if incoming_version <= previous_version {
            anyhow::bail!(
                "plugin '{}' version {} is not newer than installed version {}",
                plugin_id,
                inspection.version,
                previous_inspection.version
            );
        }
        if inspection.build <= previous_inspection.build {
            anyhow::bail!(
                "plugin '{}' build {} must be greater than installed build {}",
                plugin_id,
                inspection.build,
                previous_inspection.build
            );
        }
        if inspection.publisher_key_id != previous_inspection.publisher_key_id {
            anyhow::bail!(
                "plugin '{}' publisher changed from '{}' to '{}'; removal is required before changing publishers",
                plugin_id,
                previous_inspection.publisher_key_id.as_deref().unwrap_or("unknown"),
                inspection.publisher_key_id.as_deref().unwrap_or("unknown")
            );
        }
        Some(previous_inspection)
    } else {
        None
    };

    let stage = store
        .data_dir()
        .join(".plugin-staging")
        .join(format!("{plugin_id}-{}", unique_suffix()));
    write_distribution(&stage, &prepared)?;

    let mut state = load_state(store)?;
    let record = state.plugins.entry(plugin_id.clone()).or_default();
    let history_destination = previous.as_ref().map(|previous| {
        history_path(
            store,
            &plugin_id,
            &previous.version,
            previous.build,
            &previous.package_sha256,
        )
    });

    let moved_previous = match &history_destination {
        Some(destination) => {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::rename(&active, destination)
                .with_context(|| format!("move {} to history", active.display()))?;
            true
        }
        None => false,
    };
    if let Err(error) = std::fs::rename(&stage, &active) {
        if let Some(destination) = &history_destination {
            if moved_previous {
                let _ = std::fs::rename(destination, &active);
            }
        }
        let _ = remove_directory_under(&store.data_dir().join(".plugin-staging"), &stage);
        return Err(error)
            .with_context(|| format!("activate staged plugin at {}", active.display()));
    }

    if let (Some(previous), Some(destination)) = (previous.as_ref(), history_destination.as_ref()) {
        let old = version_record(previous, destination);
        record
            .history
            .retain(|entry| entry.directory != old.directory);
        record.history.insert(0, old);
    }
    let new_record = version_record(&inspection, &active);
    record.current = Some(new_record);
    let pruned = prune_history(record);
    if let Err(error) = save_state(store, &state) {
        let failed = store
            .data_dir()
            .join(".plugin-staging")
            .join(format!("failed-{plugin_id}-{}", unique_suffix()));
        let moved = std::fs::rename(&active, &failed).is_ok();
        if moved {
            if let Some(destination) = &history_destination {
                if moved_previous {
                    let _ = std::fs::rename(destination, &active);
                }
            }
            let _ = std::fs::remove_dir_all(&failed);
        }
        return Err(error);
    }
    cleanup_pruned(store, &pruned);
    store.refresh_route_cache()?;

    Ok(PluginOperation {
        action: if moved_previous {
            PluginOperationKind::Update
        } else {
            PluginOperationKind::Install
        },
        plugin_id,
        name: inspection.name,
        version: inspection.version,
        publisher_key_id: inspection.publisher_key_id.unwrap_or_default(),
        package_sha256: inspection.package_sha256,
        previous_version: previous.map(|inspection| inspection.version),
    })
}

pub(crate) fn list_installed(store: &PluginStore) -> Vec<InstalledPlugin> {
    let state = load_state(store).unwrap_or_default();
    let mut plugins = Vec::new();
    let Ok(entries) = std::fs::read_dir(store.plugins_root()) else {
        return plugins;
    };
    for entry in entries.flatten() {
        let directory = entry.path();
        if !directory.is_dir() {
            continue;
        }
        let manifest = match load_manifest(&directory) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        let package_path = directory.join(steward_plugin_registry::PACKAGE_FILE);
        let (inspection, error) = match store.inspect(&directory) {
            Ok(inspection) => (Some(inspection), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        plugins.push(InstalledPlugin {
            manifest: manifest.clone(),
            directory,
            package_path,
            inspection,
            error,
            history: state
                .plugins
                .get(&manifest.id)
                .map(|record| record.history.clone())
                .unwrap_or_default(),
        });
    }
    plugins.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
    plugins
}

pub(crate) fn history(store: &PluginStore, plugin_id: &str) -> Result<Vec<HistoryVersion>> {
    ensure_plugin_id(plugin_id)?;
    Ok(load_state(store)?
        .plugins
        .get(plugin_id)
        .map(|record| record.history.clone())
        .unwrap_or_default())
}

pub(crate) fn remove(store: &PluginStore, plugin_id: &str) -> Result<PluginOperation> {
    ensure_plugin_id(plugin_id)?;
    let active = store.plugins_root().join(plugin_id);
    if !active.exists() {
        anyhow::bail!("plugin '{plugin_id}' is not installed");
    }
    let inspection = store.inspect(&active)?;
    let destination = history_path(
        store,
        plugin_id,
        &inspection.version,
        inspection.build,
        &inspection.package_sha256,
    );
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::rename(&active, &destination)
        .with_context(|| format!("move {} to history", active.display()))?;

    let mut state = load_state(store)?;
    let record = state.plugins.entry(plugin_id.to_owned()).or_default();
    let removed = version_record(&inspection, &destination);
    record
        .history
        .retain(|entry| entry.directory != removed.directory);
    record.history.insert(0, removed);
    record.current = None;
    let pruned = prune_history(record);
    if let Err(error) = save_state(store, &state) {
        let _ = std::fs::rename(&destination, &active);
        return Err(error);
    }
    cleanup_pruned(store, &pruned);
    store.refresh_route_cache()?;
    Ok(PluginOperation {
        action: PluginOperationKind::Remove,
        plugin_id: plugin_id.to_owned(),
        name: inspection.name,
        version: inspection.version,
        publisher_key_id: inspection.publisher_key_id.unwrap_or_default(),
        package_sha256: inspection.package_sha256,
        previous_version: None,
    })
}

pub(crate) fn rollback(
    store: &PluginStore,
    plugin_id: &str,
    requested_version: Option<&str>,
) -> Result<PluginOperation> {
    ensure_plugin_id(plugin_id)?;
    let active = store.plugins_root().join(plugin_id);
    if !active.is_dir() {
        anyhow::bail!("plugin '{plugin_id}' is not installed");
    }
    let current_inspection = store.inspect(&active)?;
    let mut state = load_state(store)?;
    let record = state
        .plugins
        .get(plugin_id)
        .cloned()
        .context("plugin has no managed version history")?;
    let selected_index = match requested_version {
        Some(version) => record
            .history
            .iter()
            .position(|entry| entry.version == version)
            .with_context(|| format!("plugin '{plugin_id}' has no rollback version {version}"))?,
        None => record
            .history
            .first()
            .map(|_| 0)
            .context("plugin has no rollback version")?,
    };
    let selected = record.history[selected_index].clone();
    let source = path_from_record(store, &selected)?;
    let selected_inspection = store.inspect(&source)?;
    ensure_installable(&selected_inspection)?;

    let parked = history_path(
        store,
        plugin_id,
        &current_inspection.version,
        current_inspection.build,
        &current_inspection.package_sha256,
    );
    if let Some(parent) = parked.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::rename(&active, &parked)
        .with_context(|| format!("park current plugin at {}", parked.display()))?;
    if let Err(error) = std::fs::rename(&source, &active) {
        let _ = std::fs::rename(&parked, &active);
        return Err(error)
            .with_context(|| format!("restore rollback package to {}", active.display()));
    }

    let record = state.plugins.entry(plugin_id.to_owned()).or_default();
    record.current = Some(selected.clone());
    record.history.remove(selected_index);
    let parked_record = version_record(&current_inspection, &parked);
    record
        .history
        .retain(|entry| entry.directory != parked_record.directory);
    record.history.insert(0, parked_record);
    let pruned = prune_history(record);
    if let Err(error) = save_state(store, &state) {
        let _ = std::fs::rename(&active, &source);
        let _ = std::fs::rename(&parked, &active);
        return Err(error);
    }
    cleanup_pruned(store, &pruned);
    store.refresh_route_cache()?;
    Ok(PluginOperation {
        action: PluginOperationKind::Rollback,
        plugin_id: plugin_id.to_owned(),
        name: selected_inspection.name,
        version: selected_inspection.version,
        publisher_key_id: selected_inspection.publisher_key_id.unwrap_or_default(),
        package_sha256: selected_inspection.package_sha256,
        previous_version: Some(current_inspection.version),
    })
}

pub(crate) fn refresh_route_cache(store: &PluginStore) -> Result<()> {
    let mut registry = Registry::open_with_root(store.data_dir(), store.plugins_root())?;
    registry.scan()?;
    Ok(())
}

fn ensure_installable(inspection: &PackageInspection) -> Result<()> {
    if !inspection.signature_valid {
        anyhow::bail!(
            "plugin package signature is invalid: {}",
            inspection
                .signature_error
                .as_deref()
                .unwrap_or("unknown signature error")
        );
    }
    if !inspection.trusted {
        anyhow::bail!(
            "plugin package is not trusted: {}",
            inspection
                .trust_error
                .as_deref()
                .unwrap_or("no configured trust root accepted it")
        );
    }
    if !inspection.policy.errors.is_empty() {
        anyhow::bail!(
            "plugin package failed policy checks: {}",
            inspection.policy.errors.join("; ")
        );
    }
    Ok(())
}

fn ensure_plugin_id(plugin_id: &str) -> Result<()> {
    if !crate::safe_relative_path(Path::new(plugin_id)) || matches!(plugin_id, "." | "..") {
        anyhow::bail!("invalid plugin id '{plugin_id}'");
    }
    Ok(())
}

fn history_path(
    store: &PluginStore,
    plugin_id: &str,
    version: &str,
    build: u64,
    package_sha256: &str,
) -> PathBuf {
    let suffix = package_sha256.chars().take(8).collect::<String>();
    store.history_root().join(plugin_id).join(format!(
        "{}-{}-{build}-{suffix}-{}",
        unix_seconds(),
        version,
        unique_suffix()
    ))
}

fn version_record(inspection: &PackageInspection, directory: &Path) -> HistoryVersion {
    HistoryVersion {
        version: inspection.version.clone(),
        build: inspection.build,
        package_sha256: inspection.package_sha256.clone(),
        publisher_key_id: inspection.publisher_key_id.clone().unwrap_or_default(),
        installed_at: unix_seconds(),
        directory: directory.to_string_lossy().into_owned(),
    }
}

fn path_from_record(store: &PluginStore, record: &HistoryVersion) -> Result<PathBuf> {
    let path = PathBuf::from(&record.directory);
    let root = store.history_root();
    if !path.starts_with(&root) {
        anyhow::bail!("plugin history path escapes the history directory");
    }
    if !path.is_dir() {
        anyhow::bail!("plugin history directory is missing: {}", path.display());
    }
    Ok(path)
}

fn prune_history(record: &mut InstalledRecord) -> Vec<PathBuf> {
    record
        .history
        .sort_by_key(|entry| std::cmp::Reverse(entry.installed_at));
    if record.history.len() <= MAX_HISTORY_VERSIONS {
        return Vec::new();
    }
    record
        .history
        .split_off(MAX_HISTORY_VERSIONS)
        .into_iter()
        .map(|entry| PathBuf::from(entry.directory))
        .collect()
}

fn cleanup_pruned(store: &PluginStore, paths: &[PathBuf]) {
    for path in paths {
        if !path.is_dir() || !path.starts_with(store.history_root()) {
            continue;
        }
        if let Some(parent) = path.parent() {
            let _ = remove_directory_under(parent, path);
        }
    }
}

fn load_state(store: &PluginStore) -> Result<StoreState> {
    let path = store.store_state_path();
    if !path.exists() {
        return Ok(StoreState::default());
    }
    let state: StoreState = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    if state.schema_version > STORE_SCHEMA_VERSION {
        anyhow::bail!(
            "plugin store schema {} is newer than supported schema {}",
            state.schema_version,
            STORE_SCHEMA_VERSION
        );
    }
    Ok(state)
}

fn save_state(store: &PluginStore, state: &StoreState) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state).context("serialize plugin store state")?;
    atomic_write(&store.store_state_path(), &bytes)
}

//! Background bridge between the settings surface and the local plugin store.

use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use semver::{Version, VersionReq};
use serde_json::Value;
use steward_plugin_contract::capabilities::{
    CALENDAR_HOLIDAYS, CLIPBOARD_HISTORY, CLIPBOARD_WRITE, EXEC_CANCEL, EXEC_RUN, FEEDBACK_CONFIRM,
    FEEDBACK_PROGRESS, FEEDBACK_TOAST, FS_CANCEL_WALK, FS_GRANTS, FS_PICK, FS_READ, FS_READ_TEXT,
    FS_WALK, FS_WRITE, FS_WRITE_TEXT, NET_FETCH, PREFERENCES_ALL, PREFERENCES_GET, STORAGE_GET,
    STORAGE_SET, SYSTEM_APPLICATIONS, SYSTEM_OPEN_PATH, SYSTEM_OPEN_URL, SYSTEM_OPEN_WITH,
    SYSTEM_REVEAL_PATH, WINDOW_CLOSE,
};
use steward_plugin_contract::CapabilityDescriptor;
use steward_plugin_store::{MarketplaceIndex, PluginCatalog, PluginOperationKind, PluginStore};
use steward_storage::{NewCapabilityGrant, Storage};

#[derive(Debug, Clone)]
pub(crate) struct PluginCapabilitySnapshot {
    pub(crate) descriptor: CapabilityDescriptor,
    pub(crate) grant_ids: Vec<i64>,
    pub(crate) automatic: bool,
    pub(crate) legacy_compatible: bool,
    pub(crate) release_allowed: bool,
}

impl PluginCapabilitySnapshot {
    pub(crate) fn allowed(&self) -> bool {
        self.release_allowed
            && (self.automatic || self.legacy_compatible || !self.grant_ids.is_empty())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PluginManagementSnapshot {
    pub(crate) busy: bool,
    pub(crate) catalog: PluginCatalog,
    pub(crate) capabilities: BTreeMap<String, Vec<PluginCapabilitySnapshot>>,
    pub(crate) preferences: BTreeMap<String, BTreeMap<String, Value>>,
    pub(crate) message: Option<String>,
    pub(crate) error: Option<String>,
}

impl Default for PluginManagementSnapshot {
    fn default() -> Self {
        Self {
            busy: false,
            catalog: PluginCatalog {
                installed: Vec::new(),
                marketplace: MarketplaceIndex::default(),
                marketplace_errors: Vec::new(),
                revocations: None,
            },
            capabilities: BTreeMap::new(),
            preferences: BTreeMap::new(),
            message: None,
            error: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PluginManagementPort {
    state: Arc<RwLock<PluginManagementSnapshot>>,
    revision: Arc<AtomicU64>,
    commands: Option<crossbeam_channel::Sender<Command>>,
}

impl Default for PluginManagementPort {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(PluginManagementSnapshot::default())),
            revision: Arc::new(AtomicU64::new(0)),
            commands: None,
        }
    }
}

impl PluginManagementPort {
    pub(crate) fn spawn(store: PluginStore) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let port = Self {
            state: Arc::new(RwLock::new(PluginManagementSnapshot::default())),
            revision: Arc::new(AtomicU64::new(0)),
            commands: Some(tx.clone()),
        };
        let state = port.state.clone();
        let revision = port.revision.clone();
        std::thread::spawn(move || {
            let storage = Storage::open_at(store.data_dir()).ok();
            while let Ok(command) = rx.recv() {
                set_busy(&state, &revision, true, None, None);
                let outcome = match command {
                    Command::Refresh => Ok(None),
                    Command::SyncMarketplace => store.sync_marketplace().map(|report| {
                        Some(format!("Indexed {} plugin package(s).", report.entries))
                    }),
                    Command::Install(plugin_id) => store
                        .install_from_marketplace(&plugin_id)
                        .map(|operation| Some(operation_message(operation))),
                    Command::Update(plugin_id) => store
                        .update_from_marketplace(&plugin_id)
                        .map(|operation| Some(operation_message(operation))),
                    Command::Remove(plugin_id) => store
                        .remove(&plugin_id)
                        .map(|operation| Some(operation_message(operation))),
                    Command::Rollback(plugin_id) => store
                        .rollback(&plugin_id, None)
                        .map(|operation| Some(operation_message(operation))),
                    Command::GrantCapability {
                        plugin_id,
                        capability,
                        scope,
                    } => storage
                        .as_ref()
                        .context("capability storage is unavailable")
                        .and_then(|storage| {
                            grant_capability(&store, storage, &plugin_id, &capability, &scope)
                        })
                        .map(Some),
                    Command::RevokeCapability {
                        plugin_id,
                        capability,
                        scope,
                    } => storage
                        .as_ref()
                        .context("capability storage is unavailable")
                        .and_then(|storage| {
                            revoke_capability(&store, storage, &plugin_id, &capability, &scope)
                        })
                        .map(Some),
                    Command::SetPreference {
                        plugin_id,
                        key,
                        value,
                    } => storage
                        .as_ref()
                        .context("preference storage is unavailable")
                        .and_then(|storage| {
                            set_preference(&store, storage, &plugin_id, &key, &value)
                        }),
                };
                let (message, error) = match outcome {
                    Ok(message) => (message, None),
                    Err(error) => (None, Some(format!("{error:#}"))),
                };
                let catalog = store.catalog();
                let capabilities = storage
                    .as_ref()
                    .map(|storage| capability_snapshots(&catalog, storage))
                    .unwrap_or_default();
                let preferences = storage
                    .as_ref()
                    .map(|storage| preference_snapshots(&catalog, storage))
                    .unwrap_or_default();
                if let Ok(mut current) = state.write() {
                    *current = PluginManagementSnapshot {
                        busy: false,
                        catalog,
                        capabilities,
                        preferences,
                        message,
                        error,
                    };
                }
                revision.fetch_add(1, Ordering::Release);
            }
        });
        port
    }

    pub(crate) fn snapshot(&self) -> PluginManagementSnapshot {
        self.state
            .read()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(crate) fn refresh(&self) {
        self.send(Command::Refresh);
    }

    pub(crate) fn sync_marketplace(&self) {
        self.send(Command::SyncMarketplace);
    }

    pub(crate) fn install(&self, plugin_id: impl Into<String>) {
        self.send(Command::Install(plugin_id.into()));
    }

    pub(crate) fn update(&self, plugin_id: impl Into<String>) {
        self.send(Command::Update(plugin_id.into()));
    }

    pub(crate) fn remove(&self, plugin_id: impl Into<String>) {
        self.send(Command::Remove(plugin_id.into()));
    }

    pub(crate) fn rollback(&self, plugin_id: impl Into<String>) {
        self.send(Command::Rollback(plugin_id.into()));
    }

    pub(crate) fn grant_capability(
        &self,
        plugin_id: impl Into<String>,
        capability: impl Into<String>,
        scope: impl Into<String>,
    ) {
        self.send(Command::GrantCapability {
            plugin_id: plugin_id.into(),
            capability: capability.into(),
            scope: scope.into(),
        });
    }

    pub(crate) fn revoke_capability(
        &self,
        plugin_id: impl Into<String>,
        capability: impl Into<String>,
        scope: impl Into<String>,
    ) {
        self.send(Command::RevokeCapability {
            plugin_id: plugin_id.into(),
            capability: capability.into(),
            scope: scope.into(),
        });
    }

    pub(crate) fn set_preference(
        &self,
        plugin_id: impl Into<String>,
        key: impl Into<String>,
        value: Value,
    ) {
        let plugin_id = plugin_id.into();
        let key = key.into();
        let validation = {
            let current = self.snapshot();
            current
                .catalog
                .installed
                .iter()
                .find(|plugin| plugin.manifest.id == plugin_id)
                .ok_or_else(|| format!("plugin '{plugin_id}' is not installed"))
                .and_then(|plugin| {
                    plugin
                        .manifest
                        .preferences
                        .validate_value(&key, &value)
                        .map_err(|error| error.to_string())
                })
        };
        if let Err(error) = validation {
            if let Ok(mut current) = self.state.write() {
                if current
                    .catalog
                    .installed
                    .iter()
                    .any(|plugin| plugin.manifest.id == plugin_id)
                {
                    // Keep an invalid intermediate value as a UI draft. It is
                    // never sent to the plugin or persisted until it validates.
                    current
                        .preferences
                        .entry(plugin_id.clone())
                        .or_default()
                        .insert(key, value);
                }
                current.error = Some(error);
            }
            self.revision.fetch_add(1, Ordering::Release);
            return;
        }
        if let Ok(mut current) = self.state.write() {
            current
                .preferences
                .entry(plugin_id.clone())
                .or_default()
                .insert(key.clone(), value.clone());
            current.error = None;
        }
        self.revision.fetch_add(1, Ordering::Release);
        self.send(Command::SetPreference {
            plugin_id,
            key,
            value,
        });
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }
}

enum Command {
    Refresh,
    SyncMarketplace,
    Install(String),
    Update(String),
    Remove(String),
    Rollback(String),
    GrantCapability {
        plugin_id: String,
        capability: String,
        scope: String,
    },
    RevokeCapability {
        plugin_id: String,
        capability: String,
        scope: String,
    },
    SetPreference {
        plugin_id: String,
        key: String,
        value: Value,
    },
}

fn capability_snapshots(
    catalog: &PluginCatalog,
    storage: &Storage,
) -> BTreeMap<String, Vec<PluginCapabilitySnapshot>> {
    let mut snapshots = BTreeMap::new();
    for plugin in &catalog.installed {
        let Some(inspection) = &plugin.inspection else {
            continue;
        };
        let matching = inspection
            .publisher_key_id
            .as_deref()
            .and_then(|publisher| {
                storage
                    .matching_capability_grants(&inspection.app_id, publisher, &inspection.version)
                    .ok()
            })
            .unwrap_or_default();
        let mut capabilities = inspection
            .declared_capabilities
            .iter()
            .map(|descriptor| PluginCapabilitySnapshot {
                descriptor: descriptor.clone(),
                grant_ids: matching
                    .iter()
                    .filter(|grant| {
                        grant.capability == descriptor.permission && grant.scope == descriptor.scope
                    })
                    .map(|grant| grant.id)
                    .collect(),
                automatic: !descriptor.sensitive,
                legacy_compatible: inspection.legacy_compatibility,
                release_allowed: release_allows_capability(descriptor),
            })
            .collect::<Vec<_>>();
        for grant in &matching {
            if !matches!(grant.capability.as_str(), FS_READ | FS_WRITE)
                || !grant.scope.starts_with("path=")
                || capabilities.iter().any(|capability| {
                    capability.descriptor.permission == grant.capability
                        && capability.descriptor.scope == grant.scope
                })
            {
                continue;
            }
            let descriptor =
                CapabilityDescriptor::new(grant.capability.clone(), grant.scope.clone());
            capabilities.push(PluginCapabilitySnapshot {
                release_allowed: release_allows_capability(&descriptor),
                descriptor,
                grant_ids: vec![grant.id],
                automatic: false,
                legacy_compatible: false,
            });
        }
        capabilities.sort_by(|left, right| left.descriptor.cmp(&right.descriptor));
        snapshots.insert(plugin.manifest.id.clone(), capabilities);
    }
    snapshots
}

fn preference_snapshots(
    catalog: &PluginCatalog,
    storage: &Storage,
) -> BTreeMap<String, BTreeMap<String, Value>> {
    catalog
        .installed
        .iter()
        .filter_map(|plugin| {
            if plugin.manifest.preferences.is_empty() {
                return None;
            }
            let stored = storage
                .plugin_preferences(&plugin.manifest.id)
                .unwrap_or_default();
            Some((
                plugin.manifest.id.clone(),
                plugin.manifest.preferences.resolve_values(&stored),
            ))
        })
        .collect()
}

fn set_preference(
    store: &PluginStore,
    storage: &Storage,
    plugin_id: &str,
    key: &str,
    value: &Value,
) -> Result<Option<String>> {
    let catalog = store.catalog();
    let plugin = catalog
        .installed
        .iter()
        .find(|plugin| plugin.manifest.id == plugin_id)
        .with_context(|| format!("plugin '{plugin_id}' is not installed"))?;
    plugin
        .manifest
        .preferences
        .validate_value(key, value)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    storage.set_plugin_preference(plugin_id, key, value, unix_seconds())?;
    Ok(None)
}

fn release_allows_capability(descriptor: &CapabilityDescriptor) -> bool {
    match descriptor.permission.as_str() {
        CLIPBOARD_HISTORY | CLIPBOARD_WRITE | STORAGE_GET | STORAGE_SET | CALENDAR_HOLIDAYS
        | WINDOW_CLOSE | PREFERENCES_GET | PREFERENCES_ALL | FEEDBACK_TOAST | FEEDBACK_CONFIRM
        | FEEDBACK_PROGRESS | FS_PICK | FS_WALK | FS_READ_TEXT | FS_WRITE_TEXT | FS_GRANTS
        | FS_CANCEL_WALK | SYSTEM_OPEN_PATH | SYSTEM_REVEAL_PATH | SYSTEM_OPEN_WITH
        | SYSTEM_APPLICATIONS | EXEC_CANCEL => true,
        FS_READ | FS_WRITE => {
            descriptor.scope == "path=${dataDir}" || descriptor.scope.starts_with("path=")
        }
        NET_FETCH => {
            let methods = descriptor
                .scope
                .split(';')
                .find_map(|part| part.strip_prefix("methods="))
                .unwrap_or_default();
            descriptor.scope == "host=steward"
                || (descriptor.scope.contains("schemes=https")
                    && !descriptor.scope.contains("schemes=http,")
                    && !descriptor.scope.contains(",http")
                    && !methods.is_empty()
                    && methods.split(',').all(|method| {
                        matches!(method, "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE")
                    })
                    && (descriptor.scope.ends_with("port=")
                        || descriptor.scope.ends_with("port=443")))
        }
        EXEC_RUN => {
            descriptor.scope == "host=steward"
                || descriptor
                    .scope
                    .strip_prefix("executable=")
                    .is_some_and(controlled_executable_allowed)
        }
        SYSTEM_OPEN_URL => matches!(
            descriptor.scope.strip_prefix("scheme=").unwrap_or_default(),
            "https" | "mailto"
        ),
        "steward.system.clipboard" => true,
        _ => false,
    }
}

fn controlled_executable_allowed(executable: &str) -> bool {
    let executable = executable.trim();
    if !executable.to_ascii_lowercase().ends_with(".exe") {
        return false;
    }
    let name = std::path::Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    !matches!(
        name.as_str(),
        "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "wsl.exe"
            | "bash.exe"
            | "sh.exe"
            | "cscript.exe"
            | "wscript.exe"
            | "mshta.exe"
            | "rundll32.exe"
            | "python.exe"
            | "python3.exe"
            | "py.exe"
            | "node.exe"
            | "nodejs.exe"
            | "deno.exe"
            | "bun.exe"
            | "perl.exe"
            | "ruby.exe"
            | "java.exe"
            | "javaw.exe"
            | "dotnet.exe"
            | "powershell_ise.exe"
            | "wt.exe"
            | "conhost.exe"
    )
}

fn grant_capability(
    store: &PluginStore,
    storage: &Storage,
    plugin_id: &str,
    capability: &str,
    scope: &str,
) -> Result<String> {
    let plugin = store
        .list_installed()
        .into_iter()
        .find(|plugin| plugin.manifest.id == plugin_id)
        .with_context(|| format!("plugin '{plugin_id}' is not installed"))?;
    let inspection = plugin
        .inspection
        .with_context(|| format!("plugin '{plugin_id}' failed package inspection"))?;
    grant_declared_capability(storage, plugin_id, &inspection, capability, scope)
}

fn grant_declared_capability(
    storage: &Storage,
    plugin_id: &str,
    inspection: &steward_plugin_store::PackageInspection,
    capability: &str,
    scope: &str,
) -> Result<String> {
    if !inspection.installable() {
        anyhow::bail!("plugin '{plugin_id}' does not pass current trust and policy checks");
    }
    let descriptor = inspection
        .declared_capabilities
        .iter()
        .find(|descriptor| descriptor.permission == capability && descriptor.scope == scope)
        .with_context(|| format!("plugin '{plugin_id}' did not declare '{capability}'"))?;
    if !release_allows_capability(descriptor) {
        anyhow::bail!("'{capability}' is not available under the current host policy");
    }
    if !descriptor.sensitive {
        anyhow::bail!("'{capability}' is granted automatically and cannot be revoked");
    }
    if inspection.legacy_compatibility {
        anyhow::bail!("'{capability}' is covered by legacy official compatibility");
    }
    let publisher = inspection
        .publisher_key_id
        .as_deref()
        .context("package has no trusted publisher identity")?
        .to_owned();
    Version::parse(&inspection.version).context("parse installed plugin version")?;
    storage.grant_capability(&NewCapabilityGrant {
        plugin_id: plugin_id.into(),
        publisher_fingerprint: publisher,
        version_range: format!("^{}", inspection.version),
        capability: capability.into(),
        scope: scope.into(),
        granted_at: unix_seconds(),
    })?;
    Ok(format!(
        "Allowed '{capability}' for {} {}.",
        inspection.name, inspection.version
    ))
}

fn revoke_capability(
    store: &PluginStore,
    storage: &Storage,
    plugin_id: &str,
    capability: &str,
    scope: &str,
) -> Result<String> {
    let plugin = store
        .list_installed()
        .into_iter()
        .find(|plugin| plugin.manifest.id == plugin_id)
        .with_context(|| format!("plugin '{plugin_id}' is not installed"))?;
    let inspection = plugin
        .inspection
        .with_context(|| format!("plugin '{plugin_id}' failed package inspection"))?;
    revoke_declared_capability(storage, plugin_id, &inspection, capability, scope)
}

fn revoke_declared_capability(
    storage: &Storage,
    plugin_id: &str,
    inspection: &steward_plugin_store::PackageInspection,
    capability: &str,
    scope: &str,
) -> Result<String> {
    let publisher = inspection
        .publisher_key_id
        .as_deref()
        .context("package has no trusted publisher identity")?;
    let version = Version::parse(&inspection.version).context("parse installed plugin version")?;
    let grants = storage
        .active_capability_grants(plugin_id)?
        .into_iter()
        .filter(|grant| {
            grant.publisher_fingerprint == publisher
                && grant.capability == capability
                && grant.scope == scope
                && VersionReq::parse(&grant.version_range)
                    .map(|requirement| requirement.matches(&version))
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    if grants.is_empty() {
        anyhow::bail!("'{capability}' does not have an active grant");
    }
    let revoked_at = unix_seconds();
    for grant in &grants {
        storage.revoke_capability(grant.id, revoked_at)?;
    }
    Ok(format!(
        "Revoked '{capability}' from {} {}. New launches will not receive it.",
        inspection.name, inspection.version
    ))
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn operation_message(operation: steward_plugin_store::PluginOperation) -> String {
    let verb = match operation.action {
        PluginOperationKind::Install => "Installed",
        PluginOperationKind::Update => "Updated",
        PluginOperationKind::Remove => "Removed",
        PluginOperationKind::Rollback => "Rolled back",
    };
    if operation.consent_required {
        format!(
            "{verb} {} {}. Review {} new sensitive permission(s); they remain denied until approved.",
            operation.name,
            operation.version,
            operation.added_capabilities.len()
        )
    } else {
        format!("{verb} {} {}.", operation.name, operation.version)
    }
}

fn set_busy(
    state: &Arc<RwLock<PluginManagementSnapshot>>,
    revision: &Arc<AtomicU64>,
    busy: bool,
    message: Option<String>,
    error: Option<String>,
) {
    if let Ok(mut current) = state.write() {
        current.busy = busy;
        current.message = message;
        current.error = error;
    }
    revision.fetch_add(1, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use steward_plugin_registry::{
        PluginCommand, PluginManifest, PluginPreferences, Trigger, TriggerType, PACKAGE_FILE,
        SCHEMA_VERSION,
    };
    use steward_plugin_store::{
        InstalledPlugin, PackageInspection, PackagePolicyReport, PluginCatalog,
    };

    fn catalog() -> PluginCatalog {
        let preferences: PluginPreferences = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "fields": [{
                "key": "workspace",
                "title": "Workspace",
                "type": "folder"
            }]
        }))
        .unwrap();
        let manifest = PluginManifest {
            schema_version: SCHEMA_VERSION,
            id: "com.example.test".into(),
            name: "Test".into(),
            version: "1.2.3".into(),
            min_host_version: Some("0.1.0".into()),
            icon: None,
            package: PACKAGE_FILE.into(),
            preferences,
            commands: vec![PluginCommand {
                name: "test".into(),
                title: "Test".into(),
                keywords: Vec::new(),
                trigger: Trigger {
                    kind: TriggerType::Command,
                    value: None,
                },
                presentation: None,
            }],
        };
        let inspection = PackageInspection {
            manifest: manifest.clone(),
            app_id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            build: 1,
            package_sha256: "abc".into(),
            publisher_key_id: Some("SHA256:publisher".into()),
            signature_valid: true,
            signature_error: None,
            trusted: true,
            trust_error: None,
            min_runtime: None,
            min_host_version: manifest.min_host_version.clone(),
            legacy_compatibility: false,
            declared_capabilities: vec![
                CapabilityDescriptor::new("steward.clipboard.write", "host=steward"),
                CapabilityDescriptor::new("steward.storage.get", "host=steward"),
                CapabilityDescriptor::new(
                    "steward.net.fetch",
                    "host=example.com;schemes=https;methods=GET;paths=;pathPrefixes=;port=",
                ),
            ],
            policy: PackagePolicyReport {
                passed: true,
                errors: Vec::new(),
                warnings: Vec::new(),
                denied_capabilities: Vec::new(),
            },
        };
        PluginCatalog {
            installed: vec![InstalledPlugin {
                manifest,
                directory: PathBuf::new(),
                package_path: PathBuf::new(),
                inspection: Some(inspection),
                error: None,
                history: Vec::new(),
            }],
            marketplace: MarketplaceIndex::default(),
            marketplace_errors: Vec::new(),
            revocations: None,
        }
    }

    #[test]
    fn capability_snapshot_separates_automatic_and_user_grants() {
        let dir = std::env::temp_dir().join(format!(
            "steward-plugin-management-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Storage::open_at(&dir).unwrap();
        storage
            .grant_capability(&NewCapabilityGrant {
                plugin_id: "com.example.test".into(),
                publisher_fingerprint: "SHA256:publisher".into(),
                version_range: "^1.2.3".into(),
                capability: "steward.clipboard.write".into(),
                scope: "host=steward".into(),
                granted_at: 1,
            })
            .unwrap();
        let capabilities = capability_snapshots(&catalog(), &storage)
            .remove("com.example.test")
            .unwrap();
        let clipboard = capabilities
            .iter()
            .find(|capability| capability.descriptor.permission == "steward.clipboard.write")
            .unwrap();
        assert!(!clipboard.automatic);
        assert!(clipboard.allowed());
        assert_eq!(clipboard.grant_ids.len(), 1);
        let storage_access = capabilities
            .iter()
            .find(|capability| capability.descriptor.permission == "steward.storage.get")
            .unwrap();
        assert!(storage_access.automatic);
        assert!(storage_access.grant_ids.is_empty());
        let network = capabilities
            .iter()
            .find(|capability| capability.descriptor.permission == "steward.net.fetch")
            .unwrap();
        assert!(network.release_allowed);
        assert!(!network.allowed());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn selected_path_grants_are_visible_and_revocable() {
        let dir = std::env::temp_dir().join(format!(
            "steward-plugin-path-grant-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Storage::open_at(&dir).unwrap();
        let grant = storage
            .grant_capability(&NewCapabilityGrant {
                plugin_id: "com.example.test".into(),
                publisher_fingerprint: "SHA256:publisher".into(),
                version_range: "^1.2.3".into(),
                capability: FS_READ.into(),
                scope: r"path=D:\projects".into(),
                granted_at: 1,
            })
            .unwrap();
        let snapshot = capability_snapshots(&catalog(), &storage)
            .remove("com.example.test")
            .unwrap()
            .into_iter()
            .find(|capability| capability.descriptor.permission == FS_READ)
            .unwrap();
        assert_eq!(snapshot.descriptor.scope, r"path=D:\projects");
        assert_eq!(snapshot.grant_ids, vec![grant.id]);
        assert!(snapshot.release_allowed);
        assert!(snapshot.allowed());

        storage.revoke_capability(grant.id, 2).unwrap();
        assert!(!capability_snapshots(&catalog(), &storage)
            .remove("com.example.test")
            .unwrap()
            .into_iter()
            .any(|capability| capability.descriptor.permission == FS_READ));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn settings_release_policy_accepts_scoped_https_and_exact_exec() {
        assert!(release_allows_capability(&CapabilityDescriptor::new(
            "steward.net.fetch",
            "host=api.example.com;schemes=https;methods=GET;paths=;pathPrefixes=/v1;port="
        )));
        assert!(release_allows_capability(&CapabilityDescriptor::new(
            "steward.exec.run",
            "executable=git.exe"
        )));
        assert!(!release_allows_capability(&CapabilityDescriptor::new(
            "steward.exec.run",
            "executable=cmd.exe"
        )));
        assert!(!release_allows_capability(&CapabilityDescriptor::new(
            "steward.net.fetch",
            "host=api.example.com;schemes=http;methods=GET;paths=;pathPrefixes=;port="
        )));
    }

    #[test]
    fn approval_and_revocation_only_accept_declared_sensitive_capabilities() {
        let dir = std::env::temp_dir().join(format!(
            "steward-plugin-capability-actions-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Storage::open_at(&dir).unwrap();
        let catalog = catalog();
        let inspection = catalog.installed[0].inspection.as_ref().unwrap();

        assert!(grant_declared_capability(
            &storage,
            "com.example.test",
            inspection,
            "steward.storage.get",
            "host=steward",
        )
        .is_err());
        assert!(grant_declared_capability(
            &storage,
            "com.example.test",
            inspection,
            "steward.clipboard.write",
            "host=other",
        )
        .is_err());
        grant_declared_capability(
            &storage,
            "com.example.test",
            inspection,
            "steward.net.fetch",
            "host=example.com;schemes=https;methods=GET;paths=;pathPrefixes=;port=",
        )
        .unwrap();
        grant_declared_capability(
            &storage,
            "com.example.test",
            inspection,
            "steward.clipboard.write",
            "host=steward",
        )
        .unwrap();
        assert_eq!(
            storage
                .active_capability_grants("com.example.test")
                .unwrap()
                .len(),
            2
        );
        revoke_declared_capability(
            &storage,
            "com.example.test",
            inspection,
            "steward.clipboard.write",
            "host=steward",
        )
        .unwrap();
        assert_eq!(
            storage
                .active_capability_grants("com.example.test")
                .unwrap()
                .len(),
            1
        );
        revoke_declared_capability(
            &storage,
            "com.example.test",
            inspection,
            "steward.net.fetch",
            "host=example.com;schemes=https;methods=GET;paths=;pathPrefixes=;port=",
        )
        .unwrap();
        assert!(storage
            .active_capability_grants("com.example.test")
            .unwrap()
            .is_empty());
        let audit = storage.capability_audit("com.example.test").unwrap();
        assert!(audit.iter().any(|entry| entry.outcome == "granted"));
        assert!(audit.iter().any(|entry| entry.outcome == "revoked"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn invalid_preference_values_remain_local_ui_drafts() {
        let port = PluginManagementPort::default();
        if let Ok(mut state) = port.state.write() {
            state.catalog = catalog();
        }
        port.set_preference(
            "com.example.test",
            "workspace",
            serde_json::json!("relative/path"),
        );
        let snapshot = port.snapshot();
        assert!(snapshot.error.is_some());
        assert_eq!(
            snapshot
                .preferences
                .get("com.example.test")
                .and_then(|values| values.get("workspace")),
            Some(&serde_json::json!("relative/path"))
        );
    }
}

//! Local Steward plugin distribution, inspection, and lifecycle management.
//!
//! Package trust is always delegated to NSH. This crate adds the product-level
//! layer around it: the two-file distribution contract, local marketplace
//! index, version history, rollback, publisher identity, and policy reporting.

mod admission;
mod archive;
mod developer;
mod doctor;
mod marketplace;
mod package;
mod revocation;
mod scaffold;
mod store;

use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use nsh_package::TrustStore;
use serde::{Deserialize, Serialize};
use steward_plugin_contract::CapabilityDescriptor;
use steward_plugin_registry::PluginManifest;

pub use admission::{
    AdmissionCapabilityGroup, AdmissionCheck, AdmissionReview, PUBLIC_ADMISSION_POLICY_VERSION,
};
pub use developer::{
    default_developer_data_dir, package_development_project, request_development_reload,
    take_development_reload_requests, DevelopmentReloadRequest,
};
pub use doctor::{DoctorCheck, DoctorReport, DoctorStage};
pub use marketplace::{
    MarketplaceEntry, MarketplaceIndex, MarketplacePublisher, MarketplaceSyncReport,
};
pub use package::{PackageInspection, PackagePolicyReport};
pub use revocation::{
    create_revocation_list, read_revocation_list, AppliedRevocations, CreatedRevocationList,
};
pub use scaffold::{scaffold_plugin, ScaffoldOptions, ScaffoldReport};

pub const STORE_SCHEMA_VERSION: u16 = 1;
pub const MARKETPLACE_SCHEMA_VERSION: u16 = 1;
pub const MAX_ARCHIVE_ENTRY_COUNT: usize = 64;
pub const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_UNPACKED_BYTES: u64 = 96 * 1024 * 1024;
pub const MAX_HISTORY_VERSIONS: usize = 5;

#[derive(Debug, Clone)]
pub struct PluginStoreConfig {
    pub data_dir: PathBuf,
    pub plugins_root: PathBuf,
    pub trust_roots: Vec<String>,
    pub developer_mode: bool,
}

impl PluginStoreConfig {
    pub fn new(
        data_dir: impl Into<PathBuf>,
        plugins_root: impl Into<PathBuf>,
        trust_roots: Vec<String>,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            plugins_root: plugins_root.into(),
            trust_roots,
            developer_mode: false,
        }
    }

    pub fn with_developer_mode(mut self, enabled: bool) -> Self {
        self.developer_mode = enabled;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryVersion {
    pub version: String,
    pub build: u64,
    pub package_sha256: String,
    pub publisher_key_id: String,
    pub installed_at: i64,
    pub directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginOperation {
    pub action: PluginOperationKind,
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub publisher_key_id: String,
    pub package_sha256: String,
    pub previous_version: Option<String>,
    pub added_capabilities: Vec<CapabilityDescriptor>,
    pub consent_required: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginOperationKind {
    Install,
    Update,
    Remove,
    Rollback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    pub directory: PathBuf,
    pub package_path: PathBuf,
    pub inspection: Option<PackageInspection>,
    pub error: Option<String>,
    pub history: Vec<HistoryVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalog {
    pub installed: Vec<InstalledPlugin>,
    pub marketplace: MarketplaceIndex,
    pub marketplace_errors: Vec<String>,
    pub revocations: Option<AppliedRevocations>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreState {
    schema_version: u16,
    plugins: std::collections::BTreeMap<String, InstalledRecord>,
}

impl Default for StoreState {
    fn default() -> Self {
        Self {
            schema_version: STORE_SCHEMA_VERSION,
            plugins: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledRecord {
    current: Option<HistoryVersion>,
    history: Vec<HistoryVersion>,
}

pub struct PluginStore {
    config: PluginStoreConfig,
    marketplace_dir: PathBuf,
}

impl PluginStore {
    pub fn open(config: PluginStoreConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir).context("create plugin data directory")?;
        std::fs::create_dir_all(&config.plugins_root).context("create plugins directory")?;
        let store = Self {
            marketplace_dir: config.data_dir.join("marketplace"),
            config,
        };
        std::fs::create_dir_all(store.marketplace_dir.join("packages"))
            .context("create local marketplace directory")?;
        Ok(store)
    }

    pub fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }

    pub fn plugins_root(&self) -> &Path {
        &self.config.plugins_root
    }

    pub fn marketplace_dir(&self) -> &Path {
        &self.marketplace_dir
    }

    pub fn developer_mode(&self) -> bool {
        self.config.developer_mode
    }

    pub fn inspect(&self, path: &Path) -> Result<PackageInspection> {
        let prepared = archive::prepare_source(path)?;
        self.inspect_prepared(&prepared)
    }

    pub fn review(&self, path: &Path) -> Result<AdmissionReview> {
        Ok(admission::review(&self.inspect(path)?))
    }

    pub fn install(&self, path: &Path) -> Result<PluginOperation> {
        store::install(
            self,
            archive::prepare_source(path)?,
            store::ActivationMode::Install,
        )
    }

    pub fn update(&self, path: &Path) -> Result<PluginOperation> {
        store::install(
            self,
            archive::prepare_source(path)?,
            store::ActivationMode::Update,
        )
    }

    /// Replace an installed development build even when its version and build
    /// number are unchanged. Signature, trust-root, publisher, and capability
    /// policy checks are identical to a normal install.
    pub fn replace_development(&self, path: &Path) -> Result<PluginOperation> {
        if !self.config.developer_mode {
            anyhow::bail!("development replacement requires developer mode; use install or update");
        }
        store::install(
            self,
            archive::prepare_source(path)?,
            store::ActivationMode::DeveloperReplace,
        )
    }

    pub fn remove(&self, plugin_id: &str) -> Result<PluginOperation> {
        store::remove(self, plugin_id)
    }

    pub fn rollback(&self, plugin_id: &str, version: Option<&str>) -> Result<PluginOperation> {
        store::rollback(self, plugin_id, version)
    }

    pub fn list_installed(&self) -> Vec<InstalledPlugin> {
        store::list_installed(self)
    }

    pub fn history(&self, plugin_id: &str) -> Result<Vec<HistoryVersion>> {
        store::history(self, plugin_id)
    }

    pub fn refresh_route_cache(&self) -> Result<()> {
        store::refresh_route_cache(self)
    }

    pub fn marketplace(&self) -> Result<MarketplaceIndex> {
        marketplace::read_index(&self.marketplace_dir)
    }

    pub fn sync_marketplace(&self) -> Result<MarketplaceSyncReport> {
        marketplace::sync(self, false)
    }

    pub fn sync_public_marketplace(&self) -> Result<MarketplaceSyncReport> {
        marketplace::sync(self, true)
    }

    pub fn install_from_marketplace(&self, plugin_id: &str) -> Result<PluginOperation> {
        let (artifact, entry) = marketplace::resolve(self, plugin_id)?;
        let operation = self.install(&artifact)?;
        if operation.plugin_id != entry.id
            || operation.version != entry.version
            || operation.publisher_key_id != entry.publisher_key_id
            || operation.package_sha256 != entry.package_sha256
        {
            anyhow::bail!("marketplace artifact identity changed during installation");
        }
        Ok(operation)
    }

    pub fn update_from_marketplace(&self, plugin_id: &str) -> Result<PluginOperation> {
        let (artifact, entry) = marketplace::resolve(self, plugin_id)?;
        let operation = self.update(&artifact)?;
        if operation.plugin_id != entry.id
            || operation.version != entry.version
            || operation.publisher_key_id != entry.publisher_key_id
            || operation.package_sha256 != entry.package_sha256
        {
            anyhow::bail!("marketplace artifact identity changed during update");
        }
        Ok(operation)
    }

    pub fn catalog(&self) -> PluginCatalog {
        let marketplace_path = marketplace::index_path(&self.marketplace_dir);
        let (marketplace, marketplace_errors) = match self.marketplace() {
            Ok(index) => (index, Vec::new()),
            Err(_error) if !marketplace_path.exists() => (MarketplaceIndex::default(), Vec::new()),
            Err(error) => (
                MarketplaceIndex::default(),
                vec![format!("{}: {error:#}", marketplace_path.display())],
            ),
        };
        PluginCatalog {
            installed: self.list_installed(),
            marketplace,
            marketplace_errors,
            revocations: self.applied_revocations().ok().flatten(),
        }
    }

    pub fn apply_revocations(&self, path: &Path) -> Result<AppliedRevocations> {
        revocation::apply(self, path)
    }

    pub fn applied_revocations(&self) -> Result<Option<AppliedRevocations>> {
        revocation::applied(self)
    }

    pub fn doctor(&self, id_or_source: &str) -> DoctorReport {
        doctor::diagnose(self, id_or_source)
    }

    pub(crate) fn trust_store(&self) -> Result<TrustStore> {
        let mut store = TrustStore::open(self.config.data_dir.join("nsh-trust"))
            .context("open NSH trust store")?;
        for root in &self.config.trust_roots {
            store.add_root(
                nsh_package::PublicKey::from_base64(root.trim())
                    .with_context(|| format!("parse NSH trust root '{}'", short_id(root)))?,
            );
        }
        Ok(store)
    }

    pub(crate) fn inspect_prepared(
        &self,
        prepared: &archive::PreparedPackage,
    ) -> Result<PackageInspection> {
        package::inspect_prepared(&self.config, prepared.clone())
    }

    pub(crate) fn require_trust_roots(&self) -> Result<()> {
        if self.config.trust_roots.is_empty() {
            anyhow::bail!("no NSH trust root is configured");
        }
        Ok(())
    }

    pub(crate) fn store_state_path(&self) -> PathBuf {
        self.config.data_dir.join("plugin-store.json")
    }

    pub(crate) fn history_root(&self) -> PathBuf {
        self.config.data_dir.join("plugin-history")
    }
}

pub(crate) fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", unique_suffix()));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("replace {}", path.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

pub(crate) fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(crate) fn remove_directory_under(root: &Path, target: &Path) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("resolve {}", root.display()))?;
    let target = target
        .canonicalize()
        .with_context(|| format!("resolve {}", target.display()))?;
    if target == root || !target.starts_with(&root) {
        anyhow::bail!(
            "refusing to remove {} outside {}",
            target.display(),
            root.display()
        );
    }
    std::fs::remove_dir_all(&target).with_context(|| format!("remove {}", target.display()))
}

fn short_id(value: &str) -> String {
    value.chars().take(12).collect()
}

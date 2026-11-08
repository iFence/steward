//! Steward's NSH plugin catalog, route index, and signed-package launcher.

mod catalog;
mod launcher;
mod policy;
mod route;
mod trust;

use std::path::PathBuf;

use anyhow::Result;
use nsh_engine::LoadedPackage;
use nsh_package::PackageReceipt;
use steward_plugin_contract::{LaunchRequest, PluginErrorCode};
use steward_plugin_registry::PluginMeta;

use catalog::PluginCatalog;
use launcher::LaunchMetadata;
use trust::PackageTrust;

pub use route::{RouteHit, RouteIndex};

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub data_dir: PathBuf,
}

impl HostConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }
}

#[derive(Debug)]
pub struct PluginLaunch {
    pub plugin_id: String,
    pub app_id: String,
    pub package: LoadedPackage,
    pub params: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginHostError {
    #[error("plugin '{0}' is not installed")]
    NotFound(String),
    #[error("plugin '{plugin_id}' has no command '{command}'")]
    CommandNotFound { plugin_id: String, command: String },
    #[error("plugin package failed verification: {0}")]
    Package(String),
    #[error("read plugin package: {0}")]
    ReadFailed(String),
    #[error("plugin package id '{actual}' does not match manifest id '{expected}'")]
    IdMismatch { expected: String, actual: String },
    #[error("plugin package version '{actual}' does not match manifest version '{expected}'")]
    VersionMismatch { expected: String, actual: String },
    #[error("no NSH trust root is configured")]
    NoTrustRoot,
    #[error("plugin launch worker exited without a result")]
    WorkerExited,
}

impl PluginHostError {
    pub fn code(&self) -> PluginErrorCode {
        match self {
            Self::NotFound(_) => PluginErrorCode::NotFound,
            Self::CommandNotFound { .. } => PluginErrorCode::CommandNotFound,
            Self::Package(_) | Self::NoTrustRoot => PluginErrorCode::TrustFailure,
            Self::ReadFailed(_) => PluginErrorCode::ReadFailed,
            Self::IdMismatch { .. } => PluginErrorCode::IdMismatch,
            Self::VersionMismatch { .. } => PluginErrorCode::VersionMismatch,
            Self::WorkerExited => PluginErrorCode::RuntimeError,
        }
    }
}

pub struct PluginHost {
    config: HostConfig,
    routes: RouteIndex,
    catalog: PluginCatalog,
    trust: PackageTrust,
}

impl PluginHost {
    pub fn new(config: HostConfig) -> Result<Self> {
        let trust = PackageTrust::new(config.data_dir.join("nsh-trust"))?;
        Ok(Self {
            config,
            routes: RouteIndex::new(),
            catalog: PluginCatalog::default(),
            trust,
        })
    }

    pub fn add_trust_root_base64(&mut self, key: &str) -> Result<()> {
        self.trust.add_root_base64(key)
    }

    pub fn set_plugins(&mut self, metas: &[PluginMeta]) {
        self.catalog.set_plugins(metas);
        self.routes.clear();
        for meta in metas {
            self.routes
                .add_plugin(&meta.manifest.id, &meta.manifest.commands);
        }
    }

    pub fn query(&self, query: &str) -> Vec<RouteHit> {
        self.routes.match_query(query)
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Verifies a launch package on a worker after validating routing metadata
    /// and trust-root availability on the caller's thread.
    pub fn start_launch(
        &self,
        request: LaunchRequest,
    ) -> std::result::Result<
        crossbeam_channel::Receiver<std::result::Result<PluginLaunch, PluginHostError>>,
        PluginHostError,
    > {
        let metadata = self.launch_metadata(&request)?;
        let roots = self.trust.roots().to_vec();
        let data_dir = self.config.data_dir.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(launcher::load(metadata, &roots, &data_dir));
        });
        Ok(rx)
    }

    pub fn accept_receipt(&mut self, receipt: &PackageReceipt) -> Result<()> {
        self.trust
            .accept_receipt(receipt)
            .map_err(|error| anyhow::anyhow!("accept NSH package: {error}"))
    }

    fn launch_metadata(
        &self,
        request: &LaunchRequest,
    ) -> std::result::Result<LaunchMetadata, PluginHostError> {
        let meta = self
            .catalog
            .get(&request.plugin_id)
            .ok_or_else(|| PluginHostError::NotFound(request.plugin_id.clone()))?;
        if !meta
            .manifest
            .commands
            .iter()
            .any(|command| command.name == request.command)
        {
            return Err(PluginHostError::CommandNotFound {
                plugin_id: request.plugin_id.clone(),
                command: request.command.clone(),
            });
        }
        if !self.trust.has_roots() {
            return Err(PluginHostError::NoTrustRoot);
        }
        Ok(LaunchMetadata {
            request: request.clone(),
            package_path: meta.package.clone(),
            expected_id: meta.manifest.id.clone(),
            expected_version: meta.manifest.version.clone(),
        })
    }
}

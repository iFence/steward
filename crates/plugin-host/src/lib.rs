//! Steward's NSH plugin catalog, route index, and signed-package launcher.

mod route;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use nsh_engine::{verify_package, Capabilities, LoadedPackage};
use nsh_package::{PublicKey, TrustStore};
use steward_plugin_registry::PluginMeta;

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
    #[error("plugin package id '{actual}' does not match manifest id '{expected}'")]
    IdMismatch { expected: String, actual: String },
    #[error("plugin package version '{actual}' does not match manifest version '{expected}'")]
    VersionMismatch { expected: String, actual: String },
    #[error("no NSH trust root is configured")]
    NoTrustRoot,
}

pub struct PluginHost {
    config: HostConfig,
    routes: RouteIndex,
    plugins: HashMap<String, PluginMeta>,
    trust: TrustStore,
}

impl PluginHost {
    pub fn new(config: HostConfig) -> Result<Self> {
        let trust = TrustStore::open(config.data_dir.join("nsh-trust"))
            .map_err(|error| anyhow::anyhow!("open NSH trust store: {error}"))?;
        Ok(Self {
            config,
            routes: RouteIndex::new(),
            plugins: HashMap::new(),
            trust,
        })
    }

    pub fn add_trust_root_base64(&mut self, key: &str) -> Result<()> {
        let key = PublicKey::from_base64(key)
            .map_err(|error| anyhow::anyhow!("invalid NSH trust root: {error}"))?;
        self.trust.add_root(key);
        Ok(())
    }

    pub fn set_plugins(&mut self, metas: &[PluginMeta]) {
        self.plugins = metas
            .iter()
            .cloned()
            .map(|meta| (meta.manifest.id.clone(), meta))
            .collect();
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

    pub fn prepare_launch(
        &self,
        plugin_id: &str,
        command: &str,
        input: &str,
        locale: &str,
        accent: &str,
        dark: bool,
    ) -> std::result::Result<PluginLaunch, PluginHostError> {
        let meta = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| PluginHostError::NotFound(plugin_id.to_owned()))?;
        let _command_meta = meta
            .manifest
            .commands
            .iter()
            .find(|candidate| candidate.name == command)
            .ok_or_else(|| PluginHostError::CommandNotFound {
                plugin_id: plugin_id.to_owned(),
                command: command.to_owned(),
            })?;
        if self.trust.roots().is_empty() {
            return Err(PluginHostError::NoTrustRoot);
        }
        let bytes = std::fs::read(&meta.package).map_err(|error| {
            PluginHostError::Package(format!("read {}: {error}", meta.package.display()))
        })?;
        let mut package = verify_package(
            &self.trust,
            &bytes,
            &self.config.data_dir.join("nsh-assets"),
        )
        .map_err(PluginHostError::Package)?;
        if package.app.id != meta.manifest.id {
            return Err(PluginHostError::IdMismatch {
                expected: meta.manifest.id.clone(),
                actual: package.app.id.clone(),
            });
        }
        if package.app.version != meta.manifest.version {
            return Err(PluginHostError::VersionMismatch {
                expected: meta.manifest.version.clone(),
                actual: package.app.version.clone(),
            });
        }
        let mut granted = Capabilities::default();
        granted.clipboard = true;
        granted.fs.read = vec!["${dataDir}".into()];
        granted.fs.write = vec!["${dataDir}".into()];
        granted.host = vec!["steward.*".into()];
        let mut open_url = nsh_core::access::OpenUrlGrant::default();
        open_url.schemes = vec!["https".into(), "mailto".into()];
        granted.open_url = Some(open_url);
        package.bundle.policy.granted = Some(granted);
        let mut params = serde_json::Map::new();
        params.insert("pluginId".into(), plugin_id.into());
        params.insert("command".into(), command.into());
        params.insert("input".into(), input.into());
        params.insert("locale".into(), locale.into());
        params.insert("accent".into(), accent.into());
        params.insert("dark".into(), dark.into());
        Ok(PluginLaunch {
            plugin_id: plugin_id.to_owned(),
            app_id: package.app.id.clone(),
            package,
            params,
        })
    }

    pub fn accept_launch(&mut self, launch: &PluginLaunch) -> Result<()> {
        self.trust
            .accept_receipt(&launch.package.receipt)
            .map_err(|error| anyhow::anyhow!("accept NSH package: {error}"))
    }
}

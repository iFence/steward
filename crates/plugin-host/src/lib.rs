//! Steward's NSH plugin catalog, route index, and signed-package launcher.

mod authorization;
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

pub use authorization::PluginAuthorization;
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
    pub preferences: std::collections::BTreeMap<String, serde_json::Value>,
    pub authorization: PluginAuthorization,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginHostError {
    #[error("plugin '{0}' is not installed")]
    NotFound(String),
    #[error("plugin '{plugin_id}' has no command '{command}'")]
    CommandNotFound { plugin_id: String, command: String },
    #[error(
        "plugin '{plugin_id}' command '{command}' requests a presentation mode that does not match its signed manifest"
    )]
    PresentationMismatch { plugin_id: String, command: String },
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
    #[error("plugin capability is not authorized: {0}")]
    PermissionDenied(String),
    #[error("plugin feature is unavailable: {0}")]
    UnsupportedFeature(String),
    #[error("plugin host service failed: {0}")]
    HostFailure(String),
    #[error("plugin launch worker exited without a result")]
    WorkerExited,
}

impl PluginHostError {
    pub fn code(&self) -> PluginErrorCode {
        match self {
            Self::NotFound(_) => PluginErrorCode::NotFound,
            Self::CommandNotFound { .. } => PluginErrorCode::CommandNotFound,
            Self::PresentationMismatch { .. } => PluginErrorCode::InvalidInput,
            Self::Package(_) | Self::NoTrustRoot => PluginErrorCode::TrustFailure,
            Self::ReadFailed(_) => PluginErrorCode::ReadFailed,
            Self::IdMismatch { .. } => PluginErrorCode::IdMismatch,
            Self::VersionMismatch { .. } => PluginErrorCode::VersionMismatch,
            Self::PermissionDenied(_) => PluginErrorCode::PermissionDenied,
            Self::UnsupportedFeature(_) => PluginErrorCode::FeatureUnavailable,
            Self::HostFailure(_) => PluginErrorCode::HostFailure,
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

    pub fn set_plugins(&mut self, metas: &[PluginMeta]) -> bool {
        if !self.catalog.set_plugins(metas) {
            return false;
        }
        self.routes.clear();
        for meta in metas {
            self.routes.add_plugin(
                &meta.manifest.id,
                meta.icon.as_deref(),
                &meta.manifest.commands,
            );
        }
        true
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

    /// Validates and loads a plugin package synchronously.
    ///
    /// The desktop shell uses [`Self::start_launch`] so package I/O stays off
    /// its UI thread. A dedicated plugin-host process calls this method on its
    /// own main thread before creating the NSH window.
    pub fn prepare_launch(
        &self,
        request: LaunchRequest,
    ) -> std::result::Result<PluginLaunch, PluginHostError> {
        let metadata = self.launch_metadata(&request)?;
        launcher::load(metadata, self.trust.roots(), &self.config.data_dir)
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
        let command = meta
            .manifest
            .commands
            .iter()
            .find(|command| command.name == request.command)
            .ok_or_else(|| PluginHostError::CommandNotFound {
                plugin_id: request.plugin_id.clone(),
                command: request.command.clone(),
            })?;
        if command.presentation() != request.presentation {
            return Err(PluginHostError::PresentationMismatch {
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
            min_host_version: meta.manifest.min_host_version.clone(),
            preferences: meta.manifest.preferences.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};
    use steward_plugin_contract::PresentationMode;
    use steward_plugin_registry::{PluginCommand, PluginManifest, PluginMeta, Registry, Trigger};

    #[test]
    fn launch_rejects_presentation_mode_that_does_not_match_signed_metadata() {
        let data_dir = std::env::temp_dir().join(format!(
            "steward-presentation-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut host = PluginHost::new(HostConfig::new(data_dir.clone())).unwrap();
        let plugin_dir = data_dir.join("plugins").join("com.example.embedded");
        host.set_plugins(&[PluginMeta {
            manifest: PluginManifest {
                schema_version: 3,
                id: "com.example.embedded".into(),
                name: "Embedded".into(),
                version: "1.0.0".into(),
                min_host_version: Some("0.5.0".into()),
                icon: None,
                package: "plugin.nshpkg".into(),
                preferences: Default::default(),
                commands: vec![PluginCommand {
                    name: "open".into(),
                    title: "Open".into(),
                    keywords: Vec::new(),
                    trigger: Trigger {
                        kind: steward_plugin_registry::TriggerType::Command,
                        value: None,
                    },
                    presentation: Some(PresentationMode::Embedded),
                }],
            },
            package: plugin_dir.join("plugin.nshpkg"),
            dir: plugin_dir,
            icon: None,
            scanned_at: 0,
        }]);
        let request = LaunchRequest {
            plugin_id: "com.example.embedded".into(),
            command: "open".into(),
            title: "Open".into(),
            input: String::new(),
            locale: "en".into(),
            accent: "#863bff".into(),
            dark: true,
            presentation: PresentationMode::Windowed,
        };

        assert!(matches!(
            host.launch_metadata(&request),
            Err(PluginHostError::PresentationMismatch { .. })
        ));
        std::fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    #[ignore = "release performance benchmark"]
    fn synthetic_plugin_route_scaling() {
        for count in [100, 500, 1_000] {
            let root = std::env::temp_dir().join(format!(
                "steward-route-bench-{count}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let data_dir = root.join("data");
            let plugins_dir = root.join("plugins");
            write_synthetic_plugins(&plugins_dir, count);

            let scan_started = Instant::now();
            let mut registry = Registry::open_with_root(&data_dir, &plugins_dir).unwrap();
            let report = registry.scan().unwrap();
            let scan_elapsed = scan_started.elapsed();
            assert_eq!(report.plugins, count);
            drop(registry);

            let cache_started = Instant::now();
            let registry = Registry::open_with_root(&data_dir, &plugins_dir).unwrap();
            let plugins = registry.cached_plugins().unwrap();
            let cache_elapsed = cache_started.elapsed();
            assert_eq!(plugins.len(), count);

            let index_started = Instant::now();
            let mut host = PluginHost::new(HostConfig::new(data_dir.clone())).unwrap();
            assert!(host.set_plugins(&plugins));
            let index_elapsed = index_started.elapsed();
            assert_eq!(host.route_count(), count);

            let exact = query_p95(&host, "cmd0");
            let prefix = query_p95(&host, "p1 item");
            let regex = query_p95(&host, "bench2");
            println!(
                "plugins={count} scan_ms={:.2} cache_ms={:.2} index_ms={:.2} \
                 exact_p95_us={:.2} prefix_p95_us={:.2} regex_p95_us={:.2}",
                millis(scan_elapsed),
                millis(cache_elapsed),
                millis(index_elapsed),
                micros(exact),
                micros(prefix),
                micros(regex),
            );
            std::fs::remove_dir_all(root).ok();
        }
    }

    fn write_synthetic_plugins(root: &Path, count: usize) {
        for index in 0..count {
            let id = format!("com.bench.plugin{index:04}");
            let dir = root.join(&id);
            std::fs::create_dir_all(&dir).unwrap();
            let trigger = match index % 3 {
                0 => serde_json::json!({ "type": "command" }),
                1 => serde_json::json!({ "type": "prefix", "value": format!("p{index} ") }),
                _ => serde_json::json!({ "type": "regex", "value": format!("^bench{index}\\b") }),
            };
            let manifest = serde_json::json!({
                "schemaVersion": 2,
                "id": id,
                "name": format!("Bench {index}"),
                "version": "1.0.0",
                "package": "plugin.nshpkg",
                "commands": [{
                    "name": format!("cmd{index}"),
                    "title": format!("Bench {index}"),
                    "trigger": trigger,
                }],
            });
            std::fs::write(
                dir.join("plugin.json"),
                serde_json::to_vec_pretty(&manifest).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.join("plugin.nshpkg"), b"synthetic route fixture").unwrap();
        }
    }

    fn query_p95(host: &PluginHost, query: &str) -> Duration {
        for _ in 0..20 {
            assert!(!host.query(query).is_empty());
        }
        let mut samples = Vec::with_capacity(200);
        for _ in 0..200 {
            let started = Instant::now();
            let hits = host.query(query);
            samples.push(started.elapsed());
            assert!(!hits.is_empty());
        }
        samples.sort_unstable();
        let index = samples
            .len()
            .saturating_mul(95)
            .div_ceil(100)
            .saturating_sub(1);
        samples[index]
    }

    fn millis(duration: Duration) -> f64 {
        duration.as_secs_f64() * 1_000.0
    }

    fn micros(duration: Duration) -> f64 {
        duration.as_secs_f64() * 1_000_000.0
    }
}

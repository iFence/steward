use std::collections::BTreeSet;

use anyhow::{Context as _, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use nsh_core::access::{Capabilities, FileGrants};
use serde::{Deserialize, Serialize};
use steward_plugin_registry::PluginManifest;

use crate::archive::PreparedPackage;
use crate::{PluginStoreConfig, MAX_PACKAGE_BYTES};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackagePolicyReport {
    pub passed: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub denied_capabilities: Vec<String>,
}

impl PackagePolicyReport {
    fn empty() -> Self {
        Self {
            passed: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            denied_capabilities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageInspection {
    pub manifest: PluginManifest,
    pub app_id: String,
    pub name: String,
    pub version: String,
    pub build: u64,
    pub package_sha256: String,
    pub publisher_key_id: Option<String>,
    pub signature_valid: bool,
    pub signature_error: Option<String>,
    pub trusted: bool,
    pub trust_error: Option<String>,
    pub min_runtime: Option<String>,
    pub policy: PackagePolicyReport,
}

impl PackageInspection {
    pub fn installable(&self) -> bool {
        self.signature_valid && self.trusted && self.policy.errors.is_empty()
    }
}

pub(crate) fn inspect_prepared(
    config: &PluginStoreConfig,
    prepared: PreparedPackage,
) -> Result<PackageInspection> {
    inspect_bytes(
        config,
        prepared.manifest,
        &prepared.package,
        prepared.package_sha256,
    )
}

pub(crate) fn inspect_bytes(
    config: &PluginStoreConfig,
    manifest: PluginManifest,
    bytes: &[u8],
    package_sha256: String,
) -> Result<PackageInspection> {
    let mut policy = PackagePolicyReport::empty();
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        policy.passed = false;
        policy.errors.push(format!(
            "package is larger than the {} MiB policy limit",
            MAX_PACKAGE_BYTES / 1024 / 1024
        ));
    }

    let verified_header = nsh_package::verify_signature(bytes);
    let (signature_valid, signature_error, min_runtime, header_app) = match verified_header {
        Ok(header) => (
            true,
            None,
            Some(header.min_runtime().to_owned()),
            Some(header.app().clone()),
        ),
        Err(error) => (false, Some(error.to_string()), None, None),
    };
    let mut publisher_key_id = signature_valid
        .then(|| claimed_publisher_key_id(bytes))
        .flatten();

    if let Some(app) = &header_app {
        if app.id != manifest.id {
            policy.passed = false;
            policy.errors.push(format!(
                "package id '{}' does not match manifest id '{}'",
                app.id, manifest.id
            ));
        }
        if app.version != manifest.version {
            policy.passed = false;
            policy.errors.push(format!(
                "package version '{}' does not match manifest version '{}'",
                app.version, manifest.version
            ));
        }
        if app.build == 0 {
            policy.passed = false;
            policy
                .errors
                .push("package build must be greater than zero".into());
        }
    }

    let mut trusted = false;
    let mut trust_error = if config.trust_roots.is_empty() {
        Some("no NSH trust root is configured".into())
    } else {
        None
    };
    if signature_valid && config.trust_roots.is_empty() {
        policy.passed = false;
        policy.errors.push(trust_error.clone().unwrap_or_default());
    }
    if signature_valid && !config.trust_roots.is_empty() {
        let mut store = nsh_package::TrustStore::open(config.data_dir.join("nsh-trust"))
            .context("open NSH trust store")?;
        for root in &config.trust_roots {
            let key =
                nsh_package::PublicKey::from_base64(root.trim()).context("parse NSH trust root")?;
            store.add_root(key);
        }
        match nsh_engine::verify_package(&store, bytes, &config.data_dir.join("nsh-assets")) {
            Ok(package) => {
                trusted = true;
                trust_error = None;
                publisher_key_id = Some(package.receipt.key_id.clone());
                if let Some(declared) = package_capabilities(bytes) {
                    match Capabilities::from_json_lenient(&declared) {
                        Ok((declared, dropped)) => {
                            for capability in dropped {
                                policy.warnings.push(format!(
                                    "review unknown or invalid capability declaration '{capability}'"
                                ));
                            }
                            check_capabilities(&declared, &mut policy);
                        }
                        Err(error) => {
                            policy.passed = false;
                            policy
                                .errors
                                .push(format!("parse package capabilities: {error}"));
                        }
                    }
                }
            }
            Err(error) => {
                trust_error = Some(error.clone());
                policy.passed = false;
                policy
                    .errors
                    .push(format!("package is not trusted: {error}"));
            }
        }
    }

    policy.errors.sort();
    policy.errors.dedup();
    policy.warnings.sort();
    policy.warnings.dedup();
    policy.denied_capabilities.sort();
    policy.denied_capabilities.dedup();
    policy.passed &= policy.errors.is_empty();

    let app = header_app.unwrap_or_else(|| nsh_package::App {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        build: 0,
    });
    Ok(PackageInspection {
        manifest,
        app_id: app.id,
        name: app.name,
        version: app.version,
        build: app.build,
        package_sha256,
        publisher_key_id,
        signature_valid,
        signature_error,
        trusted,
        trust_error,
        min_runtime,
        policy,
    })
}

fn check_capabilities(capabilities: &Capabilities, policy: &mut PackagePolicyReport) {
    let mut denied = BTreeSet::new();
    if !capabilities.process.execute.is_empty()
        || capabilities.process.exit
        || !capabilities.process.env.is_empty()
    {
        deny(
            &mut denied,
            "process",
            "release policy denies process execution",
            policy,
        );
    }
    check_file_grants(&capabilities.fs, &mut denied, policy);
    if !capabilities.network.hosts.is_empty()
        || !capabilities.network.http.is_empty()
        || !capabilities.network.sockets.is_empty()
    {
        deny(
            &mut denied,
            "network",
            "release policy denies network access",
            policy,
        );
    }
    if capabilities.webview.is_some()
        || capabilities.webview_input
        || !capabilities.webview_media.is_empty()
    {
        deny(
            &mut denied,
            "webview",
            "release policy denies embedded web views",
            policy,
        );
    }
    if capabilities.status_bar {
        deny(
            &mut denied,
            "status_bar",
            "release policy denies status bar access",
            policy,
        );
    }
    if capabilities.picker {
        deny(
            &mut denied,
            "picker",
            "release policy denies file picker access",
            policy,
        );
    }
    if capabilities.share {
        deny(
            &mut denied,
            "share",
            "release policy denies system sharing",
            policy,
        );
    }
    if capabilities.speech {
        deny(
            &mut denied,
            "speech",
            "release policy denies speech recognition",
            policy,
        );
    }
    if capabilities.notifications {
        deny(
            &mut denied,
            "notifications",
            "release policy denies notifications",
            policy,
        );
    }
    if capabilities.audio {
        deny(
            &mut denied,
            "audio",
            "release policy denies audio playback",
            policy,
        );
    }
    if capabilities.application {
        deny(
            &mut denied,
            "application",
            "release policy denies application control",
            policy,
        );
    }
    if capabilities.home_dir {
        deny(
            &mut denied,
            "home_dir",
            "release policy denies home directory discovery",
            policy,
        );
    }
    if capabilities.windows.is_some() {
        deny(
            &mut denied,
            "windows",
            "release policy denies extra native windows",
            policy,
        );
    }
    for name in &capabilities.host {
        if !host_allowed(name) {
            deny(
                &mut denied,
                "host",
                &format!("release policy denies host function '{name}'"),
                policy,
            );
        }
    }
    if let Some(open_url) = &capabilities.open_url {
        for scheme in &open_url.schemes {
            if !matches!(scheme.as_str(), "https" | "mailto") {
                deny(
                    &mut denied,
                    "open_url",
                    &format!("release policy denies URL scheme '{scheme}'"),
                    policy,
                );
            }
        }
    }
    for name in denied {
        policy.denied_capabilities.push(name.clone());
        policy
            .warnings
            .push(format!("release policy does not grant '{name}'"));
    }
}

fn check_file_grants(
    grants: &FileGrants,
    denied: &mut BTreeSet<String>,
    policy: &mut PackagePolicyReport,
) {
    for (operation, roots) in [("read", &grants.read), ("write", &grants.write)] {
        for root in roots {
            if root != "${dataDir}" {
                deny(
                    denied,
                    "filesystem",
                    &format!("release policy does not grant filesystem {operation} root '{root}'"),
                    policy,
                );
            }
        }
    }
}

fn host_allowed(name: &str) -> bool {
    name == "steward.*" || name.starts_with("steward.")
}

fn deny(
    set: &mut BTreeSet<String>,
    category: &str,
    detail: &str,
    policy: &mut PackagePolicyReport,
) {
    set.insert(category.to_owned());
    policy.warnings.push(detail.to_owned());
}

fn claimed_publisher_key_id(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 12 || &bytes[..4] != b"NSHP" {
        return None;
    }
    let header_len = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let end = 12_usize.checked_add(header_len)?;
    let header = serde_json::from_slice::<serde_json::Value>(bytes.get(12..end)?).ok()?;
    let body = header.get("certificate")?.get("body")?.as_str()?;
    let body = BASE64.decode(body).ok()?;
    let body = serde_json::from_slice::<serde_json::Value>(&body).ok()?;
    let key = body.get("key")?.as_str()?;
    nsh_package::PublicKey::from_base64(key)
        .ok()
        .map(|key| key.id())
}

fn package_capabilities(bytes: &[u8]) -> Option<serde_json::Value> {
    if bytes.len() < 12 || &bytes[..4] != b"NSHP" {
        return None;
    }
    let header_len = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let end = 12_usize.checked_add(header_len)?;
    let header = serde_json::from_slice::<serde_json::Value>(bytes.get(12..end)?).ok()?;
    header.get("capabilities").cloned()
}

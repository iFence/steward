use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use steward_plugin_registry::{MANIFEST_FILE, PACKAGE_FILE};

use crate::PluginStore;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStage {
    Source,
    Layout,
    Manifest,
    Package,
    Signature,
    Trust,
    Compatibility,
    Capabilities,
    Policy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    pub id: String,
    pub stage: DoctorStage,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub target: String,
    pub resolved_path: Option<PathBuf>,
    pub installed: bool,
    pub passed: bool,
    pub stage: Option<DoctorStage>,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn new(target: &str, path: Option<PathBuf>, installed: bool) -> Self {
        Self {
            target: target.into(),
            resolved_path: path,
            installed,
            passed: false,
            stage: None,
            checks: Vec::new(),
        }
    }

    fn finish(mut self) -> Self {
        self.passed = !self.checks.is_empty() && self.checks.iter().all(|check| check.passed);
        self.stage = self
            .checks
            .iter()
            .find(|check| !check.passed)
            .map(|check| check.stage);
        self
    }
}

pub(crate) fn diagnose(store: &PluginStore, id_or_source: &str) -> DoctorReport {
    let supplied = PathBuf::from(id_or_source);
    let source_path = supplied.exists().then_some(supplied);
    let installed = source_path.is_none()
        && !id_or_source.contains(['/', '\\'])
        && store.plugins_root().join(id_or_source).is_dir();
    let resolved =
        source_path.or_else(|| installed.then(|| store.plugins_root().join(id_or_source)));
    let mut report = DoctorReport::new(id_or_source, resolved.clone(), installed);

    let Some(path) = resolved else {
        report.checks.push(check(
            "source",
            DoctorStage::Source,
            false,
            format!("'{id_or_source}' is neither an existing path nor an installed plugin id"),
        ));
        return report.finish();
    };
    report.checks.push(check(
        "source",
        DoctorStage::Source,
        true,
        format!("Resolved {}", path.display()),
    ));

    if installed {
        let names = directory_entries(&path);
        let expected = [MANIFEST_FILE, PACKAGE_FILE];
        let layout_ok = names.len() == 2
            && expected
                .iter()
                .all(|expected| names.iter().any(|name| name == expected));
        report.checks.push(check(
            "layout",
            DoctorStage::Layout,
            layout_ok,
            if layout_ok {
                "Installed directory contains exactly plugin.json and plugin.nshpkg".into()
            } else {
                format!(
                    "Installed directory must contain exactly plugin.json and plugin.nshpkg; found [{}]",
                    names.join(", ")
                )
            },
        ));
    } else {
        match source_layout(&path) {
            Ok(detail) => report
                .checks
                .push(check("layout", DoctorStage::Layout, true, detail)),
            Err(error) => report.checks.push(check(
                "layout",
                DoctorStage::Layout,
                false,
                format!("{error:#}"),
            )),
        }
    }

    let inspection = match store.inspect(&path) {
        Ok(inspection) => {
            report.checks.push(check(
                "manifest",
                DoctorStage::Manifest,
                true,
                format!(
                    "{} {} ({})",
                    inspection.name, inspection.version, inspection.app_id
                ),
            ));
            report.checks.push(check(
                "package",
                DoctorStage::Package,
                true,
                format!("Signed NSH package parsed with build {}", inspection.build),
            ));
            inspection
        }
        Err(error) => {
            report.checks.push(check(
                "package",
                DoctorStage::Package,
                false,
                format!("{error:#}"),
            ));
            return report.finish();
        }
    };

    report.checks.push(check(
        "id",
        DoctorStage::Manifest,
        inspection.manifest.id == inspection.app_id,
        if inspection.manifest.id == inspection.app_id {
            format!("Manifest and package id match: {}", inspection.app_id)
        } else {
            format!(
                "Manifest id '{}' differs from package id '{}'",
                inspection.manifest.id, inspection.app_id
            )
        },
    ));
    report.checks.push(check(
        "version",
        DoctorStage::Manifest,
        inspection.manifest.version == inspection.version,
        if inspection.manifest.version == inspection.version {
            format!("Manifest and package version match: {}", inspection.version)
        } else {
            format!(
                "Manifest version '{}' differs from package version '{}'",
                inspection.manifest.version, inspection.version
            )
        },
    ));
    report.checks.push(check(
        "signature",
        DoctorStage::Signature,
        inspection.signature_valid,
        inspection.signature_error.clone().unwrap_or_else(|| {
            format!(
                "Package signature is valid for {}",
                inspection
                    .publisher_key_id
                    .as_deref()
                    .unwrap_or("unknown publisher")
            )
        }),
    ));
    report.checks.push(check(
        "configured_trust_root",
        DoctorStage::Trust,
        !store.config.trust_roots.is_empty(),
        if store.config.trust_roots.is_empty() {
            "No NSH trust root is configured".into()
        } else {
            format!(
                "{} NSH trust root(s) configured",
                store.config.trust_roots.len()
            )
        },
    ));
    report.checks.push(check(
        "accepted_trust_root",
        DoctorStage::Trust,
        inspection.trusted,
        inspection
            .trust_error
            .clone()
            .unwrap_or_else(|| "Package certificate is accepted by a configured trust root".into()),
    ));
    report.checks.push(check(
        "host_api",
        DoctorStage::Compatibility,
        inspection.policy.errors.iter().all(|error| {
            !error.contains("minHostVersion")
                && !error.contains("host API")
                && !error.contains("newer than supported")
        }),
        inspection
            .min_host_version
            .as_deref()
            .map(|version| format!("Plugin requires host API {version}"))
            .unwrap_or_else(|| "Legacy compatibility package has no minHostVersion".into()),
    ));
    let capability_issues = inspection
        .policy
        .errors
        .iter()
        .filter(|error| {
            error.contains("capabilit")
                || error.contains("permission")
                || error.contains("network")
                || error.contains("executable")
                || error.contains("filesystem")
        })
        .cloned()
        .collect::<Vec<_>>();
    report.checks.push(check(
        "capabilities",
        DoctorStage::Capabilities,
        capability_issues.is_empty(),
        if capability_issues.is_empty() {
            format!(
                "{} declared capability scope(s); {} denied categor(ies)",
                inspection.declared_capabilities.len(),
                inspection.policy.denied_capabilities.len()
            )
        } else {
            capability_issues.join("; ")
        },
    ));
    report.checks.push(check(
        "policy",
        DoctorStage::Policy,
        inspection.policy.errors.is_empty(),
        if inspection.policy.errors.is_empty() {
            if inspection.policy.warnings.is_empty() {
                "All package policy checks passed".into()
            } else {
                format!(
                    "Policy passed with {} review warning(s)",
                    inspection.policy.warnings.len()
                )
            }
        } else {
            inspection.policy.errors.join("; ")
        },
    ));
    report.finish()
}

fn check(
    id: impl Into<String>,
    stage: DoctorStage,
    passed: bool,
    detail: impl Into<String>,
) -> DoctorCheck {
    DoctorCheck {
        id: id.into(),
        stage,
        passed,
        detail: detail.into(),
    }
}

fn directory_entries(path: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            let mut name = entry.file_name().to_str()?.to_owned();
            if file_type.is_dir() {
                name.push('/');
            }
            Some(name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn source_layout(path: &Path) -> anyhow::Result<String> {
    if path.is_dir() {
        let names = directory_entries(path);
        if names == [MANIFEST_FILE, PACKAGE_FILE] {
            return Ok("Source directory contains exactly plugin.json and plugin.nshpkg".into());
        }
        anyhow::bail!(
            "Source directory must contain exactly plugin.json and plugin.nshpkg; found [{}]",
            names.join(", ")
        );
    }
    let archive = zip::ZipArchive::new(
        std::fs::File::open(path)
            .map_err(|error| anyhow::anyhow!("open plugin archive: {error}"))?,
    )
    .map_err(|error| anyhow::anyhow!("read plugin archive: {error}"))?;
    let names = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|name| !name.ends_with('/'))
        .map(|name| name.into_owned())
        .collect::<Vec<_>>();
    if names.len() == 2 {
        let prefix = names[0]
            .strip_suffix(MANIFEST_FILE)
            .or_else(|| names[0].strip_suffix(PACKAGE_FILE));
        let expected_manifest = prefix.map(|prefix| format!("{prefix}{MANIFEST_FILE}"));
        let expected_package = prefix.map(|prefix| format!("{prefix}{PACKAGE_FILE}"));
        if prefix.is_some_and(|prefix| !prefix.contains('/'))
            && expected_manifest.is_some_and(|name| names.contains(&name))
            && expected_package.is_some_and(|name| names.contains(&name))
        {
            return Ok("Archive contains one plugin directory with exactly two files".into());
        }
    }
    anyhow::bail!(
        "Plugin archive must contain exactly <plugin>/plugin.json and <plugin>/plugin.nshpkg; found [{}]",
        names.join(", ")
    )
}

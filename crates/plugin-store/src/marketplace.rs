use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::admission::{self, PUBLIC_ADMISSION_POLICY_VERSION};
use crate::archive::sha256_file;
use crate::operations;
use crate::PackageInspection;
use crate::{
    atomic_write, safe_relative_path, unix_seconds, PluginStore, MARKETPLACE_SCHEMA_VERSION,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub build: u64,
    pub artifact: String,
    pub artifact_sha256: String,
    pub package_sha256: String,
    pub publisher_key_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_id: Option<String>,
    pub review_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_policy_version: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<admission::AdmissionReview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MarketplacePublisher {
    pub key_id: String,
    pub package_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceIndex {
    pub schema_version: u16,
    pub generated_at: i64,
    pub publishers: Vec<MarketplacePublisher>,
    pub entries: Vec<MarketplaceEntry>,
}

impl Default for MarketplaceIndex {
    fn default() -> Self {
        Self {
            schema_version: MARKETPLACE_SCHEMA_VERSION,
            generated_at: 0,
            publishers: Vec::new(),
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSyncReport {
    pub index_path: PathBuf,
    pub public_admission: bool,
    pub packages_scanned: usize,
    pub entries: usize,
    pub errors: Vec<String>,
}

pub(crate) fn index_path(root: &Path) -> PathBuf {
    root.join("index.json")
}

pub(crate) fn read_index(root: &Path) -> Result<MarketplaceIndex> {
    let path = index_path(root);
    if !path.exists() {
        return Ok(MarketplaceIndex::default());
    }
    let index: MarketplaceIndex = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    if index.schema_version != MARKETPLACE_SCHEMA_VERSION {
        anyhow::bail!(
            "marketplace schema {} is unsupported; expected {}",
            index.schema_version,
            MARKETPLACE_SCHEMA_VERSION
        );
    }
    for entry in &index.entries {
        let artifact = Path::new(&entry.artifact);
        if !safe_relative_path(artifact) {
            anyhow::bail!(
                "marketplace entry '{}' has an unsafe artifact path",
                entry.id
            );
        }
    }
    Ok(index)
}

pub(crate) fn sync(store: &PluginStore, public_admission: bool) -> Result<MarketplaceSyncReport> {
    let packages_dir = store.marketplace_dir().join("packages");
    std::fs::create_dir_all(&packages_dir)
        .with_context(|| format!("create {}", packages_dir.display()))?;
    let mut errors = Vec::new();
    let mut candidates = BTreeMap::<(String, String), (MarketplaceEntry, PackageInspection)>::new();
    let mut publishers = BTreeSet::new();
    let mut scanned = 0;
    let entries = std::fs::read_dir(&packages_dir)
        .with_context(|| format!("read {}", packages_dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if !path.is_file()
            || !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
        {
            continue;
        }
        scanned += 1;
        let inspection = match store.inspect(&path) {
            Ok(inspection) => inspection,
            Err(error) => {
                errors.push(format!("{}: {error:#}", path.display()));
                continue;
            }
        };
        if !inspection.installable() {
            errors.push(format!(
                "{}: package is not structurally trusted and policy-checkable ({})",
                path.display(),
                inspection
                    .policy
                    .errors
                    .iter()
                    .chain(inspection.trust_error.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            continue;
        }
        if public_admission {
            let review = admission::review(&inspection);
            if !review.accepted {
                errors.push(format!(
                    "{}: public admission rejected the package ({})",
                    path.display(),
                    review
                        .checks
                        .iter()
                        .filter(|check| !check.passed)
                        .map(|check| format!("{}: {}", check.code, check.detail))
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
                continue;
            }
            let publisher_key_id = inspection.publisher_key_id.as_deref().unwrap_or_default();
            if operations::authorized_publisher(store, publisher_key_id)?.is_none() {
                errors.push(format!(
                    "{}: public marketplace publisher key '{}' is not registered and active",
                    path.display(),
                    publisher_key_id
                ));
                continue;
            }
            if let Some(quarantine) = operations::package_quarantined(store, &inspection)? {
                errors.push(format!(
                    "{}: package is quarantined by '{}' ({})",
                    path.display(),
                    quarantine.id,
                    quarantine.reason
                ));
                continue;
            }
        }
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .context("marketplace package filename is not UTF-8")?;
        let display = path
            .strip_prefix(store.marketplace_dir())
            .context("marketplace package escaped its directory")?
            .to_string_lossy()
            .replace('\\', "/");
        let publisher_key_id = inspection.publisher_key_id.clone().unwrap_or_default();
        let publisher_id = if public_admission {
            operations::authorized_publisher(store, &publisher_key_id)?
                .map(|publisher| publisher.id)
        } else {
            None
        };
        let candidate = MarketplaceEntry {
            id: inspection.app_id.clone(),
            name: inspection.name.clone(),
            version: inspection.version.clone(),
            build: inspection.build,
            artifact: display,
            artifact_sha256: sha256_file(&path)?,
            package_sha256: inspection.package_sha256.clone(),
            publisher_key_id: publisher_key_id.clone(),
            publisher_id,
            review_required: !public_admission && !inspection.policy.warnings.is_empty(),
            admission_policy_version: public_admission.then_some(PUBLIC_ADMISSION_POLICY_VERSION),
            admission: public_admission.then(|| admission::review(&inspection)),
        };
        let key = (candidate.id.clone(), candidate.version.clone());
        if let Some(existing) = candidates.get(&key) {
            if existing.0.package_sha256 != candidate.package_sha256 {
                errors.push(format!(
                    "{}: duplicate plugin {} {} with a different package hash",
                    file_name, candidate.id, candidate.version
                ));
                continue;
            }
        }
        publishers.insert(publisher_key_id);
        candidates.insert(key, (candidate, inspection));
    }

    if public_admission {
        let keys = candidates.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let current = candidates
                .get(&key)
                .expect("marketplace candidate key exists");
            let previous = candidates
                .iter()
                .filter(|(candidate_key, (entry, inspection))| {
                    candidate_key.0 == key.0
                        && entry
                            .admission
                            .as_ref()
                            .is_some_and(|review| review.accepted)
                        && inspection.version != current.1.version
                        && compare_versions(&inspection.version, &current.1.version)
                            == std::cmp::Ordering::Less
                })
                .max_by(|(_, (left_entry, _)), (_, (right_entry, _))| {
                    compare_versions(&left_entry.version, &right_entry.version)
                })
                .map(|(_, (_, inspection))| inspection.clone());
            if let Some(previous) = previous.as_ref() {
                let candidate = candidates.get_mut(&key).expect("candidate exists");
                candidate.0.admission = Some(admission::review_with_previous(
                    &candidate.1,
                    Some(previous),
                ));
            }
        }
    }

    let mut entries = candidates
        .into_values()
        .map(|(entry, _)| entry)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| compare_versions(&right.version, &left.version))
            .then_with(|| right.build.cmp(&left.build))
    });
    let index = MarketplaceIndex {
        schema_version: MARKETPLACE_SCHEMA_VERSION,
        generated_at: unix_seconds(),
        publishers: publishers
            .into_iter()
            .map(|key_id| MarketplacePublisher {
                package_count: entries
                    .iter()
                    .filter(|entry| entry.publisher_key_id == key_id)
                    .count(),
                key_id,
            })
            .collect(),
        entries: entries.clone(),
    };
    let index_path = index_path(store.marketplace_dir());
    atomic_write(
        &index_path,
        &serde_json::to_vec_pretty(&index).context("serialize marketplace index")?,
    )?;
    Ok(MarketplaceSyncReport {
        index_path,
        public_admission,
        packages_scanned: scanned,
        entries: entries.len(),
        errors,
    })
}

pub(crate) fn resolve(store: &PluginStore, plugin_id: &str) -> Result<(PathBuf, MarketplaceEntry)> {
    let index = read_index(store.marketplace_dir())?;
    let entry = index
        .entries
        .iter()
        .filter(|entry| entry.id == plugin_id)
        .max_by(|left, right| {
            compare_versions(&left.version, &right.version)
                .then_with(|| left.build.cmp(&right.build))
        })
        .cloned()
        .with_context(|| format!("plugin '{plugin_id}' is not in the local marketplace"))?;
    let artifact = store.marketplace_dir().join(&entry.artifact);
    if !artifact.is_file() {
        anyhow::bail!("marketplace artifact is missing: {}", artifact.display());
    }
    if sha256_file(&artifact)? != entry.artifact_sha256 {
        anyhow::bail!(
            "marketplace artifact hash does not match its index entry: {}",
            artifact.display()
        );
    }
    if entry.admission_policy_version.is_some() {
        let inspection = store.inspect(&artifact)?;
        let key_id = inspection.publisher_key_id.as_deref().unwrap_or_default();
        if operations::authorized_publisher(store, key_id)?.is_none() {
            anyhow::bail!(
                "public marketplace publisher key '{key_id}' is not registered and active"
            );
        }
        if let Some(quarantine) = operations::package_quarantined(store, &inspection)? {
            anyhow::bail!(
                "public marketplace package is quarantined by '{}' ({})",
                quarantine.id,
                quarantine.reason
            );
        }
    }
    Ok((artifact, entry))
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    match (Version::parse(left), Version::parse(right)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

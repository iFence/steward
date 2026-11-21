use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::archive::sha256_file;
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
    pub review_required: bool,
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

pub(crate) fn sync(store: &PluginStore) -> Result<MarketplaceSyncReport> {
    let packages_dir = store.marketplace_dir().join("packages");
    std::fs::create_dir_all(&packages_dir)
        .with_context(|| format!("create {}", packages_dir.display()))?;
    let mut errors = Vec::new();
    let mut candidates = BTreeMap::<(String, String), MarketplaceEntry>::new();
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
        let candidate = MarketplaceEntry {
            id: inspection.app_id.clone(),
            name: inspection.name.clone(),
            version: inspection.version.clone(),
            build: inspection.build,
            artifact: display,
            artifact_sha256: sha256_file(&path)?,
            package_sha256: inspection.package_sha256,
            publisher_key_id: publisher_key_id.clone(),
            review_required: !inspection.policy.warnings.is_empty(),
        };
        let key = (candidate.id.clone(), candidate.version.clone());
        if let Some(existing) = candidates.get(&key) {
            if existing.package_sha256 != candidate.package_sha256 {
                errors.push(format!(
                    "{}: duplicate plugin {} {} with a different package hash",
                    file_name, candidate.id, candidate.version
                ));
                continue;
            }
        }
        publishers.insert(publisher_key_id);
        candidates.insert(key, candidate);
    }

    let mut entries = candidates.into_values().collect::<Vec<_>>();
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
    Ok((artifact, entry))
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    match (Version::parse(left), Version::parse(right)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

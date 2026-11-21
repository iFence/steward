use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use nsh_package::{Revocations, SecretKey};
use serde::{Deserialize, Serialize};

use crate::{atomic_write, unix_seconds, PluginStore};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppliedRevocations {
    pub sequence: u64,
    pub applied_at: i64,
    pub keys: Vec<String>,
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreatedRevocationList {
    pub path: PathBuf,
    pub root_key_id: String,
    pub sequence: u64,
    pub keys: Vec<String>,
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppliedFile {
    schema_version: u16,
    applied_at: i64,
    list: String,
}

pub(crate) fn apply(store: &PluginStore, path: &Path) -> Result<AppliedRevocations> {
    store.require_trust_roots()?;
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let list = Revocations::from_json(&text).context("parse NSH revocation list")?;
    let mut trust = store.trust_store()?;
    let sequence = trust
        .update_revocations(list.clone())
        .context("accept NSH revocation list")?;
    let (_, keys, packages) = list.contents()?;
    let applied = AppliedRevocations {
        sequence,
        applied_at: unix_seconds(),
        keys,
        packages,
    };
    let file = AppliedFile {
        schema_version: 1,
        applied_at: applied.applied_at,
        list: text,
    };
    atomic_write(
        &applied_path(store),
        &serde_json::to_vec_pretty(&file).context("serialize applied revocations")?,
    )?;
    Ok(applied)
}

pub(crate) fn applied(store: &PluginStore) -> Result<Option<AppliedRevocations>> {
    let path = applied_path(store);
    if !path.exists() {
        return Ok(None);
    }
    let file: AppliedFile = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    if file.schema_version != 1 {
        anyhow::bail!(
            "unsupported applied revocation schema {}",
            file.schema_version
        );
    }
    let list = Revocations::from_json(&file.list).context("parse applied revocation list")?;
    let (sequence, keys, packages) = list.contents()?;
    Ok(Some(AppliedRevocations {
        sequence,
        applied_at: file.applied_at,
        keys,
        packages,
    }))
}

pub fn read_revocation_list(path: &Path) -> Result<Revocations> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Revocations::from_json(&text).context("parse NSH revocation list")
}

pub fn create_revocation_list(
    root_key_path: &Path,
    previous: Option<&Path>,
    sequence: u64,
    keys: &[String],
    packages: &[String],
    output: &Path,
) -> Result<CreatedRevocationList> {
    let root_text = std::fs::read_to_string(root_key_path)
        .with_context(|| format!("read root key {}", root_key_path.display()))?;
    let root = SecretKey::from_base64(root_text.trim()).context("parse NSH root secret key")?;
    let mut merged_keys = keys.to_vec();
    let mut merged_packages = packages.to_vec();
    if let Some(previous) = previous {
        let previous = read_revocation_list(previous)?;
        let (previous_sequence, previous_keys, previous_packages) = previous.contents()?;
        if sequence <= previous_sequence {
            anyhow::bail!(
                "new revocation sequence {sequence} must exceed previous sequence {previous_sequence}"
            );
        }
        merged_keys.extend(previous_keys);
        merged_packages.extend(previous_packages);
    }
    merged_keys.sort();
    merged_keys.dedup();
    merged_packages.sort();
    merged_packages.dedup();
    let list = nsh_package::revoke(&root, sequence, &merged_keys, &merged_packages);
    atomic_write(output, list.to_json().as_bytes())?;
    Ok(CreatedRevocationList {
        path: output.to_path_buf(),
        root_key_id: root.public().id(),
        sequence,
        keys: merged_keys,
        packages: merged_packages,
    })
}

fn applied_path(store: &PluginStore) -> PathBuf {
    store.data_dir().join("applied-revocations.json")
}

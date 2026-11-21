use std::io::{Read as _, Seek as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use sha2::{Digest as _, Sha256};

use crate::{MAX_ARCHIVE_ENTRY_COUNT, MAX_PACKAGE_BYTES, MAX_UNPACKED_BYTES};
use steward_plugin_registry::manifest::load_manifest;
use steward_plugin_registry::{MANIFEST_FILE, PACKAGE_FILE};

#[derive(Clone)]
pub(crate) struct PreparedPackage {
    pub(crate) manifest: steward_plugin_registry::PluginManifest,
    pub(crate) package: Vec<u8>,
    pub(crate) package_sha256: String,
}

pub(crate) fn prepare_source(path: &Path) -> Result<PreparedPackage> {
    if path.is_dir() {
        return prepare_directory(path);
    }
    if !path.is_file() {
        anyhow::bail!("plugin source does not exist: {}", path.display());
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("zip") {
        prepare_zip(path)
    } else {
        anyhow::bail!(
            "unsupported plugin source {}; expected a plugin zip or two-file plugin directory",
            path.display()
        )
    }
}

fn prepare_directory(path: &Path) -> Result<PreparedPackage> {
    let entries = std::fs::read_dir(path)
        .with_context(|| format!("read plugin directory {}", path.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("read plugin directory {}", path.display()))?;
    if entries.len() != 2 {
        anyhow::bail!(
            "plugin directory {} must contain exactly {MANIFEST_FILE} and {PACKAGE_FILE}",
            path.display()
        );
    }
    for entry in &entries {
        let file_type = entry
            .file_type()
            .with_context(|| format!("inspect {}", entry.path().display()))?;
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        if !file_type.is_file() || !matches!(name, MANIFEST_FILE | PACKAGE_FILE) {
            anyhow::bail!(
                "plugin directory {} contains unsupported entry '{}'",
                path.display(),
                name
            );
        }
    }
    let manifest = load_manifest(path)?;
    let package_path = path.join(PACKAGE_FILE);
    let metadata = std::fs::metadata(&package_path)
        .with_context(|| format!("inspect {}", package_path.display()))?;
    if metadata.len() > MAX_PACKAGE_BYTES {
        anyhow::bail!(
            "plugin package is larger than the {} MiB policy limit",
            MAX_PACKAGE_BYTES / 1024 / 1024
        );
    }
    let package =
        std::fs::read(&package_path).with_context(|| format!("read {}", package_path.display()))?;
    Ok(finish(manifest, package))
}

fn prepare_zip(path: &Path) -> Result<PreparedPackage> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("inspect {}", path.display()))?;
    if metadata.len() > MAX_PACKAGE_BYTES {
        anyhow::bail!(
            "plugin archive is larger than the {} MiB policy limit",
            MAX_PACKAGE_BYTES / 1024 / 1024
        );
    }
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("read {}", path.display()))?;
    if archive.len() > MAX_ARCHIVE_ENTRY_COUNT {
        anyhow::bail!(
            "plugin archive has {} entries; the policy limit is {MAX_ARCHIVE_ENTRY_COUNT}",
            archive.len()
        );
    }

    let mut manifest = None;
    let mut package = None;
    let mut top_level = None;
    let mut unpacked = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("read archive entry {index}"))?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            anyhow::bail!("plugin archive contains a symbolic link");
        }
        let name = entry
            .name()
            .with_context(|| format!("decode archive entry {index} filename"))?
            .into_owned();
        if name.contains('\\') || name.contains('\0') {
            anyhow::bail!("plugin archive contains an unsafe path");
        }
        if name.ends_with('/') {
            let top = name.trim_end_matches('/');
            if top.is_empty() || top.contains(['/', ':', '.']) || top == ".." {
                anyhow::bail!("plugin archive contains nested directory entries");
            }
            if top_level.as_deref().is_some_and(|existing| existing != top) {
                anyhow::bail!("plugin archive must contain one top-level plugin directory");
            }
            top_level = Some(top.to_owned());
            continue;
        }
        let parts = name.split('/').collect::<Vec<_>>();
        if parts.len() != 2
            || parts[0].is_empty()
            || parts[0].contains(':')
            || parts[0] == "."
            || parts[0] == ".."
            || parts[1].is_empty()
        {
            anyhow::bail!(
                "plugin archive entry '{}' must be <plugin>/{MANIFEST_FILE} or <plugin>/{PACKAGE_FILE}",
                name
            );
        }
        if let Some(expected) = top_level.as_deref() {
            if parts[0] != expected {
                anyhow::bail!("plugin archive must contain one top-level plugin directory");
            }
        } else {
            top_level = Some(parts[0].to_owned());
        }
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > MAX_UNPACKED_BYTES {
            anyhow::bail!(
                "plugin archive expands beyond the {} MiB policy limit",
                MAX_UNPACKED_BYTES / 1024 / 1024
            );
        }
        let target = match parts[1] {
            MANIFEST_FILE if manifest.is_none() => &mut manifest,
            PACKAGE_FILE if package.is_none() => &mut package,
            MANIFEST_FILE | PACKAGE_FILE => {
                anyhow::bail!("plugin archive contains duplicate '{name}'")
            }
            other => anyhow::bail!("plugin archive contains unsupported file '{other}'"),
        };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("read archive entry '{name}'"))?;
        *target = Some(bytes);
    }
    let manifest = manifest.context("plugin archive is missing plugin.json")?;
    let package = package.context("plugin archive is missing plugin.nshpkg")?;
    let manifest: steward_plugin_registry::PluginManifest =
        serde_json::from_slice(&manifest).context("parse plugin.json from archive")?;
    manifest
        .validate()
        .map_err(|error| anyhow::anyhow!("plugin.json: {error}"))?;
    Ok(finish(manifest, package))
}

fn finish(manifest: steward_plugin_registry::PluginManifest, package: Vec<u8>) -> PreparedPackage {
    let package_sha256 = hex(Sha256::digest(&package));
    PreparedPackage {
        manifest,
        package,
        package_sha256,
    }
}

pub(crate) fn write_distribution(directory: &Path, prepared: &PreparedPackage) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("create staging directory {}", directory.display()))?;
    std::fs::write(
        directory.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&prepared.manifest).context("serialize plugin.json")?,
    )
    .with_context(|| format!("write {}", directory.join(MANIFEST_FILE).display()))?;
    std::fs::write(directory.join(PACKAGE_FILE), &prepared.package)
        .with_context(|| format!("write {}", directory.join(PACKAGE_FILE).display()))?;
    Ok(directory.to_path_buf())
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    file.rewind().ok();
    Ok(hex(hasher.finalize()))
}

pub(crate) fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

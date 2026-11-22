use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use steward_plugin_registry::manifest::load_manifest;
use steward_plugin_registry::{MANIFEST_FILE, PACKAGE_FILE};

use crate::{atomic_write, safe_relative_path, unique_suffix};

const RELOAD_DIRECTORY: &str = "developer/reloads";
const MAX_RELOAD_REQUESTS: usize = 64;

pub fn default_developer_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("no OS data directory available")?
        .join("Steward-Developer"))
}

/// A request for the running shell to replace an active plugin process with
/// the package currently on disk. It intentionally carries no payload beyond
/// the route needed to reopen the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DevelopmentReloadRequest {
    pub plugin_id: String,
    pub command: String,
    pub title: String,
    #[serde(default)]
    pub open: bool,
    pub requested_at: u64,
}

/// Package a plugin project's manifest and already-signed NSH package into the
/// exact two-file distribution layout accepted by install and Settings.
pub fn package_development_project(
    project_dir: &Path,
    package_path: &Path,
    output: &Path,
) -> Result<PathBuf> {
    let project_dir = project_dir
        .canonicalize()
        .with_context(|| format!("resolve plugin project {}", project_dir.display()))?;
    let manifest = load_manifest(&project_dir)?;
    let package_path = if package_path.is_absolute() {
        package_path.to_path_buf()
    } else {
        project_dir.join(package_path)
    };
    let package_path = package_path
        .canonicalize()
        .with_context(|| format!("resolve signed package {}", package_path.display()))?;
    if !package_path.starts_with(&project_dir) || !package_path.is_file() {
        anyhow::bail!(
            "signed package {} must be a file inside {}",
            package_path.display(),
            project_dir.display()
        );
    }
    if output.exists() {
        anyhow::bail!("output archive already exists: {}", output.display());
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let temporary = output.with_extension(format!("tmp-{}", unique_suffix()));
    let result = write_archive(&temporary, &manifest.id, &project_dir, &package_path);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    std::fs::rename(&temporary, output)
        .with_context(|| format!("rename {} to {}", temporary.display(), output.display()))?;
    Ok(output.to_path_buf())
}

/// Ask a running developer shell to reload one plugin from its installed
/// static package. The shell polls this queue; it never receives host-call
/// request data or signing secrets.
pub fn request_development_reload(
    data_dir: &Path,
    request: &DevelopmentReloadRequest,
) -> Result<PathBuf> {
    if !safe_relative_path(Path::new(&request.plugin_id))
        || matches!(request.plugin_id.as_str(), "." | "..")
    {
        anyhow::bail!("invalid reload plugin id '{}'", request.plugin_id);
    }
    if request.command.trim().is_empty() || request.command.len() > 128 {
        anyhow::bail!("reload command must contain 1 to 128 bytes");
    }
    if request.title.trim().is_empty() || request.title.len() > 256 {
        anyhow::bail!("reload title must contain 1 to 256 bytes");
    }
    let directory = data_dir.join(RELOAD_DIRECTORY);
    let path = directory.join(format!(
        "{}-{}.json",
        request.requested_at.max(1),
        unique_suffix()
    ));
    let bytes = serde_json::to_vec_pretty(request).context("serialize development reload")?;
    atomic_write(&path, &bytes)?;
    Ok(path)
}

/// Consume queued reload requests in deterministic order. Malformed and
/// oversize requests are removed so a broken request cannot stall the queue.
pub fn take_development_reload_requests(data_dir: &Path) -> Result<Vec<DevelopmentReloadRequest>> {
    let directory = data_dir.join(RELOAD_DIRECTORY);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", directory.display()));
        }
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.truncate(MAX_RELOAD_REQUESTS);

    let mut requests = Vec::new();
    for path in paths {
        let request = std::fs::read(&path)
            .ok()
            .filter(|bytes| bytes.len() <= 16 * 1024)
            .and_then(|bytes| serde_json::from_slice::<DevelopmentReloadRequest>(&bytes).ok())
            .filter(|request| {
                safe_relative_path(Path::new(&request.plugin_id))
                    && !request.plugin_id.trim().is_empty()
                    && !request.command.trim().is_empty()
            });
        let _ = std::fs::remove_file(&path);
        if let Some(request) = request {
            requests.push(request);
        }
    }
    Ok(requests)
}

fn write_archive(
    output: &Path,
    plugin_id: &str,
    project_dir: &Path,
    package_path: &Path,
) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .with_context(|| format!("create {}", output.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.add_directory(format!("{plugin_id}/"), options)
        .context("add plugin directory to archive")?;
    let manifest = std::fs::read(project_dir.join(MANIFEST_FILE))
        .with_context(|| format!("read {}", project_dir.join(MANIFEST_FILE).display()))?;
    zip.start_file(format!("{plugin_id}/{MANIFEST_FILE}"), options)
        .context("add plugin.json to archive")?;
    zip.write_all(&manifest)
        .context("write plugin.json to archive")?;

    zip.start_file(format!("{plugin_id}/{PACKAGE_FILE}"), options)
        .context("add plugin.nshpkg to archive")?;
    let mut package = std::fs::File::open(package_path)
        .with_context(|| format!("open {}", package_path.display()))?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = package
            .read(&mut buffer)
            .with_context(|| format!("read {}", package_path.display()))?;
        if read == 0 {
            break;
        }
        zip.write_all(&buffer[..read])
            .context("write plugin.nshpkg to archive")?;
    }
    zip.finish().context("finish plugin archive")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "steward-plugin-developer-{name}-{}-{}",
            std::process::id(),
            unique_suffix()
        ))
    }

    #[test]
    fn reload_queue_roundtrips_and_discards_bad_requests() {
        let root = scratch("reloads");
        let request = DevelopmentReloadRequest {
            plugin_id: "com.example.dev".into(),
            command: "dev".into(),
            title: "Development".into(),
            open: true,
            requested_at: 42,
        };
        request_development_reload(&root, &request).unwrap();
        std::fs::write(root.join(RELOAD_DIRECTORY).join("bad.json"), b"not json").unwrap();

        assert_eq!(
            take_development_reload_requests(&root).unwrap(),
            vec![request]
        );
        assert!(take_development_reload_requests(&root).unwrap().is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn project_packaging_uses_the_two_file_layout() {
        let root = scratch("package");
        let project = root.join("project");
        std::fs::create_dir_all(project.join("dist")).unwrap();
        std::fs::write(
            project.join(MANIFEST_FILE),
            r#"{
              "schemaVersion": 3,
              "id": "com.example.package",
              "name": "Package",
              "version": "1.0.0",
              "minHostVersion": "0.5.0",
              "package": "plugin.nshpkg",
              "commands": [{
                "name": "package",
                "title": "Package",
                "trigger": { "type": "command" }
              }]
            }"#,
        )
        .unwrap();
        std::fs::write(project.join("dist").join(PACKAGE_FILE), b"signed package").unwrap();
        let output = root.join("plugin.zip");
        package_development_project(&project, Path::new("dist/plugin.nshpkg"), &output).unwrap();

        let archive = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
        let names = archive
            .file_names()
            .filter_map(Result::ok)
            .map(|name| name.into_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&"com.example.package/plugin.json".into()));
        assert!(names.contains(&"com.example.package/plugin.nshpkg".into()));
        assert_eq!(names.iter().filter(|name| !name.ends_with('/')).count(), 2);
        std::fs::remove_dir_all(root).ok();
    }
}

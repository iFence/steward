//! Isolated developer profile resolution.
//!
//! Developer mode must not write to the installed app's database, plugin
//! directory, instance mutex, or trust configuration. It therefore resolves a
//! separate profile before storage or the single-instance lock is opened.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use steward_plugin_store::default_developer_data_dir;

pub(crate) struct DeveloperProfile {
    pub(crate) enabled: bool,
    pub(crate) data_dir: PathBuf,
    pub(crate) plugins_dir: PathBuf,
    pub(crate) additional_trust_roots: Vec<String>,
}

impl DeveloperProfile {
    pub(crate) fn resolve(args: &[String]) -> Result<Self> {
        let enabled = has_flag(args, "--developer-mode") || env_truthy("STEWARD_DEVELOPER_MODE");
        if enabled {
            resolve_developer(args)
        } else {
            resolve_installed(args)
        }
    }

    pub(crate) fn apply_environment(&self) {
        std::env::set_var("STEWARD_DATA_DIR", &self.data_dir);
        std::env::set_var("STEWARD_PLUGINS_DIR", &self.plugins_dir);
        if self.enabled {
            std::env::set_var(
                "STEWARD_NSH_TRUST_ROOTS_JSON",
                serde_json::to_string(&self.additional_trust_roots)
                    .expect("developer trust roots serialize"),
            );
            if std::env::var_os("STEWARD_INSTANCE_SUFFIX").is_none() {
                std::env::set_var("STEWARD_INSTANCE_SUFFIX", "developer");
            }
        }
    }
}

fn resolve_installed(args: &[String]) -> Result<DeveloperProfile> {
    let data_dir = option_value(args, "--data-dir")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("STEWARD_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("no OS data directory available")
                .join("Steward")
        });
    let plugins_dir = option_value(args, "--plugins-dir")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("STEWARD_PLUGINS_DIR").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.join("plugins"));
    let additional_trust_roots = std::env::var("STEWARD_NSH_TRUST_ROOT")
        .ok()
        .into_iter()
        .collect();
    Ok(DeveloperProfile {
        enabled: false,
        data_dir,
        plugins_dir,
        additional_trust_roots,
    })
}

fn resolve_developer(args: &[String]) -> Result<DeveloperProfile> {
    let data_dir = option_value(args, "--developer-data-dir")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("STEWARD_DEVELOPER_DATA_DIR").map(PathBuf::from))
        .map(Ok)
        .unwrap_or_else(default_developer_data_dir)?;
    let plugins_dir = option_value(args, "--developer-plugins-dir")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("STEWARD_DEVELOPER_PLUGINS_DIR").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.join("plugins"));
    if plugins_dir == data_dir {
        anyhow::bail!("developer plugin directory must not be the developer data root");
    }

    let mut additional_trust_roots = Vec::new();
    for value in option_values(args, "--developer-trust-root") {
        additional_trust_roots.extend(load_trust_root(&PathBuf::from(value))?);
    }
    if let Ok(value) = std::env::var("STEWARD_DEVELOPER_TRUST_ROOT") {
        additional_trust_roots.extend(load_trust_root(&PathBuf::from(value))?);
    }
    if let Ok(values) = std::env::var("STEWARD_NSH_TRUST_ROOTS_JSON") {
        if let Ok(mut parsed) = serde_json::from_str::<Vec<String>>(&values) {
            additional_trust_roots.append(&mut parsed);
        }
    }
    additional_trust_roots.retain(|root| !root.trim().is_empty());
    additional_trust_roots.sort();
    additional_trust_roots.dedup();

    Ok(DeveloperProfile {
        enabled: true,
        data_dir,
        plugins_dir,
        additional_trust_roots,
    })
}

fn load_trust_root(path_or_key: &Path) -> Result<Vec<String>> {
    if !path_or_key.exists() {
        return Ok(vec![path_or_key.to_string_lossy().trim().to_owned()]);
    }
    let text = std::fs::read_to_string(path_or_key)
        .with_context(|| format!("read developer trust root {}", path_or_key.display()))?;
    if let Ok(roots) = serde_json::from_str::<Vec<String>>(&text) {
        return Ok(roots);
    }
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect())
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|argument| argument == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn option_values<'a>(args: &'a [String], name: &'a str) -> impl Iterator<Item = &'a String> {
    args.iter()
        .enumerate()
        .filter(move |(_, argument)| *argument == name)
        .filter_map(move |(index, _)| args.get(index + 1))
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|argument| argument == name)
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_options_are_parsed_without_consuming_following_flags() {
        let args = vec![
            "--developer-mode".into(),
            "--developer-data-dir".into(),
            r"D:\dev-data".into(),
            "--developer-plugins-dir".into(),
            r"D:\dev-plugins".into(),
            "--developer-trust-root".into(),
            "root-one".into(),
            "--developer-trust-root".into(),
            "root-two".into(),
        ];
        assert!(has_flag(&args, "--developer-mode"));
        assert_eq!(
            option_value(&args, "--developer-data-dir").as_deref(),
            Some(r"D:\dev-data")
        );
        assert_eq!(
            option_values(&args, "--developer-trust-root")
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["root-one", "root-two"]
        );
    }
}

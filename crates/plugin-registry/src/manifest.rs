//! Steward plugin manifest v2.
//!
//! The manifest contains only routing metadata. Executable code and native
//! capabilities live in the signed NSH package referenced by `package`.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u16 = 2;
pub const MANIFEST_FILE: &str = "plugin.json";
pub const PACKAGE_FILE: &str = "plugin.nshpkg";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub package: String,
    pub commands: Vec<PluginCommand>,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ManifestError(format!(
                "unsupported schemaVersion {}; expected {SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() || self.id.split('.').count() < 2 {
            return Err(ManifestError(
                "id must be a non-empty reverse-domain identifier".into(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError("name must not be empty".into()));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError("version must not be empty".into()));
        }
        if self.package != PACKAGE_FILE {
            return Err(ManifestError(format!(
                "package must be '{PACKAGE_FILE}', not '{}'",
                self.package
            )));
        }
        if self
            .icon
            .as_ref()
            .is_some_and(|icon| icon.trim().is_empty())
        {
            return Err(ManifestError("icon must not be empty when present".into()));
        }
        if self.commands.is_empty() {
            return Err(ManifestError("at least one command is required".into()));
        }
        let mut names = std::collections::HashSet::new();
        for command in &self.commands {
            command.validate()?;
            if !names.insert(command.name.as_str()) {
                return Err(ManifestError(format!(
                    "duplicate command name '{}'",
                    command.name
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginCommand {
    pub name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    pub trigger: Trigger,
}

impl PluginCommand {
    fn validate(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError("command name must not be empty".into()));
        }
        if self.title.trim().is_empty() {
            return Err(ManifestError(format!(
                "command '{}' title must not be empty",
                self.name
            )));
        }
        if self
            .keywords
            .iter()
            .any(|keyword| keyword.trim().is_empty())
        {
            return Err(ManifestError(format!(
                "command '{}' has an empty keyword",
                self.name
            )));
        }
        match self.trigger.kind {
            TriggerType::Command if self.trigger.value.is_some() => Err(ManifestError(format!(
                "command '{}' trigger of type 'command' takes no value",
                self.name
            ))),
            TriggerType::Prefix | TriggerType::Regex
                if self.trigger.value.as_deref().is_none_or(str::is_empty) =>
            {
                Err(ManifestError(format!(
                    "command '{}' trigger of type '{}' requires a value",
                    self.name, self.trigger.kind
                )))
            }
            TriggerType::Regex => regex::Regex::new(self.trigger.value.as_deref().unwrap_or(""))
                .map(|_| ())
                .map_err(|error| {
                    ManifestError(format!(
                        "command '{}' has an invalid regex: {error}",
                        self.name
                    ))
                }),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    #[serde(rename = "type")]
    pub kind: TriggerType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TriggerType {
    Command,
    Prefix,
    Regex,
}

impl fmt::Display for TriggerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Command => "command",
            Self::Prefix => "prefix",
            Self::Regex => "regex",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError(pub String);

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ManifestError {}

pub fn load_manifest(dir: &Path) -> anyhow::Result<PluginManifest> {
    let path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("failed to read {}: {error}", path.display()))?;
    let manifest: PluginManifest = serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("failed to parse {}: {error}", path.display()))?;
    manifest
        .validate()
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    Ok(manifest)
}

pub fn package_path(dir: &Path) -> PathBuf {
    dir.join(PACKAGE_FILE)
}

pub fn is_safe_package_path(path: &Path) -> bool {
    matches!(
        path.components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(name)] if *name == std::ffi::OsStr::new(PACKAGE_FILE)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema_version: SCHEMA_VERSION,
            id: "it.aspirin.calendar".into(),
            name: "Calendar".into(),
            version: "0.3.0".into(),
            icon: None,
            package: PACKAGE_FILE.into(),
            commands: vec![PluginCommand {
                name: "calendar".into(),
                title: "Calendar".into(),
                keywords: vec!["日历".into()],
                trigger: Trigger {
                    kind: TriggerType::Command,
                    value: None,
                },
            }],
        }
    }

    #[test]
    fn valid_manifest_roundtrips() {
        let manifest = manifest();
        manifest.validate().unwrap();
        let text = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<PluginManifest>(&text).unwrap(),
            manifest
        );
    }

    #[test]
    fn schema_and_package_are_strict() {
        let mut bad = manifest();
        bad.schema_version = 1;
        assert!(bad.validate().is_err());
        bad = manifest();
        bad.package = "../plugin.nshpkg".into();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn dynamic_and_old_fields_are_rejected() {
        let text = r#"{
          "schemaVersion": 2,
          "id": "com.example.bad",
          "name": "Bad",
          "version": "1.0.0",
          "package": "plugin.nshpkg",
          "permissions": [],
          "commands": [
            { "name": "bad", "title": "Bad", "trigger": { "type": "dynamic" } }
          ]
        }"#;
        assert!(serde_json::from_str::<PluginManifest>(text).is_err());
    }
}

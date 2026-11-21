//! Steward plugin manifest v2.
//!
//! The manifest contains only routing metadata. Executable code and native
//! capabilities live in the signed NSH package referenced by `package`.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(
        rename = "minHostVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub min_host_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub package: String,
    #[serde(default, skip_serializing_if = "PluginPreferences::is_empty")]
    pub preferences: PluginPreferences,
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
        if !valid_plugin_id(&self.id) {
            return Err(ManifestError(
                "id must be a reverse-domain identifier with alphanumeric, '-' or '_' labels"
                    .into(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError("name must not be empty".into()));
        }
        if semver::Version::parse(&self.version).is_err() {
            return Err(ManifestError(
                "version must be a valid semantic version".into(),
            ));
        }
        if self
            .min_host_version
            .as_deref()
            .is_some_and(|version| semver::Version::parse(version).is_err())
        {
            return Err(ManifestError(
                "minHostVersion must be a valid semantic version".into(),
            ));
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
        self.preferences.validate()?;
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

const PREFERENCE_SCHEMA_VERSION: u16 = 1;
const MAX_PREFERENCE_FIELDS: usize = 64;
const MAX_PREFERENCE_TEXT: usize = 4096;

/// Declarative, host-rendered plugin preferences.
///
/// Preferences live in `plugin.json` so Settings can render and validate them
/// without loading the plugin's QuickJS VM. Stored values are keyed by plugin
/// ID and remain valid across package updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPreferences {
    #[serde(default = "default_preference_schema_version")]
    pub schema_version: u16,
    #[serde(default)]
    pub fields: Vec<PluginPreferenceField>,
    #[serde(default)]
    pub migrations: Vec<PreferenceMigration>,
}

impl Default for PluginPreferences {
    fn default() -> Self {
        Self {
            schema_version: PREFERENCE_SCHEMA_VERSION,
            fields: Vec::new(),
            migrations: Vec::new(),
        }
    }
}

fn default_preference_schema_version() -> u16 {
    PREFERENCE_SCHEMA_VERSION
}

impl PluginPreferences {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != PREFERENCE_SCHEMA_VERSION {
            return Err(ManifestError(format!(
                "unsupported preference schemaVersion {}; expected {PREFERENCE_SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        if self.fields.is_empty() {
            if self.migrations.is_empty() {
                return Ok(());
            }
            return Err(ManifestError(
                "preference migrations require at least one preference field".into(),
            ));
        }
        if self.fields.len() > MAX_PREFERENCE_FIELDS {
            return Err(ManifestError(format!(
                "preferences contain {} fields; maximum is {MAX_PREFERENCE_FIELDS}",
                self.fields.len()
            )));
        }
        let mut keys = std::collections::HashSet::new();
        for field in &self.fields {
            field.validate()?;
            if !keys.insert(field.key.as_str()) {
                return Err(ManifestError(format!(
                    "duplicate preference key '{}'",
                    field.key
                )));
            }
        }
        for migration in &self.migrations {
            if !valid_preference_key(&migration.from)
                || !valid_preference_key(&migration.to)
                || migration.from == migration.to
            {
                return Err(ManifestError(
                    "preference migration keys must be distinct valid keys".into(),
                ));
            }
            if keys.contains(migration.from.as_str()) {
                return Err(ManifestError(format!(
                    "preference migration source '{}' must be a retired key",
                    migration.from
                )));
            }
            if !keys.contains(migration.to.as_str()) {
                return Err(ManifestError(format!(
                    "preference migration destination '{}' is not declared",
                    migration.to
                )));
            }
        }
        Ok(())
    }

    pub fn field(&self, key: &str) -> Option<&PluginPreferenceField> {
        self.fields.iter().find(|field| field.key == key)
    }

    pub fn validate_value(&self, key: &str, value: &Value) -> Result<(), ManifestError> {
        let field = self
            .field(key)
            .ok_or_else(|| ManifestError(format!("unknown preference '{key}'")))?;
        field.validate_value(value)
    }

    pub fn resolve_values(
        &self,
        stored: &std::collections::BTreeMap<String, Value>,
    ) -> std::collections::BTreeMap<String, Value> {
        let mut resolved = stored.clone();
        for migration in &self.migrations {
            let destination_is_valid = self
                .field(&migration.to)
                .zip(resolved.get(&migration.to))
                .is_some_and(|(field, value)| field.validate_value(value).is_ok());
            if !destination_is_valid {
                if let Some(value) = resolved.get(&migration.from).cloned() {
                    resolved.insert(migration.to.clone(), value);
                }
            }
        }
        self.fields
            .iter()
            .filter_map(|field| {
                let value = resolved
                    .get(&field.key)
                    .filter(|value| field.validate_value(value).is_ok())
                    .cloned()
                    .or_else(|| field.default.clone())
                    .or_else(|| {
                        field
                            .empty_value()
                            .filter(|value| field.validate_value(value).is_ok())
                    })?;
                Some((field.key.clone(), value))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferenceMigration {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPreferenceField {
    pub key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub kind: PluginPreferenceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<PluginPreferenceOption>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PluginPreferenceKind {
    Text,
    Boolean,
    Select,
    Folder,
    Application,
}

impl PluginPreferenceKind {
    pub fn is_path(self) -> bool {
        matches!(self, Self::Folder | Self::Application)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPreferenceOption {
    pub value: String,
    pub label: String,
}

impl PluginPreferenceField {
    fn validate(&self) -> Result<(), ManifestError> {
        if !valid_preference_key(&self.key) {
            return Err(ManifestError(format!(
                "preference key '{}' must start with a lowercase letter and contain only letters, digits, '.', '-' or '_'",
                self.key
            )));
        }
        if self.title.trim().is_empty() {
            return Err(ManifestError(format!(
                "preference '{}' title must not be empty",
                self.key
            )));
        }
        if self
            .description
            .as_ref()
            .is_some_and(|description| description.len() > MAX_PREFERENCE_TEXT)
        {
            return Err(ManifestError(format!(
                "preference '{}' description exceeds {MAX_PREFERENCE_TEXT} bytes",
                self.key
            )));
        }
        if self
            .max_length
            .is_some_and(|limit| limit == 0 || limit > MAX_PREFERENCE_TEXT)
        {
            return Err(ManifestError(format!(
                "preference '{}' maxLength must be between 1 and {MAX_PREFERENCE_TEXT}",
                self.key
            )));
        }
        if self.kind == PluginPreferenceKind::Select {
            if self.options.is_empty() {
                return Err(ManifestError(format!(
                    "select preference '{}' requires options",
                    self.key
                )));
            }
            let mut values = std::collections::HashSet::new();
            for option in &self.options {
                if option.value.is_empty()
                    || option.label.trim().is_empty()
                    || !values.insert(option.value.as_str())
                {
                    return Err(ManifestError(format!(
                        "select preference '{}' has an empty or duplicate option",
                        self.key
                    )));
                }
            }
        } else if !self.options.is_empty() {
            return Err(ManifestError(format!(
                "preference '{}' options are only valid for select fields",
                self.key
            )));
        }
        if let Some(default) = &self.default {
            self.validate_value(default)?;
        }
        Ok(())
    }

    fn validate_value(&self, value: &Value) -> Result<(), ManifestError> {
        match self.kind {
            PluginPreferenceKind::Boolean => {
                if !value.is_boolean() {
                    return Err(ManifestError(format!(
                        "preference '{}' must be a boolean",
                        self.key
                    )));
                }
            }
            PluginPreferenceKind::Text
            | PluginPreferenceKind::Folder
            | PluginPreferenceKind::Application => {
                let Some(value) = value.as_str() else {
                    return Err(ManifestError(format!(
                        "preference '{}' must be a string",
                        self.key
                    )));
                };
                if self.required && value.trim().is_empty() {
                    return Err(ManifestError(format!(
                        "preference '{}' is required",
                        self.key
                    )));
                }
                if value.len() > self.max_length.unwrap_or(MAX_PREFERENCE_TEXT) {
                    return Err(ManifestError(format!(
                        "preference '{}' exceeds its maximum length",
                        self.key
                    )));
                }
                if self.kind.is_path() && !value.trim().is_empty() {
                    validate_local_path(value).map_err(|error| {
                        ManifestError(format!("preference '{}': {error}", self.key))
                    })?;
                }
            }
            PluginPreferenceKind::Select => {
                let Some(value) = value.as_str() else {
                    return Err(ManifestError(format!(
                        "preference '{}' must be a string",
                        self.key
                    )));
                };
                if !self.options.iter().any(|option| option.value == value) {
                    return Err(ManifestError(format!(
                        "preference '{}' has an unknown option '{}'",
                        self.key, value
                    )));
                }
            }
        }
        Ok(())
    }

    fn empty_value(&self) -> Option<Value> {
        match self.kind {
            PluginPreferenceKind::Boolean => Some(Value::Bool(false)),
            PluginPreferenceKind::Select => self
                .options
                .first()
                .map(|option| Value::String(option.value.clone())),
            PluginPreferenceKind::Text
            | PluginPreferenceKind::Folder
            | PluginPreferenceKind::Application => Some(Value::String(String::new())),
        }
    }
}

fn valid_preference_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    key.len() <= 64
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn validate_local_path(value: &str) -> Result<(), String> {
    if value.contains('\0') {
        return Err("path contains a NUL character".into());
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    if value.starts_with(r"\\") || value.starts_with("//") {
        return Err("UNC paths are not allowed".into());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("path must not contain '..'".into());
    }
    Ok(())
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn valid_plugin_id(id: &str) -> bool {
    let mut labels = 0;
    for label in id.split('.') {
        labels += 1;
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !label
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !label
                .bytes()
                .last()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            return false;
        }
    }
    labels >= 2
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
            min_host_version: None,
            icon: None,
            package: PACKAGE_FILE.into(),
            preferences: PluginPreferences::default(),
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
    fn ids_and_versions_are_strict() {
        let mut bad = manifest();
        bad.id = "..".into();
        assert!(bad.validate().is_err());
        bad.id = "com.example.".into();
        assert!(bad.validate().is_err());
        bad.id = "com.example bad".into();
        assert!(bad.validate().is_err());

        bad = manifest();
        bad.version = "not-semver".into();
        assert!(bad.validate().is_err());
        bad = manifest();
        bad.min_host_version = Some("host-api".into());
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

    #[test]
    fn hosted_preferences_validate_defaults_options_and_paths() {
        let preferences: PluginPreferences = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "fields": [
                {
                    "key": "theme",
                    "title": "Theme",
                    "type": "select",
                    "default": "system",
                    "options": [
                        { "value": "system", "label": "System" },
                        { "value": "dark", "label": "Dark" }
                    ]
                },
                {
                    "key": "repositoryRoot",
                    "title": "Repository root",
                    "type": "folder",
                    "description": "Where repositories are stored"
                },
                {
                    "key": "autoRefresh",
                    "title": "Auto refresh",
                    "type": "boolean",
                    "default": true
                }
            ]
        }))
        .unwrap();
        preferences.validate().unwrap();
        preferences
            .validate_value("repositoryRoot", &serde_json::json!(r"D:\projects"))
            .unwrap();
        assert!(preferences
            .validate_value("repositoryRoot", &serde_json::json!("relative"))
            .is_err());
        assert!(preferences
            .validate_value("repositoryRoot", &serde_json::json!(r"\\server\share"))
            .is_err());
        assert!(preferences
            .validate_value("theme", &serde_json::json!("unknown"))
            .is_err());
    }

    #[test]
    fn hosted_preferences_apply_defaults_and_key_migrations() {
        let preferences = PluginPreferences {
            schema_version: 1,
            fields: vec![
                PluginPreferenceField {
                    key: "theme".into(),
                    title: "Theme".into(),
                    description: None,
                    kind: PluginPreferenceKind::Select,
                    default: Some(serde_json::json!("system")),
                    required: false,
                    max_length: None,
                    options: vec![
                        PluginPreferenceOption {
                            value: "system".into(),
                            label: "System".into(),
                        },
                        PluginPreferenceOption {
                            value: "dark".into(),
                            label: "Dark".into(),
                        },
                    ],
                },
                PluginPreferenceField {
                    key: "apiToken".into(),
                    title: "API token".into(),
                    description: None,
                    kind: PluginPreferenceKind::Text,
                    default: None,
                    required: true,
                    max_length: Some(128),
                    options: Vec::new(),
                },
            ],
            migrations: vec![PreferenceMigration {
                from: "oldTheme".into(),
                to: "theme".into(),
            }],
        };
        preferences.validate().unwrap();
        let stored = std::collections::BTreeMap::from([
            ("oldTheme".into(), serde_json::json!("dark")),
            ("theme".into(), serde_json::json!("removed-option")),
        ]);
        let values = preferences.resolve_values(&stored);
        assert_eq!(values.get("theme"), Some(&serde_json::json!("dark")));
        assert!(!values.contains_key("apiToken"));
        assert_eq!(
            stored.get("theme"),
            Some(&serde_json::json!("removed-option"))
        );
    }
}

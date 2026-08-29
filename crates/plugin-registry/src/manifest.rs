//! Plugin manifest model, parsing and validation.
//!
//! The manifest (`plugin.json`) is the contract between a plugin author and
//! the host: identity, commands with triggers, the permission whitelist and
//! the isolation level. M2 validates strictly (unknown fields, unknown or
//! unimplemented permissions and malformed triggers are rejected) so a bad
//! plugin fails at scan time with a clear message instead of misbehaving at
//! runtime.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Name of the manifest file inside each plugin directory.
pub const MANIFEST_FILE: &str = "plugin.json";
/// Bundle entry relative to the plugin directory (esbuild IIFE output).
pub const ENTRY_FILE: &str = "dist/index.js";

/// A parsed and validated plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Reverse-domain id, globally unique.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Semantic version; a change is the only reason to re-parse a plugin.
    pub version: String,
    /// Optional launcher row icon: an inline SVG document. The app renders it
    /// next to the plugin's rows, mirroring application icons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Absolute paths/roots this plugin may read from disk; the host sandboxes
    /// `fs.read` to these. Empty by default: no filesystem access.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fs_roots: Vec<String>,
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    /// Capability whitelist; empty by default (zero permissions).
    #[serde(default)]
    pub permissions: Vec<Permission>,
    /// Isolation level; defaults to the in-process shared pool.
    #[serde(default)]
    pub isolation: Isolation,
}

impl PluginManifest {
    /// Validate identity, command triggers and the permission whitelist.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.id.trim().is_empty() {
            return Err(ManifestError("id must not be empty".into()));
        }
        let segments = self.id.split('.').collect::<Vec<_>>();
        if segments.len() < 2 {
            return Err(ManifestError(format!(
                "id '{}' must be a reverse-domain (at least two dot-separated segments)",
                self.id
            )));
        }
        for segment in segments {
            if segment.is_empty()
                || !segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(ManifestError(format!(
                    "id '{}' has an invalid segment (letters, digits, '-' and '_' only)",
                    self.id
                )));
            }
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError(format!(
                "plugin '{}': name must not be empty",
                self.id
            )));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError(format!(
                "plugin '{}': version must not be empty",
                self.id
            )));
        }
        if self
            .icon
            .as_ref()
            .is_some_and(|icon| icon.trim().is_empty())
        {
            return Err(ManifestError(format!(
                "plugin '{}': icon must not be empty when present",
                self.id
            )));
        }
        for root in &self.fs_roots {
            if root.trim().is_empty() {
                return Err(ManifestError(format!(
                    "plugin '{}': fs_roots must not contain empty paths",
                    self.id
                )));
            }
        }
        if self.commands.is_empty() {
            return Err(ManifestError(format!(
                "plugin '{}': at least one command is required",
                self.id
            )));
        }
        let mut names = std::collections::HashSet::new();
        for command in &self.commands {
            command.validate(&self.id)?;
            if !names.insert(&command.name) {
                return Err(ManifestError(format!(
                    "plugin '{}': duplicate command name '{}'",
                    self.id, command.name
                )));
            }
        }
        for permission in &self.permissions {
            if !permission.supported_in_m3() {
                return Err(ManifestError(format!(
                    "plugin '{}': permission '{}' is not supported in M3",
                    self.id, permission
                )));
            }
        }
        Ok(())
    }
}

/// One launcher command exposed by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginCommand {
    /// Command name, matched by the `command` trigger type.
    pub name: String,
    /// Human-readable command title.
    pub title: String,
    pub trigger: Trigger,
    /// Whether the view returned by this command may be popped out into its
    /// own independent window (e.g. a month calendar floats beside the
    /// launcher). Defaults to `false`; affects the launcher's detach affordance
    /// and routing only, never plugin execution or permissions.
    #[serde(default, skip_serializing_if = "is_false")]
    pub detachable: bool,
    /// Optional localized aliases / keywords (e.g. `"日历"` for `calendar`).
    /// The host fuzzy-matches queries against the name, title and these
    /// keywords (with pinyin forms for Chinese text), so plugins can declare
    /// their own multi-language search terms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

/// serde helper: omit `detachable` from the manifest / cache when it is false
/// (the default), keeping existing plugin.json files and cache rows valid.
fn is_false(value: &bool) -> bool {
    !*value
}

impl PluginCommand {
    fn validate(&self, plugin_id: &str) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError(format!(
                "plugin '{}': command name must not be empty",
                plugin_id
            )));
        }
        if self.title.trim().is_empty() {
            return Err(ManifestError(format!(
                "plugin '{}': command '{}' title must not be empty",
                plugin_id, self.name
            )));
        }
        for keyword in &self.keywords {
            if keyword.trim().is_empty() {
                return Err(ManifestError(format!(
                    "plugin '{}': command '{}' keyword must not be empty",
                    plugin_id, self.name
                )));
            }
        }
        match self.trigger.kind {
            TriggerType::Command => {
                if self.trigger.value.is_some() {
                    return Err(ManifestError(format!(
                        "plugin '{}': command '{}' trigger of type 'command' takes no value",
                        plugin_id, self.name
                    )));
                }
            }
            TriggerType::Prefix | TriggerType::Regex => {
                let value = self.trigger.value.as_deref().unwrap_or("");
                if value.is_empty() {
                    return Err(ManifestError(format!(
                        "plugin '{}': command '{}' trigger of type '{}' requires a value",
                        plugin_id, self.name, self.trigger.kind
                    )));
                }
                if self.trigger.kind == TriggerType::Regex {
                    regex::Regex::new(value).map_err(|error| {
                        ManifestError(format!(
                            "plugin '{}': command '{}' has an invalid regex: {error}",
                            plugin_id, self.name
                        ))
                    })?;
                }
            }
            TriggerType::Dynamic => {}
        }
        Ok(())
    }
}

/// A command's trigger condition; the host routes queries through these
/// before ever waking a plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    #[serde(rename = "type")]
    pub kind: TriggerType,
    /// Required for `prefix` / `regex`; must be absent for `command`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TriggerType {
    /// Fixed command name (the command's `name`).
    Command,
    /// Keyword prefix (e.g. `=`) or a natural-language phrase.
    Prefix,
    /// Regular expression match (linear-time; use sparingly).
    Regex,
    /// Participates in every query; must respect the response timeout.
    Dynamic,
}

impl fmt::Display for TriggerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Command => "command",
            Self::Prefix => "prefix",
            Self::Regex => "regex",
            Self::Dynamic => "dynamic",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Isolation {
    /// In-process isolate pool (default): QuickJS heap limit + exec timeout.
    #[serde(rename = "shared-pool")]
    #[default]
    SharedPool,
    /// A dedicated runtime subprocess per plugin instance.
    #[serde(rename = "dedicated-process")]
    DedicatedProcess,
}

/// Capability whitelist entries. Unknown permissions fail deserialization;
/// recognized-but-unimplemented ones fail validation with a clear message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Permission {
    #[serde(rename = "clipboard.read")]
    ClipboardRead,
    #[serde(rename = "clipboard.write")]
    ClipboardWrite,
    #[serde(rename = "clipboard.history")]
    ClipboardHistory,
    #[serde(rename = "open.url")]
    OpenUrl,
    #[serde(rename = "open.path")]
    OpenPath,
    #[serde(rename = "network")]
    Network,
    #[serde(rename = "fs.read")]
    FsRead,
    #[serde(rename = "fs.write")]
    FsWrite,
}

impl Permission {
    /// Permissions with a working host function in M3. The rest are
    /// recognized so the error message is precise, but rejected at scan.
    pub fn supported_in_m3(self) -> bool {
        matches!(
            self,
            Self::ClipboardRead
                | Self::ClipboardWrite
                | Self::ClipboardHistory
                | Self::OpenUrl
                | Self::OpenPath
                | Self::FsRead
                | Self::FsWrite
        )
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ClipboardRead => "clipboard.read",
            Self::ClipboardWrite => "clipboard.write",
            Self::ClipboardHistory => "clipboard.history",
            Self::OpenUrl => "open.url",
            Self::OpenPath => "open.path",
            Self::Network => "network",
            Self::FsRead => "fs.read",
            Self::FsWrite => "fs.write",
        })
    }
}

/// Manifest validation failure with a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError(pub String);

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ManifestError {}

/// Read and validate the manifest inside `dir` (the plugin directory).
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

/// Absolute path of a plugin's bundle entry (`dir/dist/index.js`).
pub fn entry_path(dir: &Path) -> std::path::PathBuf {
    dir.join(ENTRY_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calendar() -> PluginManifest {
        serde_json::from_str(
            r#"{
                "id": "com.example.calendar",
                "name": "Calendar",
                "version": "1.0.0",
                "commands": [
                    {
                        "name": "calendar",
                        "title": "Calendar",
                        "trigger": { "type": "command" }
                    }
                ],
                "permissions": ["clipboard.write"],
                "isolation": "shared-pool"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn calendar_manifest_validates() {
        let manifest = calendar();
        assert_eq!(manifest.isolation, Isolation::SharedPool);
        assert_eq!(manifest.permissions, vec![Permission::ClipboardWrite]);
        assert!(
            manifest.commands[0].keywords.is_empty(),
            "keywords default to empty for backward compatibility"
        );
        assert!(
            !manifest.commands[0].detachable,
            "detachable defaults to false for backward compatibility"
        );
        manifest.validate().unwrap();
    }

    #[test]
    fn detachable_command_roundtrips() {
        let manifest: PluginManifest = serde_json::from_str(
            r#"{
                "id": "com.example.calendar",
                "name": "Calendar",
                "version": "1.0.0",
                "commands": [
                    {
                        "name": "calendar",
                        "title": "Calendar",
                        "detachable": true,
                        "trigger": { "type": "command" }
                    }
                ]
            }"#,
        )
        .unwrap();
        assert!(manifest.commands[0].detachable);
        manifest.validate().unwrap();

        // A false value is omitted from serialization (cache stays compact).
        let ser = serde_json::to_string(&manifest).unwrap();
        assert!(ser.contains("\"detachable\":true"));

        let manifest = calendar();
        assert!(!manifest.commands[0].detachable);
        let ser = serde_json::to_string(&manifest).unwrap();
        assert!(!ser.contains("detachable"));
    }

    #[test]
    fn icon_is_optional_and_roundtrips() {
        let with_icon: PluginManifest = serde_json::from_str(
            r#"{
                "id": "com.example.calendar",
                "name": "Calendar",
                "version": "1.0.0",
                "icon": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"></svg>",
                "commands": [
                    { "name": "calendar", "title": "Calendar", "trigger": { "type": "command" } }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            with_icon.icon.as_deref(),
            Some("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"></svg>")
        );
        with_icon.validate().unwrap();

        // Manifests without an icon (the pre-icon schema) still parse.
        let without_icon = calendar();
        assert!(without_icon.icon.is_none());
        without_icon.validate().unwrap();

        // An empty icon is rejected once present.
        let mut empty = with_icon;
        empty.icon = Some("   ".into());
        assert!(empty.validate().is_err());
    }

    #[test]
    fn localized_keywords_are_accepted() {
        let manifest: PluginManifest = serde_json::from_str(
            r#"{
                "id": "com.example.calendar",
                "name": "Calendar",
                "version": "1.0.0",
                "commands": [
                    {
                        "name": "calendar",
                        "title": "Calendar",
                        "keywords": ["日历", "rili"],
                        "trigger": { "type": "command" }
                    }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.commands[0].keywords, vec!["日历", "rili"]);
        manifest.validate().unwrap();
    }

    #[test]
    fn empty_keyword_is_rejected() {
        let mut manifest = calendar();
        manifest.commands[0].keywords = vec!["  ".into()];
        let error = manifest.validate().unwrap_err();
        assert!(error.0.contains("keyword must not be empty"));
    }

    #[test]
    fn id_must_be_reverse_domain() {
        let mut manifest = calendar();
        manifest.id = "calculator".into();
        assert!(manifest.validate().is_err());
        manifest.id = "com..broken".into();
        assert!(manifest.validate().is_err());
        manifest.id = "com.example.calendar".into();
        manifest.validate().unwrap();
    }

    #[test]
    fn commands_are_required_and_unique() {
        let mut manifest = calendar();
        manifest.commands.clear();
        assert!(manifest.validate().is_err());
        let mut manifest = calendar();
        manifest.commands.push(manifest.commands[0].clone());
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn prefix_trigger_requires_value() {
        let mut manifest = calendar();
        manifest.commands[0].trigger = Trigger {
            kind: TriggerType::Prefix,
            value: None,
        };
        assert!(manifest.validate().is_err());
        manifest.commands[0].trigger.value = Some("cal ".into());
        manifest.validate().unwrap();
    }

    #[test]
    fn command_trigger_rejects_value() {
        let mut manifest = calendar();
        manifest.commands[0].trigger = Trigger {
            kind: TriggerType::Command,
            value: Some("calendar".into()),
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn invalid_regex_is_rejected() {
        let mut manifest = calendar();
        manifest.commands[0].trigger = Trigger {
            kind: TriggerType::Regex,
            value: Some("(".into()),
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn unimplemented_permissions_are_rejected() {
        let mut manifest = calendar();
        manifest.permissions = vec![Permission::Network];
        let error = manifest.validate().unwrap_err();
        assert!(
            error.0.contains("not supported in M3"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn fs_write_permission_is_accepted() {
        let mut manifest = calendar();
        manifest.permissions = vec![Permission::FsWrite];
        manifest.validate().unwrap();
        assert!(manifest.permissions[0].supported_in_m3());
    }

    #[test]
    fn clipboard_history_permission_is_accepted() {
        let mut manifest = calendar();
        manifest.permissions = vec![Permission::ClipboardHistory];
        manifest.validate().unwrap();
    }

    #[test]
    fn open_permissions_are_accepted() {
        for permission in [Permission::OpenUrl, Permission::OpenPath] {
            let mut manifest = calendar();
            manifest.permissions = vec![permission];
            manifest.validate().unwrap();
            assert!(permission.supported_in_m3());
        }
    }

    #[test]
    fn fs_read_permission_and_roots_are_accepted() {
        let mut manifest = calendar();
        manifest.permissions = vec![Permission::FsRead];
        manifest.fs_roots = vec!["C:\\data".into(), "D:\\docs".into()];
        manifest.validate().unwrap();
        assert!(manifest.permissions[0].supported_in_m3());
        assert_eq!(
            manifest.fs_roots,
            vec!["C:\\data".to_string(), "D:\\docs".to_string()]
        );

        // An empty root string is rejected.
        let mut bad = manifest;
        bad.fs_roots = vec!["  ".into()];
        assert!(bad.validate().is_err());
    }

    #[test]
    fn fs_roots_roundtrip_through_json() {
        let json = serde_json::json!({
            "id": "com.example.reader",
            "name": "Reader",
            "version": "1.0.0",
            "fs_roots": ["C:\\data"],
            "commands": [
                { "name": "read", "title": "Read", "trigger": { "type": "command" } }
            ],
            "permissions": ["fs.read"]
        });
        let manifest: PluginManifest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(manifest.fs_roots, vec!["C:\\data".to_string()]);
        assert_eq!(manifest.permissions, vec![Permission::FsRead]);
        manifest.validate().unwrap();

        // Serialization omits an empty fs_roots (cache stays compact).
        let mut empty = manifest;
        empty.fs_roots.clear();
        let ser = serde_json::to_string(&empty).unwrap();
        assert!(!ser.contains("fs_roots"));
    }

    #[test]
    fn unknown_permission_fails_deserialization() {
        let error = serde_json::from_str::<PluginManifest>(
            r#"{
                "id": "com.example.calendar",
                "name": "Calendar",
                "version": "1.0.0",
                "commands": [
                    { "name": "calendar", "title": "Calendar", "trigger": { "type": "command" } }
                ],
                "permissions": ["clipboard.erase"]
            }"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("clipboard.erase"));
    }
}

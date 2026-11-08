//! Versioned DTOs shared by Steward's launcher and plugin-host process.

use serde::{Deserialize, Serialize};

/// Version of the parent/plugin-host protocol.
pub const PROTOCOL_VERSION: u16 = 1;

/// Stable identifier for one plugin launch session.
pub type PluginSessionId = u64;

/// User intent that asks Steward to open a plugin command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchRequest {
    pub plugin_id: String,
    pub command: String,
    pub title: String,
    pub input: String,
    pub locale: String,
    pub accent: String,
    pub dark: bool,
}

/// Launch parameters made available to plugin code through NSH's host bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLaunchParams {
    pub plugin_id: String,
    pub command: String,
    pub input: String,
    pub locale: String,
    pub accent: String,
    pub dark: bool,
}

impl From<&LaunchRequest> for PluginLaunchParams {
    fn from(request: &LaunchRequest) -> Self {
        Self {
            plugin_id: request.plugin_id.clone(),
            command: request.command.clone(),
            input: request.input.clone(),
            locale: request.locale.clone(),
            accent: request.accent.clone(),
            dark: request.dark,
        }
    }
}

/// Stable failure category used by logs and launcher notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginErrorCode {
    NotFound,
    CommandNotFound,
    TrustFailure,
    IdMismatch,
    VersionMismatch,
    ReadFailed,
    LoadFailed,
    FirstFrameFailed,
    RuntimeError,
}

impl PluginErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::CommandNotFound => "command_not_found",
            Self::TrustFailure => "trust_failure",
            Self::IdMismatch => "id_mismatch",
            Self::VersionMismatch => "version_mismatch",
            Self::ReadFailed => "read_failed",
            Self::LoadFailed => "load_failed",
            Self::FirstFrameFailed => "first_frame_failed",
            Self::RuntimeError => "runtime_error",
        }
    }
}

/// User-visible transient notice rendered by the launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginNotice {
    pub title: String,
    pub message: String,
    pub plugin_id: Option<String>,
    pub code: PluginErrorCode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_request_maps_to_plugin_parameters() {
        let request = LaunchRequest {
            plugin_id: "com.example.test".into(),
            command: "run".into(),
            title: "Test".into(),
            input: "hello".into(),
            locale: "en".into(),
            accent: "#863bff".into(),
            dark: true,
        };
        let params = PluginLaunchParams::from(&request);
        assert_eq!(params.plugin_id, request.plugin_id);
        assert_eq!(params.input, request.input);
    }

    #[test]
    fn contract_rejects_unknown_fields() {
        let value = serde_json::json!({
            "pluginId": "p",
            "command": "c",
            "title": "t",
            "input": "i",
            "locale": "en",
            "accent": "#000000",
            "dark": true,
            "unexpected": 1
        });
        assert!(serde_json::from_value::<LaunchRequest>(value).is_err());
    }
}

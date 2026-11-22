//! Versioned DTOs shared by Steward's launcher and plugin-host process.

use serde::{Deserialize, Serialize};

pub mod capabilities;
pub mod host_api;

pub use capabilities::{CapabilityDescriptor, LEGACY_HOST_ACCESS};
pub use host_api::{
    HOST_API_VERSION, HOST_CALL_NAMES, HOST_CONTRACT_VERSION, HOST_ERROR_CODES,
    HOST_ERROR_ENVELOPE_VERSION, HOST_EVENT_NAMES, HOST_FAILURE_STATES, HOST_FAILURE_STATE_CODES,
    HOST_FAMILY_NAMES, HOST_FEATURE_NAMES, HOST_MINIMUM_SUPPORTED_API_VERSION,
    HOST_RETRYABLE_ERROR_CODES,
};

/// Version of the parent/plugin-host protocol.
pub const PROTOCOL_VERSION: u16 = 2;

/// Features that are always available independently of package grants.
pub const BASE_HOST_FEATURES: &[&str] = &[
    "host_info",
    "runtime_feature_discovery",
    "steward.feedback",
    "steward.navigation",
    "steward.preferences",
    "steward.secrets",
];

/// Stable identifier for one plugin launch session.
pub type PluginSessionId = u64;

/// How a plugin command presents its NSH surface.
///
/// Both modes run in the isolated plugin-host process. `Embedded` uses a
/// frameless, launcher-positioned hosted surface so the command feels like a
/// launcher page without moving plugin code into the shell process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresentationMode {
    #[default]
    Windowed,
    Embedded,
}

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
    #[serde(default)]
    pub presentation: PresentationMode,
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
    pub presentation: PresentationMode,
    pub host_api_version: String,
    pub features: Vec<String>,
    pub preferences: std::collections::BTreeMap<String, serde_json::Value>,
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
            presentation: request.presentation,
            host_api_version: HOST_API_VERSION.into(),
            features: BASE_HOST_FEATURES
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect(),
            preferences: Default::default(),
        }
    }
}

/// First message sent from the Steward shell to a plugin-host process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProcessStart {
    pub protocol_version: u16,
    pub session_id: PluginSessionId,
    pub request: LaunchRequest,
}

/// A command sent from the Steward shell to an already-running plugin host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProcessCommand {
    pub protocol_version: u16,
    pub session_id: PluginSessionId,
    pub action: PluginProcessAction,
}

/// Process-level actions that do not require an NSH host call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    deny_unknown_fields
)]
pub enum PluginProcessAction {
    Command {
        command: String,
        input: String,
    },
    Theme {
        accent: String,
        dark: bool,
    },
    Focus,
    Close,
    Back,
    SetTitle {
        title: String,
    },
    ResolveConfirmation {
        id: u64,
        confirmed: bool,
    },
    ResolveExecution {
        request_id: u64,
        result: PluginExecutionResult,
    },
}

/// A lifecycle event sent from a plugin host process to the Steward shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    deny_unknown_fields
)]
pub enum PluginProcessEvent {
    Rendered {
        protocol_version: u16,
        session_id: PluginSessionId,
    },
    Healthy {
        protocol_version: u16,
        session_id: PluginSessionId,
    },
    Notice {
        protocol_version: u16,
        session_id: PluginSessionId,
        notice: PluginNotice,
    },
    Failed {
        protocol_version: u16,
        session_id: PluginSessionId,
        code: PluginErrorCode,
        error: String,
    },
    ExecutionRequest {
        protocol_version: u16,
        session_id: PluginSessionId,
        request: PluginExecutionRequest,
    },
    ExecutionCancel {
        protocol_version: u16,
        session_id: PluginSessionId,
        request_id: u64,
    },
    Presentation {
        protocol_version: u16,
        session_id: PluginSessionId,
        event: PluginPresentationEvent,
    },
}

/// Navigation and visibility notifications emitted by a plugin presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    deny_unknown_fields
)]
pub enum PluginPresentationEvent {
    Back,
    Close,
    TitleChanged { title: String },
    VisibilityChanged { visible: bool },
}

/// One exact-executable request sent from an isolated plugin host to the
/// launcher-owned broker. The broker validates these fields again before any
/// process is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginExecutionRequest {
    pub request_id: u64,
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
}

/// A completed controlled process or a stable failure category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginExecutionResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub cancelled: bool,
    pub error_code: Option<PluginErrorCode>,
    pub error: Option<String>,
}

impl PluginExecutionResult {
    pub fn failure(code: PluginErrorCode, error: impl Into<String>) -> Self {
        Self {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            timed_out: false,
            cancelled: false,
            error_code: Some(code),
            error: Some(error.into()),
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
    ResourceLimit,
    RuntimeError,
    PermissionDenied,
    FeatureUnavailable,
    Timeout,
    ResourceExhausted,
    HostFailure,
    InvalidInput,
    Cancelled,
    ConfirmationDenied,
    IoFailure,
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
            Self::ResourceLimit => "resource_limit",
            Self::RuntimeError => "runtime_error",
            Self::PermissionDenied => "permission_denied",
            Self::FeatureUnavailable => "feature_unavailable",
            Self::Timeout => "timeout",
            Self::ResourceExhausted => "resource_exhausted",
            Self::HostFailure => "host_failure",
            Self::InvalidInput => "invalid_input",
            Self::Cancelled => "cancelled",
            Self::ConfirmationDenied => "confirmation_denied",
            Self::IoFailure => "io_failure",
        }
    }
}

/// Capability and API information returned to plugin code at launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub api_version: String,
    pub features: Vec<String>,
}

impl HostInfo {
    pub fn new(features: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut features = BASE_HOST_FEATURES
            .iter()
            .map(|feature| (*feature).to_owned())
            .chain(features.into_iter().map(Into::into))
            .collect::<Vec<_>>();
        features.sort();
        features.dedup();
        Self {
            api_version: HOST_API_VERSION.into(),
            features,
        }
    }

    pub fn supports(&self, feature: &str) -> bool {
        self.features
            .binary_search_by(|candidate| candidate.as_str().cmp(feature))
            .is_ok()
    }
}

/// User-visible transient notice rendered by the launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginNotice {
    pub title: String,
    pub message: String,
    pub plugin_id: Option<String>,
    pub code: Option<PluginErrorCode>,
    pub level: PluginNoticeLevel,
    pub kind: PluginNoticeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginNoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PluginNoticeKind {
    Toast,
    Confirmation {
        id: u64,
        confirm_label: String,
        cancel_label: String,
        destructive: bool,
    },
    Progress {
        id: String,
        current: Option<u64>,
        total: Option<u64>,
    },
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
            presentation: PresentationMode::Embedded,
        };
        let params = PluginLaunchParams::from(&request);
        assert_eq!(params.plugin_id, request.plugin_id);
        assert_eq!(params.input, request.input);
        assert_eq!(params.host_api_version, HOST_API_VERSION);
        assert_eq!(params.presentation, PresentationMode::Embedded);
        assert!(params.features.iter().any(|feature| feature == "host_info"));
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
            "presentation": "embedded",
            "unexpected": 1
        });
        assert!(serde_json::from_value::<LaunchRequest>(value).is_err());
    }

    #[test]
    fn process_protocol_roundtrips() {
        let request = LaunchRequest {
            plugin_id: "com.example.test".into(),
            command: "run".into(),
            title: "Test".into(),
            input: "hello".into(),
            locale: "en".into(),
            accent: "#863bff".into(),
            dark: true,
            presentation: PresentationMode::Windowed,
        };
        let start = PluginProcessStart {
            protocol_version: PROTOCOL_VERSION,
            session_id: 7,
            request,
        };
        let command = PluginProcessCommand {
            protocol_version: PROTOCOL_VERSION,
            session_id: 7,
            action: PluginProcessAction::Command {
                command: "open".into(),
                input: "item".into(),
            },
        };
        let event = PluginProcessEvent::Healthy {
            protocol_version: PROTOCOL_VERSION,
            session_id: 7,
        };

        assert_eq!(
            serde_json::from_str::<PluginProcessStart>(&serde_json::to_string(&start).unwrap())
                .unwrap(),
            start
        );
        assert_eq!(
            serde_json::from_str::<PluginProcessCommand>(&serde_json::to_string(&command).unwrap())
                .unwrap(),
            command
        );
        assert_eq!(
            serde_json::from_str::<PluginProcessEvent>(&serde_json::to_string(&event).unwrap())
                .unwrap(),
            event
        );
    }

    #[test]
    fn process_actions_use_a_tagged_shape() {
        let value = serde_json::to_value(PluginProcessAction::Focus).unwrap();
        assert_eq!(value, serde_json::json!({ "type": "focus" }));
    }

    #[test]
    fn presentation_modes_and_lifecycle_events_roundtrip() {
        assert_eq!(
            serde_json::to_value(PresentationMode::Embedded).unwrap(),
            serde_json::json!("embedded")
        );
        let action = PluginProcessAction::SetTitle {
            title: "Repositories".into(),
        };
        assert_eq!(
            serde_json::from_str::<PluginProcessAction>(&serde_json::to_string(&action).unwrap())
                .unwrap(),
            action
        );
        let event = PluginProcessEvent::Presentation {
            protocol_version: PROTOCOL_VERSION,
            session_id: 7,
            event: PluginPresentationEvent::VisibilityChanged { visible: false },
        };
        assert_eq!(
            serde_json::from_str::<PluginProcessEvent>(&serde_json::to_string(&event).unwrap())
                .unwrap(),
            event
        );
    }

    #[test]
    fn feedback_events_and_confirmation_actions_roundtrip() {
        let notice = PluginNotice {
            title: "Move repository".into(),
            message: "Move this repository to the Recycle Bin?".into(),
            plugin_id: Some("com.example.git".into()),
            code: None,
            level: PluginNoticeLevel::Warning,
            kind: PluginNoticeKind::Confirmation {
                id: 42,
                confirm_label: "Move".into(),
                cancel_label: "Cancel".into(),
                destructive: true,
            },
        };
        let event = PluginProcessEvent::Notice {
            protocol_version: PROTOCOL_VERSION,
            session_id: 7,
            notice,
        };
        let action = PluginProcessAction::ResolveConfirmation {
            id: 42,
            confirmed: true,
        };
        assert_eq!(
            serde_json::from_str::<PluginProcessEvent>(&serde_json::to_string(&event).unwrap())
                .unwrap(),
            event
        );
        assert_eq!(
            serde_json::from_str::<PluginProcessAction>(&serde_json::to_string(&action).unwrap())
                .unwrap(),
            action
        );
    }

    #[test]
    fn execution_requests_and_results_roundtrip() {
        let request = PluginExecutionRequest {
            request_id: 9,
            executable: "git.exe".into(),
            arguments: vec!["status".into(), "--porcelain=v1".into()],
            working_directory: r"D:\projects\steward".into(),
            timeout_ms: 30_000,
            max_output_bytes: 1024 * 1024,
        };
        let event = PluginProcessEvent::ExecutionRequest {
            protocol_version: PROTOCOL_VERSION,
            session_id: 7,
            request: request.clone(),
        };
        let result = PluginExecutionResult {
            exit_code: Some(0),
            stdout: "clean".into(),
            stderr: String::new(),
            duration_ms: 42,
            timed_out: false,
            cancelled: false,
            error_code: None,
            error: None,
        };
        let action = PluginProcessAction::ResolveExecution {
            request_id: 9,
            result: result.clone(),
        };
        assert_eq!(
            serde_json::from_str::<PluginProcessEvent>(&serde_json::to_string(&event).unwrap())
                .unwrap(),
            event
        );
        assert_eq!(
            serde_json::from_str::<PluginProcessAction>(&serde_json::to_string(&action).unwrap())
                .unwrap(),
            action
        );
    }

    #[test]
    fn host_info_discovers_features_deterministically() {
        let info = HostInfo::new(["steward.storage.get", "host_info"]);
        assert_eq!(info.api_version, HOST_API_VERSION);
        assert!(info.supports("host_info"));
        assert!(info.supports("steward.storage.get"));
        assert!(!info.supports("steward.net.fetch"));
        assert_eq!(
            info.features,
            vec![
                "host_info",
                "runtime_feature_discovery",
                "steward.feedback",
                "steward.navigation",
                "steward.preferences",
                "steward.secrets",
                "steward.storage.get"
            ]
        );
    }
}

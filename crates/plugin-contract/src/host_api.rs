//! Machine-readable plugin host call contract and shared wire DTOs.
//!
//! `contracts/plugin-api-v1.json` is the source of truth. The generated
//! constants below are checked in CI, while these DTOs validate the same
//! committed payload fixtures that drive the TypeScript SDK tests.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod generated {
    include!("host_contract.generated.rs");
}

pub use generated::{
    HOST_API_VERSION, HOST_CALL_NAMES, HOST_CONTRACT_VERSION, HOST_ERROR_CODES,
    HOST_ERROR_ENVELOPE_VERSION, HOST_EVENT_NAMES, HOST_FAILURE_STATES, HOST_FAILURE_STATE_CODES,
    HOST_FAMILY_NAMES, HOST_FEATURE_NAMES, HOST_MINIMUM_SUPPORTED_API_VERSION,
    HOST_RETRYABLE_ERROR_CODES,
};

#[cfg(test)]
pub(crate) const CONTRACT_JSON: &str = include_str!("../../../contracts/plugin-api-v1.json");
#[cfg(test)]
pub(crate) const COMPATIBILITY_FIXTURES_JSON: &str =
    include_str!("../../../contracts/plugin-api-compatibility-v1.json");

/// Stable public host-call rejection returned to plugin code.
///
/// NSH transports rejection messages as strings, so Steward serializes this
/// object as JSON in that string. The SDK parses the JSON without inspecting
/// prose or error-message patterns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostCallError {
    pub version: u32,
    pub code: HostErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl HostCallError {
    pub fn new(code: HostErrorCode, message: impl Into<String>) -> Self {
        Self {
            version: HOST_ERROR_ENVELOPE_VERSION,
            retryable: code.retryable(),
            code,
            message: message.into(),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(HostErrorCode::InvalidInput, message)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(HostErrorCode::PermissionDenied, message)
    }

    pub fn host_failure(message: impl Into<String>) -> Self {
        Self::new(HostErrorCode::HostFailure, message)
    }

    pub fn to_wire(&self) -> String {
        serde_json::to_string(self).expect("host call error must serialize")
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        serde_json::from_str(value).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostErrorCode {
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

impl HostErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
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

    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::ResourceExhausted | Self::HostFailure | Self::Cancelled
        )
    }
}

pub fn parse_call_data<T: DeserializeOwned>(
    call_name: &str,
    value: &Value,
) -> Result<T, HostCallError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        HostCallError::invalid_input(format!(
            "{call_name} has an invalid request payload: {error}"
        ))
    })
}

pub fn require_null_payload(call_name: &str, value: &Value) -> Result<(), HostCallError> {
    if value.is_null() {
        Ok(())
    } else {
        Err(HostCallError::invalid_input(format!(
            "{call_name} does not accept request data"
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencesGetRequest {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToastRequest {
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmRequest {
    pub title: String,
    pub message: String,
    #[serde(default)]
    pub confirm_label: Option<String>,
    #[serde(default)]
    pub cancel_label: Option<String>,
    #[serde(default)]
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgressRequest {
    pub id: String,
    pub message: String,
    #[serde(default)]
    pub current: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipboardWriteRequest {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageGetRequest {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageSetRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretGetRequest {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretSetRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretDeleteRequest {
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarHolidaysRequest {
    pub year: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathPickMode {
    FolderRead,
    FolderWrite,
    FileRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPickRequest {
    pub mode: PathPickMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelWalkRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalkRequest {
    pub id: String,
    pub root: String,
    #[serde(default)]
    pub max_results: Option<u64>,
    #[serde(default)]
    pub max_depth: Option<u64>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadTextRequest {
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteTextRequest {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemPathRequest {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemOpenWithRequest {
    pub application_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkFetchRequest {
    pub url: String,
    pub method: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    pub timeout_ms: u64,
    #[serde(default)]
    pub max_response_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionCancelRequest {
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetTitleRequest {
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathResponse {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelledResponse {
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeletedResponse {
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmResponse {
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipboardEntry {
    pub id: String,
    pub text: String,
    #[serde(rename = "copied_at")]
    pub copied_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemGrant {
    pub path: String,
    pub read: bool,
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalkEntry {
    pub path: String,
    pub relative_path: String,
    pub directory: bool,
    pub reparse_point: bool,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalkResult {
    pub root: String,
    pub entries: Vec<WalkEntry>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledApplication {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpResponse {
    pub status: u16,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body: String,
    pub redirects: u32,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessRunResponse {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub cancelled: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{json, Value};

    use super::*;
    use crate::capabilities;

    fn contract() -> Value {
        serde_json::from_str(CONTRACT_JSON).expect("contract JSON")
    }

    fn call<'a>(contract: &'a Value, name: &str) -> &'a Value {
        contract["calls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|call| call["name"] == name)
            .unwrap_or_else(|| panic!("missing contract call {name}"))
    }

    fn request<'a>(contract: &'a Value, name: &str) -> &'a Value {
        &call(contract, name)["request"]["examples"][0]
    }

    fn response<'a>(contract: &'a Value, name: &str) -> &'a Value {
        &call(contract, name)["response"]["examples"][0]
    }

    fn parse<T: DeserializeOwned>(contract: &Value, name: &str) -> T {
        parse_call_data(name, request(contract, name)).unwrap()
    }

    #[test]
    fn generated_constants_match_the_contract_fixture() {
        let contract = contract();
        assert_eq!(HOST_API_VERSION, contract["hostApiVersion"]);
        assert_eq!(HOST_CONTRACT_VERSION as u64, contract["contractVersion"]);
        assert_eq!(
            HOST_ERROR_ENVELOPE_VERSION as u64,
            contract["errorEnvelopeVersion"]
        );
        let expected_calls = contract["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| call["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(HOST_CALL_NAMES, expected_calls);
        let expected_events = contract["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(HOST_EVENT_NAMES, expected_events);
    }

    #[test]
    fn public_error_codes_are_data_and_match_the_generated_contract() {
        let contract = contract();
        let expected = contract["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["code"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(HOST_ERROR_CODES, expected);
        let expected_retryable = contract["errors"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|error| error["retryable"].as_bool() == Some(true))
            .map(|error| error["code"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(HOST_RETRYABLE_ERROR_CODES, expected_retryable);
        for error in contract["errors"].as_array().unwrap() {
            let value = error["code"].as_str().unwrap();
            let code: HostErrorCode = serde_json::from_value(json!(value)).unwrap();
            assert_eq!(code.as_str(), value);
            assert_eq!(code.retryable(), error["retryable"].as_bool().unwrap());
        }
    }

    #[test]
    fn host_call_errors_roundtrip_as_structured_wire_data() {
        let error = HostCallError::new(HostErrorCode::PermissionDenied, "not granted");
        let wire = error.to_wire();
        assert_eq!(HostCallError::from_wire(&wire), Some(error));
        assert!(wire.starts_with("{\"version\":1,\"code\":\"permission_denied\""));
    }

    #[test]
    fn every_committed_request_fixture_is_accepted_by_a_shared_rust_dto() {
        let contract = contract();
        parse::<PreferencesGetRequest>(&contract, "steward.preferences.get");
        parse::<ToastRequest>(&contract, "steward.feedback.toast");
        parse::<ConfirmRequest>(&contract, "steward.feedback.confirm");
        parse::<ProgressRequest>(&contract, "steward.feedback.progress");
        parse::<ClipboardWriteRequest>(&contract, "steward.clipboard.write");
        parse::<StorageGetRequest>(&contract, "steward.storage.get");
        parse::<StorageSetRequest>(&contract, "steward.storage.set");
        parse::<SecretGetRequest>(&contract, "steward.secrets.get");
        parse::<SecretSetRequest>(&contract, "steward.secrets.set");
        parse::<SecretDeleteRequest>(&contract, "steward.secrets.delete");
        parse::<CalendarHolidaysRequest>(&contract, "steward.calendar.holidays");
        parse::<PathPickRequest>(&contract, "steward.fs.pick");
        parse::<CancelWalkRequest>(&contract, "steward.fs.cancelWalk");
        parse::<WalkRequest>(&contract, "steward.fs.walk");
        parse::<ReadTextRequest>(&contract, "steward.fs.readText");
        parse::<WriteTextRequest>(&contract, "steward.fs.writeText");
        parse::<SystemPathRequest>(&contract, "steward.system.openPath");
        parse::<SystemPathRequest>(&contract, "steward.system.revealPath");
        parse::<SystemOpenWithRequest>(&contract, "steward.system.openWith");
        parse::<NetworkFetchRequest>(&contract, "steward.net.fetch");
        parse::<crate::PluginExecutionRequest>(&contract, "steward.exec.run");
        parse::<ExecutionCancelRequest>(&contract, "steward.exec.cancel");
        parse::<SetTitleRequest>(&contract, "steward.navigation.setTitle");
    }

    #[test]
    fn every_committed_response_fixture_matches_a_shared_rust_dto() {
        let contract = contract();
        let _: Value = response(&contract, "steward.preferences.get").clone();
        let _: BTreeMap<String, Value> =
            serde_json::from_value(response(&contract, "steward.preferences.all").clone()).unwrap();
        let _: ConfirmResponse =
            serde_json::from_value(response(&contract, "steward.feedback.confirm").clone())
                .unwrap();
        for name in [
            "steward.feedback.toast",
            "steward.feedback.progress",
            "steward.clipboard.write",
            "steward.storage.set",
            "steward.secrets.set",
            "steward.navigation.back",
            "steward.navigation.close",
            "steward.navigation.setTitle",
            "steward.system.openWith",
        ] {
            let value: serde_json::Map<String, Value> =
                serde_json::from_value(response(&contract, name).clone()).unwrap();
            assert!(
                value.is_empty(),
                "{name} fixture must have an empty response"
            );
        }
        let _: Vec<ClipboardEntry> =
            serde_json::from_value(response(&contract, "steward.clipboard.history").clone())
                .unwrap();
        let _: PathResponse =
            serde_json::from_value(response(&contract, "steward.fs.pick").clone()).unwrap();
        let _: CancelledResponse =
            serde_json::from_value(response(&contract, "steward.fs.cancelWalk").clone()).unwrap();
        let _: WalkResult =
            serde_json::from_value(response(&contract, "steward.fs.walk").clone()).unwrap();
        let _: String =
            serde_json::from_value(response(&contract, "steward.fs.readText").clone()).unwrap();
        let _: String =
            serde_json::from_value(response(&contract, "steward.secrets.get").clone()).unwrap();
        let _: Vec<String> =
            serde_json::from_value(response(&contract, "steward.secrets.list").clone()).unwrap();
        let _: DeletedResponse =
            serde_json::from_value(response(&contract, "steward.secrets.delete").clone()).unwrap();
        let _: PathResponse =
            serde_json::from_value(response(&contract, "steward.fs.writeText").clone()).unwrap();
        let _: Vec<FilesystemGrant> =
            serde_json::from_value(response(&contract, "steward.fs.grants").clone()).unwrap();
        let _: Vec<InstalledApplication> =
            serde_json::from_value(response(&contract, "steward.system.applications").clone())
                .unwrap();
        let _: PathResponse =
            serde_json::from_value(response(&contract, "steward.system.openPath").clone()).unwrap();
        let _: PathResponse =
            serde_json::from_value(response(&contract, "steward.system.revealPath").clone())
                .unwrap();
        let _: HttpResponse =
            serde_json::from_value(response(&contract, "steward.net.fetch").clone()).unwrap();
        let _: ProcessRunResponse =
            serde_json::from_value(response(&contract, "steward.exec.run").clone()).unwrap();
        let _: CancelledResponse =
            serde_json::from_value(response(&contract, "steward.exec.cancel").clone()).unwrap();
    }

    #[test]
    fn capability_constants_match_the_contracts_public_call_scopes() {
        let contract = contract();
        let expected = contract["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|capability| capability["permission"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        let actual = BTreeSet::from([
            capabilities::PREFERENCES_GET,
            capabilities::PREFERENCES_ALL,
            capabilities::FEEDBACK_TOAST,
            capabilities::FEEDBACK_CONFIRM,
            capabilities::FEEDBACK_PROGRESS,
            capabilities::CLIPBOARD_HISTORY,
            capabilities::CLIPBOARD_WRITE,
            capabilities::STORAGE_GET,
            capabilities::STORAGE_SET,
            capabilities::SECRETS_GET,
            capabilities::SECRETS_SET,
            capabilities::SECRETS_DELETE,
            capabilities::SECRETS_LIST,
            capabilities::CALENDAR_HOLIDAYS,
            capabilities::WINDOW_CLOSE,
            capabilities::NAVIGATION_BACK,
            capabilities::NAVIGATION_CLOSE,
            capabilities::NAVIGATION_SET_TITLE,
            capabilities::FS_PICK,
            capabilities::FS_WALK,
            capabilities::FS_READ_TEXT,
            capabilities::FS_WRITE_TEXT,
            capabilities::FS_GRANTS,
            capabilities::FS_CANCEL_WALK,
            capabilities::SYSTEM_APPLICATIONS,
            capabilities::SYSTEM_OPEN_PATH,
            capabilities::SYSTEM_REVEAL_PATH,
            capabilities::SYSTEM_OPEN_WITH,
            capabilities::NET_FETCH,
            capabilities::EXEC_RUN,
            capabilities::EXEC_CANCEL,
        ]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn every_api_family_has_hardened_failure_and_quota_metadata() {
        let contract = contract();
        let families = contract["families"].as_array().unwrap();
        assert_eq!(HOST_FAMILY_NAMES.len(), families.len());
        assert_eq!(
            HOST_FAILURE_STATES.len(),
            contract["failureStates"].as_array().unwrap().len()
        );
        assert_eq!(HOST_FAILURE_STATE_CODES.len(), HOST_FAILURE_STATES.len());
        assert_eq!(
            contract["stability"]["minimumSupportedApiVersion"],
            HOST_MINIMUM_SUPPORTED_API_VERSION
        );
        for (index, family) in families.iter().enumerate() {
            assert_eq!(HOST_FAMILY_NAMES[index], family["name"]);
            assert!(
                family["maxOperationTimeoutMs"].as_u64().is_some(),
                "{} has no maximum operation timeout",
                family["name"]
            );
            assert!(
                !family["quotas"].as_array().unwrap().is_empty(),
                "{} has no quotas",
                family["name"]
            );
            for (state, code) in HOST_FAILURE_STATES
                .iter()
                .zip(HOST_FAILURE_STATE_CODES.iter())
            {
                let expected = contract["failureStates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|failure| failure["state"] == *state)
                    .unwrap();
                assert_eq!(expected["code"], *code);
                let parsed: HostErrorCode =
                    serde_json::from_value(serde_json::json!(code)).unwrap();
                let error = HostCallError::new(parsed, format!("{state} test failure"));
                assert_eq!(error.code.as_str(), *code);
            }
        }
    }

    #[test]
    fn prior_api_line_fixtures_still_parse_through_current_host_dtos() {
        let fixtures: Value =
            serde_json::from_str(COMPATIBILITY_FIXTURES_JSON).expect("compatibility fixtures");
        for line in fixtures["lines"].as_array().unwrap() {
            for fixture in line["calls"].as_array().unwrap() {
                let name = fixture["name"].as_str().unwrap();
                assert!(
                    HOST_CALL_NAMES.contains(&name),
                    "compatibility fixture {name} is not registered"
                );
                match name {
                    "steward.fs.pick" => {
                        parse_call_data::<PathPickRequest>(name, &fixture["request"]).unwrap();
                        parse_call_data::<PathResponse>(name, &fixture["response"]).unwrap();
                    }
                    "steward.fs.readText" => {
                        parse_call_data::<ReadTextRequest>(name, &fixture["request"]).unwrap();
                        parse_call_data::<String>(name, &fixture["response"]).unwrap();
                    }
                    "steward.system.openPath" | "steward.system.revealPath" => {
                        parse_call_data::<SystemPathRequest>(name, &fixture["request"]).unwrap();
                        parse_call_data::<PathResponse>(name, &fixture["response"]).unwrap();
                    }
                    "steward.net.fetch" => {
                        parse_call_data::<NetworkFetchRequest>(name, &fixture["request"]).unwrap();
                        parse_call_data::<HttpResponse>(name, &fixture["response"]).unwrap();
                    }
                    "steward.exec.run" => {
                        parse_call_data::<crate::PluginExecutionRequest>(name, &fixture["request"])
                            .unwrap();
                        parse_call_data::<ProcessRunResponse>(name, &fixture["response"]).unwrap();
                    }
                    "steward.exec.cancel" => {
                        parse_call_data::<ExecutionCancelRequest>(name, &fixture["request"])
                            .unwrap();
                        parse_call_data::<CancelledResponse>(name, &fixture["response"]).unwrap();
                    }
                    "steward.navigation.setTitle" => {
                        parse_call_data::<SetTitleRequest>(name, &fixture["request"]).unwrap();
                    }
                    "steward.navigation.back" | "steward.window.close" => {
                        require_null_payload(name, &fixture["request"]).unwrap();
                    }
                    "steward.preferences.get" => {
                        parse_call_data::<PreferencesGetRequest>(name, &fixture["request"])
                            .unwrap();
                        parse_call_data::<String>(name, &fixture["response"]).unwrap();
                    }
                    "steward.feedback.confirm" => {
                        parse_call_data::<ConfirmRequest>(name, &fixture["request"]).unwrap();
                        parse_call_data::<ConfirmResponse>(name, &fixture["response"]).unwrap();
                    }
                    "steward.clipboard.write" => {
                        parse_call_data::<ClipboardWriteRequest>(name, &fixture["request"])
                            .unwrap();
                    }
                    "steward.secrets.get" => {
                        parse_call_data::<SecretGetRequest>(name, &fixture["request"]).unwrap();
                        parse_call_data::<String>(name, &fixture["response"]).unwrap();
                    }
                    "steward.secrets.set" => {
                        parse_call_data::<SecretSetRequest>(name, &fixture["request"]).unwrap();
                    }
                    "steward.secrets.delete" => {
                        parse_call_data::<SecretDeleteRequest>(name, &fixture["request"]).unwrap();
                        parse_call_data::<DeletedResponse>(name, &fixture["response"]).unwrap();
                    }
                    "steward.secrets.list" => {
                        require_null_payload(name, &fixture["request"]).unwrap();
                        parse_call_data::<Vec<String>>(name, &fixture["response"]).unwrap();
                    }
                    other => panic!("compatibility fixture {other} has no DTO assertion"),
                }
            }
        }
    }
}

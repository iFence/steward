// Generated from contracts/plugin-api-v1.json. Do not edit directly.
// Run `pnpm contract:generate` after changing the contract.

#[rustfmt::skip]
pub const HOST_API_VERSION: &str = "1.0.0";
#[rustfmt::skip]
pub const HOST_CONTRACT_VERSION: u32 = 1;
#[rustfmt::skip]
pub const HOST_ERROR_ENVELOPE_VERSION: u32 = 1;
#[rustfmt::skip]
pub const HOST_MINIMUM_SUPPORTED_API_VERSION: &str = "0.3.0";
#[rustfmt::skip]
pub const HOST_ERROR_CODES: &[&str] = &[
    "permission_denied",
    "feature_unavailable",
    "timeout",
    "resource_exhausted",
    "host_failure",
    "invalid_input",
    "cancelled",
    "confirmation_denied",
    "io_failure",
];
#[rustfmt::skip]
pub const HOST_RETRYABLE_ERROR_CODES: &[&str] = &[
    "timeout",
    "resource_exhausted",
    "host_failure",
    "cancelled",
];
#[rustfmt::skip]
pub const HOST_CALL_NAMES: &[&str] = &[
    "steward.preferences.get",
    "steward.preferences.all",
    "steward.feedback.toast",
    "steward.feedback.confirm",
    "steward.feedback.progress",
    "steward.clipboard.history",
    "steward.clipboard.write",
    "steward.storage.get",
    "steward.storage.set",
    "steward.calendar.holidays",
    "steward.window.close",
    "steward.navigation.back",
    "steward.navigation.close",
    "steward.navigation.setTitle",
    "steward.fs.pick",
    "steward.fs.cancelWalk",
    "steward.fs.walk",
    "steward.fs.readText",
    "steward.fs.writeText",
    "steward.fs.grants",
    "steward.system.applications",
    "steward.system.openPath",
    "steward.system.revealPath",
    "steward.system.openWith",
    "steward.net.fetch",
    "steward.exec.run",
    "steward.exec.cancel",
];
#[rustfmt::skip]
pub const HOST_EVENT_NAMES: &[&str] = &[
    "steward.command",
    "steward.theme",
    "steward.visibility",
    "steward.progress",
];
#[rustfmt::skip]
pub const HOST_FEATURE_NAMES: &[&str] = &[
    "host_info",
    "runtime_feature_discovery",
    "steward.feedback",
    "steward.navigation",
    "steward.preferences",
];
#[rustfmt::skip]
pub const HOST_FAMILY_NAMES: &[&str] = &[
    "preferences",
    "feedback",
    "clipboard",
    "storage",
    "calendar",
    "navigation",
    "filesystem",
    "system",
    "network",
    "process",
];
#[rustfmt::skip]
pub const HOST_FAILURE_STATES: &[&str] = &[
    "disabled",
    "revoked",
    "expired",
    "malformed",
    "timedOut",
    "cancelled",
    "resourceExhausted",
];
#[rustfmt::skip]
pub const HOST_FAILURE_STATE_CODES: &[&str] = &[
    "feature_unavailable",
    "permission_denied",
    "permission_denied",
    "invalid_input",
    "timeout",
    "cancelled",
    "resource_exhausted",
];

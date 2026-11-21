//! Public capability names, descriptions, and package declaration helpers.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CLIPBOARD_HISTORY: &str = "steward.clipboard.history";
pub const CLIPBOARD_WRITE: &str = "steward.clipboard.write";
pub const STORAGE_GET: &str = "steward.storage.get";
pub const STORAGE_SET: &str = "steward.storage.set";
pub const CALENDAR_HOLIDAYS: &str = "steward.calendar.holidays";
pub const WINDOW_CLOSE: &str = "steward.window.close";
pub const NAVIGATION_BACK: &str = "steward.navigation.back";
pub const NAVIGATION_CLOSE: &str = "steward.navigation.close";
pub const NAVIGATION_SET_TITLE: &str = "steward.navigation.setTitle";
pub const PREFERENCES_GET: &str = "steward.preferences.get";
pub const PREFERENCES_ALL: &str = "steward.preferences.all";
pub const FEEDBACK_TOAST: &str = "steward.feedback.toast";
pub const FEEDBACK_CONFIRM: &str = "steward.feedback.confirm";
pub const FEEDBACK_PROGRESS: &str = "steward.feedback.progress";

pub const FS_READ: &str = "steward.fs.read";
pub const FS_WRITE: &str = "steward.fs.write";
pub const FS_PICK: &str = "steward.fs.pick";
pub const FS_WALK: &str = "steward.fs.walk";
pub const FS_READ_TEXT: &str = "steward.fs.readText";
pub const FS_WRITE_TEXT: &str = "steward.fs.writeText";
pub const FS_GRANTS: &str = "steward.fs.grants";
pub const FS_CANCEL_WALK: &str = "steward.fs.cancelWalk";
pub const NET_FETCH: &str = "steward.net.fetch";
pub const EXEC_RUN: &str = "steward.exec.run";
pub const EXEC_CANCEL: &str = "steward.exec.cancel";
pub const SYSTEM_OPEN_URL: &str = "steward.system.openUrl";
pub const SYSTEM_OPEN_PATH: &str = "steward.system.openPath";
pub const SYSTEM_REVEAL_PATH: &str = "steward.system.revealPath";
pub const SYSTEM_OPEN_WITH: &str = "steward.system.openWith";
pub const SYSTEM_APPLICATIONS: &str = "steward.system.applications";
pub const LEGACY_HOST_ACCESS: &str = "steward.*";

pub const LEGACY_OFFICIAL_PLUGIN_IDS: &[&str] = &[
    "it.aspirin.calendar",
    "it.aspirin.clipboard-history",
    "it.aspirin.ui-showcase",
];

/// One user-facing, resource-scoped capability declaration.
///
/// Permission declarations are requests, not grants. A matching active grant
/// is still required before a sensitive capability reaches the plugin runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    pub permission: String,
    pub scope: String,
    pub description: String,
    pub sensitive: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub legacy_wildcard: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl CapabilityDescriptor {
    pub fn new(permission: impl Into<String>, scope: impl Into<String>) -> Self {
        let permission = permission.into();
        let scope = scope.into();
        Self {
            description: describe(&permission, &scope),
            sensitive: is_sensitive(&permission, &scope),
            legacy_wildcard: is_legacy_wildcard(&permission),
            permission,
            scope,
        }
    }
}

pub fn is_legacy_official_plugin_id(plugin_id: &str) -> bool {
    LEGACY_OFFICIAL_PLUGIN_IDS.contains(&plugin_id)
}

/// Produces stable descriptions from the serialized NSH `capabilities` object.
pub fn describe_declared_capabilities(value: &Value) -> Vec<CapabilityDescriptor> {
    let mut descriptors = BTreeSet::new();
    let Some(root) = value.as_object() else {
        return Vec::new();
    };

    describe_host(root.get("host"), &mut descriptors);
    describe_filesystem(root.get("fs"), &mut descriptors);
    describe_network(root.get("network"), &mut descriptors);
    describe_process(root.get("process"), &mut descriptors);
    describe_open_url(root.get("openUrl"), &mut descriptors);
    describe_boolean(
        root,
        "clipboard",
        "steward.system.clipboard",
        &mut descriptors,
    );
    describe_boolean(root, "picker", "steward.system.picker", &mut descriptors);
    describe_boolean(root, "share", "steward.system.share", &mut descriptors);
    describe_boolean(root, "speech", "steward.system.speech", &mut descriptors);
    describe_boolean(
        root,
        "notifications",
        "steward.system.notifications",
        &mut descriptors,
    );
    describe_boolean(root, "audio", "steward.system.audio", &mut descriptors);
    describe_boolean(
        root,
        "application",
        "steward.system.application",
        &mut descriptors,
    );
    describe_boolean(root, "homeDir", "steward.system.homeDir", &mut descriptors);
    describe_boolean(
        root,
        "statusBar",
        "steward.system.statusBar",
        &mut descriptors,
    );
    describe_boolean(
        root,
        "webviewInput",
        "steward.webview.input",
        &mut descriptors,
    );
    describe_webview(root.get("webview"), &mut descriptors);
    describe_windows(root.get("windows"), &mut descriptors);

    descriptors.into_iter().collect()
}

pub fn declaration_is_auto_granted(descriptor: &CapabilityDescriptor) -> bool {
    !descriptor.sensitive
}

fn describe_host(value: Option<&Value>, output: &mut BTreeSet<CapabilityDescriptor>) {
    let Some(names) = value.and_then(Value::as_array) else {
        return;
    };
    for name in names.iter().filter_map(Value::as_str) {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        output.insert(CapabilityDescriptor::new(name, "host=steward"));
    }
}

fn describe_filesystem(value: Option<&Value>, output: &mut BTreeSet<CapabilityDescriptor>) {
    let Some(fs) = value.and_then(Value::as_object) else {
        return;
    };
    describe_paths(fs.get("read"), FS_READ, output);
    describe_paths(fs.get("write"), FS_WRITE, output);
}

fn describe_paths(
    value: Option<&Value>,
    permission: &str,
    output: &mut BTreeSet<CapabilityDescriptor>,
) {
    let Some(roots) = value.and_then(Value::as_array) else {
        return;
    };
    for root in roots.iter().filter_map(Value::as_str) {
        if !root.trim().is_empty() {
            output.insert(CapabilityDescriptor::new(
                permission,
                format!("path={root}"),
            ));
        }
    }
}

fn describe_network(value: Option<&Value>, output: &mut BTreeSet<CapabilityDescriptor>) {
    let Some(network) = value.and_then(Value::as_object) else {
        return;
    };
    if let Some(hosts) = network.get("hosts").and_then(Value::as_array) {
        for host in hosts.iter().filter_map(Value::as_str) {
            output.insert(CapabilityDescriptor::new(
                NET_FETCH,
                format!("host={}", host.to_ascii_lowercase()),
            ));
        }
    }
    if let Some(grants) = network.get("http").and_then(Value::as_array) {
        for grant in grants.iter().filter_map(Value::as_object) {
            output.insert(CapabilityDescriptor::new(NET_FETCH, http_scope(grant)));
        }
    }
    if let Some(sockets) = network.get("sockets").and_then(Value::as_array) {
        for socket in sockets.iter().filter_map(Value::as_object) {
            let host = socket
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let port = socket
                .get("port")
                .and_then(Value::as_u64)
                .map(|port| port.to_string())
                .unwrap_or_else(|| "*".into());
            output.insert(CapabilityDescriptor::new(
                "steward.net.socket",
                format!("host={host};port={port}"),
            ));
        }
    }
}

fn http_scope(grant: &Map<String, Value>) -> String {
    let host = grant
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let schemes = scope_list(grant.get("schemes"));
    let methods = scope_list(grant.get("methods"));
    let paths = scope_list(grant.get("paths"));
    let prefixes = scope_list(grant.get("pathPrefixes"));
    let port = grant
        .get("port")
        .and_then(Value::as_u64)
        .map(|port| port.to_string())
        .unwrap_or_default();
    format!(
        "host={host};schemes={};methods={};paths={};pathPrefixes={};port={port}",
        schemes.join(","),
        methods.join(","),
        paths.join(","),
        prefixes.join(",")
    )
}

fn scope_list(value: Option<&Value>) -> Vec<String> {
    let mut values = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn describe_process(value: Option<&Value>, output: &mut BTreeSet<CapabilityDescriptor>) {
    let Some(process) = value.and_then(Value::as_object) else {
        return;
    };
    if let Some(executables) = process.get("execute").and_then(Value::as_array) {
        for executable in executables.iter().filter_map(Value::as_str) {
            output.insert(CapabilityDescriptor::new(
                EXEC_RUN,
                format!("executable={executable}"),
            ));
        }
    }
    if process.get("exit").and_then(Value::as_bool) == Some(true) {
        output.insert(CapabilityDescriptor::new(
            "steward.exec.exit",
            "process=self",
        ));
    }
    if let Some(names) = process.get("env").and_then(Value::as_array) {
        for name in names.iter().filter_map(Value::as_str) {
            output.insert(CapabilityDescriptor::new(
                "steward.exec.env",
                format!("name={name}"),
            ));
        }
    }
}

fn describe_open_url(value: Option<&Value>, output: &mut BTreeSet<CapabilityDescriptor>) {
    let Some(value) = value else {
        return;
    };
    if value.as_bool() == Some(true) {
        output.insert(CapabilityDescriptor::new(SYSTEM_OPEN_URL, "scheme=*"));
        return;
    }
    let Some(schemes) = value
        .as_object()
        .and_then(|grant| grant.get("schemes"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for scheme in schemes.iter().filter_map(Value::as_str) {
        output.insert(CapabilityDescriptor::new(
            SYSTEM_OPEN_URL,
            format!("scheme={}", scheme.to_ascii_lowercase()),
        ));
    }
}

fn describe_boolean(
    root: &Map<String, Value>,
    field: &str,
    permission: &str,
    output: &mut BTreeSet<CapabilityDescriptor>,
) {
    if root.get(field).and_then(Value::as_bool) == Some(true) {
        output.insert(CapabilityDescriptor::new(permission, "system"));
    }
}

fn describe_webview(value: Option<&Value>, output: &mut BTreeSet<CapabilityDescriptor>) {
    let Some(hosts) = value.and_then(Value::as_array) else {
        return;
    };
    for host in hosts.iter().filter_map(|value| {
        value
            .as_str()
            .or_else(|| value.get("host").and_then(Value::as_str))
    }) {
        output.insert(CapabilityDescriptor::new(
            "steward.webview",
            format!("host={}", host.to_ascii_lowercase()),
        ));
    }
}

fn describe_windows(value: Option<&Value>, output: &mut BTreeSet<CapabilityDescriptor>) {
    if value.is_some_and(|windows| windows != false && !windows.is_null()) {
        output.insert(CapabilityDescriptor::new(
            "steward.system.windows",
            "window=extra",
        ));
    }
}

fn is_sensitive(permission: &str, scope: &str) -> bool {
    if matches!(
        permission,
        STORAGE_GET
            | STORAGE_SET
            | WINDOW_CLOSE
            | NAVIGATION_BACK
            | NAVIGATION_CLOSE
            | NAVIGATION_SET_TITLE
            | PREFERENCES_GET
            | PREFERENCES_ALL
            | FEEDBACK_TOAST
            | FEEDBACK_CONFIRM
            | FEEDBACK_PROGRESS
            | FS_GRANTS
            | FS_CANCEL_WALK
            | EXEC_CANCEL
    ) {
        return false;
    }
    if matches!(permission, FS_READ | FS_WRITE) && scope == "path=${dataDir}" {
        return false;
    }
    true
}

fn is_legacy_wildcard(permission: &str) -> bool {
    permission.ends_with('*')
}

fn describe(permission: &str, scope: &str) -> String {
    match permission {
        CLIPBOARD_HISTORY => "Read Steward's recent clipboard history.".into(),
        CLIPBOARD_WRITE => "Write text to the system clipboard.".into(),
        STORAGE_GET => "Read this plugin's private Steward storage.".into(),
        STORAGE_SET => "Write this plugin's private Steward storage.".into(),
        CALENDAR_HOLIDAYS => "Read Steward's cached holiday schedule.".into(),
        WINDOW_CLOSE => "Close this plugin window.".into(),
        NAVIGATION_BACK => "Return from this plugin page to the launcher.".into(),
        NAVIGATION_CLOSE => "Close this plugin page.".into(),
        NAVIGATION_SET_TITLE => "Set the title of this plugin page.".into(),
        PREFERENCES_GET => "Read a declared preference for this plugin.".into(),
        PREFERENCES_ALL => "Read this plugin's declared preferences.".into(),
        FEEDBACK_TOAST => "Show a transient Steward notification.".into(),
        FEEDBACK_CONFIRM => "Ask the user to confirm an action.".into(),
        FEEDBACK_PROGRESS => "Report Steward-rendered progress for a long operation.".into(),
        FS_READ => format!("Read files under {scope}."),
        FS_WRITE => format!("Modify files under {scope}."),
        FS_PICK => "Ask the user to choose a file or folder and remember that choice.".into(),
        FS_WALK => "List file metadata under roots the user selected for this plugin.".into(),
        FS_READ_TEXT => "Read text from roots the user selected for this plugin.".into(),
        FS_WRITE_TEXT => "Write text under roots the user selected for this plugin.".into(),
        FS_GRANTS => "List filesystem roots the user selected for this plugin.".into(),
        FS_CANCEL_WALK => "Cancel a filesystem walk started by this plugin.".into(),
        NET_FETCH => format!("Make an authorized network request ({scope})."),
        EXEC_RUN => format!("Run an approved executable ({scope})."),
        EXEC_CANCEL => "Cancel a controlled process started by this plugin.".into(),
        SYSTEM_OPEN_URL => format!("Open an authorized URL ({scope})."),
        SYSTEM_OPEN_PATH => "Open a selected path with its default application.".into(),
        SYSTEM_REVEAL_PATH => "Show a selected path in File Explorer.".into(),
        SYSTEM_OPEN_WITH => "Open a selected path with an installed application.".into(),
        SYSTEM_APPLICATIONS => "List applications installed for the current user.".into(),
        LEGACY_HOST_ACCESS => {
            "Legacy access to every Steward host call; reserved for official migration.".into()
        }
        permission if permission.ends_with('*') => {
            format!("Use every Steward host call matching '{permission}'.")
        }
        permission if permission.starts_with("steward.system.") => {
            format!("Use the Steward system capability '{permission}'.")
        }
        permission if permission.starts_with("steward.webview") => {
            format!("Display web content in an embedded view ({scope}).")
        }
        permission if permission.starts_with("steward.exec.") => {
            format!("Use a controlled process capability ({scope}).")
        }
        permission if permission.starts_with("steward.net.") => {
            format!("Use a scoped network capability ({scope}).")
        }
        permission if permission.starts_with("steward.") => {
            format!("Call the Steward host function '{permission}'.")
        }
        _ => format!("Use the plugin capability '{permission}'."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_are_resource_scoped_and_deterministic() {
        let capabilities = serde_json::json!({
            "host": ["steward.clipboard.write", "steward.storage.get"],
            "fs": { "read": ["${dataDir}", "D:/projects"] },
            "network": {
                "http": [{
                    "host": "EXAMPLE.com",
                    "schemes": ["https"],
                    "methods": ["GET"],
                    "pathPrefixes": ["/api"]
                }]
            },
            "openUrl": { "schemes": ["HTTPS", "mailto"] }
        });

        let descriptors = describe_declared_capabilities(&capabilities);
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.permission == CLIPBOARD_WRITE
                && descriptor.scope == "host=steward"
                && descriptor.sensitive
        }));
        assert!(descriptors
            .iter()
            .any(|descriptor| { descriptor.permission == STORAGE_GET && !descriptor.sensitive }));
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.permission == FS_READ
                && descriptor.scope == "path=${dataDir}"
                && !descriptor.sensitive
        }));
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.permission == NET_FETCH
                && descriptor.scope.contains("host=example.com")
                && descriptor.scope.contains("schemes=https")
                && descriptor.scope.contains("pathPrefixes=/api")
        }));
        assert_eq!(descriptors, describe_declared_capabilities(&capabilities));
    }

    #[test]
    fn legacy_wildcard_is_marked_as_a_migration_only_permission() {
        let descriptors = describe_declared_capabilities(&serde_json::json!({
            "host": ["steward.*"]
        }));
        assert_eq!(descriptors.len(), 1);
        assert!(descriptors[0].legacy_wildcard);
        assert_eq!(descriptors[0].permission, LEGACY_HOST_ACCESS);
        assert!(is_legacy_official_plugin_id("it.aspirin.calendar"));
        assert!(!is_legacy_official_plugin_id("com.example.test"));
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nsh_core::access::{Capabilities, FileGrants, HttpGrant, OpenUrlGrant};
use semver::Version;
use steward_plugin_contract::capabilities::{
    declaration_is_auto_granted, describe_declared_capabilities, is_legacy_official_plugin_id,
    CALENDAR_HOLIDAYS, CLIPBOARD_HISTORY, CLIPBOARD_WRITE, EXEC_CANCEL, EXEC_RUN, FEEDBACK_CONFIRM,
    FEEDBACK_PROGRESS, FEEDBACK_TOAST, FS_CANCEL_WALK, FS_GRANTS, FS_PICK, FS_READ, FS_READ_TEXT,
    FS_WALK, FS_WRITE, FS_WRITE_TEXT, NAVIGATION_BACK, NAVIGATION_CLOSE, NAVIGATION_SET_TITLE,
    NET_FETCH, PREFERENCES_ALL, PREFERENCES_GET, STORAGE_GET, STORAGE_SET, SYSTEM_APPLICATIONS,
    SYSTEM_OPEN_PATH, SYSTEM_OPEN_URL, SYSTEM_OPEN_WITH, SYSTEM_REVEAL_PATH, WINDOW_CLOSE,
};
use steward_plugin_contract::{CapabilityDescriptor, HostInfo, HOST_API_VERSION};
use steward_storage::{NewCapabilityAuditEntry, Storage};

use crate::PluginHostError;

const CURRENT_HOST_CALLS: &[&str] = &[
    CLIPBOARD_HISTORY,
    CLIPBOARD_WRITE,
    STORAGE_GET,
    STORAGE_SET,
    CALENDAR_HOLIDAYS,
    WINDOW_CLOSE,
    NAVIGATION_BACK,
    NAVIGATION_CLOSE,
    NAVIGATION_SET_TITLE,
    PREFERENCES_GET,
    PREFERENCES_ALL,
    FEEDBACK_TOAST,
    FEEDBACK_CONFIRM,
    FEEDBACK_PROGRESS,
    FS_PICK,
    FS_WALK,
    FS_READ_TEXT,
    FS_WRITE_TEXT,
    FS_GRANTS,
    FS_CANCEL_WALK,
    SYSTEM_OPEN_PATH,
    SYSTEM_REVEAL_PATH,
    SYSTEM_OPEN_WITH,
    SYSTEM_APPLICATIONS,
];

#[derive(Debug, Clone)]
pub struct PluginAuthorization {
    pub plugin_id: String,
    pub publisher_fingerprint: String,
    pub version: String,
    pub features: Vec<String>,
    pub legacy_compatibility: bool,
    pub declared_capabilities: Capabilities,
    pub granted_capabilities: Capabilities,
}

impl PluginAuthorization {
    pub(crate) fn host_info(&self) -> HostInfo {
        HostInfo::new(self.features.clone())
    }

    /// Reloads the active exact grants for this launch. Runtime services use
    /// this so revoking a capability affects an already-open plugin.
    pub fn active_grants(
        &self,
        data_dir: &Path,
    ) -> Result<BTreeSet<(String, String)>, PluginHostError> {
        let storage = Storage::open_at(data_dir).map_err(|error| {
            PluginHostError::HostFailure(format!("open capability store: {error:#}"))
        })?;
        let grants = storage
            .matching_capability_grants(&self.plugin_id, &self.publisher_fingerprint, &self.version)
            .map_err(|error| {
                PluginHostError::HostFailure(format!("read active capability grants: {error:#}"))
            })?;
        Ok(grants
            .into_iter()
            .map(|grant| (grant.capability, grant.scope))
            .collect())
    }
}

pub(crate) struct AuthorizationResult {
    pub(crate) granted: Capabilities,
    pub(crate) authorization: PluginAuthorization,
}

pub(crate) fn authorize(
    data_dir: &Path,
    plugin_id: &str,
    min_host_version: Option<&str>,
    publisher_fingerprint: &str,
    version: &str,
    declared: Option<&Capabilities>,
) -> Result<AuthorizationResult, PluginHostError> {
    check_host_version(min_host_version, plugin_id)?;
    let legacy_compatibility =
        is_legacy_official_plugin_id(plugin_id) && min_host_version.is_none();
    let declared = declared.cloned().unwrap_or_else(|| {
        if legacy_compatibility {
            Capabilities::all()
        } else {
            Capabilities::default()
        }
    });
    let descriptors = describe_declared_capabilities(&declared.to_json());
    if !legacy_compatibility && descriptors.iter().any(|item| item.legacy_wildcard) {
        audit_descriptor_outcomes(
            data_dir,
            plugin_id,
            publisher_fingerprint,
            version,
            &descriptors,
            &BTreeMap::new(),
            "denied",
        )?;
        return Err(PluginHostError::PermissionDenied(format!(
            "plugin '{plugin_id}' declares the migration-only 'steward.*' permission"
        )));
    }

    let storage = Storage::open_at(data_dir).map_err(|error| {
        PluginHostError::HostFailure(format!("open capability store: {error:#}"))
    })?;
    let active = storage
        .matching_capability_grants(plugin_id, publisher_fingerprint, version)
        .map_err(|error| {
            PluginHostError::HostFailure(format!("read active capability grants: {error:#}"))
        })?;
    let grants = active
        .iter()
        .map(|grant| (grant.capability.clone(), grant.scope.clone()))
        .collect::<BTreeSet<_>>();

    let mut allowed = BTreeMap::new();
    for descriptor in &descriptors {
        let is_allowed = legacy_compatibility
            || declaration_is_auto_granted(descriptor)
            || grants.contains(&(descriptor.permission.clone(), descriptor.scope.clone()));
        allowed.insert(
            (descriptor.permission.clone(), descriptor.scope.clone()),
            is_allowed,
        );
    }
    audit_descriptor_outcomes(
        data_dir,
        plugin_id,
        publisher_fingerprint,
        version,
        &descriptors,
        &allowed,
        if legacy_compatibility {
            "legacy_allowed"
        } else {
            "launch_authorization"
        },
    )?;

    let granted = if legacy_compatibility {
        crate::policy::release_capabilities()
    } else {
        restrict_to_permissions(&declared, &allowed)
    };
    let features = features(&descriptors, &allowed, legacy_compatibility);
    let authorization = PluginAuthorization {
        plugin_id: plugin_id.into(),
        publisher_fingerprint: publisher_fingerprint.into(),
        version: version.into(),
        features,
        legacy_compatibility,
        declared_capabilities: declared.clone(),
        granted_capabilities: granted.clone(),
    };
    Ok(AuthorizationResult {
        granted,
        authorization,
    })
}

fn check_host_version(
    min_host_version: Option<&str>,
    plugin_id: &str,
) -> Result<(), PluginHostError> {
    let Some(required) = min_host_version else {
        if is_legacy_official_plugin_id(plugin_id) {
            return Ok(());
        }
        return Err(PluginHostError::UnsupportedFeature(format!(
            "plugin '{plugin_id}' does not declare minHostVersion"
        )));
    };
    let required = Version::parse(required).map_err(|error| {
        PluginHostError::UnsupportedFeature(format!("invalid minHostVersion '{required}': {error}"))
    })?;
    let current = Version::parse(HOST_API_VERSION).expect("host API version must be semver");
    if required > current {
        return Err(PluginHostError::UnsupportedFeature(format!(
            "plugin requires host API {required}, but Steward supports {current}"
        )));
    }
    Ok(())
}

fn restrict_to_permissions(
    declared: &Capabilities,
    allowed: &BTreeMap<(String, String), bool>,
) -> Capabilities {
    let mut granted = Capabilities::default();
    granted.clipboard =
        declared.clipboard && permission_allowed(allowed, "steward.system.clipboard", "system");
    granted.open_url = declared
        .open_url
        .as_ref()
        .map(|grant| restricted_open_url(grant, allowed));
    granted.fs = FileGrants::default();
    granted.fs.read = declared
        .fs
        .read
        .iter()
        .filter(|root| permission_allowed(allowed, FS_READ, &format!("path={root}")))
        .cloned()
        .collect();
    granted.fs.write = declared
        .fs
        .write
        .iter()
        .filter(|root| permission_allowed(allowed, FS_WRITE, &format!("path={root}")))
        .cloned()
        .collect();
    granted.host = declared
        .host
        .iter()
        .filter(|name| permission_allowed(allowed, name, "host=steward"))
        .cloned()
        .collect();
    granted.process.execute = declared
        .process
        .execute
        .iter()
        .filter(|executable| {
            permission_allowed(allowed, EXEC_RUN, &format!("executable={executable}"))
        })
        .cloned()
        .collect();
    granted.process.exit = false;
    granted.process.env.clear();
    granted.network.hosts.clear();
    granted.network.sockets.clear();
    granted.network.http = declared
        .network
        .http
        .iter()
        .filter(|grant| http_permission_allowed(grant, allowed))
        .cloned()
        .collect();

    clamp_to_release_ceiling(granted)
}

fn clamp_to_release_ceiling(mut granted: Capabilities) -> Capabilities {
    let release = crate::policy::release_capabilities();
    granted
        .fs
        .read
        .retain(|root| release.fs.read.contains(root));
    granted
        .fs
        .write
        .retain(|root| release.fs.write.contains(root));
    let process_call_allowed = granted.host.iter().any(|name| name == EXEC_RUN);
    granted.process.execute.retain(|executable| {
        process_call_allowed
            && !executable
                .chars()
                .any(|character| matches!(character, '\r' | '\n' | '\0'))
            && executable.to_ascii_lowercase().ends_with(".exe")
            && !is_shell_executable(executable)
    });
    granted.process.exit = false;
    granted.process.env.clear();

    let network_call_allowed = granted.host.iter().any(|name| name == NET_FETCH);
    granted.network.hosts.clear();
    granted.network.sockets.clear();
    granted.network.http.retain(|grant| {
        network_call_allowed
            && grant.schemes.iter().any(|scheme| scheme == "https")
            && grant.schemes.iter().all(|scheme| scheme == "https")
            && !grant.methods.is_empty()
            && grant.methods.iter().all(|method| {
                matches!(
                    method.as_str(),
                    "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE"
                )
            })
            && grant.port.is_none_or(|port| port == 443)
            && !grant.host.as_str().is_empty()
    });
    granted.webview = None;
    granted.webview_input = false;
    granted.webview_media.clear();
    granted.status_bar = false;
    granted.picker = false;
    granted.share = false;
    granted.speech = false;
    granted.notifications = false;
    granted.audio = false;
    granted.application = false;
    granted.home_dir = false;
    granted.windows = None;
    if let Some(open_url) = &mut granted.open_url {
        open_url.schemes.retain(|scheme| {
            release
                .open_url
                .as_ref()
                .is_some_and(|grant| grant.schemes.contains(scheme))
        });
        if open_url.schemes.is_empty() {
            granted.open_url = None;
        }
    }
    granted
}

fn is_shell_executable(executable: &str) -> bool {
    let name = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "wsl.exe"
            | "bash.exe"
            | "sh.exe"
            | "cscript.exe"
            | "wscript.exe"
            | "mshta.exe"
            | "rundll32.exe"
            | "python.exe"
            | "python3.exe"
            | "py.exe"
            | "node.exe"
            | "nodejs.exe"
            | "deno.exe"
            | "bun.exe"
            | "perl.exe"
            | "ruby.exe"
            | "java.exe"
            | "javaw.exe"
            | "dotnet.exe"
            | "powershell_ise.exe"
            | "wt.exe"
            | "conhost.exe"
    )
}

fn restricted_open_url(
    declared: &OpenUrlGrant,
    allowed: &BTreeMap<(String, String), bool>,
) -> OpenUrlGrant {
    let mut granted = OpenUrlGrant::default();
    granted.schemes = declared
        .schemes
        .iter()
        .filter(|scheme| {
            permission_allowed(
                allowed,
                SYSTEM_OPEN_URL,
                &format!("scheme={}", scheme.to_ascii_lowercase()),
            )
        })
        .cloned()
        .collect();
    granted
}

fn http_permission_allowed(grant: &HttpGrant, allowed: &BTreeMap<(String, String), bool>) -> bool {
    permission_allowed(allowed, NET_FETCH, &http_scope(grant))
}

fn http_scope(grant: &HttpGrant) -> String {
    let schemes = sorted_list(&grant.schemes);
    let methods = sorted_list(&grant.methods);
    let paths = sorted_list(&grant.paths);
    let prefixes = sorted_list(&grant.path_prefixes);
    let port = grant.port.map(|port| port.to_string()).unwrap_or_default();
    format!(
        "host={};schemes={};methods={};paths={};pathPrefixes={};port={port}",
        grant.host.as_str(),
        schemes.join(","),
        methods.join(","),
        paths.join(","),
        prefixes.join(",")
    )
}

fn sorted_list(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn permission_allowed(
    allowed: &BTreeMap<(String, String), bool>,
    permission: &str,
    scope: &str,
) -> bool {
    allowed
        .get(&(permission.to_owned(), scope.to_owned()))
        .copied()
        .unwrap_or(false)
}

fn features(
    descriptors: &[CapabilityDescriptor],
    allowed: &BTreeMap<(String, String), bool>,
    legacy: bool,
) -> Vec<String> {
    let mut features = BTreeSet::new();
    if legacy {
        features.extend(
            CURRENT_HOST_CALLS
                .iter()
                .map(|feature| (*feature).to_owned()),
        );
        features.insert("plugin.data".into());
        features.insert("steward.system.openUrl:https".into());
        features.insert("steward.system.openUrl:mailto".into());
        return features.into_iter().collect();
    }
    for descriptor in descriptors {
        if !permission_allowed(allowed, &descriptor.permission, &descriptor.scope) {
            continue;
        }
        if descriptor.permission.starts_with("steward.") && descriptor.scope == "host=steward" {
            features.insert(descriptor.permission.clone());
        }
        if descriptor.permission == SYSTEM_OPEN_URL {
            features.insert(format!(
                "{}:{}",
                descriptor.permission,
                descriptor.scope.trim_start_matches("scheme=")
            ));
        }
        if matches!(descriptor.permission.as_str(), FS_READ | FS_WRITE)
            && descriptor.scope == "path=${dataDir}"
        {
            features.insert("plugin.data".into());
        }
        if matches!(descriptor.permission.as_str(), STORAGE_GET | STORAGE_SET) {
            features.insert("plugin.data".into());
        }
        if matches!(
            descriptor.permission.as_str(),
            NET_FETCH | EXEC_RUN | EXEC_CANCEL
        ) {
            features.insert(descriptor.permission.clone());
        }
    }
    features.into_iter().collect()
}

fn audit_descriptor_outcomes(
    data_dir: &Path,
    plugin_id: &str,
    publisher_fingerprint: &str,
    version: &str,
    descriptors: &[CapabilityDescriptor],
    allowed: &BTreeMap<(String, String), bool>,
    origin: &str,
) -> Result<(), PluginHostError> {
    let storage = Storage::open_at(data_dir).map_err(|error| {
        PluginHostError::HostFailure(format!("open capability store: {error:#}"))
    })?;
    let occurred_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    for descriptor in descriptors {
        let approved = allowed
            .get(&(descriptor.permission.clone(), descriptor.scope.clone()))
            .copied()
            .unwrap_or(false);
        storage
            .append_capability_audit(&NewCapabilityAuditEntry {
                plugin_id: plugin_id.into(),
                publisher_fingerprint: publisher_fingerprint.into(),
                capability: descriptor.permission.clone(),
                scope: descriptor.scope.clone(),
                outcome: if approved {
                    "allowed".into()
                } else {
                    "denied".into()
                },
                occurred_at,
                detail: Some(format!("origin={origin};version={version}")),
            })
            .map_err(|error| {
                PluginHostError::HostFailure(format!("write capability audit: {error:#}"))
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "steward-host-authorization-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn sensitive_calls_require_exact_active_grants() {
        let dir = temp_dir("grants");
        let declared = Capabilities::from_json(&serde_json::json!({
            "host": ["steward.storage.get", "steward.clipboard.write"]
        }))
        .unwrap();
        let storage = Storage::open_at(&dir).unwrap();
        let grant = storage
            .grant_capability(&steward_storage::NewCapabilityGrant {
                plugin_id: "com.example.test".into(),
                publisher_fingerprint: "SHA256:publisher".into(),
                version_range: "^1".into(),
                capability: CLIPBOARD_WRITE.into(),
                scope: "host=steward".into(),
                granted_at: 1,
            })
            .unwrap();
        let result = authorize(
            &dir,
            "com.example.test",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.0.0",
            Some(&declared),
        )
        .unwrap();
        assert_eq!(result.granted.host, vec![STORAGE_GET, CLIPBOARD_WRITE]);
        assert!(result
            .authorization
            .features
            .iter()
            .any(|feature| feature == CLIPBOARD_WRITE));
        assert!(result
            .authorization
            .features
            .iter()
            .any(|feature| feature == "plugin.data"));

        storage.revoke_capability(grant.id, 2).unwrap();
        let revoked = authorize(
            &dir,
            "com.example.test",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.1.0",
            Some(&declared),
        )
        .unwrap();
        assert_eq!(revoked.granted.host, vec![STORAGE_GET]);
        let audit = storage.capability_audit("com.example.test").unwrap();
        assert!(audit
            .iter()
            .any(|entry| { entry.capability == CLIPBOARD_WRITE && entry.outcome == "allowed" }));
        assert!(audit
            .iter()
            .any(|entry| { entry.capability == CLIPBOARD_WRITE && entry.outcome == "denied" }));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn updates_do_not_inherit_grants_for_new_permissions() {
        let dir = temp_dir("capability-upgrade");
        let storage = Storage::open_at(&dir).unwrap();
        storage
            .grant_capability(&steward_storage::NewCapabilityGrant {
                plugin_id: "com.example.test".into(),
                publisher_fingerprint: "SHA256:publisher".into(),
                version_range: "^1".into(),
                capability: STORAGE_GET.into(),
                scope: "host=steward".into(),
                granted_at: 1,
            })
            .unwrap();
        let upgraded = Capabilities::from_json(&serde_json::json!({
            "host": ["steward.storage.get", "steward.clipboard.write"]
        }))
        .unwrap();
        let result = authorize(
            &dir,
            "com.example.test",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.5.0",
            Some(&upgraded),
        )
        .unwrap();
        assert_eq!(result.granted.host, vec![STORAGE_GET]);
        let audit = storage.capability_audit("com.example.test").unwrap();
        assert!(audit
            .iter()
            .any(|entry| { entry.capability == CLIPBOARD_WRITE && entry.outcome == "denied" }));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn legacy_official_packages_keep_the_current_release_ceiling() {
        let dir = temp_dir("legacy");
        let declared = Capabilities::from_json(&serde_json::json!({
            "host": ["steward.*"]
        }))
        .unwrap();
        let result = authorize(
            &dir,
            "it.aspirin.calendar",
            None,
            "SHA256:official",
            "0.2.3",
            Some(&declared),
        )
        .unwrap();
        assert!(result.authorization.legacy_compatibility);
        assert!(result
            .granted
            .host
            .iter()
            .any(|permission| permission == "steward.*"));
        assert!(result
            .authorization
            .features
            .iter()
            .any(|feature| feature == CLIPBOARD_HISTORY));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn migrated_official_packages_use_named_grants() {
        let dir = temp_dir("migrated-official");
        let declared = Capabilities::from_json(&serde_json::json!({
            "host": [CALENDAR_HOLIDAYS]
        }))
        .unwrap();
        let denied = authorize(
            &dir,
            "it.aspirin.calendar",
            Some(HOST_API_VERSION),
            "SHA256:official",
            "0.2.4",
            Some(&declared),
        )
        .unwrap();
        assert!(!denied.authorization.legacy_compatibility);
        assert!(denied.granted.host.is_empty());

        let storage = Storage::open_at(&dir).unwrap();
        storage
            .grant_capability(&steward_storage::NewCapabilityGrant {
                plugin_id: "it.aspirin.calendar".into(),
                publisher_fingerprint: "SHA256:official".into(),
                version_range: "^0.2.4".into(),
                capability: CALENDAR_HOLIDAYS.into(),
                scope: "host=steward".into(),
                granted_at: 1,
            })
            .unwrap();
        let allowed = authorize(
            &dir,
            "it.aspirin.calendar",
            Some(HOST_API_VERSION),
            "SHA256:official",
            "0.2.4",
            Some(&declared),
        )
        .unwrap();
        assert_eq!(allowed.granted.host, vec![CALENDAR_HOLIDAYS]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn third_party_wildcards_and_missing_host_versions_fail_closed() {
        let dir = temp_dir("wildcard");
        let wildcard = Capabilities::from_json(&serde_json::json!({
            "host": ["steward.*"]
        }))
        .unwrap();
        assert!(matches!(
            authorize(
                &dir,
                "com.example.test",
                Some(HOST_API_VERSION),
                "SHA256:publisher",
                "1.0.0",
                Some(&wildcard),
            ),
            Err(PluginHostError::PermissionDenied(_))
        ));
        assert!(matches!(
            authorize(
                &dir,
                "com.example.test",
                None,
                "SHA256:publisher",
                "1.0.0",
                None,
            ),
            Err(PluginHostError::UnsupportedFeature(_))
        ));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn supported_prior_lines_launch_and_future_hosts_fail_before_runtime() {
        let dir = temp_dir("host-version-lines");
        for minimum in ["0.3.0", "0.4.0", "0.5.0", HOST_API_VERSION] {
            let result = authorize(
                &dir,
                "com.example.test",
                Some(minimum),
                "SHA256:publisher",
                "1.0.0",
                None,
            )
            .unwrap();
            assert_eq!(
                result.authorization.host_info().api_version,
                HOST_API_VERSION
            );
        }
        let future =
            (semver::Version::parse(HOST_API_VERSION).unwrap().major + 1).to_string() + ".0.0";
        let error = match authorize(
            &dir,
            "com.example.test",
            Some(&future),
            "SHA256:publisher",
            "1.0.0",
            None,
        ) {
            Ok(_) => panic!("a future host API requirement must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(error, PluginHostError::UnsupportedFeature(_)));
        assert_eq!(
            error.code(),
            steward_plugin_contract::PluginErrorCode::FeatureUnavailable
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn preferences_feedback_and_navigation_are_automatic_named_capabilities() {
        let dir = temp_dir("feedback-preferences");
        let declared = Capabilities::from_json(&serde_json::json!({
            "host": [
                PREFERENCES_GET,
                PREFERENCES_ALL,
                FEEDBACK_TOAST,
                FEEDBACK_CONFIRM,
                FEEDBACK_PROGRESS,
                NAVIGATION_BACK,
                NAVIGATION_CLOSE,
                NAVIGATION_SET_TITLE
            ]
        }))
        .unwrap();
        let result = authorize(
            &dir,
            "com.example.test",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.0.0",
            Some(&declared),
        )
        .unwrap();
        for feature in [
            PREFERENCES_GET,
            PREFERENCES_ALL,
            FEEDBACK_TOAST,
            FEEDBACK_CONFIRM,
            FEEDBACK_PROGRESS,
            NAVIGATION_BACK,
            NAVIGATION_CLOSE,
            NAVIGATION_SET_TITLE,
        ] {
            assert!(result.granted.host.iter().any(|name| name == feature));
            assert!(result
                .authorization
                .features
                .iter()
                .any(|name| name == feature));
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn filesystem_and_system_calls_require_named_user_grants() {
        let dir = temp_dir("filesystem-system");
        let permissions = [
            FS_PICK,
            FS_WALK,
            FS_READ_TEXT,
            FS_WRITE_TEXT,
            FS_GRANTS,
            FS_CANCEL_WALK,
            SYSTEM_OPEN_PATH,
            SYSTEM_REVEAL_PATH,
            SYSTEM_OPEN_WITH,
            SYSTEM_APPLICATIONS,
        ];
        let declared = Capabilities::from_json(&serde_json::json!({
            "host": permissions
        }))
        .unwrap();
        let denied = authorize(
            &dir,
            "com.example.files",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.0.0",
            Some(&declared),
        )
        .unwrap();
        assert_eq!(
            denied.granted.host,
            vec![FS_GRANTS.to_owned(), FS_CANCEL_WALK.to_owned()]
        );

        let storage = Storage::open_at(&dir).unwrap();
        for permission in permissions {
            storage
                .grant_capability(&steward_storage::NewCapabilityGrant {
                    plugin_id: "com.example.files".into(),
                    publisher_fingerprint: "SHA256:publisher".into(),
                    version_range: "^1".into(),
                    capability: permission.into(),
                    scope: "host=steward".into(),
                    granted_at: 1,
                })
                .unwrap();
        }
        let allowed = authorize(
            &dir,
            "com.example.files",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.0.0",
            Some(&declared),
        )
        .unwrap();
        for permission in permissions {
            assert!(allowed.granted.host.iter().any(|name| name == permission));
            assert!(allowed
                .authorization
                .features
                .iter()
                .any(|name| name == permission));
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn network_and_process_require_matching_named_and_resource_grants() {
        let dir = temp_dir("network-process");
        let declared = Capabilities::from_json(&serde_json::json!({
            "host": [NET_FETCH, EXEC_RUN, EXEC_CANCEL],
            "network": {
                "http": [{
                    "host": "api.example.com",
                    "schemes": ["https"],
                    "methods": ["GET"],
                    "pathPrefixes": ["/v1"]
                }]
            },
            "process": { "execute": ["git.exe"] }
        }))
        .unwrap();
        let descriptors = describe_declared_capabilities(&declared.to_json());
        let network_scope = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.permission == NET_FETCH && descriptor.scope.contains("schemes=https")
            })
            .unwrap()
            .scope
            .clone();

        let denied = authorize(
            &dir,
            "com.example.tools",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.0.0",
            Some(&declared),
        )
        .unwrap();
        assert!(denied.granted.network.http.is_empty());
        assert!(denied.granted.process.execute.is_empty());
        assert!(denied.granted.host.iter().any(|name| name == EXEC_CANCEL));

        let storage = Storage::open_at(&dir).unwrap();
        for (capability, scope) in [
            (NET_FETCH, "host=steward".to_owned()),
            (NET_FETCH, network_scope),
            (EXEC_RUN, "host=steward".to_owned()),
            (EXEC_RUN, "executable=git.exe".to_owned()),
        ] {
            storage
                .grant_capability(&steward_storage::NewCapabilityGrant {
                    plugin_id: "com.example.tools".into(),
                    publisher_fingerprint: "SHA256:publisher".into(),
                    version_range: "^1".into(),
                    capability: capability.into(),
                    scope,
                    granted_at: 1,
                })
                .unwrap();
        }
        let allowed = authorize(
            &dir,
            "com.example.tools",
            Some(HOST_API_VERSION),
            "SHA256:publisher",
            "1.0.0",
            Some(&declared),
        )
        .unwrap();
        assert_eq!(allowed.granted.network.http.len(), 1);
        assert_eq!(allowed.granted.process.execute, vec!["git.exe"]);
        assert!(allowed
            .authorization
            .features
            .iter()
            .any(|name| name == NET_FETCH));
        assert!(allowed
            .authorization
            .features
            .iter()
            .any(|name| name == EXEC_RUN));
        std::fs::remove_dir_all(dir).ok();
    }
}

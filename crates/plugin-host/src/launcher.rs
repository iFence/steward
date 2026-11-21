use std::path::{Path, PathBuf};

use steward_plugin_contract::{HostInfo, LaunchRequest, PluginLaunchParams};
use steward_plugin_registry::PluginPreferences;
use steward_storage::Storage;

use crate::authorization;
use crate::trust;
use crate::{PluginHostError, PluginLaunch};

#[derive(Debug)]
pub(crate) struct LaunchMetadata {
    pub(crate) request: LaunchRequest,
    pub(crate) package_path: PathBuf,
    pub(crate) expected_id: String,
    pub(crate) expected_version: String,
    pub(crate) min_host_version: Option<String>,
    pub(crate) preferences: PluginPreferences,
}

pub(crate) fn load(
    metadata: LaunchMetadata,
    roots: &[String],
    data_dir: &Path,
) -> Result<PluginLaunch, PluginHostError> {
    let bytes = std::fs::read(&metadata.package_path).map_err(|error| {
        PluginHostError::ReadFailed(format!("{}: {error}", metadata.package_path.display()))
    })?;
    let mut package =
        trust::verify_with_roots(roots, &bytes, data_dir).map_err(PluginHostError::Package)?;
    if package.app.id != metadata.expected_id {
        return Err(PluginHostError::IdMismatch {
            expected: metadata.expected_id,
            actual: package.app.id.clone(),
        });
    }
    if package.app.version != metadata.expected_version {
        return Err(PluginHostError::VersionMismatch {
            expected: metadata.expected_version,
            actual: package.app.version.clone(),
        });
    }
    let authorized = authorization::authorize(
        data_dir,
        &metadata.expected_id,
        metadata.min_host_version.as_deref(),
        &package.receipt.key_id,
        &package.app.version,
        package.bundle.policy.declared.as_ref(),
    )?;
    let host_info = authorized.authorization.host_info();
    crate::policy::apply_launch_policy(&mut package, &authorized.authorization, authorized.granted);
    let storage = Storage::open_at(data_dir).map_err(|error| {
        PluginHostError::HostFailure(format!("open plugin preference store: {error:#}"))
    })?;
    let stored_preferences =
        storage
            .plugin_preferences(&metadata.expected_id)
            .map_err(|error| {
                PluginHostError::HostFailure(format!("read plugin preferences: {error:#}"))
            })?;
    let preferences = metadata.preferences.resolve_values(&stored_preferences);
    let params = launch_params(&metadata.request, host_info, &preferences);
    Ok(PluginLaunch {
        plugin_id: metadata.request.plugin_id.clone(),
        app_id: package.app.id.clone(),
        package,
        params,
        preferences,
        authorization: authorized.authorization,
    })
}

fn launch_params(
    request: &LaunchRequest,
    host_info: HostInfo,
    preferences: &std::collections::BTreeMap<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut params = PluginLaunchParams::from(request);
    params.host_api_version = host_info.api_version;
    params.features = host_info.features;
    params.preferences = preferences.clone();
    serde_json::json!({
        "pluginId": params.plugin_id,
        "command": params.command,
        "input": params.input,
        "locale": params.locale,
        "accent": params.accent,
        "dark": params.dark,
        "hostApiVersion": params.host_api_version,
        "features": params.features,
        "preferences": params.preferences,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_parameters_use_the_public_camel_case_shape() {
        let request = LaunchRequest {
            plugin_id: "com.example.test".into(),
            command: "run".into(),
            title: "Test".into(),
            input: "hello".into(),
            locale: "en".into(),
            accent: "#863bff".into(),
            dark: true,
        };
        let params = launch_params(
            &request,
            HostInfo::new(Vec::<String>::new()),
            &Default::default(),
        );
        assert_eq!(
            params.get("pluginId").and_then(|value| value.as_str()),
            Some("com.example.test")
        );
        assert_eq!(
            params.get("input").and_then(|value| value.as_str()),
            Some("hello")
        );
        assert!(params
            .get("features")
            .and_then(|value| value.as_array())
            .is_some_and(|features| features.iter().any(|feature| feature == "host_info")));
        assert_eq!(params.get("preferences"), Some(&serde_json::json!({})));
    }

    #[test]
    fn launch_parameters_expose_resolved_preferences() {
        let request = LaunchRequest {
            plugin_id: "com.example.test".into(),
            command: "run".into(),
            title: "Test".into(),
            input: String::new(),
            locale: "en".into(),
            accent: "#863bff".into(),
            dark: true,
        };
        let preferences =
            std::collections::BTreeMap::from([("theme".into(), serde_json::json!("dark"))]);
        let params = launch_params(&request, HostInfo::new(Vec::<String>::new()), &preferences);
        assert_eq!(
            params
                .get("preferences")
                .and_then(|preferences| preferences.get("theme")),
            Some(&serde_json::json!("dark"))
        );
    }
}

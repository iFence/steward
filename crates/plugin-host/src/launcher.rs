use std::path::{Path, PathBuf};

use steward_plugin_contract::{LaunchRequest, PluginLaunchParams};

use crate::policy;
use crate::trust;
use crate::{PluginHostError, PluginLaunch};

#[derive(Debug)]
pub(crate) struct LaunchMetadata {
    pub(crate) request: LaunchRequest,
    pub(crate) package_path: PathBuf,
    pub(crate) expected_id: String,
    pub(crate) expected_version: String,
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
    policy::apply_release_policy(&mut package);
    let params = launch_params(&metadata.request);
    Ok(PluginLaunch {
        plugin_id: metadata.request.plugin_id.clone(),
        app_id: package.app.id.clone(),
        package,
        params,
    })
}

fn launch_params(request: &LaunchRequest) -> serde_json::Map<String, serde_json::Value> {
    let params = PluginLaunchParams::from(request);
    serde_json::json!({
        "pluginId": params.plugin_id,
        "command": params.command,
        "input": params.input,
        "locale": params.locale,
        "accent": params.accent,
        "dark": params.dark,
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
        let params = launch_params(&request);
        assert_eq!(
            params.get("pluginId").and_then(|value| value.as_str()),
            Some("com.example.test")
        );
        assert_eq!(
            params.get("input").and_then(|value| value.as_str()),
            Some("hello")
        );
    }
}

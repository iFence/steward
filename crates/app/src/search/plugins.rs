use steward_plugin_host::{PluginHost, RouteHit};

use crate::launcher_model::{FeatureId, LauncherSection};

use super::{row, RowAction, SearchContribution, SearchSource};

const MAX_ROUTE_RESULTS: usize = 32;

pub(crate) struct PluginSource<'a> {
    pub host: &'a PluginHost,
    pub label: &'a str,
}

impl SearchSource for PluginSource<'_> {
    fn contribute(&mut self, query: &str) -> SearchContribution {
        let mut contribution = SearchContribution::default();
        for RouteHit {
            plugin_id,
            command,
            title,
            icon,
            input,
            ..
        } in self.host.query(query).into_iter().take(MAX_ROUTE_RESULTS)
        {
            let icon = icon.or_else(|| Some(crate::icons::owned(crate::icons::PLUGIN)));
            contribution.push(
                FeatureId::Plugin,
                LauncherSection::Plugins,
                row(
                    format!("plugin:{plugin_id}:{command}"),
                    title.clone(),
                    self.label,
                    icon,
                ),
                RowAction::OpenPlugin {
                    plugin_id,
                    command,
                    title,
                    input,
                },
            );
        }
        contribution
    }
}

#[cfg(test)]
mod tests {
    use steward_plugin_host::HostConfig;
    use steward_plugin_registry::{
        PluginCommand, PluginManifest, PluginMeta, Trigger, TriggerType, PACKAGE_FILE,
        SCHEMA_VERSION,
    };

    use super::PluginSource;
    use crate::search::SearchSource;

    #[test]
    fn plugin_results_keep_the_manifest_icon() {
        let icon = r##"<svg viewBox="0 0 1 1"><circle fill="#FF6D00" /></svg>"##;
        let data_dir = std::env::temp_dir().join(format!(
            "steward-plugin-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let plugin_dir = data_dir.join("plugins").join("calendar");
        let mut host = steward_plugin_host::PluginHost::new(HostConfig::new(data_dir.clone()))
            .expect("create plugin host");
        let mut meta = PluginMeta {
            manifest: PluginManifest {
                schema_version: SCHEMA_VERSION,
                id: "it.aspirin.calendar".into(),
                name: "Calendar".into(),
                version: "0.2.3".into(),
                icon: Some(icon.into()),
                package: PACKAGE_FILE.into(),
                commands: vec![PluginCommand {
                    name: "calendar".into(),
                    title: "Calendar".into(),
                    keywords: vec!["日历".into(), "rili".into(), "rl".into()],
                    trigger: Trigger {
                        kind: TriggerType::Command,
                        value: None,
                    },
                }],
            },
            dir: plugin_dir.clone(),
            package: plugin_dir.join(PACKAGE_FILE),
            icon: Some(icon.into()),
            scanned_at: 0,
        };
        host.set_plugins(std::slice::from_ref(&meta));

        for query in ["calendar", "日历", "rili", "rl"] {
            let mut source = PluginSource {
                host: &host,
                label: "Plugin",
            };
            let contribution = source.contribute(query);
            assert_eq!(
                contribution
                    .items
                    .first()
                    .and_then(|item| item.icon.source())
                    .as_deref(),
                Some(icon),
                "{query} should produce a plugin row with the manifest icon"
            );
        }

        meta.manifest.icon = None;
        meta.icon = None;
        host.set_plugins(&[meta]);
        let mut source = PluginSource {
            host: &host,
            label: "Plugin",
        };
        let contribution = source.contribute("calendar");
        assert_eq!(
            contribution
                .items
                .first()
                .and_then(|item| item.icon.source())
                .as_deref(),
            Some(crate::icons::PLUGIN),
            "plugins without a manifest icon should use the fallback icon"
        );

        drop(host);
        std::fs::remove_dir_all(&data_dir).ok();
    }
}

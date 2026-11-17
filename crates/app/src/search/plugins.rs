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
            input,
            ..
        } in self.host.query(query).into_iter().take(MAX_ROUTE_RESULTS)
        {
            contribution.push(
                FeatureId::Plugin,
                LauncherSection::Plugins,
                row(
                    format!("plugin:{plugin_id}:{command}"),
                    title.clone(),
                    self.label,
                    None,
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

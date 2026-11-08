use std::collections::HashMap;

use steward_plugin_registry::PluginMeta;

#[derive(Debug, Default)]
pub(crate) struct PluginCatalog {
    plugins: HashMap<String, PluginMeta>,
}

impl PluginCatalog {
    pub(crate) fn set_plugins(&mut self, metas: &[PluginMeta]) {
        self.plugins = metas
            .iter()
            .cloned()
            .map(|meta| (meta.manifest.id.clone(), meta))
            .collect();
    }

    pub(crate) fn get(&self, plugin_id: &str) -> Option<&PluginMeta> {
        self.plugins.get(plugin_id)
    }
}

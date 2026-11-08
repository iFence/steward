use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use steward_core_engine::Engine;
use steward_storage::Storage;

use super::{row, RowAction, SearchContribution, SearchSource};

const MAX_APP_RESULTS: usize = 32;

pub(crate) struct ApplicationSource<'a> {
    pub engine: &'a Engine,
    pub storage: &'a Rc<RefCell<Storage>>,
    pub icon_cache: &'a mut HashMap<PathBuf, Option<String>>,
    pub label: &'a str,
}

impl SearchSource for ApplicationSource<'_> {
    fn contribute(&mut self, query: &str) -> SearchContribution {
        let hits = {
            let storage = self.storage.borrow();
            self.engine
                .query_scored(query, &|path| storage.frequency_str(path))
        };
        let mut contribution = SearchContribution::default();
        for hit in hits.into_iter().take(MAX_APP_RESULTS) {
            let icon = self.icon(&hit.app.path);
            contribution.push(
                row(
                    format!("app:{}", hit.app.path.display()),
                    hit.app.name,
                    self.label,
                    icon,
                ),
                RowAction::LaunchApp(hit.app.path),
            );
        }
        contribution
    }
}

impl ApplicationSource<'_> {
    fn icon(&mut self, path: &Path) -> Option<String> {
        if let Some(icon) = self.icon_cache.get(path) {
            return icon.clone();
        }
        #[cfg(target_os = "windows")]
        let icon = crate::app_icons::app_icon_data_url(path);
        #[cfg(not(target_os = "windows"))]
        let icon = None;
        self.icon_cache.insert(path.to_path_buf(), icon.clone());
        icon
    }
}

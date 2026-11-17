//! Launcher search sources and their stable contribution boundary.

mod applications;
mod builtin;
mod files;
mod plugins;
mod settings;

use std::path::PathBuf;

use steward_ui_components::LauncherRow;
use tokio::sync::oneshot;

use crate::launcher_model::{ActionId, FeatureId, LauncherItem, LauncherSection, Visibility};

pub(crate) use applications::ApplicationSource;
pub(crate) use builtin::BuiltinSource;
pub(crate) use files::{FileSearchCache, FileSearchSource};
pub(crate) use plugins::PluginSource;
pub(crate) use settings::{SettingsIndex, SettingsSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAction {
    LaunchApp(PathBuf),
    OpenFile(PathBuf),
    RevealInExplorer(PathBuf),
    ShowProperties(PathBuf),
    CopyText(String),
    OpenUrl(String),
    OpenSettings(String),
    DismissNotice,
    OpenPlugin {
        plugin_id: String,
        command: String,
        title: String,
        input: String,
    },
}

#[derive(Debug)]
pub struct SearchReply {
    pub generation: u64,
    pub items: Vec<LauncherItem>,
    pub rows: Vec<LauncherRow>,
    pub actions: Vec<RowAction>,
}

pub struct SearchRequest {
    pub generation: u64,
    pub query: String,
    pub reply: oneshot::Sender<SearchReply>,
}

#[derive(Debug, Default)]
pub(crate) struct SearchContribution {
    pub items: Vec<LauncherItem>,
    pub rows: Vec<LauncherRow>,
    pub actions: Vec<RowAction>,
}

impl SearchContribution {
    pub(crate) fn push(
        &mut self,
        feature: FeatureId,
        section: LauncherSection,
        row: LauncherRow,
        action: RowAction,
    ) {
        let item = LauncherItem::from_row(feature, section, row.clone());
        debug_assert_eq!(item.action, ActionId::new(item.id.clone()));
        debug_assert_eq!(item.visibility, Visibility::Visible);
        self.items.push(item);
        self.rows.push(row);
        self.actions.push(action);
    }

    fn append(&mut self, mut other: SearchContribution) {
        self.items.append(&mut other.items);
        self.rows.append(&mut other.rows);
        self.actions.append(&mut other.actions);
    }
}

pub(crate) trait SearchSource {
    fn contribute(&mut self, query: &str) -> SearchContribution;
}

pub(crate) fn collect(sources: &mut [&mut dyn SearchSource], query: &str) -> SearchContribution {
    let mut combined = SearchContribution::default();
    for source in sources {
        combined.append(source.contribute(query));
    }
    combined
}

pub(crate) fn row(
    id: impl Into<String>,
    title: impl Into<String>,
    subtitle: impl Into<String>,
    icon: Option<String>,
) -> LauncherRow {
    LauncherRow {
        id: id.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        icon,
        subtitle_placement: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedSource(&'static str);

    impl SearchSource for FixedSource {
        fn contribute(&mut self, query: &str) -> SearchContribution {
            let mut contribution = SearchContribution::default();
            contribution.push(
                FeatureId::Application,
                LauncherSection::Commands,
                row(self.0, format!("{query}:{}", self.0), "", None),
                RowAction::CopyText(self.0.into()),
            );
            contribution
        }
    }

    #[test]
    fn contributions_preserve_source_order() {
        let mut first = FixedSource("first");
        let mut second = FixedSource("second");
        let mut sources: [&mut dyn SearchSource; 2] = [&mut first, &mut second];
        let contribution = collect(&mut sources, "query");
        assert_eq!(
            contribution
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(matches!(
            contribution.actions.as_slice(),
            [RowAction::CopyText(first), RowAction::CopyText(second)]
                if first == "first" && second == "second"
        ));
        assert_eq!(contribution.items.len(), contribution.rows.len());
        assert_eq!(contribution.items.len(), contribution.actions.len());
        assert_eq!(contribution.items[1].action.as_str(), "second");
    }
}

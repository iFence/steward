//! Launcher search sources and their stable contribution boundary.

mod applications;
mod builtin;
mod files;
mod plugins;
mod settings;

use std::cmp::Reverse;

use steward_ui_components::LauncherRow;
use tokio::sync::oneshot;

use crate::launcher_model::{ActionId, FeatureId, LauncherItem, LauncherSection, RowAction};

pub(crate) use applications::ApplicationSource;
pub(crate) use builtin::BuiltinSource;
pub(crate) use files::{FileSearchCache, FileSearchSource};
pub(crate) use plugins::PluginSource;
pub(crate) use settings::{SettingsIndex, SettingsSource};

#[derive(Debug)]
pub struct SearchReply {
    pub generation: u64,
    pub items: Vec<LauncherItem>,
}

pub struct SearchRequest {
    pub generation: u64,
    pub query: String,
    pub reply: oneshot::Sender<SearchReply>,
}

#[derive(Debug, Default)]
pub(crate) struct SearchContribution {
    pub items: Vec<LauncherItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchQuality {
    Fuzzy,
    Contains,
    Prefix,
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SourcePriority {
    File,
    Settings,
    ApplicationOrPlugin,
    Command,
}

impl SearchContribution {
    pub(crate) fn push(
        &mut self,
        feature: FeatureId,
        section: LauncherSection,
        row: LauncherRow,
        action: RowAction,
    ) {
        let item = LauncherItem::from_row(feature, section, row, action);
        debug_assert_eq!(item.action_id, ActionId::new(item.id.clone()));
        self.items.push(item);
    }

    /// Rank by match quality first, then by result kind. The stable sort keeps
    /// each source's own relevance ordering for otherwise equal matches.
    pub(crate) fn sort_for_query(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        if query.is_empty() || self.items.len() < 2 {
            return;
        }
        let mut ranked: Vec<_> = std::mem::take(&mut self.items)
            .into_iter()
            .map(|item| RankedContribution {
                rank: result_rank(&query, &item),
                item,
            })
            .collect();
        ranked.sort_by_key(|entry| Reverse(entry.rank));
        for entry in ranked {
            self.items.push(entry.item);
        }
    }

    fn append(&mut self, mut other: SearchContribution) {
        self.items.append(&mut other.items);
    }
}

struct RankedContribution {
    rank: (MatchQuality, SourcePriority),
    item: LauncherItem,
}

fn result_rank(query: &str, item: &LauncherItem) -> (MatchQuality, SourcePriority) {
    let priority = match item.section {
        LauncherSection::Favorites | LauncherSection::Commands => SourcePriority::Command,
        LauncherSection::Applications | LauncherSection::Plugins => {
            SourcePriority::ApplicationOrPlugin
        }
        LauncherSection::SystemSettings => SourcePriority::Settings,
        LauncherSection::Files => SourcePriority::File,
    };
    if priority == SourcePriority::Command {
        return (MatchQuality::Exact, priority);
    }

    let mut quality = match_quality(query, &item.title);
    if item.section == LauncherSection::Files {
        if let RowAction::OpenFile(path) = &item.action {
            if let Some(stem) = path.file_stem() {
                quality = quality.max(match_quality(query, &stem.to_string_lossy()));
            }
        }
    }
    (quality, priority)
}

fn match_quality(query: &str, value: &str) -> MatchQuality {
    let value = value.to_lowercase();
    if value == query {
        MatchQuality::Exact
    } else if value.starts_with(query) {
        MatchQuality::Prefix
    } else if value.contains(query) {
        MatchQuality::Contains
    } else {
        MatchQuality::Fuzzy
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
    use std::path::PathBuf;

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
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(matches!(
            contribution
                .items
                .iter()
                .map(|item| &item.action)
                .collect::<Vec<_>>()
                .as_slice(),
            [RowAction::CopyText(first), RowAction::CopyText(second)]
                if first == "first" && second == "second"
        ));
        assert_eq!(contribution.items[1].action_id.as_str(), "second");
    }

    #[test]
    fn equal_matches_prefer_applications_and_plugins_over_files() {
        let mut contribution = SearchContribution::default();
        contribution.push(
            FeatureId::FileSearch,
            LauncherSection::Files,
            row("file:calendar.pdf", "calendar.pdf", "", None),
            RowAction::OpenFile(PathBuf::from(r"C:\Documents\calendar.pdf")),
        );
        contribution.push(
            FeatureId::Plugin,
            LauncherSection::Plugins,
            row("plugin:calendar", "Calendar", "", None),
            RowAction::OpenPlugin {
                plugin_id: "it.aspirin.calendar".into(),
                command: "calendar".into(),
                title: "Calendar".into(),
                input: "calendar".into(),
                presentation: steward_plugin_contract::PresentationMode::Windowed,
            },
        );
        contribution.push(
            FeatureId::Application,
            LauncherSection::Applications,
            row("app:calendar", "Calendar", "", None),
            RowAction::LaunchApp(PathBuf::from(r"C:\Program Files\Calendar\calendar.exe")),
        );

        contribution.sort_for_query("calendar");

        assert_eq!(
            contribution
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["plugin:calendar", "app:calendar", "file:calendar.pdf"]
        );
    }

    #[test]
    fn a_stronger_file_match_can_outrank_a_fuzzy_application_match() {
        let mut contribution = SearchContribution::default();
        contribution.push(
            FeatureId::Application,
            LauncherSection::Applications,
            row("app:cal", "Cal", "", None),
            RowAction::LaunchApp(PathBuf::from(r"C:\Program Files\Cal\cal.exe")),
        );
        contribution.push(
            FeatureId::FileSearch,
            LauncherSection::Files,
            row("file:calendar", "calendar.pdf", "", None),
            RowAction::OpenFile(PathBuf::from(r"C:\Documents\calendar.pdf")),
        );

        contribution.sort_for_query("calendar");

        assert_eq!(contribution.items[0].id, "file:calendar");
        assert_eq!(contribution.items[1].id, "app:cal");
    }

    #[test]
    fn generated_commands_remain_above_file_results() {
        let mut contribution = SearchContribution::default();
        contribution.push(
            FeatureId::FileSearch,
            LauncherSection::Files,
            row("file:answer", "2+2", "", None),
            RowAction::OpenFile(PathBuf::from(r"C:\Documents\2+2.txt")),
        );
        contribution.push(
            FeatureId::Calculator,
            LauncherSection::Commands,
            row("calc", "Calculator", "4", None),
            RowAction::CopyText("4".into()),
        );

        contribution.sort_for_query("2+2");

        assert_eq!(contribution.items[0].id, "calc");
        assert_eq!(contribution.items[1].id, "file:answer");
    }
}

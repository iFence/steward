//! UI-independent launcher contributions and stable action identifiers.

use std::path::PathBuf;

use steward_plugin_contract::PresentationMode;
use steward_ui_components::{LauncherRow, SubtitlePlacement};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowAction {
    LaunchApp(PathBuf),
    RunAsAdministrator(PathBuf),
    OpenFile(PathBuf),
    RevealInExplorer(PathBuf),
    #[allow(dead_code)] // Reserved for the file context menu tracked in the parity plan.
    ShowProperties(PathBuf),
    CopyText(String),
    OpenUrl(String),
    OpenSettings(String),
    DismissNotice,
    ConfirmNotice {
        plugin_id: String,
        id: u64,
        label: String,
    },
    OpenPlugin {
        plugin_id: String,
        command: String,
        title: String,
        input: String,
        presentation: PresentationMode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ActionId(String);

impl ActionId {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ActionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FeatureId {
    Application,
    Calculator,
    Url,
    WindowsSettings,
    StewardSettings,
    FileSearch,
    Plugin,
    Notice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LauncherSection {
    Favorites,
    Applications,
    Files,
    SystemSettings,
    Commands,
    Plugins,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LauncherIcon {
    None,
    Source(String),
}

impl LauncherIcon {
    pub(crate) fn from_source(source: Option<String>) -> Self {
        source.map_or(Self::None, Self::Source)
    }

    pub(crate) fn source(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Source(source) => Some(source.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HotkeyBinding(String);

impl HotkeyBinding {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let hotkey = value
            .parse::<global_hotkey::hotkey::HotKey>()
            .map_err(|error| format!("invalid hotkey '{value}': {error}"))?;
        Ok(Self(hotkey.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LauncherItem {
    pub(crate) id: String,
    pub(crate) feature: FeatureId,
    pub(crate) section: LauncherSection,
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) subtitle_placement: SubtitlePlacement,
    pub(crate) icon: LauncherIcon,
    pub(crate) action_id: ActionId,
    pub(crate) action: RowAction,
}

impl LauncherItem {
    pub(crate) fn from_row(
        feature: FeatureId,
        section: LauncherSection,
        row: LauncherRow,
        action: RowAction,
    ) -> Self {
        let action_id = ActionId::new(row.id.clone());
        Self {
            feature,
            section,
            title: row.title,
            subtitle: (!row.subtitle.is_empty()).then_some(row.subtitle),
            subtitle_placement: row.subtitle_placement,
            icon: LauncherIcon::from_source(row.icon),
            id: row.id,
            action_id,
            action,
        }
    }

    pub(crate) fn to_row(&self) -> LauncherRow {
        LauncherRow {
            id: self.id.clone(),
            title: self.title.clone(),
            subtitle: self.subtitle.clone().unwrap_or_default(),
            icon: self.icon.source(),
            subtitle_placement: self.subtitle_placement,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> LauncherRow {
        LauncherRow {
            id: "app:notepad".into(),
            title: "Notepad".into(),
            subtitle: "Application".into(),
            icon: Some("data:image/png;base64,test".into()),
            subtitle_placement: SubtitlePlacement::AfterTitle,
        }
    }

    #[test]
    fn items_keep_a_stable_action_id_and_convert_to_a_ui_row() {
        let item = LauncherItem::from_row(
            FeatureId::Application,
            LauncherSection::Applications,
            row(),
            RowAction::LaunchApp(PathBuf::from("C:/notepad.exe")),
        );
        assert_eq!(item.action_id.as_str(), "app:notepad");
        assert_eq!(item.to_row().title, "Notepad");
        assert_eq!(item.to_row().subtitle, "Application");
        assert_eq!(
            item.to_row().subtitle_placement,
            SubtitlePlacement::AfterTitle
        );
    }

    #[test]
    fn hotkey_bindings_are_canonicalized_and_validated() {
        let binding = HotkeyBinding::parse("control+alt+Space").unwrap();
        assert_eq!(binding.as_str(), "control+alt+Space");
        assert!(HotkeyBinding::parse("not-a-hotkey").is_err());
    }
}

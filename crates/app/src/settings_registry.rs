//! Stable registration metadata for settings that may enter backups or UI.

use std::collections::BTreeMap;

use crate::config::{
    CHECK_UPDATES_SETTING, LANGUAGE_SETTING, SETTINGS_HOTKEY_SETTING, SUMMON_HOTKEY_SETTING,
    THEME_COLOR_SETTING,
};
use crate::launcher_model::FeatureId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    General,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsDefinition {
    pub(crate) key: &'static str,
    pub(crate) feature: FeatureId,
    pub(crate) default_value: &'static str,
    pub(crate) backup_allowed: bool,
    pub(crate) requires_permission: bool,
    pub(crate) page: SettingsPage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsRegistrationError {
    pub(crate) key: &'static str,
}

impl std::fmt::Display for SettingsRegistrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "setting '{}' is already registered", self.key)
    }
}

impl std::error::Error for SettingsRegistrationError {}

#[derive(Debug, Default)]
pub(crate) struct SettingsRegistry {
    definitions: BTreeMap<&'static str, SettingsDefinition>,
}

impl SettingsRegistry {
    pub(crate) fn builtin() -> Result<Self, SettingsRegistrationError> {
        let mut registry = Self::default();
        for definition in [
            SettingsDefinition {
                key: LANGUAGE_SETTING,
                feature: FeatureId::StewardSettings,
                default_value: "system",
                backup_allowed: true,
                requires_permission: false,
                page: SettingsPage::General,
            },
            SettingsDefinition {
                key: THEME_COLOR_SETTING,
                feature: FeatureId::StewardSettings,
                default_value: "#863bff",
                backup_allowed: true,
                requires_permission: false,
                page: SettingsPage::General,
            },
            SettingsDefinition {
                key: SUMMON_HOTKEY_SETTING,
                feature: FeatureId::StewardSettings,
                default_value: "control+alt+Space",
                backup_allowed: true,
                requires_permission: false,
                page: SettingsPage::General,
            },
            SettingsDefinition {
                key: SETTINGS_HOTKEY_SETTING,
                feature: FeatureId::StewardSettings,
                default_value: "control+Comma",
                backup_allowed: true,
                requires_permission: false,
                page: SettingsPage::General,
            },
            SettingsDefinition {
                key: CHECK_UPDATES_SETTING,
                feature: FeatureId::StewardSettings,
                default_value: "true",
                backup_allowed: true,
                requires_permission: false,
                page: SettingsPage::General,
            },
        ] {
            registry.register(definition)?;
        }
        Ok(registry)
    }

    pub(crate) fn register(
        &mut self,
        definition: SettingsDefinition,
    ) -> Result<(), SettingsRegistrationError> {
        if self.definitions.contains_key(definition.key) {
            return Err(SettingsRegistrationError {
                key: definition.key,
            });
        }
        self.definitions.insert(definition.key, definition);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn get(&self, key: &str) -> Option<&SettingsDefinition> {
        self.definitions.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_stable_backup_metadata() {
        let registry = SettingsRegistry::builtin().unwrap();
        let theme = registry.get(THEME_COLOR_SETTING).unwrap();
        assert!(theme.backup_allowed);
        assert_eq!(theme.page, SettingsPage::General);
    }

    #[test]
    fn duplicate_settings_are_rejected() {
        let mut registry = SettingsRegistry::default();
        let definition = SettingsDefinition {
            key: "example",
            feature: FeatureId::StewardSettings,
            default_value: "one",
            backup_allowed: false,
            requires_permission: false,
            page: SettingsPage::General,
        };
        registry.register(definition.clone()).unwrap();
        assert!(registry.register(definition).is_err());
    }
}

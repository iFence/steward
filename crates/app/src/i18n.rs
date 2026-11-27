//! Localization (i18n) for the Steward native app, using the Fluent system
//! (the same stack Zed uses). `.ftl` resources live in `crates/app/i18n/`
//! and are embedded at compile time via `rust-embed`.

use anyhow::{Context as _, Result};
use i18n_embed::{fluent::FluentLanguageLoader, DesktopLanguageRequester};
use rust_embed::RustEmbed;
use unic_langid::LanguageIdentifier;

/// Embedded Fluent resources: `i18n/{language}/main.ftl`.
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Assets;

/// Localized strings consumed by the NSH launcher surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiStrings {
    pub search_placeholder: String,
    pub application: String,
    pub file_search_unavailable: String,
    pub file_search_repair: String,
    pub settings: String,
    pub plugin: String,
    pub calculator: String,
    pub open_url: String,
    pub launcher_navigate: String,
    pub launcher_open: String,
    pub launcher_close: String,
    pub launcher_confirm: String,
    pub launcher_actions: String,
    pub launcher_copy: String,
    pub launcher_open_application: String,
    pub launcher_open_file: String,
    pub launcher_open_settings: String,
    pub launcher_open_plugin: String,
    pub launcher_open_location: String,
    pub launcher_copy_path: String,
    pub launcher_properties: String,
    pub launcher_action_application_window: String,
    pub launcher_action_recent_files: String,
    pub launcher_action_run_as_administrator: String,
    pub launcher_action_run_as_different_user: String,
    pub launcher_action_show_in_file_explorer: String,
    pub launcher_action_search: String,
    pub launcher_action_no_results: String,
}

/// Compiled Fluent resources bound to the `main` domain (see `main.ftl`).
pub struct Localization {
    loader: FluentLanguageLoader,
}

impl Default for Localization {
    fn default() -> Self {
        Self::new().expect("failed to initialize localization")
    }
}

impl Localization {
    /// Build the localization, selecting the best available language:
    /// the OS languages are queried, with English as a guaranteed fallback.
    pub fn new() -> Result<Self> {
        Self::new_with_language(None)
    }

    /// Build the localization. A persisted language setting (`preferred`)
    /// wins over the OS languages; otherwise the OS languages are queried.
    /// English is always the final fallback.
    pub fn new_with_language(preferred: Option<&str>) -> Result<Self> {
        let fallback: LanguageIdentifier = "en"
            .parse()
            .context("parse English fallback language identifier")?;
        let loader = FluentLanguageLoader::new("main", fallback);

        let mut requested = Vec::new();
        if let Some(code) = preferred {
            if let Ok(language) = code.parse() {
                requested.push(language);
            }
        }
        if requested.is_empty() {
            requested = DesktopLanguageRequester::requested_languages();
        }
        // Guarantee a fallback even if the OS language is not supported.
        requested.push("en".parse().context("parse English language identifier")?);

        // Errors here are non-fatal: we still fall back to English lookups,
        // and a missing `.ftl` resource surfaces as the message ID itself.
        let _ = i18n_embed::select(&loader, &Assets, &requested);

        Ok(Self { loader })
    }

    /// Code of the currently active language (e.g. `zh`, `en`).
    pub fn language(&self) -> String {
        self.loader
            .current_languages()
            .first()
            .map(|language| language.to_string())
            .unwrap_or_else(|| "en".to_string())
    }

    /// Switch the active language at runtime; unsupported codes fall back to
    /// English. All existing `Localization` handles share the same loader, so
    /// every surface (launcher, settings window) picks up the new language on
    /// its next render.
    pub fn select_language(&self, code: &str) {
        let fallback: LanguageIdentifier = "en".parse().expect("parse English fallback");
        let selected: LanguageIdentifier = code.parse().unwrap_or(fallback.clone());
        let _ = i18n_embed::select(&self.loader, &Assets, &[selected, fallback]);
    }

    /// Look up a message by its Fluent message ID. Missing messages fall back
    /// to the ID itself (matching `FluentLanguageLoader::get` semantics).
    pub fn translate(&self, message_id: &str) -> String {
        self.loader.get(message_id)
    }

    /// Snapshot the strings rendered by the launcher's Rust surface.
    pub fn ui_strings(&self) -> UiStrings {
        UiStrings {
            search_placeholder: self.translate("search-placeholder"),
            application: self.translate("application"),
            file_search_unavailable: self.translate("file-search-unavailable"),
            file_search_repair: self.translate("file-search-repair"),
            settings: self.translate("settings-category"),
            plugin: self.translate("plugin"),
            calculator: self.translate("calculator"),
            open_url: self.translate("open-url"),
            launcher_navigate: self.translate("launcher-navigate"),
            launcher_open: self.translate("launcher-open"),
            launcher_close: self.translate("launcher-close"),
            launcher_confirm: self.translate("launcher-confirm"),
            launcher_actions: self.translate("launcher-actions"),
            launcher_copy: self.translate("launcher-copy"),
            launcher_open_application: self.translate("launcher-open-application"),
            launcher_open_file: self.translate("launcher-open-file"),
            launcher_open_settings: self.translate("launcher-open-settings"),
            launcher_open_plugin: self.translate("launcher-open-plugin"),
            launcher_open_location: self.translate("launcher-open-location"),
            launcher_copy_path: self.translate("launcher-copy-path"),
            launcher_properties: self.translate("launcher-properties"),
            launcher_action_application_window: self
                .translate("launcher-action-application-window"),
            launcher_action_recent_files: self.translate("launcher-action-recent-files"),
            launcher_action_run_as_administrator: self
                .translate("launcher-action-run-as-administrator"),
            launcher_action_run_as_different_user: self
                .translate("launcher-action-run-as-different-user"),
            launcher_action_show_in_file_explorer: self
                .translate("launcher-action-show-in-file-explorer"),
            launcher_action_search: self.translate("launcher-action-search"),
            launcher_action_no_results: self.translate("launcher-action-no-results"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_language_wins_and_runtime_switch_works() {
        let localization = Localization::new_with_language(Some("en")).unwrap();
        assert_eq!(localization.translate("app-autostart"), "Launch at Startup");
        assert_eq!(localization.translate("open-in-browser"), "Open in Browser");
        assert_eq!(localization.translate("command"), "Command");
        assert_eq!(localization.ui_strings().launcher_navigate, "Navigate");

        // Switching at runtime affects every shared handle immediately.
        localization.select_language("zh");
        assert_ne!(localization.ui_strings().launcher_navigate, "Navigate");
        assert_eq!(localization.translate("app-autostart"), "开机自启");
        assert_eq!(localization.translate("open-in-browser"), "用浏览器打开");
        assert_eq!(localization.translate("command"), "命令");
        assert_eq!(localization.language(), "zh");
        assert_ne!(
            localization.ui_strings().search_placeholder,
            "Search apps, files, settings, or type a command..."
        );

        for language in ["en", "zh", "de", "fr", "ja", "ko", "ru"] {
            localization.select_language(language);
            for message_id in [
                "launcher-navigate",
                "launcher-confirm",
                "launcher-actions",
                "launcher-action-application-window",
                "launcher-action-recent-files",
                "launcher-action-run-as-administrator",
                "launcher-action-run-as-different-user",
                "launcher-action-show-in-file-explorer",
                "launcher-action-search",
                "launcher-action-no-results",
                "settings-hotkeys",
                "settings-settings-hotkey",
                "settings-update-check-on-startup",
                "settings-update-idle",
                "settings-checking-for-updates",
                "settings-update-available",
                "settings-downloading-update",
                "settings-installing-update",
                "settings-up-to-date",
                "settings-update-check-failed",
                "settings-check-for-updates",
                "settings-download-and-install",
                "settings-update-later",
                "settings-release-notes",
                "settings-plugins",
                "settings-plugin-capabilities",
                "settings-plugin-no-capabilities",
                "settings-plugin-capability-automatic",
                "settings-plugin-capability-legacy",
                "settings-plugin-capability-granted",
                "settings-plugin-capability-denied",
                "settings-plugin-capability-blocked",
                "settings-plugin-capability-allow",
                "settings-plugin-capability-revoke",
                "settings-plugin-refresh",
                "settings-plugin-sync-marketplace",
                "settings-plugin-revocations",
                "settings-plugin-search",
                "settings-plugin-installed",
                "settings-plugin-no-plugins",
                "settings-plugin-no-search-results",
                "settings-plugin-official",
                "settings-plugin-trusted",
                "settings-plugin-policy-failed",
                "settings-plugin-untrusted",
                "settings-plugin-invalid-signature",
                "settings-plugin-unknown-publisher",
                "settings-plugin-update",
                "settings-plugin-rollback",
                "settings-plugin-remove",
                "settings-plugin-author",
                "settings-plugin-publisher",
                "settings-plugin-package-hash",
                "settings-plugin-policy-review",
                "settings-plugin-marketplace",
                "settings-plugin-marketplace-empty",
                "settings-plugin-install",
                "settings-plugin-install-zip",
                "settings-plugin-install-directory",
                "settings-plugin-reload",
                "settings-plugin-developer-diagnostics",
                "settings-plugin-developer-data-dir",
                "settings-plugin-developer-payloads-omitted",
                "settings-plugin-developer-first-frame",
                "settings-plugin-developer-host-failures",
                "settings-plugin-developer-denied",
                "settings-plugin-developer-exits",
                "settings-plugin-developer-crashes",
                "settings-plugin-developer-traces",
                "settings-plugin-developer-logs",
            ] {
                assert_ne!(
                    localization.translate(message_id),
                    message_id,
                    "missing {message_id} translation for {language}"
                );
            }
        }

        localization.select_language("ko");
        assert_eq!(localization.translate("app-autostart"), "로그인 시 실행");

        // Unsupported codes fall back to English.
        localization.select_language("xx");
        assert_eq!(localization.translate("app-autostart"), "Launch at Startup");
    }
}

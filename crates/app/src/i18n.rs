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

        // Switching at runtime affects every shared handle immediately.
        localization.select_language("zh");
        assert_eq!(localization.translate("app-autostart"), "开机自启");
        assert_eq!(localization.translate("open-in-browser"), "用浏览器打开");
        assert_eq!(localization.translate("command"), "命令");
        assert_eq!(localization.language(), "zh");

        localization.select_language("ko");
        assert_eq!(localization.translate("app-autostart"), "로그인 시 실행");

        // Unsupported codes fall back to English.
        localization.select_language("xx");
        assert_eq!(localization.translate("app-autostart"), "Launch at Startup");
    }
}

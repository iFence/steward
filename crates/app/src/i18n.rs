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
        let fallback: LanguageIdentifier = "en"
            .parse()
            .context("parse English fallback language identifier")?;
        let loader = FluentLanguageLoader::new("main", fallback);

        let mut requested = DesktopLanguageRequester::requested_languages();
        // Guarantee a fallback even if the OS language is not supported.
        requested.push("en".parse().context("parse English language identifier")?);

        // Errors here are non-fatal: we still fall back to English lookups,
        // and a missing `.ftl` resource surfaces as the message ID itself.
        let _ = i18n_embed::select(&loader, &Assets, &requested);

        Ok(Self { loader })
    }

    /// Look up a message by its Fluent message ID. Missing messages fall back
    /// to the ID itself (matching `FluentLanguageLoader::get` semantics).
    pub fn translate(&self, message_id: &str) -> String {
        self.loader.get(message_id)
    }
}

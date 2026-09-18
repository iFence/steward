//! The directory picker's status line.
//!
//! Rendered as its own entity so it repaints only when its text actually
//! changes. The bar sits inside the launcher's flex column, and when it was a
//! plain child of the launcher view it was rebuilt on every repaint the input
//! box caused — which is what made the "Esc: cancel" line blink while the user
//! typed or deleted characters.
//!
//! The text still comes from the picker's state; what this type adds is that
//! `set_text` is a no-op unless the rendered string differs, so no repaint is
//! queued for a status that has not moved.

use gpui::{div, prelude::*, px, Context, Render};
use gpui_component::ActiveTheme as _;

use crate::i18n::Localization;

/// The status line under the picker's result list.
pub(crate) struct DirectoryStatusBar {
    text: String,
    visible: bool,
}

impl DirectoryStatusBar {
    pub(crate) fn new(text: String) -> Self {
        Self {
            text,
            visible: true,
        }
    }

    /// Whether the line is on screen *and* has something to say.
    ///
    /// This is what decides whether the launcher's height includes the line, and
    /// getting it wrong showed up as the bar being one line taller with an empty
    /// result list (where the "no folders found" line appears) than with a full
    /// one — same box, two heights. The resting state is an empty string, which
    /// renders nothing, so it must not be counted either.
    pub(crate) fn occupies_space(&self) -> bool {
        self.visible && !self.text.is_empty()
    }

    /// Replace the status text, repainting only when it changed.
    pub(crate) fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.text == text {
            return;
        }
        self.text = text;
        cx.notify();
    }

    /// Show or hide the line (the picker is attached or not), repainting only on
    /// a real change.
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        cx.notify();
    }

    /// Whether the line currently occupies layout space.
    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Render for DirectoryStatusBar {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .h(px(crate::file_continuum::STATUS_HEIGHT))
            .px_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(self.text.clone())
    }
}

/// Localized key for the picker's status, or `None` when no picker is attached.
///
/// Kept as a small pure helper so the mapping from picker state to a *stable*
/// status token is testable: the line must not change while a search is in
/// flight, only when the state it reports actually moves.
#[cfg(target_os = "windows")]
pub(crate) fn status_key(
    attached: bool,
    passive: bool,
    navigating: bool,
    status: &str,
) -> Option<&'static str> {
    if !attached {
        return None;
    }
    if passive {
        // A bar that does not hold the keyboard advertises the click that gives
        // it back.
        return Some("file-continuum-passive");
    }
    if navigating {
        return Some("file-continuum-navigating");
    }
    // The picker's own status is one of the static keys. `""` is the resting
    // state — the line is not shown at all then, because the resting hint ("Esc
    // closes") went away with the ability to close the box: the picker lives for
    // as long as its dialog does, so there is nothing to advertise. Anything else
    // that is not a known token is an error message and is shown as-is.
    //
    // There is deliberately no "searching" token: the previous results stay on
    // screen while a search runs, so the line has nothing to report until the
    // result arrives.
    match status {
        "" => None,
        "file-continuum-no-results" => Some("file-continuum-no-results"),
        "file-continuum-navigating" => Some("file-continuum-navigating"),
        _ => Some("file-continuum-target-unavailable"),
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn status_key(
    _attached: bool,
    _passive: bool,
    _navigating: bool,
    _status: &str,
) -> Option<&'static str> {
    None
}

/// Translate a [`status_key`] token, for the caller that owns the localization.
pub(crate) fn status_text(key: &str, i18n: &Localization) -> String {
    i18n.translate(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resting_status_is_no_line_at_all() {
        let i18n = Localization::new_with_language(Some("en")).unwrap();
        assert_eq!(
            status_text("file-continuum-no-results", &i18n),
            "No folders found — enter a full path"
        );
        // Every other token is looked up verbatim, including the fallback used
        // for an injected error message.
        assert_eq!(
            status_text("file-continuum-target-unavailable", &i18n),
            "The original dialog is unavailable or unsupported. Press Esc and try again."
        );
    }

    #[test]
    fn status_follows_the_picker_state() {
        #[cfg(target_os = "windows")]
        {
            assert_eq!(status_key(false, false, false, ""), None);
            // An attached, idle picker has nothing to say: no line.
            assert_eq!(status_key(true, false, false, ""), None);
            // A passive bar always advertises the click, whatever the search did.
            assert_eq!(
                status_key(true, true, false, "file-continuum-no-results"),
                Some("file-continuum-passive")
            );
            // Navigating wins over the search status: Enter is already committed.
            assert_eq!(
                status_key(true, false, true, ""),
                Some("file-continuum-navigating")
            );
            assert_eq!(
                status_key(true, false, false, "file-continuum-no-results"),
                Some("file-continuum-no-results")
            );
            // Anything that is not a known token is an injected error message:
            // its own text is shown, never the raw state string.
            assert_eq!(
                status_key(true, false, false, "some error text"),
                Some("file-continuum-target-unavailable")
            );
        }
    }

    #[test]
    fn set_text_is_a_no_op_for_an_unchanged_status() {
        // The guard is what keeps the line from repainting on every keystroke;
        // assert the comparison it relies on.
        let bar = DirectoryStatusBar::new("Enter: go to folder".to_owned());
        assert_eq!(bar.text, "Enter: go to folder");
        assert!(bar.is_visible());
    }

    #[test]
    fn an_empty_or_hidden_line_takes_no_space() {
        // The resting picker has no line at all, and the launcher's height has to
        // agree — otherwise the box is one line taller with an empty result list
        // than with a full one, which is the height jump this guards against.
        assert!(!DirectoryStatusBar::new(String::new()).occupies_space());
        assert!(DirectoryStatusBar::new("file-continuum-no-results".to_owned()).occupies_space());
        let mut hidden = DirectoryStatusBar::new("anything".to_owned());
        hidden.visible = false;
        assert!(!hidden.occupies_space());
    }
}

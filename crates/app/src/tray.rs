//! System tray icon and its context menu.

#[cfg(any(target_os = "windows", target_os = "macos"))]
use anyhow::{Context as _, Result};
use std::cell::RefCell;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

use crate::config::{MENU_QUIT, MENU_SETTINGS, MENU_STATUS};
use crate::i18n::Localization;

/// What the tray's status line is reporting.
///
/// The tray menu is the only part of the app that is visible without summoning
/// the launcher, so it is where the file index reports itself: a full-volume
/// build runs for a while, and "nothing seems to be happening" is the worst
/// possible answer for a data-gathering step the user cannot watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayStatus {
    /// Nothing indexed yet and nothing running (the index is unavailable).
    Idle,
    /// A build or USN catch-up pass is in flight.
    Indexing,
    /// An index is ready, holding this many records.
    Indexed(usize),
}

impl TrayStatus {
    /// Localized status line, e.g. `Files: 128,430 indexed`.
    ///
    /// The count is grouped by thousands because the interesting signal is the
    /// order of magnitude, not the exact figure.
    pub(crate) fn text(&self, i18n: &Localization) -> String {
        match self {
            Self::Idle => i18n.translate("files-no-index").to_owned(),
            Self::Indexing => i18n.translate("files-indexing").to_owned(),
            Self::Indexed(records) => {
                format!(
                    "{}: {}",
                    i18n.translate("files-indexed"),
                    group_thousands(*records)
                )
            }
        }
    }

    /// The same state as a tray tooltip, which is visible without opening the
    /// menu at all.
    pub(crate) fn tooltip(&self, i18n: &Localization) -> String {
        format!("Steward - {}", self.text(i18n))
    }
}

/// `128430` -> `128,430`.
///
/// Hand-rolled rather than localized: the app has no number-formatting
/// dependency, and a thousands separator is the only feature this needs. It uses
/// a comma for every language the launcher ships, which matches what those
/// locales do for a plain integer count in a UI label.
fn group_thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Live handle on the tray's status line.
///
/// `muda`'s `MenuItem` is `Rc`-based and therefore neither `Send` nor `Sync`, so
/// this lives in [`LauncherState`](crate::launcher::LauncherState) — created and
/// updated on the same thread that created the menu, which is what Windows
/// requires for `SetMenuItemInfo` to reach the right menu.
pub(crate) struct TrayStatusItem {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    item: MenuItem,
    /// The tray icon itself: it owns the native handle, so keeping it here is
    /// what keeps the icon (and its tooltip) alive and updatable.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    tray: TrayIcon,
    /// Last text pushed to the menu, so a poll tick that changes nothing does no
    /// work and produces no flicker.
    last: RefCell<Option<String>>,
}

impl TrayStatusItem {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn new(item: MenuItem, tray: TrayIcon) -> Self {
        Self {
            item,
            tray,
            last: RefCell::new(None),
        }
    }

    /// Push `status` into the menu label and the tray tooltip.
    ///
    /// Called on every file-index poll tick; the cached text keeps it to one
    /// native call per actual change.
    pub(crate) fn update(&self, status: TrayStatus, i18n: &Localization) {
        let text = status.text(i18n);
        if self.last.borrow().as_deref() == Some(text.as_str()) {
            return;
        }
        *self.last.borrow_mut() = Some(text.clone());
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            self.item.set_text(&text);
            if let Err(error) = self.tray.set_tooltip(Some(status.tooltip(i18n))) {
                eprintln!("failed to update the tray tooltip: {error:#}");
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = &text;
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn setup_tray(i18n: &Localization) -> Result<TrayStatusItem> {
    let icon = load_tray_icon()?;

    // The tray menu is deliberately minimal: a status line, then Settings and
    // Quit. The autostart toggle lives in the settings window.
    //
    // The status line starts disabled: it reports, it is not an action, and a
    // clickable row that does nothing is worse than a greyed one.
    let status = MenuItem::with_id(MENU_STATUS, TrayStatus::Idle.text(i18n), false, None);
    let settings = MenuItem::with_id(MENU_SETTINGS, i18n.translate("app-settings"), true, None);
    let quit = MenuItem::with_id(MENU_QUIT, i18n.translate("app-quit"), true, None);
    let separator = PredefinedMenuItem::separator();

    let menu = Menu::new();
    menu.append(&status)?;
    menu.append(&separator)?;
    menu.append(&settings)?;
    menu.append(&separator)?;
    menu.append(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_tooltip(TrayStatus::Idle.tooltip(i18n))
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()
        .context("build system tray icon")?;
    let handle = TrayStatusItem::new(status, tray);
    // Start from the idle label so the first `update` is a real change.
    *handle.last.borrow_mut() = Some(TrayStatus::Idle.text(i18n));
    Ok(handle)
}

#[cfg(target_os = "windows")]
fn load_tray_icon() -> Result<Icon> {
    // Resource 1 is the app icon (assets/icon.ico), shared by the tray,
    // the exe shell icon and the taskbar icon.
    Icon::from_resource(1, Some((32, 32))).context("load tray icon from embedded resources")
}

#[cfg(target_os = "macos")]
fn load_tray_icon() -> Result<Icon> {
    let png = include_bytes!("../../assets/steward-dark.png");
    let image = image::load_from_memory(png).context("decode bundled steward-dark.png")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).context("create macOS tray icon")
}

/// No tray on other platforms: the launcher still runs, it just has nowhere to
/// report its status.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn setup_tray(i18n: &Localization) -> Result<TrayStatusItem> {
    let _ = i18n;
    Ok(TrayStatusItem {
        last: RefCell::new(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(7), "7");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(12_345), "12,345");
        assert_eq!(group_thousands(128_430), "128,430");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }
}

//! System tray icon and its context menu.

#[cfg(any(target_os = "windows", target_os = "macos"))]
use anyhow::{Context as _, Result};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    Icon as TrayIcon, TrayIconBuilder,
};

use crate::config::{MENU_QUIT, MENU_SETTINGS};
use crate::i18n::Localization;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn setup_tray(i18n: &Localization) -> Result<()> {
    let icon = load_tray_icon()?;

    // The tray menu is deliberately minimal: the autostart toggle lives in the
    // settings window, so the menu only carries Settings and Quit.
    let settings = MenuItem::with_id(MENU_SETTINGS, i18n.translate("app-settings"), true, None);
    let quit = MenuItem::with_id(MENU_QUIT, i18n.translate("app-quit"), true, None);
    let separator = PredefinedMenuItem::separator();

    let menu = Menu::new();
    menu.append(&settings)?;
    menu.append(&separator)?;
    menu.append(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("Steward")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()
        .context("build system tray icon")?;
    // The tray icon must outlive the event loop.
    Box::leak(Box::new(tray));

    Ok(())
}

#[cfg(target_os = "windows")]
fn load_tray_icon() -> Result<TrayIcon> {
    // Resource 1 is the app icon (assets/icon.ico), shared by the tray,
    // the exe shell icon and the taskbar icon.
    TrayIcon::from_resource(1, Some((32, 32))).context("load tray icon from embedded resources")
}

#[cfg(target_os = "macos")]
fn load_tray_icon() -> Result<TrayIcon> {
    let png = include_bytes!("../../assets/steward-dark.png");
    let image = image::load_from_memory(png).context("decode bundled steward-dark.png")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    TrayIcon::from_rgba(rgba.into_raw(), width, height).context("create macOS tray icon")
}

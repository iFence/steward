//! Launcher-wide constants and the global action set.

use gpui::actions;

actions!(steward, [HideWindow]);

/// The launcher bar is deliberately long and short: wide enough to hold a
/// search box plus quick-launch chips, short enough to sit unobtrusively in
/// the middle of the screen. These are logical (CSS) pixels: on a 200% scaled
/// display GPUI renders them at 2× physical pixels, like any other app.
pub const LAUNCHER_WIDTH: f32 = 760.0;
pub const LAUNCHER_HEIGHT: f32 = 60.0;
/// Height of the launcher header containing the search input.
pub const LAUNCHER_HEADER_HEIGHT: f32 = 60.0;
/// Height and horizontal inset of the search input. The surrounding header
/// area is the launcher's window drag region.
pub const LAUNCHER_INPUT_HEIGHT: f32 = 28.0;
pub const LAUNCHER_INPUT_INSET: f32 = 12.0;
/// Height of the launcher status bar below the result list.
pub const LAUNCHER_STATUS_BAR_HEIGHT: f32 = 34.0;
/// Vertical inset around the launcher content. The header's non-input area is
/// the window's drag handle; the input box itself is not draggable.
pub const LAUNCHER_MARGIN: f32 = 4.0;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub const MENU_SETTINGS: &str = "settings";
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub const MENU_QUIT: &str = "quit";
/// Whether hiding the launcher destroys its window — reclaiming the
/// per-window renderer, swapchain and GPU atlas — instead of keeping it
/// allocated. Measured on the Windows dev machine (docs/benchmarks.md):
/// closing the window reclaims almost nothing — the DirectX devices and
/// DirectWrite text system are platform-level and stay allocated for the GPUI
/// session — while re-summoning then costs ~150 ms. Keeping the window hidden
/// re-summons in ~10-30 ms at the same memory, so hide is the default.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
pub const CLOSE_ON_HIDE: bool = false;

/// The launcher's accent color when no theme color is configured (Tinycast's
/// brand violet). Doubles as the default selection/caret color. Surface and
/// ink colors live in `steward_ui_components::palette`, adapted from
/// Tinycast's design system.
pub const DEFAULT_ACCENT: u32 = steward_ui_components::palette::ACCENT;
/// Storage key for the persisted theme color (a `#rrggbb` hex string).
pub const THEME_COLOR_SETTING: &str = "theme_color";
/// Storage key for the persisted language code (e.g. `zh`, `en`).
pub const LANGUAGE_SETTING: &str = "language";
/// Storage key for the persisted summon hotkey (a `HotKey::into_string()`
/// string, e.g. `control+alt+Space`).
pub const SUMMON_HOTKEY_SETTING: &str = "summon_hotkey";
/// Storage key for checking GitHub Releases when Steward starts.
pub const CHECK_UPDATES_SETTING: &str = "check_updates_on_startup";

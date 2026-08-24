//! Launcher-wide constants and the global action set.

use gpui::actions;

actions!(steward, [HideWindow]);

/// The launcher bar is deliberately long and short: wide enough to hold a
/// search box plus quick-launch chips, short enough to sit unobtrusively in
/// the middle of the screen. These are logical (CSS) pixels: on a 200% scaled
/// display GPUI renders them at 2× physical pixels, like any other app.
pub const LAUNCHER_WIDTH: f32 = 760.0;
pub const LAUNCHER_HEIGHT: f32 = 60.0;
/// Width of the non-interactive margin around the input box. This margin is
/// the window's drag handle; the input box itself is not draggable.
pub const LAUNCHER_MARGIN: f32 = 4.0;

/// Fixed row height of a launcher result. Must match `results_list.rs` so the
/// window resize stays in sync with the rendered list.
pub const RESULT_ROW_HEIGHT: f32 = 42.0;
/// Maximum number of results shown before the drop-down scrolls.
pub const MAX_RESULT_ROWS: usize = 8;

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
/// Storage key for the persisted settings-window hotkey (e.g. `control+comma`).
pub const SETTINGS_HOTKEY_SETTING: &str = "settings_hotkey";

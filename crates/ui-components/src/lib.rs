//! Business-level UI components for the launcher.

use gpui::App;

pub mod results_list;

pub use results_list::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialize the gpui-component stack.
///
/// Runs the full component initialization (theme, global state, root, popover,
/// menu, list, ...) so every gpui-component surface — including the settings
/// window's `Settings` widget — works. The default theme is light, which does
/// not match the launcher's dark bar; switch to the dark theme so any
/// gpui-component surface (scrollbars, settings window, future plugin panels,
/// ...) picks up dark colors. Must run once, with the application context,
/// before any window opens.
pub fn init_components(cx: &mut App) {
    gpui_component::init(cx);
    gpui_component::theme::Theme::change(gpui_component::theme::ThemeMode::Dark, None, cx);
}

//! Business-level UI components for the launcher.

use gpui::App;

pub mod results_list;

pub use results_list::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialize the gpui-component theme.
///
/// The default theme is light, which does not match the launcher's dark bar;
/// switch to the dark theme so any gpui-component surface (scrollbars, future
/// plugin panels, ...) picks up dark colors. Must run once, with the
/// application context, before the launcher window opens.
pub fn init_theme(cx: &mut App) {
    gpui_component::theme::init(cx);
    gpui_component::theme::Theme::change(gpui_component::theme::ThemeMode::Dark, None, cx);
}

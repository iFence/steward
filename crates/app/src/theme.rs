//! Color helpers and the adaptive frosted-glass surface for the launcher.

use gpui::{rgb, App, Hsla};
use gpui_component::theme::{Theme, ThemeTokens};

/// Parse a `#rrggbb` hex string into an RGB integer, or `None` when malformed.
pub(crate) fn parse_hex_color(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

/// Relative luminance (Rec. 709 linear-light, 0..1) of a `0xRRGGBB` color —
/// the WCAG definition. Used by the adaptive scrim to decide how much of the
/// launcher surface must show over the window's blurred backdrop.
pub(crate) fn relative_luminance(color: u32) -> f32 {
    let channel = |value: u32| {
        let c = (value & 0xFF) as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let r = channel(color >> 16);
    let g = channel(color >> 8);
    let b = channel(color);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Pick the scrim opacity for a given backdrop luminance: the lowest alpha
/// that keeps the composited launcher surface ([`palette::BACKGROUND`] over
/// the blurred backdrop) at or below [`palette::SCRIM_TARGET_LUMINANCE`],
/// floored at [`palette::SCRIM_ALPHA`] and capped at
/// [`palette::SCRIM_ALPHA_MAX`]. Over a dark desktop this returns the floor,
/// so the current frosted-glass look is unchanged; over a bright backdrop (a
/// white document behind the bar) it rises toward the cap so white ink keeps
/// its contrast.
pub(crate) fn adaptive_scrim_alpha(backdrop_luminance: f32) -> f32 {
    let floor = steward_ui_components::palette::SCRIM_ALPHA;
    let background_luminance = relative_luminance(steward_ui_components::palette::BACKGROUND);
    let floor_surface = floor * background_luminance + (1.0 - floor) * backdrop_luminance;
    if floor_surface <= steward_ui_components::palette::SCRIM_TARGET_LUMINANCE {
        return floor;
    }
    let alpha = (steward_ui_components::palette::SCRIM_TARGET_LUMINANCE - backdrop_luminance)
        / (background_luminance - backdrop_luminance);
    alpha.clamp(floor, steward_ui_components::palette::SCRIM_ALPHA_MAX)
}

/// Selection wash opacity for the current scrim: Tinycast's white 0.10 over
/// the standard frosted surface, raised toward [`palette::SELECTION_WASH_MAX`]
/// as the scrim rises. A bright backdrop lightens the whole bar, and a fixed
/// 0.10 wash would read as too faint there, so the wash follows the scrim so
/// the selected row stays clearly brighter than its neighbors.
pub(crate) fn adaptive_selection_wash(scrim_alpha: f32) -> f32 {
    let floor = steward_ui_components::palette::SCRIM_ALPHA;
    let span = steward_ui_components::palette::SCRIM_ALPHA_MAX - floor;
    let t = ((scrim_alpha - floor) / span).clamp(0.0, 1.0);
    steward_ui_components::palette::SELECTION_WASH
        + (steward_ui_components::palette::SELECTION_WASH_MAX
            - steward_ui_components::palette::SELECTION_WASH)
            * t
}

/// Apply the Steward palette (surface colors matching the launcher bar, drawn
/// from Tinycast's design system) and the given accent color to the global
/// gpui-component theme, then rebuild the derived semantic tokens and the
/// Base-layer theme (scrollbars, resize handles). Call once at startup after
/// `init_components`, and again whenever the user picks a new theme color in
/// the settings window.
pub(crate) fn apply_steward_theme(cx: &mut App, accent: u32) {
    let background = Hsla::from(rgb(steward_ui_components::palette::BACKGROUND));
    let background_alt = Hsla::from(rgb(steward_ui_components::palette::BACKGROUND_ALT));
    let border = Hsla::from(rgb(steward_ui_components::palette::BORDER));
    let foreground = Hsla::from(rgb(steward_ui_components::palette::FOREGROUND));
    let muted_foreground = Hsla::from(rgb(steward_ui_components::palette::MUTED_FOREGROUND));
    let accent = Hsla::from(rgb(accent));
    // Tinycast paints ink as white at fixed alpha stops (`selection` white
    // 0.10, `rowHover` white 0.05) rather than tinting it with the accent.
    let white = Hsla::from(rgb(0xffffff));

    let theme = Theme::global_mut(cx);
    // Surfaces: the settings window's background, side bar, groups and popups
    // now share the launcher's color instead of the near-black dark default.
    theme.background = background;
    theme.foreground = foreground;
    theme.popover = background;
    theme.popover_foreground = foreground;
    theme.border = border;
    theme.input = border;
    theme.muted = background_alt;
    theme.muted_foreground = muted_foreground;
    theme.secondary = background_alt;
    theme.secondary_foreground = foreground;
    theme.secondary_hover = white.opacity(0.05);
    theme.accent = background_alt;
    theme.accent_foreground = foreground;
    theme.colors.list = background;
    theme.list_hover = white.opacity(0.05);
    theme.group_box = background;
    theme.group_box_foreground = foreground;
    theme.sidebar = background;
    theme.sidebar_border = border;
    theme.sidebar_foreground = foreground;
    theme.sidebar_accent = background_alt;
    theme.tiles = background;
    theme.table = background;
    theme.title_bar = background;
    theme.title_bar_border = border;
    theme.window_border = border;
    theme.tab_bar = background;
    theme.tab = background;
    theme.tab_active = background_alt;
    // Accent: caret, focus ring, primary controls and the selected-row border
    // follow the chosen theme color; the selected-row fill itself is the
    // neutral white 0.10 wash, exactly like Tinycast's list selection.
    theme.primary = accent;
    theme.primary_hover = accent.opacity(0.85);
    theme.primary_active = accent.opacity(0.75);
    theme.primary_foreground = white;
    theme.button_primary = accent;
    theme.button_primary_hover = accent.opacity(0.85);
    theme.button_primary_active = accent.opacity(0.75);
    theme.button_primary_foreground = white;
    theme.caret = accent;
    theme.selection = accent.opacity(0.35);
    theme.ring = accent;
    theme.list_active = white.opacity(0.10);
    theme.list_active_border = accent.opacity(0.6);
    theme.drop_target = accent.opacity(0.2);

    // Re-derive the full legacy token set (sidebar included — the sidebar
    // widget reads `tokens.sidebar`, not `colors.sidebar`) from the mutated
    // colors, then push the Base-layer theme (scrollbars, resize handles).
    theme.tokens = ThemeTokens::from(theme.colors);
    Theme::sync_base(cx);
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;
    use steward_ui_components::palette;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-3,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn relative_luminance_of_extremes_and_surface() {
        assert_close(relative_luminance(0x000000), 0.0);
        assert_close(relative_luminance(0xffffff), 1.0);
        assert_close(
            relative_luminance(steward_ui_components::palette::BACKGROUND),
            0.0147,
        );
    }

    #[test]
    fn adaptive_scrim_keeps_the_frosted_look_over_dark_backdrops() {
        // Over black or a typical dark wallpaper the floor alpha is enough to
        // keep the surface at or below the target, so the scrim stays put.
        assert_close(adaptive_scrim_alpha(0.0), palette::SCRIM_ALPHA);
        assert_close(adaptive_scrim_alpha(0.02), palette::SCRIM_ALPHA);
        assert_close(adaptive_scrim_alpha(0.1), palette::SCRIM_ALPHA);
    }

    #[test]
    fn adaptive_scrim_rises_over_bright_backdrops() {
        // A pure-white backdrop caps the scrim at SCRIM_ALPHA_MAX ...
        assert_close(adaptive_scrim_alpha(1.0), palette::SCRIM_ALPHA_MAX);
        // ... a mostly-white page lands inside the floor..ceiling band ...
        let mid = adaptive_scrim_alpha(0.5);
        assert!(mid > palette::SCRIM_ALPHA);
        assert!(mid < palette::SCRIM_ALPHA_MAX);
        // ... and the surface stays at or under the target luminance, so the
        // white ink keeps ~7:1 contrast at the brightest backdrop.
        let background_lum = relative_luminance(palette::BACKGROUND);
        for backdrop in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let alpha = adaptive_scrim_alpha(backdrop);
            let surface = alpha * background_lum + (1.0 - alpha) * backdrop;
            if alpha < palette::SCRIM_ALPHA_MAX {
                assert!(
                    surface <= palette::SCRIM_TARGET_LUMINANCE + 1e-3,
                    "surface {surface} over target for backdrop {backdrop}"
                );
            }
        }
        // At the cap the ceiling binds (a pure-white backdrop), leaving a
        // surface of ~0.113 — still a ~6.4:1 contrast for the white ink.
        let capped = adaptive_scrim_alpha(1.0);
        let surface = capped * background_lum + (1.0 - capped) * 1.0;
        assert!((1.0 + 0.05) / (surface + 0.05) > 4.5);
    }

    #[test]
    fn selection_wash_follows_the_scrim() {
        // At the floor scrim the wash is Tinycast's 0.10; at the cap it has
        // doubled, and it stays monotonic in between.
        assert_close(
            adaptive_selection_wash(palette::SCRIM_ALPHA),
            palette::SELECTION_WASH,
        );
        assert_close(
            adaptive_selection_wash(palette::SCRIM_ALPHA_MAX),
            palette::SELECTION_WASH_MAX,
        );
        let mid = adaptive_selection_wash(0.5 * (palette::SCRIM_ALPHA + palette::SCRIM_ALPHA_MAX));
        assert!(mid > palette::SELECTION_WASH && mid < palette::SELECTION_WASH_MAX);
    }
}

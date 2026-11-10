//! Steward's visual palette, adapted from Tinycast's design system
//! (github.com/abue-ammar/tinycast, docs/ui.md). Tinycast paints a dark
//! neutral panel — black 40% scrim over behind-window vibrancy — with white
//! ink at fixed alpha stops and a violet brand hue. Steward renders that
//! scrim as [`BACKGROUND`] composited over the window's blurred backdrop
//! (Windows Acrylic / macOS vibrancy) at [`SCRIM_ALPHA`], so the frosted
//! glass shows through while the launcher keeps a fixed dark look regardless
//! of the OS theme. Over a bright backdrop (a white document or browser
//! window behind the bar) the scrim is raised adaptively toward
//! [`SCRIM_ALPHA_MAX`] so the white ink stays readable — see the decision
//! record on backdrop adaptation in docs/architecture.md. Opaque windows
//! (the settings window) use the full [`BACKGROUND`]. Keep the alpha stops
//! in sync with Tinycast's tokens; the surface is the one judgment call.

/// Main surface: launcher bar, result rows, settings window. Stands in for
/// Tinycast's `panelScrim` (black 0.40 over vibrancy), made opaque for
/// opaque windows.
pub const BACKGROUND: u32 = 0x202024;
/// Opacity at which translucent launcher surfaces paint [`BACKGROUND`] over
/// the window's blurred backdrop. The 0.75 floor keeps text comfortably
/// readable while retaining a visible Acrylic/vibrancy blur. This is the floor
/// for the adaptive scrim (see [`SCRIM_ALPHA_MAX`]).
pub const SCRIM_ALPHA: f32 = 0.75;
/// Ceiling for the adaptive scrim. Over a bright backdrop the launcher raises
/// its scrim toward this value (see `adaptive_scrim_alpha` in the app); past
/// it the backdrop contributes so little that the bar reads as a solid panel
/// instead of frosted glass, so the scrim never rises further.
pub const SCRIM_ALPHA_MAX: f32 = 0.90;
/// Target relative luminance (Rec. 709 linear-light, 0..1) of the composited
/// launcher surface. The adaptive scrim picks the lowest opacity that keeps
/// the surface at or below this luminance over the current backdrop; 0.10
/// yields a ~7:1 white-on-dark contrast ratio. Over a pure-white backdrop the
/// [`SCRIM_ALPHA_MAX`] ceiling binds first, leaving a ~0.113 surface (~6.4:1,
/// still WCAG AA for normal text).
pub const SCRIM_TARGET_LUMINANCE: f32 = 0.10;
/// Selection wash opacity on translucent launcher surfaces over the base
/// frosted scrim. Tinycast's `selection` (white 0.10) blended onto the row.
pub const SELECTION_WASH: f32 = 0.10;
/// Ceiling for the adaptive selection wash, reached at [`SCRIM_ALPHA_MAX`]: a
/// bright backdrop lightens the whole bar, and a fixed 0.10 wash reads too
/// faint there, so the wash rises toward this value as the scrim does.
pub const SELECTION_WASH_MAX: f32 = 0.20;
/// Alt surface: hover fill, secondary controls, active tabs. Tinycast's
/// `rowHover` (white 0.05) blended onto [`BACKGROUND`].
pub const BACKGROUND_ALT: u32 = 0x2c2c31;
/// Selected result row: a neutral gray-white fill, independent of the
/// user's accent color and clear against the subdued row dividers.
pub const ROW_BACKGROUND_SELECTED: u32 = 0x414146;
/// Borders and input outlines. Tinycast's `border` (white 0.20) blended onto
/// [`BACKGROUND`].
pub const BORDER: u32 = 0x4d4d50;
/// Primary text and the caret. Tinycast's `textPrimary` (white 1.00).
pub const FOREGROUND: u32 = 0xffffff;
/// Secondary / placeholder text and trailing kind labels. Tinycast's
/// `textTertiary` (white 0.40) blended onto [`BACKGROUND`].
pub const MUTED_FOREGROUND: u32 = 0x79797c;
/// Secondary launcher text such as result metadata, control-bar labels, and
/// result position. This needs more contrast than [`MUTED_FOREGROUND`] because
/// the adaptive launcher surface can rise to `#363638` over a pure-white
/// backdrop; this value keeps the 11-12 px labels above the WCAG AA text
/// threshold there.
pub const RESULT_LABEL_FOREGROUND: u32 = 0xb8b8bb;
/// Opacity for input placeholders over the adaptive launcher surface. The
/// placeholder uses the primary foreground at this opacity so it remains
/// readable even when the scrim is at [`SCRIM_ALPHA_MAX`], while staying
/// visually subordinate to typed text.
pub const PLACEHOLDER_OPACITY: f32 = 0.72;
/// Selection wash, applied at white 0.10. Tinycast's `selection`.
pub const SELECTION: u32 = 0xffffff;
/// Mouse-hover wash, applied at white 0.05 — always fainter than
/// [`SELECTION`]. Tinycast's `rowHover`.
pub const HOVER: u32 = 0xffffff;
/// Tinycast's brand violet, `Color(red: 0.525, green: 0.231, blue: 1.0)`.
/// Default accent for the launcher and settings, tinting selection, caret,
/// focus rings and primary controls.
pub const ACCENT: u32 = 0x863bff;

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_luminance(color: u32) -> f32 {
        let channel = |value: u32| {
            let value = value as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color >> 16)
            + 0.7152 * channel(color >> 8 & 0xff)
            + 0.0722 * channel(color & 0xff)
    }

    fn contrast_ratio(first: u32, second: u32) -> f32 {
        let first = relative_luminance(first);
        let second = relative_luminance(second);
        let (lighter, darker) = if first > second {
            (first, second)
        } else {
            (second, first)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn result_labels_stay_readable_on_bright_backdrops() {
        // Worst case: the maximum scrim over a pure-white window.
        let surface = 0x363638;
        assert!(contrast_ratio(RESULT_LABEL_FOREGROUND, surface) >= 4.5);
        assert!(
            contrast_ratio(RESULT_LABEL_FOREGROUND, surface)
                > contrast_ratio(MUTED_FOREGROUND, surface)
        );
    }
}

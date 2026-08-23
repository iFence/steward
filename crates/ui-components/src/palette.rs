//! Steward's visual palette, adapted from Tinycast's design system
//! (github.com/abue-ammar/tinycast, docs/ui.md). Tinycast paints a dark
//! neutral panel — black 40% scrim over behind-window vibrancy — with white
//! ink at fixed alpha stops and a violet brand hue. Steward's windows are
//! opaque by design (the launcher must look identical over any desktop), so
//! the scrim is folded into a solid neutral surface and the white ramp is
//! pre-blended onto it. Keep the alpha stops in sync with Tinycast's tokens;
//! the surface is the one judgment call.

/// Main surface: launcher bar, result rows, settings window. Stands in for
/// Tinycast's `panelScrim` (black 0.40 over vibrancy), made opaque.
pub const BACKGROUND: u32 = 0x202024;
/// Alt surface: hover fill, secondary controls, active tabs. Tinycast's
/// `rowHover` (white 0.05) blended onto [`BACKGROUND`].
pub const BACKGROUND_ALT: u32 = 0x2c2c31;
/// Borders and input outlines. Tinycast's `border` (white 0.20) blended onto
/// [`BACKGROUND`].
pub const BORDER: u32 = 0x4d4d50;
/// Primary text and the caret. Tinycast's `textPrimary` (white 1.00).
pub const FOREGROUND: u32 = 0xffffff;
/// Secondary / placeholder text and trailing kind labels. Tinycast's
/// `textTertiary` (white 0.40) blended onto [`BACKGROUND`].
pub const MUTED_FOREGROUND: u32 = 0x79797c;
/// Selection wash, applied at white 0.10. Tinycast's `selection`.
pub const SELECTION: u32 = 0xffffff;
/// Mouse-hover wash, applied at white 0.05 — always fainter than
/// [`SELECTION`]. Tinycast's `rowHover`.
pub const HOVER: u32 = 0xffffff;
/// Tinycast's brand violet, `Color(red: 0.525, green: 0.231, blue: 1.0)`.
/// Default accent for the launcher and settings, tinting selection, caret,
/// focus rings and primary controls.
pub const ACCENT: u32 = 0x863bff;

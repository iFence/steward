//! Small shared color helpers for NSH shell surfaces.

use steward_ui_components::palette;

/// Relative luminance (Rec. 709) of an `0xRRGGBB` color.
pub(crate) fn relative_luminance(color: u32) -> f32 {
    let channel = |value: u32| {
        let c = (value & 0xff) as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color >> 16) + 0.7152 * channel(color >> 8) + 0.0722 * channel(color)
}

/// Pick the launcher scrim opacity for the luminance behind the window.
///
/// The lowest opacity that keeps the composited surface dark enough is used,
/// bounded by the readable translucent floor and the readability ceiling.
/// `None` means the platform could not sample the backdrop, so the normal floor
/// is retained.
pub(crate) fn adaptive_scrim_alpha(backdrop_luminance: Option<f32>) -> f32 {
    let Some(backdrop) = backdrop_luminance.filter(|value| value.is_finite()) else {
        return palette::SCRIM_ALPHA;
    };
    let backdrop = backdrop.clamp(0.0, 1.0);
    let background = relative_luminance(palette::BACKGROUND);
    let floor_surface = palette::SCRIM_ALPHA * background + (1.0 - palette::SCRIM_ALPHA) * backdrop;
    if floor_surface <= palette::SCRIM_TARGET_LUMINANCE {
        return palette::SCRIM_ALPHA;
    }

    let alpha = (palette::SCRIM_TARGET_LUMINANCE - backdrop) / (background - backdrop);
    alpha.clamp(palette::SCRIM_ALPHA, palette::SCRIM_ALPHA_MAX)
}

#[cfg(test)]
mod tests {
    use super::{adaptive_scrim_alpha, relative_luminance};
    use steward_ui_components::palette;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn luminance_uses_rec_709_linear_light() {
        assert_close(relative_luminance(0xffffff), 1.0);
        assert_close(relative_luminance(palette::BACKGROUND), 0.0147);
    }

    #[test]
    fn scrim_stays_frosted_over_dark_backdrops() {
        assert_close(adaptive_scrim_alpha(None), palette::SCRIM_ALPHA);
        assert_close(adaptive_scrim_alpha(Some(f32::NAN)), palette::SCRIM_ALPHA);
        assert_close(adaptive_scrim_alpha(Some(0.02)), palette::SCRIM_ALPHA);
    }

    #[test]
    fn scrim_darkens_over_bright_backdrops() {
        assert_close(adaptive_scrim_alpha(Some(1.0)), palette::SCRIM_ALPHA_MAX);
        let mid = adaptive_scrim_alpha(Some(0.5));
        assert!(mid > palette::SCRIM_ALPHA);
        assert!(mid < palette::SCRIM_ALPHA_MAX);
    }
}

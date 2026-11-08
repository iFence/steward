//! Small shared color helpers for NSH shell surfaces.

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

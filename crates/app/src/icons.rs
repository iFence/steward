//! Product-owned SVG icons embedded when the operating system or plugin does
//! not provide one.

pub(crate) const OPEN_URL: &str = include_str!("../../../assets/icons/open-url.svg");
pub(crate) const CALCULATOR: &str = include_str!("../../../assets/icons/calculator.svg");
pub(crate) const APPLICATION: &str = include_str!("../../../assets/icons/application.svg");
pub(crate) const FILE: &str = include_str!("../../../assets/icons/file.svg");
pub(crate) const SETTINGS: &str = include_str!("../../../assets/icons/settings.svg");
pub(crate) const PLUGIN: &str = include_str!("../../../assets/icons/plugin.svg");

pub(crate) fn owned(icon: &'static str) -> String {
    icon.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_icons_are_svg_documents() {
        for icon in [OPEN_URL, CALCULATOR, APPLICATION, FILE, SETTINGS, PLUGIN] {
            let icon = icon.trim();
            assert!(icon.starts_with("<svg"));
            assert!(icon.ends_with("</svg>"));
        }
    }
}

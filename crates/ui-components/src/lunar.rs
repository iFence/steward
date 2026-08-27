//! Lunar (Chinese calendar) labels for the calendar grid.
//!
//! The launcher renders lunar info host-side via `tyme4rs`, so every plugin
//! `calendar` view gets it without extending the view contract. Each day cell
//! shows the lunar day name under the solar day; the first day of a lunar
//! month shows the month name instead, and festivals / solar terms take
//! precedence over both.

use tyme4rs::tyme::solar::SolarDay;
use tyme4rs::tyme::Culture;

/// Lunar labels for one Gregorian date, rendered as the second line of a
/// calendar day cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LunarInfo {
    /// Lunar day name, e.g. `初一` / `十五` / `三十`.
    pub day_name: String,
    /// Lunar month name (e.g. `正月` / `闰四月`), set only on the first day
    /// of a lunar month so the grid can mark month starts.
    pub month_name: Option<String>,
    /// Festival name (traditional lunar first, modern solar second), e.g.
    /// `春节` / `中秋节` / `国庆节`.
    pub festival: Option<String>,
    /// Solar term name when the day is one of the 24 terms, e.g. `立春`.
    pub solar_term: Option<String>,
}

impl LunarInfo {
    /// The single label rendered under the day number, by priority:
    /// festival > solar term > lunar month name (month start) > lunar day.
    pub fn label(&self) -> &str {
        self.festival
            .as_deref()
            .or(self.solar_term.as_deref())
            .or(self.month_name.as_deref())
            .unwrap_or(&self.day_name)
    }

    /// Whether the label is a festival or solar term and should be accented.
    pub fn highlighted(&self) -> bool {
        self.festival.is_some() || self.solar_term.is_some()
    }
}

/// Chinese lunar day names, `初一` .. `三十` (matches `tyme4rs` conventions).
const LUNAR_DAY_NAMES: [&str; 30] = [
    "初一", "初二", "初三", "初四", "初五", "初六", "初七", "初八", "初九", "初十", "十一", "十二",
    "十三", "十四", "十五", "十六", "十七", "十八", "十九", "二十", "廿一", "廿二", "廿三", "廿四",
    "廿五", "廿六", "廿七", "廿八", "廿九", "三十",
];

fn lunar_day_name(day: usize) -> String {
    LUNAR_DAY_NAMES
        .get(day.saturating_sub(1))
        .map(|name| (*name).to_string())
        .unwrap_or_default()
}

/// Lunar info for a Gregorian date, or `None` when the date is invalid.
pub fn lunar_info(year: i32, month: u32, day: u32) -> Option<LunarInfo> {
    // `SolarDay::validate` panics on out-of-range months (it builds a
    // `SolarMonth` via `unwrap`), so reject them before calling into tyme4rs.
    if !(1..=12).contains(&month) || day == 0 || day > 31 {
        return None;
    }
    let solar = SolarDay::new(year as isize, month as usize, day as usize).ok()?;
    let lunar = solar.get_lunar_day();
    let lunar_month = lunar.get_lunar_month();
    let day_name = lunar_day_name(lunar.get_day());
    let month_name = (lunar.get_day() == 1).then(|| lunar_month.get_name());
    // Traditional lunar festivals win over modern solar festivals.
    let festival = lunar
        .get_festival()
        .map(|festival| festival.get_name())
        .or_else(|| solar.get_festival().map(|festival| festival.get_name()));
    let term_day = solar.get_term_day();
    let solar_term = (term_day.get_day_index() == 0).then(|| term_day.get_solar_term().get_name());
    Some(LunarInfo {
        day_name,
        month_name,
        festival,
        solar_term,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_festival_shows_lunar_month_and_festival() {
        // 2024-02-10 is the first day of the lunar year (甲辰龙年正月初一).
        let info = lunar_info(2024, 2, 10).expect("valid date");
        assert_eq!(info.day_name, "初一");
        assert_eq!(info.month_name.as_deref(), Some("正月"));
        assert_eq!(info.festival.as_deref(), Some("春节"));
        assert!(info.solar_term.is_none());
        assert_eq!(info.label(), "春节");
        assert!(info.highlighted());
    }

    #[test]
    fn mid_autumn_festival_beats_lunar_day_name() {
        // 2024-09-17 is 中秋节 (八月十五).
        let info = lunar_info(2024, 9, 17).expect("valid date");
        assert_eq!(info.day_name, "十五");
        assert!(info.month_name.is_none());
        assert_eq!(info.festival.as_deref(), Some("中秋节"));
        assert_eq!(info.label(), "中秋节");
        assert!(info.highlighted());
    }

    #[test]
    fn plain_days_show_lunar_day_name() {
        // 2024-02-14 is 正月初五, no festival or term.
        let info = lunar_info(2024, 2, 14).expect("valid date");
        assert_eq!(info.day_name, "初五");
        assert!(info.month_name.is_none());
        assert!(info.festival.is_none());
        assert!(info.solar_term.is_none());
        assert_eq!(info.label(), "初五");
        assert!(!info.highlighted());
    }

    #[test]
    fn leap_month_start_marks_the_month_name() {
        // 2020-05-23 is 闰四月初一 (2020 has a leap 4th month); the next day
        // is a plain lunar day of that leap month.
        let first = lunar_info(2020, 5, 23).expect("valid date");
        assert_eq!(first.month_name.as_deref(), Some("闰四月"));
        assert_eq!(first.day_name, "初一");
        let second = lunar_info(2020, 5, 24).expect("valid date");
        assert!(second.month_name.is_none());
        assert_eq!(second.day_name, "初二");
    }

    #[test]
    fn solar_terms_are_detected_on_their_day() {
        // 2024-02-04 is 立春; 2024-12-21 is 冬至 (also 冬至节, a festival).
        let lichun = lunar_info(2024, 2, 4).expect("valid date");
        assert_eq!(lichun.solar_term.as_deref(), Some("立春"));
        assert_eq!(lichun.label(), "立春");
        assert!(lichun.highlighted());

        let dongzhi = lunar_info(2024, 12, 21).expect("valid date");
        assert_eq!(dongzhi.festival.as_deref(), Some("冬至节"));
        assert_eq!(dongzhi.solar_term.as_deref(), Some("冬至"));
        assert_eq!(dongzhi.label(), "冬至节");
    }

    #[test]
    fn modern_solar_festivals_are_labeled() {
        // 2024-10-01 is 国庆节 (a modern solar festival).
        let info = lunar_info(2024, 10, 1).expect("valid date");
        assert_eq!(info.festival.as_deref(), Some("国庆节"));
        assert_eq!(info.label(), "国庆节");
        assert!(info.highlighted());
    }

    #[test]
    fn invalid_dates_yield_no_lunar_info() {
        assert!(lunar_info(2024, 0, 1).is_none());
        assert!(lunar_info(2024, 13, 1).is_none());
        assert!(lunar_info(2024, 2, 30).is_none());
    }
}

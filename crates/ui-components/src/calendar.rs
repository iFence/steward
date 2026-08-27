//! A month calendar grid rendered in the launcher when a plugin returns a
//! `calendar` view (M2 extension to the plugin view contract).
//!
//! The grid is a plain stacked layout like the results list: one header row,
//! one weekday row and six 7-cell week rows, with a left rail labelling each
//! row's ISO week number. Day cells are individually clickable; the app owns
//! keyboard navigation and confirmation, so this component only reports
//! clicks and highlights the currently selected date.

use std::rc::Rc;

use gpui::{
    div, prelude::FluentBuilder as _, px, rgb, App, AppContext, Context, ElementId, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _,
};

use crate::lunar::lunar_info;

// Toggle the launcher's pinned state while a calendar view is open. The app
// handles it: when pinned, losing window activation no longer hides the
// launcher, so the calendar stays visible while the user works elsewhere.
gpui::actions!(steward, [ToggleCalendarPin]);

/// Parsed plugin calendar view: one month grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarData {
    pub year: i32,
    /// 1..=12.
    pub month: u32,
    /// ISO date `YYYY-MM-DD` of "today" (accent highlight).
    pub today: String,
    /// ISO date the keyboard selection starts at.
    pub selected: String,
    /// 0 = Sunday-first, 1 = Monday-first.
    pub start_of_week: u8,
}

/// Callback fired when a day is clicked: the day's ISO date.
pub type CalendarSelectCallback = Rc<dyn Fn(String, &mut App)>;

/// Grid metrics (logical px); the app uses [`CALENDAR_GRID_HEIGHT`] to size
/// the launcher window when a calendar view is active.
pub const CALENDAR_HEADER_HEIGHT: f32 = 34.0;
pub const CALENDAR_WEEKDAY_HEIGHT: f32 = 26.0;
pub const CALENDAR_ROW_HEIGHT: f32 = 52.0;
pub const CALENDAR_ROWS: usize = 6;
/// Fixed width of the left week-number rail (logical px).
pub const CALENDAR_WEEK_COLUMN_WIDTH: f32 = 38.0;
/// Card chrome around the grid: a 1px hairline border and vertical padding.
pub const CALENDAR_CARD_BORDER: f32 = 1.0;
pub const CALENDAR_CARD_PADDING_Y: f32 = 4.0;
pub const CALENDAR_CARD_PADDING_X: f32 = 6.0;
/// Total outer height of the calendar card: chrome + header + weekday row +
/// the six week rows. GPUI lays out with a border-box model, so pinning both
/// the card and the launcher window to this value fits the grid exactly.
pub const CALENDAR_GRID_HEIGHT: f32 = CALENDAR_CARD_BORDER * 2.0
    + CALENDAR_CARD_PADDING_Y * 2.0
    + CALENDAR_HEADER_HEIGHT
    + CALENDAR_WEEKDAY_HEIGHT
    + CALENDAR_ROWS as f32 * CALENDAR_ROW_HEIGHT;

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Number of days in `month` (1..=12) of `year`.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Days since 0001-01-01 (a Monday in the proleptic Gregorian calendar) for a
/// civil date. The weekday is derived from this count modulo 7.
fn days_since_epoch(year: i32, month: u32, day: u32) -> i64 {
    let year = year as i64;
    let month = month as i64;
    let day = day as i64;
    let days_before_year = 365 * (year - 1) + (year - 1) / 4 - (year - 1) / 100 + (year - 1) / 400;
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_before_month = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]
        .get((month - 1) as usize)
        .copied()
        .unwrap_or(0)
        + if leap && month > 2 { 1 } else { 0 };
    days_before_year + days_before_month + day - 1
}

/// ISO date string `YYYY-MM-DD` for a civil date.
pub fn iso_date(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// ISO 8601 week number (1..=53) of a civil date: weeks start on Monday and
/// week 1 contains the year's first Thursday. Dates near the year boundary
/// belong to the neighbouring ISO year (e.g. 2027-01-01 is 2026-W53).
pub fn iso_week(year: i32, month: u32, day: u32) -> u32 {
    let ordinal = days_since_epoch(year, month, day) - days_since_epoch(year, 1, 1) + 1;
    // Weekday as 1 = Monday .. 7 = Sunday (ISO convention).
    let weekday = ((days_since_epoch(year, month, day) + 1).rem_euclid(7) + 6) % 7 + 1;
    let mut week = (ordinal - weekday + 10) / 7;
    if week < 1 {
        week = weeks_in_iso_year(year - 1) as i64;
    } else if week > weeks_in_iso_year(year) as i64 {
        week = 1;
    }
    week as u32
}

/// Number of weeks in an ISO year: 53 when Jan 1 is a Thursday, or a
/// Wednesday in a leap year; 52 otherwise.
fn weeks_in_iso_year(year: i32) -> u32 {
    let jan1_weekday = ((days_since_epoch(year, 1, 1) + 1).rem_euclid(7) + 6) % 7 + 1;
    if jan1_weekday == 4 || (jan1_weekday == 3 && is_leap_year(year)) {
        53
    } else {
        52
    }
}

/// Inverse of [`days_since_epoch`]: the civil date (`year`, `month`, `day`)
/// for a day count since 0001-01-01. Howard Hinnant's `civil_from_days`,
/// shifted for this crate's epoch.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    // Our epoch (0001-01-01) precedes Hinnant's (1970-01-01) by 719162 days.
    let days = days + 719468 - 719162;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

/// Number of blank cells before the first day of `month` for the given
/// `start_of_week` (0 = Sunday-first, 1 = Monday-first).
fn leading_blanks(year: i32, month: u32, start_of_week: u8) -> usize {
    let weekday = (days_since_epoch(year, month, 1) + 1).rem_euclid(7) as u32; // 0 = Sunday
    ((weekday + 7 - (start_of_week % 7) as u32) % 7) as usize
}

/// The month laid out as a 7-column grid of `CALENDAR_ROWS * 7` cells,
/// padded with `None` blanks before the first and after the last day.
pub fn month_grid(year: i32, month: u32, start_of_week: u8) -> Vec<Option<u32>> {
    let days = days_in_month(year, month);
    let leading = leading_blanks(year, month, start_of_week);
    let mut cells = vec![None; leading];
    cells.extend((1..=days).map(Some));
    while cells.len() < CALENDAR_ROWS * 7 {
        cells.push(None);
    }
    cells.truncate(CALENDAR_ROWS * 7);
    cells
}

/// "W{n}" ISO week label for the week row starting at grid cell `row * 7`
/// (counted from the month grid's first cell). The row's week is the ISO
/// week of the Thursday inside it, which is unambiguous for both Monday- and
/// Sunday-first layouts.
pub fn week_label(year: i32, month: u32, start_of_week: u8, row: usize) -> String {
    let leading = leading_blanks(year, month, start_of_week);
    let row_start = days_since_epoch(year, month, 1) - leading as i64 + row as i64 * 7;
    let weekday = (row_start + 1).rem_euclid(7); // 0 = Sunday .. 6 = Saturday
    let thursday = row_start + (4 - weekday).rem_euclid(7);
    let (ty, tm, td) = civil_from_days(thursday);
    format!("W{}", iso_week(ty, tm, td))
}

/// The state backing the calendar grid; mirrors the results-list pattern.
pub struct CalendarViewState {
    data: CalendarData,
    /// Localized month label (e.g. "August 2026" / "2026 年 8 月").
    month_label: String,
    /// Localized weekday labels, ordered from `start_of_week`.
    weekday_labels: [String; 7],
    selected: String,
    on_select: Option<CalendarSelectCallback>,
    /// Whether the launcher is pinned open (blur no longer hides it).
    pinned: bool,
    max_height: f32,
}

impl Render for CalendarViewState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let data = self.data.clone();
        let cells = month_grid(data.year, data.month, data.start_of_week);
        let selected = self.selected.clone();
        let today = data.today.clone();
        let pinned = self.pinned;

        let header = div()
            .id(ElementId::from("cal-header"))
            .h(px(CALENDAR_HEADER_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .text_color(rgb(crate::palette::FOREGROUND))
            .text_sm()
            .child(div().flex_1().child(self.month_label.clone()))
            .child(pin_button(pinned, cx));

        let weekday_row = div()
            .id(ElementId::from("cal-weekdays"))
            .h(px(CALENDAR_WEEKDAY_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            // Blank rail cell keeps the weekday labels aligned with the day
            // columns below.
            .child(week_rail_cell(None));
        let weekday_row = self.weekday_labels.iter().fold(weekday_row, |row, label| {
            row.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(crate::palette::MUTED_FOREGROUND))
                    .text_size(px(11.0))
                    .child(label.to_owned()),
            )
        });

        let mut week_rows = Vec::new();
        for (week_index, week) in cells.chunks(7).enumerate() {
            let week_number = week_label(data.year, data.month, data.start_of_week, week_index);
            let mut row = div()
                .id(ElementId::from(format!("cal-week-{week_index}")))
                .w_full()
                .h(px(CALENDAR_ROW_HEIGHT))
                .flex()
                .flex_row()
                .child(week_rail_cell(Some((week_number, week_index))));
            for day in week {
                let cell = match day {
                    Some(day) => {
                        let iso = iso_date(data.year, data.month, *day);
                        let is_today = iso == today;
                        let is_selected = iso == selected;
                        let lunar = lunar_info(data.year, data.month, *day);
                        let cell_iso = iso.clone();
                        div()
                            .id(ElementId::from(format!("cal-day-{iso}")))
                            .flex_1()
                            .h_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .mx(px(2.0))
                            .text_color(if is_today {
                                rgb(crate::palette::ACCENT)
                            } else {
                                rgb(crate::palette::FOREGROUND)
                            })
                            .text_sm()
                            .cursor_pointer()
                            .when(is_selected, |this| {
                                this.bg(rgb(crate::palette::SELECTION).opacity(0.14))
                            })
                            .when(!is_selected && is_today, |this| {
                                this.bg(rgb(crate::palette::ACCENT).opacity(0.18))
                            })
                            .when(!is_selected, |this| {
                                this.hover(|style| {
                                    style.bg(rgb(crate::palette::HOVER).opacity(0.05))
                                })
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(cb) = this.on_select.clone() {
                                    cb(cell_iso.clone(), cx);
                                }
                                cx.notify();
                            }))
                            .child(day.to_string())
                            .when(lunar.is_some(), |this| {
                                let info = lunar.as_ref().expect("checked");
                                this.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(if info.highlighted() {
                                            rgb(crate::palette::ACCENT)
                                        } else {
                                            rgb(crate::palette::MUTED_FOREGROUND)
                                        })
                                        .child(info.label().to_string()),
                                )
                            })
                    }
                    None => div()
                        .id(ElementId::from(format!(
                            "cal-blank-{week_index}-{}",
                            week.len()
                        )))
                        .flex_1()
                        .h_full(),
                };
                row = row.child(cell);
            }
            week_rows.push(row);
        }

        div()
            .id(ElementId::from("calendar-grid"))
            .h(px(self.max_height))
            .w_full()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0xffffff).opacity(0.08))
            .bg(rgb(crate::palette::BACKGROUND_ALT).opacity(0.35))
            .px(px(CALENDAR_CARD_PADDING_X))
            .py(px(CALENDAR_CARD_PADDING_Y))
            .child(header)
            .child(weekday_row)
            .children(week_rows)
    }
}

/// The fixed-width left rail cell: a week-number label in week rows, or a
/// blank spacer in the weekday header that keeps the day columns aligned. A
/// hairline separates the rail from the day grid.
fn week_rail_cell(week: Option<(String, usize)>) -> impl IntoElement {
    let cell = div()
        .w(px(CALENDAR_WEEK_COLUMN_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .border_r_1()
        .border_color(rgb(0xffffff).opacity(0.06));
    match week {
        Some((label, row)) => cell
            .id(ElementId::from(format!("cal-week-number-{row}")))
            .text_color(rgb(crate::palette::MUTED_FOREGROUND))
            .text_size(px(11.0))
            .child(label),
        None => cell.id(ElementId::from("cal-week-rail-spacer")),
    }
}

/// The pin/unpin toggle in the calendar header. Clicking it dispatches
/// [`ToggleCalendarPin`]; the launcher flips the shared pinned state and
/// pushes it back through [`CalendarView::set_pinned`]. Both states share the
/// same pushpin glyph; the accent border/fill marks the pinned state.
fn pin_button(pinned: bool, cx: &mut Context<CalendarViewState>) -> impl IntoElement {
    div()
        .id(ElementId::from("cal-pin-toggle"))
        .flex()
        .items_center()
        .justify_center()
        .h(px(22.0))
        .px_2()
        .rounded_full()
        .cursor_pointer()
        .border_1()
        .border_color(rgb(0xffffff).opacity(if pinned { 0.20 } else { 0.08 }))
        .text_size(px(14.0))
        .text_color(if pinned {
            rgb(crate::palette::ACCENT)
        } else {
            rgb(crate::palette::MUTED_FOREGROUND)
        })
        .when(pinned, |this| {
            this.bg(rgb(crate::palette::ACCENT).opacity(0.12))
        })
        .when(!pinned, |this| {
            this.hover(|style| style.bg(rgb(crate::palette::HOVER).opacity(0.05)))
        })
        .on_click(cx.listener(|_, _, _, cx| {
            cx.dispatch_action(&ToggleCalendarPin);
        }))
        .child("📌")
}

/// The launcher's calendar grid: a small entity wrapper so the app can update
/// data/selection without owning the state directly.
#[derive(Clone)]
pub struct CalendarView {
    state: Entity<CalendarViewState>,
}

impl CalendarView {
    pub fn new<C>(
        on_select: Option<CalendarSelectCallback>,
        _window: &mut gpui::Window,
        cx: &mut Context<C>,
    ) -> Self {
        let state = cx.new(|_| CalendarViewState {
            data: CalendarData {
                year: 1970,
                month: 1,
                today: String::new(),
                selected: String::new(),
                start_of_week: 1,
            },
            month_label: String::new(),
            weekday_labels: Default::default(),
            selected: String::new(),
            on_select,
            pinned: false,
            max_height: CALENDAR_GRID_HEIGHT,
        });
        Self { state }
    }

    /// Replace the month, labels and selection (called on every search /
    /// plugin-view merge).
    pub fn set_data<C: gpui::AppContext>(
        &self,
        data: CalendarData,
        month_label: String,
        weekday_labels: [String; 7],
        selected: String,
        cx: &mut C,
    ) {
        self.state.update(cx, |this, cx| {
            this.data = data;
            this.month_label = month_label;
            this.weekday_labels = weekday_labels;
            this.selected = selected;
            cx.notify();
        });
    }

    /// Move the keyboard selection (clamped to the displayed month).
    pub fn set_selected<C: gpui::AppContext>(&self, selected: &str, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            this.selected = selected.to_string();
            cx.notify();
        });
    }

    /// Reflect the launcher's pinned state.
    pub fn set_pinned<C: gpui::AppContext>(&self, pinned: bool, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            this.pinned = pinned;
            cx.notify();
        });
    }

    pub fn render<C>(&self, max_height: f32, cx: &mut Context<C>) -> impl IntoElement {
        self.state.update(cx, |this, _| {
            this.max_height = max_height;
        });
        self.state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_grid_pads_leading_and_trailing_blanks() {
        // 2026-08-01 is a Saturday; Monday-first -> 5 leading blanks, 31 days,
        // 42 cells total.
        let cells = month_grid(2026, 8, 1);
        assert_eq!(cells.len(), CALENDAR_ROWS * 7);
        assert_eq!(cells.iter().filter(|c| c.is_some()).count(), 31);
        assert!(cells.iter().take(5).all(Option::is_none));
        assert_eq!(cells[5], Some(1));
        assert_eq!(cells[5 + 30], Some(31));
        assert!(cells[36..].iter().all(Option::is_none));
    }

    #[test]
    fn sunday_first_shifts_the_leading_blanks() {
        let sunday_first = month_grid(2026, 8, 0);
        let monday_first = month_grid(2026, 8, 1);
        assert!(sunday_first.iter().take(6).all(Option::is_none));
        assert_eq!(sunday_first[6], Some(1));
        assert_eq!(monday_first[5], Some(1));
    }

    #[test]
    fn leap_year_february_has_29_days() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
    }

    #[test]
    fn iso_date_pads_to_four_and_two_digits() {
        assert_eq!(iso_date(2026, 8, 1), "2026-08-01");
        assert_eq!(iso_date(26, 12, 31), "0026-12-31");
    }

    #[test]
    fn iso_week_matches_iso_8601_reference_dates() {
        assert_eq!(iso_week(2026, 8, 27), 35);
        assert_eq!(iso_week(2026, 1, 1), 1);
        assert_eq!(iso_week(2026, 12, 31), 53);
        assert_eq!(iso_week(2027, 1, 1), 53, "Jan 1 2027 is 2026-W53");
        assert_eq!(iso_week(2025, 12, 29), 1, "Dec 29 2025 starts 2026-W1");
        assert_eq!(iso_week(2028, 1, 1), 52, "Jan 1 2028 is 2027-W52");
        assert_eq!(iso_week(2029, 1, 1), 1);
    }

    #[test]
    fn civil_from_days_inverts_days_since_epoch() {
        for days in [0, 1, 305, 719_162, 739_854] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_since_epoch(year, month, day), days);
        }
        assert_eq!(civil_from_days(719_162), (1970, 1, 1));
        assert_eq!(civil_from_days(739_854), (2026, 8, 27));
    }

    #[test]
    fn week_label_uses_the_iso_week_of_each_row() {
        // August 2026, Monday-first: rows run Jul 27-Aug 2 (W31) through
        // Aug 31-Sep 6 (W36).
        let labels: Vec<String> = (0..6).map(|row| week_label(2026, 8, 1, row)).collect();
        assert_eq!(labels, ["W31", "W32", "W33", "W34", "W35", "W36"]);
        // Sunday-first shifts the rows but keeps the Thursday rule: the first
        // row (Jul 26-Aug 1) is W31 and the second (Aug 2-8) is W32.
        assert_eq!(week_label(2026, 8, 0, 0), "W31");
        assert_eq!(week_label(2026, 8, 0, 1), "W32");
        // January 2027 starts mid-week: the first row (Dec 28 2026 - Jan 3
        // 2027) belongs to 2026-W53.
        assert_eq!(week_label(2027, 1, 1, 0), "W53");
    }
}

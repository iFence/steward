//! A month calendar grid rendered in the launcher when a plugin returns a
//! `calendar` view (M2 extension to the plugin view contract).
//!
//! The grid is a plain stacked layout like the results list: one header row,
//! one weekday row and six 7-cell week rows. Day cells are individually
//! clickable; the app owns keyboard navigation and confirmation, so this
//! component only reports clicks and highlights the currently selected date.

use std::rc::Rc;

use gpui::{
    div, prelude::FluentBuilder as _, px, rgb, App, AppContext, Context, ElementId, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _,
};

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
pub const CALENDAR_ROW_HEIGHT: f32 = 44.0;
pub const CALENDAR_ROWS: usize = 6;
pub const CALENDAR_GRID_HEIGHT: f32 =
    CALENDAR_HEADER_HEIGHT + CALENDAR_WEEKDAY_HEIGHT + CALENDAR_ROWS as f32 * CALENDAR_ROW_HEIGHT;

/// Number of days in `month` (1..=12) of `year`.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap {
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

/// The month laid out as a 7-column grid of `CALENDAR_ROWS * 7` cells,
/// padded with `None` blanks before the first and after the last day.
pub fn month_grid(year: i32, month: u32, start_of_week: u8) -> Vec<Option<u32>> {
    let days = days_in_month(year, month);
    // Day 0 is a Monday; shift by +1 so 0 = Sunday ... 6 = Saturday.
    let weekday = (days_since_epoch(year, month, 1) + 1).rem_euclid(7) as u32;
    let leading = (weekday + 7 - (start_of_week % 7) as u32) % 7;
    let mut cells = vec![None; leading as usize];
    cells.extend((1..=days).map(Some));
    while cells.len() < CALENDAR_ROWS * 7 {
        cells.push(None);
    }
    cells.truncate(CALENDAR_ROWS * 7);
    cells
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
    max_height: f32,
}

impl Render for CalendarViewState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let data = self.data.clone();
        let cells = month_grid(data.year, data.month, data.start_of_week);
        let selected = self.selected.clone();
        let today = data.today.clone();

        let header = div()
            .id(ElementId::from("cal-header"))
            .h(px(CALENDAR_HEADER_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .px_3()
            .text_color(rgb(crate::palette::FOREGROUND))
            .text_sm()
            .child(self.month_label.clone());

        let weekday_row = div()
            .id(ElementId::from("cal-weekdays"))
            .h(px(CALENDAR_WEEKDAY_HEIGHT))
            .w_full()
            .flex()
            .flex_row();
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
            let mut row = div()
                .id(ElementId::from(format!("cal-week-{week_index}")))
                .w_full()
                .h(px(CALENDAR_ROW_HEIGHT))
                .flex()
                .flex_row();
            for day in week {
                let cell = match day {
                    Some(day) => {
                        let iso = iso_date(data.year, data.month, *day);
                        let is_today = iso == today;
                        let is_selected = iso == selected;
                        let cell_iso = iso.clone();
                        div()
                            .id(ElementId::from(format!("cal-day-{iso}")))
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
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
            .child(header)
            .child(weekday_row)
            .children(week_rows)
    }
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
}

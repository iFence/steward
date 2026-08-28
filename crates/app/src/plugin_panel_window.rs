//! Generic plugin-view windows: pop a plugin command's view into its own
//! independent, always-on-top window so the launcher can keep doing other work
//! (e.g. a calendar floating beside the bar). The window host dispatches on the
//! plugin view type (`calendar` renders the month grid, `list` renders the
//! plugin's items as rows), so any command that declares `detachable: true` can
//! use the same mechanism without coupling the app to a specific plugin.
//!
//! Detached windows live outside `LauncherState.window`: the global summon
//! hotkey and the launcher's blur/foreground hide logic never touch them, so
//! the main panel keeps working while the widget stays put.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    div, prelude::*, px, rgb, size, AnyWindowHandle, App, Bounds, Context, ElementId, FocusHandle,
    KeyDownEvent, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowKind, WindowOptions,
};
use steward_ui_components::{
    days_in_month, iso_date, palette, CalendarView, ResultItem, ResultList, ResultListDelegate,
    CALENDAR_GRID_HEIGHT,
};

use crate::config::{
    MAX_RESULT_ROWS, PLUGIN_WIDGET_PADDING, PLUGIN_WIDGET_WIDTH, RESULT_ROW_HEIGHT,
};
use crate::i18n::Localization;
use crate::launcher::{
    calendar_month_label, calendar_weekday_labels, parse_calendar_view, parse_iso_date,
    plugin_view_items, ActiveCalendar, LauncherState, StewardApp,
};
use crate::platform;

/// Height of the invisible drag handle along the top of a detached plugin
/// window (logical px). The rest of the window stays interactive.
const PANEL_DRAG_HEIGHT: f32 = 12.0;
/// Height of the month/year navigation toolbar shown above a detached calendar
/// panel (logical px).
const PANEL_NAV_HEIGHT: f32 = 26.0;
/// Storage key persisting the detached panel window's logical size
/// (`"<width> <height>"`), so reopening keeps the user's chosen size.
const PANEL_WINDOW_SIZE_KEY: &str = "panel_window_size";

/// Height of a list panel window for `count` plugin rows, capped like the
/// launcher's drop-down (`MAX_RESULT_ROWS`).
fn list_height(count: usize) -> f32 {
    RESULT_ROW_HEIGHT * count.min(MAX_RESULT_ROWS) as f32
}

/// The view payload a detached window renders. The host dispatches on the
/// `type` field of the raw plugin view.
enum PanelKind {
    Calendar(ActiveCalendar),
    List(Vec<ResultItem>),
}

/// A detached plugin-view window: one entity per open widget.
pub(crate) struct PluginPanelWindow {
    plugin_id: String,
    command: String,
    state: Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    kind: PanelKind,
    calendar: Option<CalendarView>,
    list: Option<ResultList>,
    /// Keyboard-selected ISO date for a calendar panel (`YYYY-MM-DD`).
    selection: String,
    focus: FocusHandle,
}

impl Render for PluginPanelWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scrim = self.state.borrow().scrim_alpha;
        let padding = PLUGIN_WIDGET_PADDING;
        let has_nav = matches!(self.kind, PanelKind::Calendar(_));
        let nav_h = if has_nav { PANEL_NAV_HEIGHT } else { 0.0 };
        // Grow the content with the window: the drag handle and padding stay
        // fixed, everything else fills the client area so resizing the window
        // (larger or smaller) scales the calendar/list rather than adding dead
        // space or clipping it.
        let content_h = (window.viewport_size().height.as_f32() - PANEL_DRAG_HEIGHT - nav_h - padding * 2.0)
            .max(120.0);
        let nav_toolbar = if has_nav {
            self.nav_toolbar(cx).into_any_element()
        } else {
            div().into_any_element()
        };
        let root = div()
            .id(ElementId::from("plugin-panel"))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(palette::BACKGROUND).opacity(scrim))
            .text_lg()
            .text_color(rgb(palette::FOREGROUND))
            // Invisible drag handle along the top edge: the only region that
            // moves the window, so day/pin clicks below stay interactive.
            .child(
                div()
                    .id(ElementId::from("panel-drag"))
                    .h(px(PANEL_DRAG_HEIGHT))
                    .w_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(nav_toolbar)
            .child(
                div()
                    .flex_1()
                    .p(px(padding))
                    .child(match &self.kind {
                        PanelKind::Calendar(_) => div()
                            .id(ElementId::from("panel-calendar"))
                            .h(px(content_h))
                            .w_full()
                            .child(
                                self.calendar
                                    .as_ref()
                                    .expect("calendar panel has a CalendarView")
                                    .render(content_h, cx),
                            ),
                        PanelKind::List(_) => {
                            let height = content_h;
                            div()
                                .id(ElementId::from("panel-list"))
                                .h(px(height))
                                .w_full()
                                .child(
                                    self.list
                                        .as_ref()
                                        .expect("list panel has a ResultList")
                                        .render(height, palette::SELECTION_WASH, cx),
                                )
                        }
                    }),
            );
        root
    }
}

impl PluginPanelWindow {
    fn new(
        state: Rc<RefCell<LauncherState>>,
        i18n: Rc<Localization>,
        plugin_id: String,
        command: String,
        kind: PanelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let mut calendar = None;
        let mut list = None;
        let mut selection = String::new();

        match &kind {
            PanelKind::Calendar(active) => {
                let select_state = state.clone();
                let select_plugin = active.plugin_id.clone();
                let on_select: steward_ui_components::CalendarSelectCallback =
                    Rc::new(move |date: String, _cx: &mut App| {
                        if select_state
                            .borrow()
                            .plugin_host
                            .borrow_mut()
                            .invoke_item(&select_plugin, &date)
                            .is_none()
                        {
                            eprintln!(
                                "[steward] plugin {} not ready for item.invoke",
                                select_plugin
                            );
                        }
                    });
                let dock_state = state.clone();
                let dock_plugin = active.plugin_id.clone();
                let dock_command = command.clone();
                let on_toggle_pin: steward_ui_components::PinToggleCallback =
                    Rc::new(move |pinned: bool, cx: &mut App| {
                        // In the detached window, unpinning means docking the
                        // view back into the launcher.
                        if !pinned {
                            dock_panel_back(&dock_state, &dock_plugin, &dock_command, cx);
                        }
                    });
                let view = CalendarView::new(Some(on_select), Some(on_toggle_pin), window, cx);
                let language = i18n.language();
                view.set_data(
                    active.data.clone(),
                    calendar_month_label(&language, active.data.year, active.data.month),
                    calendar_weekday_labels(&language, active.data.start_of_week),
                    active.data.selected.clone(),
                    cx,
                );
                view.set_detachable(true, cx);
                view.set_pinned(true, cx);
                selection = active.data.selected.clone();
                calendar = Some(view);
            }
            PanelKind::List(items) => {
                let items_rc = Rc::new(RefCell::new(items.clone()));
                let host = state.borrow().plugin_host.clone();
                let title = i18n.translate("command");
                let delegate = ResultListDelegate::new().type_label(title).on_confirm(
                    move |index, _cx: &mut App| {
                        let item = items_rc.borrow().get(index).cloned();
                        if let Some(ResultItem::Plugin {
                            plugin_id, item_id, ..
                        }) = item
                        {
                            if host
                                .borrow_mut()
                                .invoke_item(&plugin_id, &item_id)
                                .is_none()
                            {
                                eprintln!(
                                    "[steward] plugin {} is not ready for item.invoke",
                                    plugin_id
                                );
                            }
                        }
                        false
                    },
                );
                let view = ResultList::new(delegate, window, cx);
                let icons = {
                    let state = state.borrow();
                    items
                        .iter()
                        .map(|row| match row {
                            ResultItem::Plugin { plugin_id, .. } => state
                                .plugin_icons
                                .borrow()
                                .get(plugin_id)
                                .cloned()
                                .flatten(),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                };
                view.set_results(items.clone(), icons, cx);
                list = Some(view);
            }
        }

        focus.focus(window, cx);
        Self {
            plugin_id,
            command,
            state,
            i18n,
            kind,
            calendar,
            list,
            selection,
            focus,
        }
    }

    /// The month/year navigation toolbar shown above a detached calendar
    /// panel. Prev/next month and prev/next year buttons re-feed a new month
    /// into the shared calendar view; the pin button stays in the card header.
    fn nav_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(ElementId::from("panel-nav"))
            .h(px(PANEL_NAV_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .px(px(PLUGIN_WIDGET_PADDING))
            .gap(px(4.0))
            .child(
                div()
                    .id(ElementId::from("nav-prev-year"))
                    .h(px(22.0))
                    .min_w(px(26.0))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(0xffffff).opacity(0.08))
                    .hover(|style| style.bg(rgb(palette::HOVER).opacity(0.05)))
                    .text_color(rgb(palette::MUTED_FOREGROUND))
                    .text_sm()
                    .on_click(cx.listener(|this, _, _, cx| this.navigate_year(-1, cx)))
                    .child("<<"),
            )
            .child(
                div()
                    .id(ElementId::from("nav-prev-month"))
                    .h(px(22.0))
                    .min_w(px(26.0))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(0xffffff).opacity(0.08))
                    .hover(|style| style.bg(rgb(palette::HOVER).opacity(0.05)))
                    .text_color(rgb(palette::MUTED_FOREGROUND))
                    .text_sm()
                    .on_click(cx.listener(|this, _, _, cx| this.navigate_month(-1, cx)))
                    .child("<"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id(ElementId::from("nav-next-month"))
                    .h(px(22.0))
                    .min_w(px(26.0))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(0xffffff).opacity(0.08))
                    .hover(|style| style.bg(rgb(palette::HOVER).opacity(0.05)))
                    .text_color(rgb(palette::MUTED_FOREGROUND))
                    .text_sm()
                    .on_click(cx.listener(|this, _, _, cx| this.navigate_month(1, cx)))
                    .child(">"),
            )
            .child(
                div()
                    .id(ElementId::from("nav-next-year"))
                    .h(px(22.0))
                    .min_w(px(26.0))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(0xffffff).opacity(0.08))
                    .hover(|style| style.bg(rgb(palette::HOVER).opacity(0.05)))
                    .text_color(rgb(palette::MUTED_FOREGROUND))
                    .text_sm()
                    .on_click(cx.listener(|this, _, _, cx| this.navigate_year(1, cx)))
                    .child(">>"),
            )
    }

    /// Move the displayed month by `delta` months, keeping the day-of-month
    /// (clamped) and re-feeding the calendar view with its localized label.
    fn navigate_month(&mut self, delta: i32, cx: &mut App) {
        let PanelKind::Calendar(active) = &mut self.kind else {
            return;
        };
        let year = active.data.year;
        let month = active.data.month;
        let (next_year, next_month) = shift_month(year, month, delta);
        self.apply_month(next_year, next_month, cx);
    }

    /// Move the displayed year by `delta` years, keeping the month and day.
    fn navigate_year(&mut self, delta: i32, cx: &mut App) {
        let PanelKind::Calendar(active) = &mut self.kind else {
            return;
        };
        let year = active.data.year;
        let month = active.data.month;
        self.apply_month(year + delta, month, cx);
    }

    /// Re-feed `(year, month)` into the calendar: update the data, clamp the
    /// selected day, recompute the localized label and mirror the selection.
    fn apply_month(&mut self, year: i32, month: u32, cx: &mut App) {
        let PanelKind::Calendar(active) = &mut self.kind else {
            return;
        };
        let day = parse_iso_date(&active.data.selected)
            .map(|(_, _, day)| day)
            .unwrap_or(1)
            .min(days_in_month(year, month));
        active.data.year = year;
        active.data.month = month;
        active.data.selected = iso_date(year, month, day);
        let selected = active.data.selected.clone();
        self.selection = selected.clone();
        let language = self.i18n.language();
        let label = calendar_month_label(&language, year, month);
        let weekdays = calendar_weekday_labels(&language, active.data.start_of_week);
        let data = active.data.clone();
        if let Some(cal) = &self.calendar {
            cal.set_data(data, label, weekdays, selected, cx);
        }
    }

    /// Keyboard handling: the detached widget owns the same selection /
    /// confirm keys as the launcher, plus Esc to dock it back.
    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match &self.kind {
            PanelKind::Calendar(_) => match keystroke.key.as_str() {
                "up" | "down" | "left" | "right" => {
                    let delta = match keystroke.key.as_str() {
                        "up" => -7,
                        "down" => 7,
                        "left" => -1,
                        _ => 1,
                    };
                    self.move_selection(delta, cx);
                    cx.stop_propagation();
                }
                "enter" => {
                    self.confirm_date(cx);
                    cx.stop_propagation();
                }
                "escape" => {
                    let (plugin_id, command) = (self.plugin_id.clone(), self.command.clone());
                    let state = self.state.clone();
                    dock_panel_back(&state, &plugin_id, &command, cx);
                    cx.stop_propagation();
                }
                _ => {}
            },
            PanelKind::List(_) => match keystroke.key.as_str() {
                "up" => {
                    if let Some(view) = self.list.as_mut() {
                        view.select_relative(-1, window, cx);
                    }
                    cx.stop_propagation();
                }
                "down" => {
                    if let Some(view) = self.list.as_mut() {
                        view.select_relative(1, window, cx);
                    }
                    cx.stop_propagation();
                }
                "enter" => {
                    if let Some(view) = self.list.as_ref() {
                        view.confirm_selected(window, cx);
                    }
                    cx.stop_propagation();
                }
                "escape" => {
                    let (plugin_id, command) = (self.plugin_id.clone(), self.command.clone());
                    let state = self.state.clone();
                    dock_panel_back(&state, &plugin_id, &command, cx);
                    cx.stop_propagation();
                }
                _ => {}
            },
        }
    }

    /// Move the calendar selection by `delta` days, clamped to the displayed
    /// month (mirrors the launcher's `StewardApp::calendar_move`).
    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some((year, month, day)) = parse_iso_date(&self.selection) else {
            return;
        };
        let days = steward_ui_components::days_in_month(year, month);
        let new_day = (day as i32 + delta).clamp(1, days as i32) as u32;
        self.selection = steward_ui_components::iso_date(year, month, new_day);
        if let Some(view) = self.calendar.as_ref() {
            view.set_selected(&self.selection, cx);
        }
        cx.notify();
    }

    /// Confirm the selected date: dispatch `item.invoke` to the plugin.
    fn confirm_date(&mut self, _cx: &mut Context<Self>) {
        let plugin_id = self.plugin_id.clone();
        let selected = self.selection.clone();
        if self
            .state
            .borrow()
            .plugin_host
            .borrow_mut()
            .invoke_item(&plugin_id, &selected)
            .is_none()
        {
            eprintln!("[steward] plugin {plugin_id} not ready for item.invoke");
        }
    }
}

/// Pop the currently active panel (e.g. the calendar grid) into its own window.
/// If the command's view is already open, it is focused instead.
pub(crate) fn open_plugin_panel_window(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut App,
) {
    let (plugin_id, command, detachable, view) = {
        let state = state.borrow();
        let Some(active) = state.plugin_calendar.borrow().clone() else {
            return;
        };
        let Some(view) = state.plugin_view(&active.plugin_id, &active.command) else {
            return;
        };
        (
            active.plugin_id.clone(),
            active.command.clone(),
            active.detachable,
            view,
        )
    };
    open_plugin_panel(state, i18n, plugin_id, command, view, detachable, cx);
}

/// Open a plugin command's view in its own window (the generic core; calendar
/// and list views are both supported). Returns the handle, or `None` when the
/// view type is not hosted or the panel data is unavailable.
pub(crate) fn open_plugin_panel(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    plugin_id: String,
    command: String,
    view: serde_json::Value,
    detachable: bool,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    // Already open: bring it forward.
    if let Some(handle) = state.borrow().plugin_window(&plugin_id, &command) {
        let _ = handle.update(cx, |_, window, cx| {
            cx.activate(true);
            window.refresh();
        });
        return Some(handle);
    }

    let kind = parse_calendar_view(&view, &plugin_id, &command, detachable)
        .map(PanelKind::Calendar)
        .or_else(|| {
            plugin_view_items(&view)
                .map(|_| PanelKind::List(list_items(&view, &plugin_id, &command)))
        })?;
    let (width, height) =
        read_panel_size(state).unwrap_or_else(|| (panel_width(), panel_height(&kind)));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            appears_transparent: true,
            ..Default::default()
        }),
        show: true,
        focus: true,
        kind: WindowKind::PopUp,
        // PopUp is draggable (is_movable defaults true); GPUI's PopUp branch
        // omits WS_THICKFRAME even with is_resizable, so make_resizable patches
        // the style after creation. The render then fills the client area.
        is_resizable: true,
        is_minimizable: false,
        window_min_size: Some(size(px(220.0), px(200.0))),
        window_background: WindowBackgroundAppearance::Blurred,
        ..Default::default()
    };
    let state_clone = state.clone();
    let i18n_clone = i18n.clone();
    let handle = cx
        .open_window(options, |window, cx| {
            window.set_window_title("Steward");
            // Add the resize border the PopUp kind does not provide.
            platform::make_resizable(window);
            cx.new(|cx| {
                PluginPanelWindow::new(
                    state_clone.clone(),
                    i18n_clone.clone(),
                    plugin_id.clone(),
                    command.clone(),
                    kind, // moved into the entity
                    window,
                    cx,
                )
            })
        })
        .expect("failed to open the plugin panel window")
        .into();
    state
        .borrow_mut()
        .panel_view_windows
        .borrow_mut()
        .insert((plugin_id, command), handle);
    Some(handle)
}

/// Focus an already-open plugin view window (no-op when it is not open).
pub(crate) fn focus_plugin_panel_window(
    state: &Rc<RefCell<LauncherState>>,
    plugin_id: &str,
    command: &str,
    cx: &mut App,
) {
    if let Some(handle) = state.borrow().plugin_window(plugin_id, command) {
        let _ = handle.update(cx, |_, window, cx| {
            cx.activate(true);
            window.refresh();
        });
    }
}

/// Dock a detached plugin view back into the launcher: close the window, drop
/// it from the registry, and re-render the launcher so the panel reappears.
pub(crate) fn dock_panel_back(
    state: &Rc<RefCell<LauncherState>>,
    plugin_id: &str,
    command: &str,
    cx: &mut App,
) {
    let handle = state
        .borrow_mut()
        .panel_view_windows
        .borrow_mut()
        .remove(&(plugin_id.to_string(), command.to_string()));
    if let Some(handle) = handle {
        // Remember the user's manual size so the next open reuses it.
        if let Ok(size) = handle.update(cx, |_, window, _| window.viewport_size()) {
            let _ = state
                .borrow()
                .storage
                .borrow()
                .set_setting(
                    PANEL_WINDOW_SIZE_KEY,
                    &format!("{} {}", size.width.as_f32(), size.height.as_f32()),
                );
        }
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    }
    // Re-render the launcher so the grid / view reappears inline.
    let launcher = state.borrow().window;
    if let Some(launcher) = launcher {
        if let Some(app) = launcher.downcast::<StewardApp>() {
            let _ = app.update(cx, |app, window, cx| app.apply_plugin_views(window, cx));
        }
    }
}

/// Called from the launcher's `on_window_closed`: when `window_id` belongs to a
/// detached plugin view, drop it from the registry and re-render the launcher.
pub(crate) fn panel_window_closed(
    state: &Rc<RefCell<LauncherState>>,
    window_id: gpui::WindowId,
    cx: &mut App,
) {
    let key = {
        let s = state.borrow();
        let windows = s.panel_view_windows.borrow();
        windows
            .iter()
            .find(|(_, handle)| handle.window_id() == window_id)
            .map(|(key, _)| key.clone())
    };
    let Some((plugin_id, command)) = key else {
        return;
    };
    state
        .borrow_mut()
        .panel_view_windows
        .borrow_mut()
        .remove(&(plugin_id, command));
    let launcher = state.borrow().window;
    if let Some(launcher) = launcher {
        if let Some(app) = launcher.downcast::<StewardApp>() {
            let _ = app.update(cx, |app, window, cx| app.apply_plugin_views(window, cx));
        }
    }
}

fn panel_width() -> f32 {
    PLUGIN_WIDGET_WIDTH
}

fn panel_height(kind: &PanelKind) -> f32 {
    let content = match kind {
        PanelKind::Calendar(_) => CALENDAR_GRID_HEIGHT,
        PanelKind::List(items) => list_height(items.len()),
    };
    let nav = if matches!(kind, PanelKind::Calendar(_)) {
        PANEL_NAV_HEIGHT
    } else {
        0.0
    };
    content + PLUGIN_WIDGET_PADDING * 2.0 + PANEL_DRAG_HEIGHT + nav
}

/// Read the persisted detached-panel window size (`"<width> <height>"`),
/// returning `None` when absent or below the minimum usable size.
fn read_panel_size(state: &Rc<RefCell<LauncherState>>) -> Option<(f32, f32)> {
    let raw = state
        .borrow()
        .storage
        .borrow()
        .get_setting(PANEL_WINDOW_SIZE_KEY)?;
    let mut parts = raw.split_whitespace();
    let width: f32 = parts.next()?.parse().ok()?;
    let height: f32 = parts.next()?.parse().ok()?;
    // Enforce a comfortable minimum width so a previous narrow size is
    // replaced by the (wider) default until the user explicitly sets wider.
    (width >= 520.0 && height >= 200.0).then_some((width, height))
}

/// Move `(year, month)` by `delta` months, wrapping across year boundaries.
fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + month as i32 - 1 + delta;
    (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
}

/// Build the plugin rows for a detached `list` view.
fn list_items(view: &serde_json::Value, plugin_id: &str, command: &str) -> Vec<ResultItem> {
    let Some(items) = plugin_view_items(view) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let item_id = item["id"].as_str()?;
            if item_id.is_empty() {
                return None;
            }
            let title = item["title"].as_str().unwrap_or("").to_string();
            let subtitle = item
                .get("subtitle")
                .and_then(|value| value.as_str())
                .unwrap_or(command)
                .to_string();
            Some(ResultItem::Plugin {
                plugin_id: plugin_id.to_string(),
                item_id: item_id.to_string(),
                title,
                subtitle,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use steward_ui_components::CalendarData;

    #[test]
    fn list_items_builds_plugin_rows() {
        let view = serde_json::json!({
            "type": "list",
            "items": [
                { "id": "a", "title": "Alpha", "subtitle": "first" },
                { "id": "b", "title": "Beta" },
                { "id": "", "title": "NoId" }
            ]
        });
        let items = list_items(&view, "com.example.p", "cmd");
        assert_eq!(items.len(), 2);
        assert!(matches!(
            &items[0],
            ResultItem::Plugin { plugin_id, item_id, title, subtitle }
                if plugin_id == "com.example.p"
                    && item_id == "a"
                    && title == "Alpha"
                    && subtitle == "first"
        ));
        assert!(matches!(
            &items[1],
            ResultItem::Plugin { item_id, title, subtitle, .. }
                if item_id == "b" && title == "Beta" && subtitle == "cmd"
        ));
    }

    #[test]
    fn list_items_ignores_non_list_views() {
        let view = serde_json::json!({
            "type": "calendar",
            "year": 2026,
            "month": 8,
            "today": "2026-08-28"
        });
        assert!(list_items(&view, "p", "cmd").is_empty());
    }

    #[test]
    fn panel_height_follows_view_content() {
        let items = vec![ResultItem::Plugin {
            plugin_id: "p".into(),
            item_id: "a".into(),
            title: "A".into(),
            subtitle: "s".into(),
        }];
        assert!(panel_height(&PanelKind::List(items)) >= RESULT_ROW_HEIGHT);

        let cal = ActiveCalendar {
            data: CalendarData {
                year: 2026,
                month: 8,
                today: "2026-08-28".into(),
                selected: "2026-08-28".into(),
                start_of_week: 1,
            },
            plugin_id: "p".into(),
            command: "calendar".into(),
            detachable: true,
        };
        assert!(panel_height(&PanelKind::Calendar(cal)) >= CALENDAR_GRID_HEIGHT);
    }

    #[test]
    fn shift_month_wraps_across_year_boundaries() {
        assert_eq!(shift_month(2026, 8, 1), (2026, 9));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 8, 12), (2027, 8));
        assert_eq!(shift_month(2026, 8, -13), (2025, 7));
    }
}

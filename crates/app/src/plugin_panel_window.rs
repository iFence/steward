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

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    div, prelude::*, px, rgb, size, svg, AnyWindowHandle, App, Bounds, Context, ElementId,
    FocusHandle, KeyDownEvent, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowKind, WindowOptions,
};
use steward_ui_components::{
    days_in_month, iso_date, palette, ActionBar, ActionRef, CalendarView, DetailBlock, DetailData,
    DetailView, FieldKind, FormData, FormField, FormOption, FormValue, FormView, GridData,
    GridItem, GridView, ResultItem, ResultList, ResultListDelegate, SearchBar,
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

/// Height of the visible title bar above a detached plugin window: the drag
/// strip, the icon action bar (when the view declares an `actionPanel`) and
/// the close button.
const PANEL_TITLEBAR_HEIGHT: f32 = 32.0;
/// Height of the month/year navigation toolbar shown above a detached calendar
/// panel (logical px).
const PANEL_NAV_HEIGHT: f32 = 26.0;
/// Storage key persisting the detached panel window's logical size
/// (`"<width> <height>"`), so reopening keeps the user's chosen size.
const PANEL_WINDOW_SIZE_KEY: &str = "panel_window_size";
/// Default content height of a detached detail / form panel (the owner can
/// resize; this is the nominal starting height).
const DETAIL_FORM_PANEL_HEIGHT: f32 = 260.0;
/// Lucide `x` icon (24x24, stroke 2, `currentColor`), used by the title bar's
/// close button.
const CLOSE_ICON_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6L6 18"/><path d="M6 6l12 12"/></svg>"#;

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
    Detail(DetailData),
    Form(FormData),
    Grid(GridData),
    Search,
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
    detail: Option<DetailView>,
    form: Option<FormView>,
    grid: Option<GridView>,
    search_bar: Option<SearchBar>,
    search_list: Option<ResultList>,
    /// The latest `search.query` result view (a `list`/`grid`) rendered below
    /// the search bar. `None` until the first result lands.
    search_results: Option<serde_json::Value>,
    /// Generation for `search.query` invocations; a stale response is dropped.
    search_gen: u64,
    action_bar: Option<ActionBar>,
    /// Actions declared by the current view's `actionPanel`, rendered as a
    /// footer bar. `None` when the view has no action panel.
    actions: Vec<ActionRef>,
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
        let content_h = (window.viewport_size().height.as_f32()
            - PANEL_TITLEBAR_HEIGHT
            - nav_h
            - padding * 2.0)
            .max(120.0);
        let nav_toolbar = if has_nav {
            self.nav_toolbar(cx).into_any_element()
        } else {
            div().into_any_element()
        };
        let content = div()
            .flex_1()
            .flex_col()
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
                PanelKind::Detail(_) => div()
                    .id(ElementId::from("panel-detail"))
                    .h(px(content_h))
                    .w_full()
                    .child(
                        self.detail
                            .as_ref()
                            .expect("detail panel has a DetailView")
                            .render(content_h, cx),
                    ),
                PanelKind::Form(_) => div()
                    .id(ElementId::from("panel-form"))
                    .h(px(content_h))
                    .w_full()
                    .child(
                        self.form
                            .as_ref()
                            .expect("form panel has a FormView")
                            .render(content_h, cx),
                    ),
                PanelKind::Grid(_) => div()
                    .id(ElementId::from("panel-grid"))
                    .h(px(content_h))
                    .w_full()
                    .child(
                        self.grid
                            .as_ref()
                            .expect("grid panel has a GridView")
                            .render(content_h, cx),
                    ),
                PanelKind::Search => {
                    let mut col = div()
                        .id(ElementId::from("panel-search"))
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_3();
                    if let Some(bar) = self.search_bar.as_ref() {
                        col = col.child(bar.render(cx));
                    }
                    let result_height = (content_h - 48.0).max(80.0);
                    col = col.child(
                        div()
                            .id(ElementId::from("panel-search-results"))
                            .h(px(result_height))
                            .w_full()
                            .child(
                                self.search_list
                                    .as_ref()
                                    .expect("search panel has a ResultList")
                                    .render(result_height, palette::SELECTION_WASH, cx),
                            ),
                    );
                    col
                }
            });
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
            // A visible title bar: most of it is a drag handle (move the
            // window), with the icon action bar (if any) and a close button on
            // the right. Interactive children sit above the drag surface.
            .child(
                div()
                    .id(ElementId::from("panel-titlebar"))
                    .h(px(PANEL_TITLEBAR_HEIGHT))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_1()
                    .gap_1()
                    .child(
                        div()
                            .id(ElementId::from("panel-drag"))
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .when_some(self.action_bar.as_ref(), |this, bar| {
                        this.child(bar.render(cx))
                    })
                    .child(self.close_button(cx)),
            )
            .child(nav_toolbar)
            .child(content);
        root
    }
}

impl PluginPanelWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        state: Rc<RefCell<LauncherState>>,
        i18n: Rc<Localization>,
        plugin_id: String,
        command: String,
        kind: PanelKind,
        panel_actions: Vec<ActionRef>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let mut this = Self {
            plugin_id,
            command,
            state,
            i18n,
            kind: PanelKind::List(Vec::new()),
            calendar: None,
            list: None,
            detail: None,
            form: None,
            grid: None,
            search_bar: None,
            search_list: None,
            search_results: None,
            search_gen: 0,
            action_bar: None,
            actions: Vec::new(),
            selection: String::new(),
            focus,
        };
        this.init_views(kind, panel_actions, window, cx);
        this.focus.focus(window, cx);
        this
    }

    /// Rebuild the sub-views (calendar / list / detail / form) and the action
    /// bar from a new `PanelKind` and its action refs. Shared by `new` and
    /// [`Self::set_view`] so an already-open panel can be re-targeted to a new
    /// view (e.g. a list item's `select` returning a `detail`).
    fn init_views(
        &mut self,
        kind: PanelKind,
        panel_actions: Vec<ActionRef>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.calendar = None;
        self.list = None;
        self.detail = None;
        self.form = None;
        self.grid = None;
        self.search_bar = None;
        self.search_list = None;
        self.search_results = None;
        self.search_gen = 0;
        self.action_bar = None;
        self.actions = panel_actions.clone();
        self.selection = String::new();

        match &kind {
            PanelKind::Calendar(active) => {
                let select_state = self.state.clone();
                let select_plugin = active.plugin_id.clone();
                let select_command = self.command.clone();
                let on_select: steward_ui_components::CalendarSelectCallback =
                    Rc::new(move |date: String, _cx: &mut App| {
                        if select_state
                            .borrow()
                            .plugin_host
                            .borrow_mut()
                            .invoke_item(&select_plugin, &select_command, &date)
                            .is_none()
                        {
                            eprintln!(
                                "[steward] plugin {} not ready for item.invoke",
                                select_plugin
                            );
                        }
                    });
                let dock_state = self.state.clone();
                let dock_plugin = active.plugin_id.clone();
                let dock_command = self.command.clone();
                let on_toggle_pin: steward_ui_components::PinToggleCallback =
                    Rc::new(move |pinned: bool, cx: &mut App| {
                        if !pinned {
                            dock_panel_back(&dock_state, &dock_plugin, &dock_command, cx);
                        }
                    });
                let view = CalendarView::new(Some(on_select), Some(on_toggle_pin), window, cx);
                let language = self.i18n.language();
                view.set_data(
                    active.data.clone(),
                    calendar_month_label(&language, active.data.year, active.data.month),
                    calendar_weekday_labels(&language, active.data.start_of_week),
                    active.data.selected.clone(),
                    cx,
                );
                view.set_detachable(true, cx);
                view.set_pinned(true, cx);
                self.selection = active.data.selected.clone();
                self.calendar = Some(view);
            }
            PanelKind::List(items) => {
                let items_rc = Rc::new(RefCell::new(items.clone()));
                let host = self.state.borrow().plugin_host.clone();
                let panel_command = self.command.clone();
                let title = self.i18n.translate("command");
                let delegate = ResultListDelegate::new().type_label(title).on_confirm(
                    move |index, _cx: &mut App| {
                        let item = items_rc.borrow().get(index).cloned();
                        if let Some(ResultItem::Plugin {
                            plugin_id,
                            item_id,
                            command: item_command,
                            ..
                        }) = item
                        {
                            let cmd = if item_command.is_empty() {
                                panel_command.as_str()
                            } else {
                                item_command.as_str()
                            };
                            if host
                                .borrow_mut()
                                .invoke_item(&plugin_id, cmd, &item_id)
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
                    let state = self.state.borrow();
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
                self.list = Some(view);
                if !self.actions.is_empty() {
                    let list = self.list.as_ref().expect("list set").clone();
                    let list_state = self.state.clone();
                    let list_plugin = self.plugin_id.clone();
                    // A list view's action bar targets the currently selected
                    // row: on run, read the live selection and forward its
                    // item id (if any) to `action.invoke`.
                    let on_run: steward_ui_components::ActionRunCallback =
                        Rc::new(move |action_id, _item_id, cx: &mut App| {
                            let item_id = list.selected_item(cx).and_then(|item| match item {
                                ResultItem::Plugin { item_id, .. } => Some(item_id),
                                _ => None,
                            });
                            if list_state
                                .borrow()
                                .plugin_host
                                .borrow_mut()
                                .invoke_action(&list_plugin, &action_id, item_id.as_deref())
                                .is_none()
                            {
                                eprintln!(
                                    "[steward] plugin {list_plugin} not ready for action.invoke"
                                );
                            }
                        });
                    self.action_bar = Some(ActionBar::new(self.actions.clone(), Some(on_run), cx));
                }
            }
            PanelKind::Detail(data) => {
                let action_state = self.state.clone();
                let action_plugin = self.plugin_id.clone();
                let on_run: steward_ui_components::ActionRunCallback =
                    Rc::new(move |id, item_id, _| {
                        if action_state
                            .borrow()
                            .plugin_host
                            .borrow_mut()
                            .invoke_action(&action_plugin, &id, item_id.as_deref())
                            .is_none()
                        {
                            eprintln!(
                                "[steward] plugin {action_plugin} not ready for action.invoke"
                            );
                        }
                    });
                let view = DetailView::new(data.clone(), cx);
                self.detail = Some(view);
                if !self.actions.is_empty() {
                    self.action_bar = Some(ActionBar::new(self.actions.clone(), Some(on_run), cx));
                }
            }
            PanelKind::Form(data) => {
                let form_state = self.state.clone();
                let form_plugin = self.plugin_id.clone();
                let on_submit: steward_ui_components::FormSubmitCallback =
                    Rc::new(move |values, _| {
                        let payload = form_values_to_json(&values);
                        if form_state
                            .borrow()
                            .plugin_host
                            .borrow_mut()
                            .invoke_submit(&form_plugin, &payload)
                            .is_none()
                        {
                            eprintln!("[steward] plugin {form_plugin} not ready for form.submit");
                        }
                    });
                let view = FormView::new(data.clone(), Some(on_submit), cx);
                self.form = Some(view);
                if !self.actions.is_empty() {
                    self.action_bar = Some(ActionBar::new(self.actions.clone(), None, cx));
                }
            }
            PanelKind::Grid(data) => {
                let grid_state = self.state.clone();
                let grid_plugin = self.plugin_id.clone();
                let grid_command = self.command.clone();
                let on_select: steward_ui_components::GridSelectCallback =
                    Rc::new(move |id: String, _cx: &mut App| {
                        if grid_state
                            .borrow()
                            .plugin_host
                            .borrow_mut()
                            .invoke_item(&grid_plugin, &grid_command, &id)
                            .is_none()
                        {
                            eprintln!("[steward] plugin {grid_plugin} not ready for item.invoke");
                        }
                    });
                let view = GridView::new(data.clone(), Some(on_select), cx);
                self.grid = Some(view);
                if !self.actions.is_empty() {
                    self.action_bar = Some(ActionBar::new(self.actions.clone(), None, cx));
                }
            }
            PanelKind::Search => {
                let search_state = self.state.clone();
                let search_plugin = self.plugin_id.clone();
                let search_command = self.command.clone();
                let on_input: steward_ui_components::SearchInputCallback =
                    Rc::new(move |query: String, _cx: &mut App| {
                        let gen = {
                            let s = search_state.borrow();
                            let gen = s.search_gen.get() + 1;
                            s.search_gen.set(gen);
                            gen
                        };
                        if search_state
                            .borrow()
                            .plugin_host
                            .borrow_mut()
                            .invoke_search(gen, &search_plugin, &search_command, &query)
                            .is_none()
                        {
                            eprintln!(
                                "[steward] plugin {search_plugin} not ready for search.query"
                            );
                        }
                    });
                let placeholder = self.i18n.translate("search-placeholder");
                let bar = SearchBar::new(placeholder, Some(on_input), None, cx);
                self.search_bar = Some(bar);

                let items_rc = Rc::new(RefCell::new(Vec::<ResultItem>::new()));
                let host = self.state.borrow().plugin_host.clone();
                let panel_command = self.command.clone();
                let delegate = ResultListDelegate::new()
                    .type_label(self.i18n.translate("command"))
                    .on_confirm(move |index, _cx: &mut App| {
                        let item = items_rc.borrow().get(index).cloned();
                        if let Some(ResultItem::Plugin {
                            plugin_id,
                            item_id,
                            command: item_command,
                            ..
                        }) = item
                        {
                            let cmd = if item_command.is_empty() {
                                panel_command.as_str()
                            } else {
                                item_command.as_str()
                            };
                            if host
                                .borrow_mut()
                                .invoke_item(&plugin_id, cmd, &item_id)
                                .is_none()
                            {
                                eprintln!(
                                    "[steward] plugin {} is not ready for item.invoke",
                                    plugin_id
                                );
                            }
                        }
                        false
                    });
                self.search_list = Some(ResultList::new(delegate, window, cx));
            }
        }
        self.kind = kind;
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

    /// The title bar's close button: parks the window back and removes it from
    /// the registry (equivalent to Esc), so a plugin window can always be
    /// dismissed.
    fn close_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let plugin_id = self.plugin_id.clone();
        let command = self.command.clone();
        div()
            .id(ElementId::from("panel-close"))
            .h(px(28.0))
            .w(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(0xffffff).opacity(0.12))
            .hover(|style| style.bg(rgb(palette::HOVER).opacity(0.05)))
            .text_color(rgb(palette::MUTED_FOREGROUND))
            .on_click(cx.listener(move |_, _, _, cx| {
                dock_panel_back(&state, &plugin_id, &command, cx);
            }))
            .child(
                svg()
                    .data(CLOSE_ICON_SVG)
                    .w(px(14.0))
                    .h(px(14.0))
                    .text_color(rgb(palette::MUTED_FOREGROUND)),
            )
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
                        view.ensure_selected(cx);
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
            PanelKind::Detail(_) | PanelKind::Form(_) | PanelKind::Grid(_) | PanelKind::Search => {
                // Escape docks the detached detail / form / grid / search panel
                // back into the
                // launcher; everything else is handled by the widgets.
                if keystroke.key.as_str() == "escape" {
                    let (plugin_id, command) = (self.plugin_id.clone(), self.command.clone());
                    let state = self.state.clone();
                    dock_panel_back(&state, &plugin_id, &command, cx);
                    cx.stop_propagation();
                }
            }
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
            .invoke_item(&plugin_id, &self.command, &selected)
            .is_none()
        {
            eprintln!("[steward] plugin {plugin_id} not ready for item.invoke");
        }
    }

    /// Push a fresh `search.query` result into the search panel: drop stale
    /// generations, store the view, and rebuild the result rows.
    pub(crate) fn apply_search_result(
        &mut self,
        gen: u64,
        result: &serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if gen != self.state.borrow().search_gen.get() {
            return;
        }
        self.search_results = Some(result.clone());
        if let Some(list) = self.search_list.as_ref() {
            let items = list_items(result, &self.plugin_id, &self.command);
            let icons = {
                let state = self.state.borrow();
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
            list.set_results(items, icons, cx);
        }
        cx.notify();
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
    let (kind, actions) = parse_panel_view(&view, &plugin_id, &command, detachable)?;

    // Already open: re-target it to the new view and bring it forward.
    if let Some(handle) = state.borrow().plugin_window(&plugin_id, &command) {
        // An already-open panel just comes to the front (the caller replaces a
        // changed view by docking the old panel back first — see the
        // `ItemView` path in `events.rs`).
        let _ = handle.update(cx, |_, window, cx| {
            cx.activate(true);
            window.refresh();
        });
        return Some(handle);
    }

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
                    actions,
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

/// Push a `search.query` result into an open search panel for `(plugin_id,
/// command)`. No-op when no such panel is open (the launcher keeps the result
/// in its shared search-results map instead).
pub(crate) fn apply_search_result_to_panel(
    state: &Rc<RefCell<LauncherState>>,
    plugin_id: &str,
    command: &str,
    gen: u64,
    result: &serde_json::Value,
    cx: &mut App,
) {
    let Some(handle) = state.borrow().plugin_window(plugin_id, command) else {
        return;
    };
    if let Some(panel) = handle.downcast::<PluginPanelWindow>() {
        let _ = panel.update(cx, |panel, _window, cx| {
            panel.apply_search_result(gen, result, cx);
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
            let _ = state.borrow().storage.borrow().set_setting(
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
        PanelKind::Detail(_) | PanelKind::Form(_) => DETAIL_FORM_PANEL_HEIGHT,
        PanelKind::Grid(_) => 320.0,
        PanelKind::Search => 360.0,
    };
    let nav = if matches!(kind, PanelKind::Calendar(_)) {
        PANEL_NAV_HEIGHT
    } else {
        0.0
    };
    // The title bar (drag strip + icon action bar + close button) sits above
    // the content; the action bar no longer takes a separate footer row.
    content + PLUGIN_WIDGET_PADDING * 2.0 + PANEL_TITLEBAR_HEIGHT + nav
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
                .unwrap_or("")
                .to_string();
            Some(ResultItem::Plugin {
                plugin_id: plugin_id.to_string(),
                command: command.to_string(),
                item_id: item_id.to_string(),
                title,
                subtitle,
            })
        })
        .collect()
}

/// Parse a plugin view into a `PanelKind` plus the actions declared by its
/// `actionPanel`. Returns `None` for a view type the panel cannot host.
fn parse_panel_view(
    view: &serde_json::Value,
    plugin_id: &str,
    command: &str,
    detachable: bool,
) -> Option<(PanelKind, Vec<ActionRef>)> {
    if let Some(active) = parse_calendar_view(view, plugin_id, command, detachable) {
        return Some((PanelKind::Calendar(active), extract_actions(view)));
    }
    if plugin_view_items(view).is_some() {
        return Some((
            PanelKind::List(list_items(view, plugin_id, command)),
            extract_actions(view),
        ));
    }
    if let Some(data) = parse_detail_view(view) {
        return Some((PanelKind::Detail(data), extract_actions(view)));
    }
    if let Some(data) = parse_form_view(view) {
        return Some((PanelKind::Form(data), extract_actions(view)));
    }
    if let Some(data) = parse_grid_view(view) {
        return Some((PanelKind::Grid(data), extract_actions(view)));
    }
    if is_search_view(view) {
        return Some((PanelKind::Search, extract_actions(view)));
    }
    None
}

/// The raw view payload, unwrapping the runtime's `{ "view": ... }` envelope.
fn unwrap_view(view: &serde_json::Value) -> &serde_json::Value {
    view.get("view").unwrap_or(view)
}

/// Parse a plugin `detail` view into its display data.
fn parse_detail_view(view: &serde_json::Value) -> Option<DetailData> {
    let view = unwrap_view(view);
    if view.get("type").and_then(|kind| kind.as_str()) != Some("detail") {
        return None;
    }
    let title = view.get("title")?.as_str()?.to_string();
    let subtitle = view
        .get("subtitle")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let blocks = view
        .get("content")
        .and_then(|content| content.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .map(
                    |block| match block.get("type").and_then(|kind| kind.as_str()) {
                        Some("code") => DetailBlock::Code {
                            value: block
                                .get("value")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string(),
                            language: block
                                .get("language")
                                .and_then(|value| value.as_str())
                                .map(|value| value.to_string()),
                        },
                        Some("separator") => DetailBlock::Separator,
                        _ => DetailBlock::Text(
                            block
                                .get("value")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string(),
                        ),
                    },
                )
                .collect()
        })
        .unwrap_or_default();
    Some(DetailData {
        title,
        subtitle,
        blocks,
    })
}

/// Parse a plugin `form` view into its display data.
fn parse_form_view(view: &serde_json::Value) -> Option<FormData> {
    let view = unwrap_view(view);
    if view.get("type").and_then(|kind| kind.as_str()) != Some("form") {
        return None;
    }
    let title = view
        .get("title")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let submit_label = view
        .get("submitLabel")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let fields = view
        .get("fields")
        .and_then(|fields| fields.as_array())
        .map(|fields| fields.iter().filter_map(parse_form_field).collect())
        .unwrap_or_default();
    Some(FormData {
        title,
        fields,
        submit_label,
    })
}

fn parse_form_field(field: &serde_json::Value) -> Option<FormField> {
    let id = field.get("id")?.as_str()?.to_string();
    let label = field
        .get("label")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let kind = FieldKind::from_wire(
        field
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("text"),
    )?;
    let placeholder = field
        .get("placeholder")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let options = field
        .get("options")
        .and_then(|value| value.as_array())
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    Some(FormOption {
                        id: option.get("id")?.as_str()?.to_string(),
                        label: option
                            .get("label")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let value = match kind {
        FieldKind::Toggle => field
            .get("value")
            .and_then(|value| value.as_bool())
            .map(FormValue::Bool),
        _ => field
            .get("value")
            .and_then(|value| value.as_str())
            .map(|value| FormValue::String(value.to_string())),
    };
    let required = field
        .get("required")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Some(FormField {
        id,
        label,
        kind,
        placeholder,
        options,
        value,
        required,
    })
}

/// Parse a plugin `grid` view into its display data.
fn parse_grid_view(view: &serde_json::Value) -> Option<GridData> {
    let view = unwrap_view(view);
    if view.get("type").and_then(|kind| kind.as_str()) != Some("grid") {
        return None;
    }
    let columns = view
        .get("columns")
        .and_then(|value| value.as_u64())
        .unwrap_or(4)
        .clamp(1, 8) as usize;
    let selected = view
        .get("selectedId")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let items = view
        .get("items")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(GridItem {
                        id: item.get("id")?.as_str()?.to_string(),
                        title: item
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        subtitle: item
                            .get("subtitle")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string()),
                        icon: item
                            .get("icon")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string()),
                        badge: item
                            .get("badge")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(GridData {
        columns,
        items,
        selected,
    })
}

/// Whether a plugin view is a `search` view.
fn is_search_view(view: &serde_json::Value) -> bool {
    unwrap_view(view).get("type").and_then(|kind| kind.as_str()) == Some("search")
}

/// Extract the `actionPanel.actions` array from a view's JSON payload.
fn extract_actions(view: &serde_json::Value) -> Vec<ActionRef> {
    let view = unwrap_view(view);
    view.get("actionPanel")
        .and_then(|panel| panel.get("actions"))
        .and_then(|actions| actions.as_array())
        .map(|actions| {
            actions
                .iter()
                .filter_map(|action| {
                    let id = action.get("id")?.as_str()?.to_string();
                    let title = action
                        .get("title")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                    let icon = action
                        .get("icon")
                        .and_then(|value| value.as_str())
                        .map(|value| value.to_string());
                    Some(ActionRef { id, title, icon })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Serialize a form's collected values into a JSON object for `form.submit`.
fn form_values_to_json(values: &HashMap<String, FormValue>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in values {
        let json = match value {
            FormValue::String(text) => serde_json::Value::String(text.clone()),
            FormValue::Bool(flag) => serde_json::Value::Bool(*flag),
        };
        map.insert(key.clone(), json);
    }
    serde_json::Value::Object(map)
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
            ResultItem::Plugin { plugin_id, item_id, title, subtitle, .. }
                if plugin_id == "com.example.p"
                    && item_id == "a"
                    && title == "Alpha"
                    && subtitle == "first"
        ));
        assert!(matches!(
            &items[1],
            ResultItem::Plugin { item_id, title, subtitle, command, .. }
                if item_id == "b" && title == "Beta" && subtitle.is_empty()
                    && command == "cmd"
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
            command: "cmd".into(),
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

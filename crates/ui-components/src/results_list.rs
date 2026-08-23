//! A simple, non-virtualized results list for the launcher.
//!
//! The launcher caps its drop-down at `MAX_RESULT_ROWS` (8) rows, so M1 does
//! not need a virtualized list. The initial M1 implementation reused
//! `gpui-component`'s `List`/virtual list; on Windows it rendered rows at the
//! wrong positions (faint or missing row text) and painted a stray white quad
//! in the bottom-right corner of the drop-down. A plain stacked `div` list
//! gives full control over layout, selection, hover and colors, and removes
//! that rendering path entirely.
//!
//! The search box is owned by the `app` crate: each keystroke runs a query
//! there and pushes the rows plus their (optional) icons via
//! [`ResultList::set_results`]. Confirmation (Enter / click) is surfaced back
//! through the `on_confirm` callback so the app can launch the application and
//! bump its usage frequency.

use std::{rc::Rc, sync::Arc};

use gpui::{
    div, img, prelude::FluentBuilder, px, rgb, App, AppContext, Context, ElementId, Entity, Image,
    ImageSource, InteractiveElement, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement, Styled as _,
};
use gpui_component::ActiveTheme;

use steward_core_engine::AppEntry;

/// Design (96-DPI) geometry of a result row, in logical pixels. GPUI scales
/// these with the display DPI like any other app UI.
const DESIGN_ROW_HEIGHT: f32 = 48.0;
const DESIGN_ICON_SIZE: f32 = 24.0;
/// Rows visible at once in the drop-down. Must match the app's
/// `MAX_RESULT_ROWS`; the list renders exactly this many rows so the pinned
/// GPUI Windows revision never has to clip overflowing children (its scroll
/// container paints them unclipped, spilling below the drop-down).
pub const VISIBLE_ROWS: usize = 8;

/// The state backing the results list. Kept as its own entity so updates
/// (`set_results`, selection moves) can happen without a window.
pub struct ResultListState {
    items: Vec<AppEntry>,
    icons: Vec<Option<Arc<Image>>>,
    type_label: String,
    max_height: f32,
    selected: Option<usize>,
    on_confirm: Option<Rc<dyn Fn(usize)>>,
}

impl Render for ResultListState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let type_label = self.type_label.clone();
        let range = self.visible_range();
        let selected = self.selected;
        let rows = self.items[range.clone()]
            .iter()
            .enumerate()
            .map(|(offset, app)| {
                let index = range.start + offset;
                render_row(
                    app,
                    self.icons.get(index).cloned().flatten(),
                    &type_label,
                    selected == Some(index),
                    index,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        // Exactly `VISIBLE_ROWS` rows are rendered; nothing overflows, so the
        // container needs no scroll/clip machinery (which is broken in the
        // pinned GPUI revision on Windows).
        div()
            .id(ElementId::from("results-rows"))
            .flex()
            .flex_col()
            .w_full()
            .children(rows)
    }
}

impl ResultListState {
    /// The slice of items to render: a `VISIBLE_ROWS`-tall window anchored so
    /// the selected row is always visible (at the bottom once the list is long
    /// enough to scroll).
    fn visible_range(&self) -> std::ops::Range<usize> {
        let viewport_rows = (self.max_height / DESIGN_ROW_HEIGHT).max(1.0) as usize;
        let len = self.items.len();
        let top = self
            .selected
            .unwrap_or(0)
            .min(len.saturating_sub(1))
            .saturating_sub(viewport_rows.saturating_sub(1))
            .min(len);
        top..(top + viewport_rows).min(len)
    }
}

/// A result row: fixed height (matches the app's row metric), app icon on the
/// left (when available), name and path on the right (truncated). Selected
/// rows get the launcher's blue tint plus border; hovered rows get a dark
/// surface tint.
fn render_row(
    app: &AppEntry,
    icon: Option<Arc<Image>>,
    type_label: &str,
    selected: bool,
    index: usize,
    cx: &mut Context<ResultListState>,
) -> impl IntoElement {
    div()
        .id(ElementId::from(app.path.to_string_lossy().into_owned()))
        .h(px(DESIGN_ROW_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .cursor_pointer()
        .bg(rgb(0x232332))
        .when(selected, |this| {
            let primary = cx.theme().primary;
            this.bg(primary.opacity(0.2))
        })
        .when(!selected, |this| {
            this.hover(|style| style.bg(rgb(0x313244).opacity(0.8)))
        })
        .when_some(icon, |this, icon| {
            this.child(
                img(ImageSource::Image(icon))
                    .w(px(DESIGN_ICON_SIZE))
                    .h(px(DESIGN_ICON_SIZE)),
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(cb) = this.on_confirm.clone() {
                cb(index);
            }
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .truncate()
                .text_color(rgb(0xcdd6f4))
                .child(app.name.to_owned()),
        )
        .child(
            div()
                .truncate()
                .max_w(px(DESIGN_ROW_HEIGHT * 7.5))
                .text_color(rgb(0x6c7086))
                .text_xs()
                .child(type_label.to_string()),
        )
}

/// Builds the `ResultList` with an optional confirm callback.
pub struct ResultListDelegate {
    on_confirm: Option<Rc<dyn Fn(usize)>>,
    type_label: String,
}

impl ResultListDelegate {
    pub fn new() -> Self {
        Self {
            on_confirm: None,
            type_label: "Application".into(),
        }
    }

    /// Label shown on the right of every row (the localized word for
    /// "Application"), replacing the raw executable path.
    pub fn type_label(mut self, label: impl Into<String>) -> Self {
        self.type_label = label.into();
        self
    }

    /// Register a callback fired with the confirmed row index (Enter / click).
    pub fn on_confirm(mut self, cb: impl Fn(usize) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(cb));
        self
    }
}

impl Default for ResultListDelegate {
    fn default() -> Self {
        Self::new()
    }
}

/// The launcher results list: a plain stacked list of rows. Rows are pushed in
/// from the app; the drop-down height is derived from
/// [`ResultList::visible_count`] by the enclosing window.
#[derive(Clone)]
pub struct ResultList {
    state: Entity<ResultListState>,
}

impl ResultList {
    /// Create the list inside a window context (from the app's root entity).
    pub fn new<C>(
        delegate: ResultListDelegate,
        _window: &mut gpui::Window,
        cx: &mut Context<C>,
    ) -> Self {
        let state = cx.new(|_| ResultListState {
            items: Vec::new(),
            icons: Vec::new(),
            type_label: delegate.type_label,
            max_height: 0.0,
            selected: None,
            on_confirm: delegate.on_confirm,
        });
        Self { state }
    }

    /// Replace the displayed rows (and their icons, aligned with `items`) and
    /// re-render. Called by the app after every search; no window access is
    /// required (works from a plain entity context).
    pub fn set_results<C>(
        &self,
        items: Vec<AppEntry>,
        icons: Vec<Option<Arc<Image>>>,
        cx: &mut Context<C>,
    ) {
        self.state.update(cx, |this, cx| {
            this.items = items;
            this.icons = icons;
            // Default-select the first row so Enter (or the highlight) works
            // immediately after typing, without a manual Down press.
            this.selected = (!this.items.is_empty()).then_some(0);
            cx.notify();
        });
    }

    /// Replace the icons (aligned with the current `items`) without touching
    /// the rows or selection — used when icons finish loading asynchronously.
    pub fn set_icons<C: AppContext>(&self, icons: Vec<Option<Arc<Image>>>, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            this.icons = icons;
            cx.notify();
        });
    }

    /// Number of results from the latest update (used for window sizing).
    pub fn visible_count(&self, cx: &App) -> usize {
        self.state.read(cx).items.len()
    }

    /// Replace the type label shown on the right of every row. Used when the
    /// UI language changes at runtime — the label is stored state, so it does
    /// not re-translate on its own.
    pub fn set_type_label<C: AppContext>(&self, label: impl Into<String>, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            this.type_label = label.into();
            cx.notify();
        });
    }

    /// Move selection by `delta` rows (negative moves up), clamping to bounds.
    pub fn select_relative<C>(
        &self,
        delta: i32,
        _window: &mut gpui::Window,
        cx: &mut Context<C>,
    ) -> Option<usize> {
        let mut next = None;
        self.state.update(cx, |this, cx| {
            if !this.items.is_empty() {
                // The first press selects the first row (not the second):
                // treat "nothing selected" as -1 so `+1` lands on index 0.
                let current = this.selected.map(|i| i as i32).unwrap_or(-1);
                let index = (current + delta).clamp(0, this.items.len() as i32 - 1) as usize;
                this.selected = Some(index);
                next = Some(index);
                cx.notify();
            }
        });
        next
    }

    /// Confirm the currently selected row, invoking the delegate's `on_confirm`
    /// callback. Returns whether a confirmation actually fired (no-op when
    /// nothing is selected).
    pub fn confirm_selected<C>(&self, _window: &mut gpui::Window, cx: &mut Context<C>) -> bool {
        let mut confirmed = false;
        self.state.update(cx, |this, cx| {
            if let Some(index) = this.selected {
                if index < this.items.len() {
                    if let Some(cb) = this.on_confirm.clone() {
                        cb(index);
                        confirmed = true;
                    }
                }
            }
            cx.notify();
        });
        confirmed
    }

    /// Render the scrollable list element, capped at `max_height` so rows
    /// beyond the visible drop-down can be scrolled into view.
    pub fn render<C>(&self, max_height: f32, cx: &mut Context<C>) -> impl IntoElement {
        self.state.update(cx, |this, _| {
            this.max_height = max_height;
        });
        div()
            .id(ElementId::from("results-list"))
            .h(px(max_height))
            .child(self.state.clone())
    }
}

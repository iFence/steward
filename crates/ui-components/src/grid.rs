//! A generic plugin `grid` view: an N-column grid of tappable cells, rendered
//! with the same card chrome as the calendar so it matches the launcher.
//! Keyboard navigation and confirmation are owned by the app (like the
//! calendar's day grid); the component only highlights the selected cell and
//! reports clicks.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, rgb, App, AppContext, Context, ElementId, Entity, IntoElement, Render,
};
use gpui_component::ActiveTheme;

use crate::palette;

/// One cell in a plugin grid view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridItem {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<String>,
    pub badge: Option<String>,
}

/// The data a plugin `grid` view carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridData {
    pub columns: usize,
    pub items: Vec<GridItem>,
    /// The keyboard-selected cell id.
    pub selected: Option<String>,
}

/// Callback fired when a cell is clicked: the cell's id.
pub type GridSelectCallback = Rc<dyn Fn(String, &mut App)>;

/// The state backing a rendered `GridView`. Kept as its own entity so the app
/// can update selection without touching the hosting window.
pub struct GridViewState {
    data: GridData,
    max_height: f32,
    on_select: Option<GridSelectCallback>,
}

impl Render for GridViewState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let data = self.data.clone();
        let columns = data.columns.max(1);
        let rows = data.items.len().div_ceil(columns).max(1);
        let row_height = ((self.max_height - 8.0) / rows as f32).max(28.0);
        let accent = cx.theme().primary;
        let mut grid_rows: Vec<gpui::AnyElement> = Vec::new();
        for (r, chunk) in data.items.chunks(columns).enumerate() {
            let mut row = div()
                .id(ElementId::from(format!("grid-row-{r}")))
                .w_full()
                .h(px(row_height))
                .flex()
                .flex_row()
                .gap_2();
            for item in chunk {
                let selected = data.selected.as_deref() == Some(item.id.as_str());
                let cell_id = item.id.clone();
                row = row.child(
                    div()
                        .id(ElementId::from(format!("grid-cell-{}", item.id)))
                        .flex_1()
                        .h_full()
                        .min_w(px(40.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .rounded_lg()
                        .border_1()
                        .px_2()
                        .cursor_pointer()
                        .border_color(rgb(0xffffff).opacity(0.08))
                        .bg(rgb(palette::BACKGROUND_ALT).opacity(0.35))
                        .when(selected, |this| {
                            this.bg(rgb(palette::SELECTION).opacity(0.16))
                                .border_color(accent.opacity(0.6))
                        })
                        .when(!selected, |this| {
                            this.hover(|style| style.bg(rgb(palette::HOVER).opacity(0.05)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(cb) = this.on_select.clone() {
                                cb(cell_id.clone(), cx);
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .truncate()
                                .w_full()
                                .text_align(gpui::TextAlign::Center)
                                .text_color(rgb(palette::FOREGROUND))
                                .text_sm()
                                .child(item.title.clone()),
                        )
                        .when_some(item.subtitle.clone(), |this, subtitle| {
                            this.child(
                                div()
                                    .truncate()
                                    .w_full()
                                    .text_align(gpui::TextAlign::Center)
                                    .text_color(rgb(palette::MUTED_FOREGROUND))
                                    .text_size(px(11.0))
                                    .child(subtitle),
                            )
                        }),
                );
            }
            grid_rows.push(row.into_any_element());
        }

        div()
            .id(ElementId::from("grid-view"))
            .h(px(self.max_height))
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().flex_1().flex_col().gap_2().children(grid_rows))
    }
}

/// A rendered plugin `grid` view handle, mirroring `CalendarView`.
#[derive(Clone)]
pub struct GridView {
    state: Entity<GridViewState>,
}

impl GridView {
    pub fn new<C>(
        data: GridData,
        on_select: Option<GridSelectCallback>,
        cx: &mut Context<C>,
    ) -> Self {
        let state = cx.new(|_| GridViewState {
            data,
            max_height: 120.0,
            on_select,
        });
        Self { state }
    }

    pub fn set_data<C: AppContext>(&self, data: GridData, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            this.data = data;
            cx.notify();
        });
    }

    pub fn set_selected<C: AppContext>(&self, selected: Option<&str>, cx: &mut C) {
        self.state.update(cx, |this, cx| {
            this.data.selected = selected.map(|s| s.to_string());
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

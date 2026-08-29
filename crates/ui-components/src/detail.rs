//! A plugin `detail` view: a title, optional subtitle and a stack of content
//! blocks (text / code / separator). Rendered with the launcher's Tinycast
//! palette so it matches the rest of the app.

use gpui::{
    div, prelude::*, px, rgb, App, AppContext, Context, ElementId, Entity, IntoElement, Render,
};

use crate::palette;

/// One block in a detail view's content column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailBlock {
    /// A paragraph of body text (wrapped across lines).
    Text(String),
    /// A code block rendered in a monospace font on a subtle inset surface.
    Code {
        value: String,
        language: Option<String>,
    },
    /// A thin horizontal separator.
    Separator,
}

/// The data a plugin `detail` view carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DetailData {
    pub title: String,
    pub subtitle: Option<String>,
    pub blocks: Vec<DetailBlock>,
}

/// The state backing a rendered `DetailView`. Kept as its own entity so the
/// app can update its content without touching the hosting window.
pub struct DetailViewState {
    data: DetailData,
}

impl Render for DetailViewState {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.data.title.clone();
        let subtitle = self.data.subtitle.clone();
        let blocks = self.data.blocks.clone();
        div()
            .id(ElementId::from("detail-view"))
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_color(rgb(palette::FOREGROUND))
                    .text_lg()
                    .child(title),
            )
            .when_some(subtitle, |this, subtitle| {
                this.child(
                    div()
                        .text_color(rgb(palette::MUTED_FOREGROUND))
                        .text_sm()
                        .child(subtitle),
                )
            })
            .child(
                div()
                    .flex_1()
                    .flex_col()
                    .gap_2()
                    .child(render_blocks(&blocks)),
            )
    }
}

fn render_blocks(blocks: &[DetailBlock]) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(blocks.iter().map(render_block))
}

fn render_block(block: &DetailBlock) -> gpui::Div {
    match block {
        DetailBlock::Text(text) => div()
            .text_color(rgb(palette::FOREGROUND))
            .text_sm()
            .child(text.to_string()),
        DetailBlock::Code { value, .. } => div()
            .px_3()
            .py_2()
            .bg(rgb(palette::BACKGROUND_ALT))
            .border_1()
            .border_color(rgb(0xffffff).opacity(0.08))
            .text_color(rgb(palette::FOREGROUND))
            .text_sm()
            .child(value.clone()),
        DetailBlock::Separator => div().h(px(1.0)).w_full().bg(rgb(0xffffff).opacity(0.10)),
    }
}

/// A rendered plugin `detail` view handle, mirroring `CalendarView`.
#[derive(Clone)]
pub struct DetailView {
    state: Entity<DetailViewState>,
}

impl DetailView {
    /// Create the view with an initial (possibly empty) data payload.
    pub fn new<C>(data: DetailData, cx: &mut Context<C>) -> Self {
        let state = cx.new(|_| DetailViewState { data });
        Self { state }
    }

    /// Replace the displayed content and re-render.
    pub fn set_data<C: AppContext>(&self, data: DetailData, cx: &mut C) {
        self.state.update(cx, |state, cx| {
            state.data = data;
            cx.notify();
        });
    }

    /// The current content payload (for a detached window to re-read).
    pub fn data(&self, cx: &App) -> DetailData {
        self.state.read(cx).data.clone()
    }

    /// Render the view, filling the requested height.
    pub fn render<C>(&self, height: f32, _cx: &mut Context<C>) -> impl IntoElement {
        div()
            .id(ElementId::from("detail-view-container"))
            .h(px(height))
            .w_full()
            .child(self.state.clone())
    }
}

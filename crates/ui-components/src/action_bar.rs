//! A plugin action bar: a horizontal strip of action buttons rendered under a
//! `list` / `detail` / `form` view when the plugin declares an `actionPanel`.
//! Clicking a button dispatches `action.invoke` to the plugin.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, rgb, svg, App, AppContext, Context, ElementId, Entity, IntoElement,
    StatefulInteractiveElement,
};

use crate::palette;

/// One action the plugin exposes in its action panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRef {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
}

/// Callback fired when an action button is clicked: `(action_id, item_id?)`.
pub type ActionRunCallback = Rc<dyn Fn(String, Option<String>, &mut App)>;

/// The state backing a rendered action bar.
pub struct ActionBarState {
    actions: Vec<ActionRef>,
    /// The currently selected list item id, passed along to the plugin action.
    selected_item: Option<String>,
    on_run: Option<ActionRunCallback>,
}

impl Render for ActionBarState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let actions = self.actions.clone();
        let selected = self.selected_item.clone();
        div()
            .id(ElementId::from("action-bar"))
            .flex()
            .flex_row()
            .gap_1()
            .children(actions.iter().map(|action| {
                let id = action.id.clone();
                let title = action.title.clone();
                let icon = action.icon.clone();
                let selected = selected.clone();
                div()
                    .id(ElementId::from(format!("action-{id}")))
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
                    .text_color(rgb(palette::FOREGROUND))
                    .text_sm()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(cb) = this.on_run.clone() {
                            cb(id.clone(), selected.clone(), cx);
                        }
                    }))
                    .child(if let Some(icon) = icon {
                        svg()
                            .data(icon.as_bytes())
                            .w(px(16.0))
                            .h(px(16.0))
                            .into_any_element()
                    } else {
                        div().text_sm().child(title).into_any_element()
                    })
            }))
    }
}

/// A rendered plugin action bar handle.
#[derive(Clone)]
pub struct ActionBar {
    state: Entity<ActionBarState>,
}

impl ActionBar {
    /// Create the bar with the given actions (and no selected item yet).
    pub fn new<C>(
        actions: Vec<ActionRef>,
        on_run: Option<ActionRunCallback>,
        cx: &mut Context<C>,
    ) -> Self {
        let state = cx.new(|_| ActionBarState {
            actions,
            selected_item: None,
            on_run,
        });
        Self { state }
    }

    /// Replace the actions and the optional currently-selected item.
    pub fn set_actions<C: AppContext>(&self, actions: Vec<ActionRef>, cx: &mut C) {
        self.state.update(cx, |state, cx| {
            state.actions = actions;
            cx.notify();
        });
    }

    /// Update the selected item id (passed to the plugin when an action runs).
    pub fn set_selected_item<C: AppContext>(&self, selected: Option<String>, cx: &mut C) {
        self.state.update(cx, |state, cx| {
            state.selected_item = selected;
            cx.notify();
        });
    }

    /// The actions currently shown (for the app to read before rendering).
    pub fn actions(&self, cx: &App) -> Vec<ActionRef> {
        self.state.read(cx).actions.clone()
    }

    /// Render the bar with a fixed height.
    pub fn render<C>(&self, _cx: &mut Context<C>) -> impl IntoElement {
        div()
            .id(ElementId::from("action-bar-container"))
            .h(px(28.0))
            .child(self.state.clone())
    }
}

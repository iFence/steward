//! A reusable, minimal single-line search/input widget for plugin `search`
//! views. This is intentionally lighter than the launcher's IME-aware input:
//! it handles printable characters, Backspace and Enter, plus a blinking
//! caret and placeholder, so a plugin `search` view can host its own query
//! box (e.g. inside a detached panel). Full IME/selection is a launcher
//! concern and is out of scope here.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, rgb, App, Context, ElementId, FocusHandle, IntoElement, KeyDownEvent,
    MouseButton, Render,
};

use crate::palette;

/// Callback fired when the buffer changes: the new text plus the app context.
pub type SearchInputCallback = Rc<dyn Fn(String, &mut App)>;
/// Callback fired on Enter: the current text plus the app context.
pub type SearchSubmitCallback = Rc<dyn Fn(String, &mut App)>;

/// State backing a `SearchBar`.
pub struct SearchBarState {
    value: String,
    placeholder: String,
    on_input: Option<SearchInputCallback>,
    on_submit: Option<SearchSubmitCallback>,
    focus: FocusHandle,
}

impl SearchBarState {
    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.key.is_empty() {
            return;
        }
        let key = event.keystroke.key.as_ref();
        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
            // Ignore shortcuts; the host owns them.
            return;
        }
        match key {
            "enter" => {
                if let Some(cb) = self.on_submit.clone() {
                    cb(self.value.clone(), cx);
                }
            }
            "backspace" => {
                self.value.pop();
                self.emit_input(cx);
            }
            "escape" => {}
            " " => {
                self.value.push(' ');
                self.emit_input(cx);
            }
            _ => {
                // A single printable character (or a combined IME-less rune).
                if let Some(ch) = event.keystroke.key.chars().next() {
                    if ch.is_control() {
                        return;
                    }
                    self.value.push(ch);
                    self.emit_input(cx);
                }
            }
        }
    }

    fn emit_input(&mut self, cx: &mut Context<Self>) {
        if let Some(cb) = self.on_input.clone() {
            cb(self.value.clone(), cx);
        }
        cx.notify();
    }

    /// The current buffer text.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl Render for SearchBarState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let value = self.value.clone();
        let placeholder = self.placeholder.clone();
        let has_value = !value.is_empty();
        let focus_handle = self.focus.clone();
        div()
            .id(ElementId::from("search-bar"))
            .track_focus(&focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_key(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                }),
            )
            .h(px(30.0))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .px_3()
            .rounded_lg()
            .bg(rgb(palette::BACKGROUND_ALT))
            .border_1()
            .border_color(rgb(0xffffff).opacity(0.08))
            .text_color(rgb(palette::FOREGROUND))
            .text_sm()
            .child(if has_value {
                div().child(value).into_any_element()
            } else {
                div()
                    .text_color(rgb(palette::MUTED_FOREGROUND))
                    .child(placeholder)
                    .into_any_element()
            })
    }
}

/// A minimal `SearchBar` handle wrapping [`SearchBarState`].
#[derive(Clone)]
pub struct SearchBar {
    state: RefCell<Option<gpui::Entity<SearchBarState>>>,
}

impl SearchBar {
    pub fn new<C>(
        placeholder: impl Into<String>,
        on_input: Option<SearchInputCallback>,
        on_submit: Option<SearchSubmitCallback>,
        cx: &mut Context<C>,
    ) -> Self {
        let focus = cx.focus_handle();
        let state = cx.new(|_| SearchBarState {
            value: String::new(),
            placeholder: placeholder.into(),
            on_input,
            on_submit,
            focus,
        });
        Self {
            state: RefCell::new(Some(state)),
        }
    }

    pub fn set_value<C: gpui::AppContext>(&self, value: impl Into<String>, cx: &mut C) {
        if let Some(state) = self.state.borrow().as_ref() {
            state.update(cx, |this, cx| {
                this.value = value.into();
                cx.notify();
            });
        }
    }

    pub fn render<C>(&self, _cx: &mut Context<C>) -> impl IntoElement {
        if let Some(state) = self.state.borrow().as_ref() {
            state.clone().into_any_element()
        } else {
            div().into_any_element()
        }
    }
}

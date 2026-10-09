//! Host-owned input state for `input` leaves of a plugin UI tree.
//!
//! The text buffer lives in the host, not the plugin: keystrokes update a local
//! entity and only a debounced `change` event crosses the process boundary.
//! That keeps typing latency independent of the plugin's `view.invoke` round
//! trip, and means a slow plugin never drops the user's input.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    div, prelude::*, px, rgb, App, Context, ElementId, Entity, FocusHandle, IntoElement,
    KeyDownEvent, MouseButton, Render,
};

use crate::palette;

/// A change/submit callback: the current buffer text plus the app context.
pub type InputCallback = Rc<dyn Fn(String, &mut App)>;

/// A single host-owned input entity.
pub struct VirtualInputState {
    id: String,
    value: String,
    placeholder: String,
    multiline: bool,
    password: bool,
    focus: FocusHandle,
    on_change: Option<InputCallback>,
    on_submit: Option<InputCallback>,
}

impl VirtualInputState {
    fn emit_change(&self, cx: &mut Context<Self>) {
        if let Some(callback) = self.on_change.clone() {
            callback(self.value.clone(), cx);
        }
        cx.notify();
    }

    fn emit_submit(&self, cx: &mut Context<Self>) {
        if let Some(callback) = self.on_submit.clone() {
            callback(self.value.clone(), cx);
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key: &str = event.keystroke.key.as_ref();
        if key.is_empty() {
            return;
        }
        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
            return;
        }
        match key {
            "enter" => {
                if self.multiline {
                    self.value.push('\n');
                    self.emit_change(cx);
                } else {
                    self.emit_submit(cx);
                }
            }
            "backspace" => {
                self.value.pop();
                self.emit_change(cx);
            }
            "escape" => {}
            " " => {
                self.value.push(' ');
                self.emit_change(cx);
            }
            _ => {
                if let Some(ch) = event.keystroke.key.chars().next() {
                    if !ch.is_control() {
                        self.value.push(ch);
                        self.emit_change(cx);
                    }
                }
            }
        }
    }
}

impl Render for VirtualInputState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let value = self.value.clone();
        let has_value = !value.is_empty();
        let shown = if self.password {
            "•".repeat(value.chars().count())
        } else {
            value
        };
        let placeholder = self.placeholder.clone();
        let focus = self.focus.clone();
        div()
            .id(ElementId::from(format!("ui-input-{}", self.id)))
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_key(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                }),
            )
            .min_h(px(28.0))
            .w_full()
            .flex()
            .items_center()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgb(palette::BACKGROUND_ALT))
            .border_1()
            .border_color(rgb(0xffffff).opacity(0.08))
            .text_color(rgb(palette::FOREGROUND))
            .text_sm()
            .child(if has_value {
                div().child(shown).into_any_element()
            } else {
                div()
                    .text_color(rgb(palette::MUTED_FOREGROUND))
                    .child(placeholder)
                    .into_any_element()
            })
    }
}

/// A handle to one host-owned input, shared with the rendered tree.
#[derive(Clone)]
pub struct VirtualInput {
    state: Entity<VirtualInputState>,
}

impl VirtualInput {
    /// The current buffer text.
    pub fn value(&self, cx: &App) -> String {
        self.state.read(cx).value.clone()
    }

    /// Render the input entity as an element.
    pub fn element(&self) -> impl IntoElement {
        self.state.clone()
    }
}

/// The per-view map of host-owned inputs, keyed by element id.
///
/// A tree is synced once when it arrives; rendering then looks the entities up
/// without creating anything, so a repaint never allocates state.
#[derive(Clone, Default)]
pub struct InputStore {
    map: Rc<RefCell<HashMap<String, VirtualInput>>>,
}

impl InputStore {
    /// Reconcile the store with a new tree: create inputs that appeared, update
    /// the presentation of existing ones (keeping their typed text), and drop
    /// inputs whose node left the tree.
    pub fn sync(&self, node: &super::spec::Node, sink: &super::render::EventSink, cx: &mut App) {
        let mut seen = Vec::new();
        self.collect(node, sink, cx, &mut seen);
        self.map.borrow_mut().retain(|id, _| seen.contains(id));
    }

    fn collect(
        &self,
        node: &super::spec::Node,
        sink: &super::render::EventSink,
        cx: &mut App,
        seen: &mut Vec<String>,
    ) {
        if let Some(props) = &node.input {
            seen.push(node.id.clone());
            let change: InputCallback = {
                let id = node.id.clone();
                let callback_id = node
                    .callback(super::spec::EventKind::Change)
                    .map(str::to_string);
                let sink = sink.clone();
                Rc::new(move |value: String, _cx: &mut App| {
                    sink(
                        callback_id.as_deref(),
                        &id,
                        super::spec::EventKind::Change,
                        Some(value),
                    )
                })
            };
            let submit: InputCallback = {
                let id = node.id.clone();
                let callback_id = node
                    .callback(super::spec::EventKind::Submit)
                    .map(str::to_string);
                let sink = sink.clone();
                Rc::new(move |value: String, _cx: &mut App| {
                    sink(
                        callback_id.as_deref(),
                        &id,
                        super::spec::EventKind::Submit,
                        Some(value),
                    )
                })
            };
            let has_callback = node.callback(super::spec::EventKind::Change).is_some()
                || node.callback(super::spec::EventKind::Submit).is_some();
            // Read the entry out before deciding: `if let Some(entry) =
            // self.map.borrow().get(..)` keeps the `Ref` alive for the whole
            // `if let` - the `else` arm included - so the `borrow_mut` below
            // would panic with "already borrowed" the first time a tree
            // declares an input this view has not seen yet.
            let existing = self.map.borrow().get(&node.id).cloned();
            if let Some(existing) = existing {
                existing.state.update(cx, |state, _| {
                    state.placeholder = props.placeholder.clone();
                    state.multiline = props.multiline;
                    state.password = props.password;
                    state.on_change = has_callback.then(|| change.clone());
                    state.on_submit = has_callback.then_some(submit.clone());
                });
            } else {
                let props = props.clone();
                let id = node.id.clone();
                let state = cx.new(|cx| VirtualInputState {
                    id,
                    value: props.value,
                    placeholder: props.placeholder,
                    multiline: props.multiline,
                    password: props.password,
                    focus: cx.focus_handle(),
                    on_change: has_callback.then_some(change),
                    on_submit: has_callback.then_some(submit),
                });
                self.map
                    .borrow_mut()
                    .insert(node.id.clone(), VirtualInput { state });
            }
        }
        for child in &node.children {
            self.collect(child, sink, cx, seen);
        }
    }

    /// The input registered for `id`, if the tree declared one.
    pub fn get(&self, id: &str) -> Option<VirtualInput> {
        self.map.borrow().get(id).cloned()
    }
}

//! A plugin `form` view: a title and a stack of fields, plus a Submit button
//! that hands the collected values back to the host. Only the toggle/select
//! fields are fully interactive in this pass; text-style fields render their
//! current value (or placeholder) and are edited by the plugin in a later
//! milestone.

use std::{collections::HashMap, rc::Rc};

use gpui::{
    div, prelude::*, px, rgb, App, AppContext, Context, ElementId, Entity, InteractiveElement,
    IntoElement, Render, StatefulInteractiveElement,
};
use gpui_component::ActiveTheme;

use crate::palette;

/// A single form field, deserialized from the plugin's `form` view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub placeholder: Option<String>,
    pub options: Vec<FormOption>,
    pub value: Option<FormValue>,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Multiline,
    Password,
    Toggle,
    Select,
}

impl FieldKind {
    /// Parse the wire `type` string used by the plugin API.
    pub fn from_wire(value: &str) -> Option<Self> {
        Some(match value {
            "text" => Self::Text,
            "multiline" => Self::Multiline,
            "password" => Self::Password,
            "toggle" => Self::Toggle,
            "select" => Self::Select,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormOption {
    pub id: String,
    pub label: String,
}

/// A field value: either a string (text/select) or a boolean (toggle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormValue {
    String(String),
    Bool(bool),
}

impl FormValue {
    /// Render the value as the string sent in a `form.submit` payload.
    pub fn to_string_value(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Bool(value) => value.to_string(),
        }
    }
}

/// The data a plugin `form` view carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormData {
    pub title: Option<String>,
    pub fields: Vec<FormField>,
    pub submit_label: Option<String>,
}

/// Callback fired when the user hits Submit; the map has every field id mapped
/// to its current value.
pub type FormSubmitCallback = Rc<dyn Fn(HashMap<String, FormValue>, &mut App)>;

/// The state backing a rendered `FormView`.
pub struct FormViewState {
    data: FormData,
    values: HashMap<String, FormValue>,
    on_submit: Option<FormSubmitCallback>,
}

impl Render for FormViewState {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.data.title.clone();
        let submit_label = self
            .data
            .submit_label
            .clone()
            .unwrap_or_else(|| "Submit".to_string());
        let fields = self.data.fields.clone();
        let values = self.values.clone();
        let accent = cx.theme().primary;
        div()
            .id(ElementId::from("form-view"))
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .gap_3()
            .when_some(title, |this, title| {
                this.child(
                    div()
                        .text_color(rgb(palette::FOREGROUND))
                        .text_lg()
                        .child(title),
                )
            })
            .child(
                div().flex_1().flex_col().gap_3().children(
                    fields
                        .iter()
                        .map(|field| self.render_field(field, values.get(&field.id), cx)),
                ),
            )
            .child(
                div()
                    .id(ElementId::from("form-submit"))
                    .h(px(30.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(accent)
                    .text_color(rgb(0xffffff))
                    .text_sm()
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                    .child(submit_label),
            )
    }
}

impl FormViewState {
    fn render_field(
        &self,
        field: &FormField,
        value: Option<&FormValue>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = field.id.clone();
        let label = field.label.clone();
        let value = value.cloned();
        div()
            .id(ElementId::from(format!("form-field-{id}")))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_color(rgb(palette::MUTED_FOREGROUND))
                    .text_sm()
                    .child(label),
            )
            .child(match field.kind {
                FieldKind::Toggle => {
                    let current = value
                        .and_then(|v| match v {
                            FormValue::Bool(b) => Some(b),
                            _ => None,
                        })
                        .unwrap_or(false);
                    let on_id = id.clone();
                    div()
                        .id(ElementId::from(format!("form-toggle-{on_id}")))
                        .h(px(26.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded_full()
                        .cursor_pointer()
                        .border_1()
                        .border_color(rgb(0xffffff).opacity(0.12))
                        .when(current, |this| this.text_color(rgb(0xffffff)))
                        .when(!current, |this| {
                            this.text_color(rgb(palette::MUTED_FOREGROUND))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle(&on_id, cx);
                        }))
                        .child(if current { "On" } else { "Off" })
                }
                FieldKind::Select => {
                    let selected = value
                        .and_then(|v| match v {
                            FormValue::String(s) => Some(s),
                            _ => None,
                        })
                        .unwrap_or_default();
                    div()
                        .id(ElementId::from(format!("form-select-{id}")))
                        .flex()
                        .flex_row()
                        .gap_2()
                        .children(field.options.iter().map(|opt| {
                            let on_id = id.clone();
                            let is_selected = selected == opt.id;
                            string_option(opt, is_selected, on_id, cx)
                        }))
                }
                _ => {
                    let text = value
                        .as_ref()
                        .and_then(|v| match v {
                            FormValue::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .or_else(|| field.placeholder.clone())
                        .unwrap_or_default();
                    div()
                        .id(ElementId::from(format!("form-text-{id}")))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(palette::BACKGROUND_ALT))
                        .border_1()
                        .border_color(rgb(0xffffff).opacity(0.08))
                        .text_color(rgb(palette::FOREGROUND))
                        .text_sm()
                        .child(text)
                }
            })
    }

    fn toggle(&mut self, field_id: &str, cx: &mut Context<Self>) {
        let current = self
            .values
            .get(field_id)
            .and_then(|v| match v {
                FormValue::Bool(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(false);
        self.values
            .insert(field_id.to_string(), FormValue::Bool(!current));
        cx.notify();
    }

    fn select_field(&mut self, field_id: &str, option_id: &str, cx: &mut Context<Self>) {
        self.values.insert(
            field_id.to_string(),
            FormValue::String(option_id.to_string()),
        );
        cx.notify();
    }

    fn submit(&self, cx: &mut Context<Self>) {
        if let Some(cb) = self.on_submit.clone() {
            cb(self.values.clone(), cx);
        }
    }

    /// The current values, for the app to serialize into `form.submit`.
    pub fn values(&self) -> HashMap<String, FormValue> {
        self.values.clone()
    }
}

fn string_option(
    opt: &FormOption,
    selected: bool,
    field_id: String,
    cx: &mut Context<FormViewState>,
) -> impl IntoElement {
    let option_id = opt.id.clone();
    let accent = cx.theme().primary;
    div()
        .id(ElementId::from(format!(
            "form-option-{}-{option_id}",
            field_id
        )))
        .h(px(26.0))
        .px_3()
        .flex()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .border_1()
        .border_color(rgb(0xffffff).opacity(0.12))
        .when(selected, |this| this.bg(accent).text_color(rgb(0xffffff)))
        .when(!selected, |this| {
            this.text_color(rgb(palette::MUTED_FOREGROUND))
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_field(&field_id, &option_id, cx);
        }))
        .child(opt.label.clone())
}

/// A rendered plugin `form` view handle, mirroring `DetailView`.
#[derive(Clone)]
pub struct FormView {
    state: Entity<FormViewState>,
}

impl FormView {
    /// Create the view with initial data and a submit callback.
    pub fn new<C>(
        data: FormData,
        on_submit: Option<FormSubmitCallback>,
        cx: &mut Context<C>,
    ) -> Self {
        let values = data
            .fields
            .iter()
            .map(|field| {
                (
                    field.id.clone(),
                    field.value.clone().unwrap_or_default_for_kind(field.kind),
                )
            })
            .collect();
        let state = cx.new(|_| FormViewState {
            data,
            values,
            on_submit,
        });
        Self { state }
    }

    /// Replace the displayed data and re-render.
    pub fn set_data<C: AppContext>(&self, data: FormData, cx: &mut C) {
        self.state.update(cx, |state, cx| {
            state.data = data;
            state.values = state
                .data
                .fields
                .iter()
                .map(|field| {
                    (
                        field.id.clone(),
                        field.value.clone().unwrap_or_default_for_kind(field.kind),
                    )
                })
                .collect();
            cx.notify();
        });
    }

    /// The current collected values.
    pub fn values(&self, cx: &App) -> HashMap<String, FormValue> {
        self.state.read(cx).values()
    }

    /// Render the view, filling the requested height.
    pub fn render<C>(&self, height: f32, _cx: &mut Context<C>) -> impl IntoElement {
        div()
            .id(ElementId::from("form-view-container"))
            .h(px(height))
            .w_full()
            .child(self.state.clone())
    }
}

trait DefaultForKind {
    fn unwrap_or_default_for_kind(self, kind: FieldKind) -> FormValue;
}

impl DefaultForKind for Option<FormValue> {
    fn unwrap_or_default_for_kind(self, kind: FieldKind) -> FormValue {
        self.unwrap_or_else(|| match kind {
            FieldKind::Toggle => FormValue::Bool(false),
            _ => FormValue::String(String::new()),
        })
    }
}

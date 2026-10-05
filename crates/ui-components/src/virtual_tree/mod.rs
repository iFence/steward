//! Declarative plugin UI tree: the `{ "type": "ui" }` view.
//!
//! The plugin process never touches gpui. It returns a serializable element
//! tree, [`spec`] validates it as untrusted data, and [`render`] replays the
//! validated tree into real gpui-component elements every frame. This keeps the
//! process isolation and the low-memory contract while giving plugins a layout
//! and style surface that is not a fixed catalogue of views.

pub mod input;
pub mod render;
pub mod spec;
pub mod view;

pub use input::{InputStore, VirtualInput};
pub use render::{render, EventSink, RenderEnv};
pub use spec::{
    validate_view, ArgKind, Axis, Color, EventKind, Kind, Length, Node, ProgressProps, StyleValue,
    UiError,
};
pub use view::VirtualTreeView;

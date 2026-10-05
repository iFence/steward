//! First-party window overlay: transient plugin toasts.
//!
//! Plugin `showToast` notifications arrive as a host event; this module keeps
//! them in an app-global center and renders them. It is registered as a
//! `RootPlugin`, so every window whose root is a `gpui-base::Root` (settings,
//! the plugin workspace) draws the stack automatically, and the launcher (whose
//! root is the app view) renders the same helper itself.

use std::{cell::RefCell, time::Duration, time::Instant};

use gpui::{
    div, prelude::*, px, rgb, AnyElement, App, Context, Entity, FocusHandle, IntoElement, Render,
    WeakEntity, Window,
};
use gpui_base::RootPlugin;

use steward_ui_components::palette;

/// At most this many toasts are kept; the oldest is dropped first.
const MAX_TOASTS: usize = 4;

#[derive(Clone)]
struct Toast {
    message: String,
    kind: String,
    expires_at: Instant,
}

#[derive(Default)]
struct ToastCenter {
    toasts: RefCell<Vec<Toast>>,
    overlays: RefCell<Vec<WeakEntity<ToastOverlay>>>,
}
impl gpui::Global for ToastCenter {}

/// The per-window overlay layer that draws the toast stack.
pub struct ToastOverlay {
    _focus: FocusHandle,
}

impl ToastOverlay {
    fn build(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        cx.global_mut::<ToastCenter>()
            .overlays
            .borrow_mut()
            .push(weak);
        Self {
            _focus: cx.focus_handle(),
        }
    }
}

impl Render for ToastOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        render_toasts(cx)
    }
}

impl RootPlugin for ToastOverlay {}

/// Initialize the toast center and register the overlay for `Root`-rooted
/// windows. Call once, during application init.
pub fn init(cx: &mut App) {
    if cx.try_global::<ToastCenter>().is_none() {
        cx.set_global(ToastCenter::default());
    }
    gpui_component::Root::register_plugin::<ToastOverlay>(cx, ToastOverlay::build);
}

/// Show a transient toast. `duration_ms` is clamped to a readable range.
pub fn show_toast(cx: &mut App, message: String, kind: String, duration_ms: u64) {
    let duration = Duration::from_millis(duration_ms.clamp(800, 10_000));
    {
        let center = cx.global_mut::<ToastCenter>();
        let mut toasts = center.toasts.borrow_mut();
        toasts.push(Toast {
            message,
            kind,
            expires_at: Instant::now() + duration,
        });
        if toasts.len() > MAX_TOASTS {
            toasts.remove(0);
        }
    }
    notify_overlays(cx);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(duration).await;
        cx.update(|cx| {
            let now = Instant::now();
            {
                let center = cx.global_mut::<ToastCenter>();
                center
                    .toasts
                    .borrow_mut()
                    .retain(|toast| toast.expires_at > now);
            }
            notify_overlays(cx);
        });
    })
    .detach();
}

fn notify_overlays(cx: &mut App) {
    let overlays: Vec<Entity<ToastOverlay>> = cx
        .global::<ToastCenter>()
        .overlays
        .borrow()
        .iter()
        .filter_map(|weak| weak.upgrade())
        .collect();
    for overlay in overlays {
        overlay.update(cx, |_overlay, cx| cx.notify());
    }
}

/// The toast stack, positioned bottom-right. Safe to call before [`init`].
pub fn render_toasts(cx: &App) -> AnyElement {
    let Some(center) = cx.try_global::<ToastCenter>() else {
        return div().into_any_element();
    };
    let now = Instant::now();
    let toasts: Vec<Toast> = center
        .toasts
        .borrow()
        .iter()
        .filter(|toast| toast.expires_at > now)
        .cloned()
        .collect();
    div()
        .absolute()
        .bottom(px(28.0))
        .right(px(28.0))
        .flex()
        .flex_col()
        .gap_2()
        .items_end()
        .children(toasts.into_iter().map(toast_element))
        .into_any_element()
}

fn toast_element(toast: Toast) -> impl IntoElement {
    let accent = match toast.kind.as_str() {
        "error" => rgb(0xE5484D),
        "success" => rgb(0x30A46C),
        _ => rgb(0x3B82F6),
    };
    div()
        .max_w(px(360.0))
        .px_4()
        .py_3()
        .rounded_lg()
        .bg(rgb(palette::BACKGROUND_ALT))
        .border_1()
        .border_color(accent)
        .text_color(rgb(palette::FOREGROUND))
        .text_sm()
        .child(toast.message)
}

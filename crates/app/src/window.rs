//! GPUI Fast window shell around NSH AppViews.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    px, size, App, AppContext, Bounds, Focusable, QuitMode, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use nsh_platform_gpui::{App as NshApp, Root};

use crate::config::{LAUNCHER_HEIGHT, LAUNCHER_WIDTH};
use crate::nsh_surface::{new_view, shell_options};
use crate::platform;
use crate::services::AppServices;
use crate::settings::StewardSettings;
use crate::shell::StewardLauncher;
use crate::state::AppState;

pub(crate) fn init_common(cx: &mut App) {
    cx.set_quit_mode(QuitMode::Explicit);
}

pub(crate) fn open_launcher_window(
    cx: &mut App,
    _state: Rc<RefCell<AppState>>,
    services: Arc<AppServices>,
    data_dir: std::path::PathBuf,
) -> gpui::AnyWindowHandle {
    let bounds = Bounds::centered(None, size(px(LAUNCHER_WIDTH), px(LAUNCHER_HEIGHT)), cx);
    let show_on_start = std::env::var_os("STEWARD_SHOW_ON_START").is_some();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                appears_transparent: true,
                ..Default::default()
            }),
            window_background: WindowBackgroundAppearance::Blurred,
            show: show_on_start,
            focus: show_on_start,
            kind: WindowKind::PopUp,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title("Steward");
            cx.new(|cx| {
                let was_active = std::rc::Rc::new(Cell::new(false));
                let activation_state = was_active.clone();
                cx.observe_window_activation(window, move |_, window, cx| {
                    if window.is_window_active() {
                        activation_state.set(true);
                    } else if activation_state.get() && platform::is_visible(window) {
                        activation_state.set(false);
                        hide_window(window, cx);
                    }
                })
                .detach();
                let view = new_view(
                    shell_options(services.clone(), data_dir.clone()),
                    window,
                    cx,
                );
                view.load(NshApp::Rust(Root::of::<StewardLauncher>()));
                view.focus_handle(cx).focus(window, cx);
                view
            })
        },
    )
    .expect("open Steward launcher window")
    .into()
}

pub(crate) fn open_settings_window(
    cx: &mut App,
    state: Rc<RefCell<AppState>>,
    services: Arc<AppServices>,
    data_dir: std::path::PathBuf,
) -> gpui::AnyWindowHandle {
    if let Some(handle) = state.borrow().settings_window {
        let _ = handle.update(cx, |_, window, cx| {
            platform::show(window, 760.0, 560.0);
            cx.activate(true);
        });
        return handle;
    }
    let bounds = Bounds::centered(None, size(px(760.0), px(560.0)), cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Steward Settings".into()),
                    ..Default::default()
                }),
                show: true,
                focus: true,
                is_resizable: true,
                ..Default::default()
            },
            move |window, cx| {
                window.set_window_title("Steward Settings");
                cx.new(|cx| {
                    let view = new_view(
                        shell_options(services.clone(), data_dir.clone()),
                        window,
                        cx,
                    );
                    view.load(NshApp::Rust(Root::of::<StewardSettings>()));
                    view
                })
            },
        )
        .expect("open Steward settings window");
    state.borrow_mut().settings_window = Some(handle.into());
    handle.into()
}

pub(crate) fn hide_window(window: &mut Window, _cx: &mut App) {
    platform::hide(window);
}

pub(crate) fn resize_launcher(window: &mut Window, height: f32) {
    if (window.viewport_size().height.as_f32() - height).abs() > 1.0 {
        window.resize(gpui::size(
            px(LAUNCHER_WIDTH),
            px(height.max(LAUNCHER_HEIGHT)),
        ));
    }
}

pub(crate) fn focus_plugin_window(state: &Rc<RefCell<AppState>>, plugin_id: &str, cx: &mut App) {
    if let Some(plugin) = state.borrow().plugin_windows.get(plugin_id) {
        let handle = plugin.handle;
        let _ = handle.update(cx, |_, window, cx| {
            platform::show(window, 900.0, 680.0);
            cx.activate(true);
        });
    }
}

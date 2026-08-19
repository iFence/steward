//! Steward desktop entry point (M0 skeleton).
//!
//! A GPUI window that can be summoned and hidden with the global hotkey
//! `Ctrl+Alt+Space` (or hidden with `Esc`). GPUI does not provide a
//! system-level hotkey API, so registration is delegated to the
//! `global-hotkey` crate and events are bridged into the GPUI main loop
//! through a channel that is polled by a foreground task.
//!
//! Window visibility on Windows is driven through the native HWND
//! (`ShowWindow(SW_HIDE/SW_SHOW)`), because `App::hide` is a no-op on the
//! Windows backend of the pinned GPUI revision. Other platforms fall back to
//! `App::hide` / `App::activate` (see docs/architecture.md, M4 will polish
//! non-Windows behavior).

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::time::Duration;

use anyhow::{Context as _, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gpui::{
    actions, div, prelude::*, px, rgb, size, AnyWindowHandle, App, AppContext, Bounds, Context,
    FocusHandle, KeyBinding, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;

actions!(steward, [HideWindow]);

const TOGGLE_HOTKEY_HINT: &str = "Ctrl+Alt+Space 呼出/隐藏 · Esc 隐藏";

struct StewardApp {
    focus_handle: FocusHandle,
}

impl Render for StewardApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .on_action(|_: &HideWindow, window, cx| hide_window(window, cx))
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(rgb(0x1e1e2e))
            .text_xl()
            .text_color(rgb(0xcdd6f4))
            .child(div().text_2xl().child("Steward"))
            .child(format!("v{}", env!("CARGO_PKG_VERSION")))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x89b4fa))
                    .child(TOGGLE_HOTKEY_HINT),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.0), px(520.0)), cx);

        cx.bind_keys([KeyBinding::new("escape", HideWindow, None)]);
        cx.on_window_closed(|cx, _window_id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    window.set_window_title("Steward");
                    cx.activate(false);
                    cx.new(|cx| {
                        let focus_handle = cx.focus_handle();
                        focus_handle.focus(window, cx);
                        StewardApp { focus_handle }
                    })
                },
            )
            .expect("failed to open the main window");

        if let Err(error) = setup_global_hotkey(window.into(), cx) {
            eprintln!("failed to register global hotkey: {error:#}");
        }

        cx.activate(true);
    });
}

fn setup_global_hotkey(window: AnyWindowHandle, cx: &mut App) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("create global hotkey manager")?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
    manager.register(hotkey).context("register global hotkey")?;
    // The hidden message window must stay alive for the app lifetime.
    Box::leak(Box::new(manager));

    let events = GlobalHotKeyEvent::receiver();
    cx.spawn(async move |cx| loop {
        while let Ok(event) = events.try_recv() {
            if event.state == HotKeyState::Pressed {
                toggle_window_visibility(&window, cx);
            }
        }
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    })
    .detach();

    Ok(())
}

fn toggle_window_visibility(window: &AnyWindowHandle, cx: &mut impl AppContext) {
    let _ = (*window).update(cx, |_, window, cx| {
        if platform::is_visible(window) {
            hide_window(window, cx);
        } else {
            show_window(window, cx);
        }
    });
}

fn hide_window(window: &mut Window, _cx: &mut App) {
    platform::hide(window);
    #[cfg(not(target_os = "windows"))]
    _cx.hide();
}

fn show_window(window: &mut Window, _cx: &mut App) {
    #[cfg(not(target_os = "windows"))]
    _cx.activate(true);
    platform::show(window);
    window.refresh();
}

#[cfg(target_os = "windows")]
mod platform {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::HWND,
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
            ShowWindow, SW_HIDE, SW_SHOW,
        },
    };

    fn hwnd(window: &Window) -> Option<HWND> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn is_visible(window: &Window) -> bool {
        hwnd(window).is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 })
    }

    pub fn hide(window: &Window) {
        if let Some(hwnd) = hwnd(window) {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    pub fn show(window: &Window) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            force_foreground(hwnd);
        }
    }

    /// Windows restricts `SetForegroundWindow`; attaching the input queues of
    /// the involved threads is the standard workaround for launcher-style apps.
    unsafe fn force_foreground(hwnd: HWND) {
        let current_thread = GetCurrentThreadId();
        let target_thread = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());

        if current_thread != target_thread {
            AttachThreadInput(current_thread, target_thread, 1);
        }
        if current_thread != foreground_thread && foreground_thread != 0 {
            AttachThreadInput(current_thread, foreground_thread, 1);
        }

        SetForegroundWindow(hwnd);

        if current_thread != target_thread {
            AttachThreadInput(current_thread, target_thread, 0);
        }
        if current_thread != foreground_thread && foreground_thread != 0 {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};

    use gpui::Window;

    static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

    pub fn is_visible(_window: &Window) -> bool {
        WINDOW_VISIBLE.load(Ordering::Relaxed)
    }

    pub fn hide(_window: &Window) {
        WINDOW_VISIBLE.store(false, Ordering::Relaxed);
    }

    pub fn show(_window: &Window) {
        WINDOW_VISIBLE.store(true, Ordering::Relaxed);
    }
}

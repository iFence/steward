//! Steward desktop entry point.
//!
//! Startup is silent: the app registers a system tray icon (Windows/macOS)
//! and the global hotkey `Ctrl+Alt+Space`, but opens no window. The launcher
//! bar — a wide, short, borderless popup centered on the primary display —
//! is summoned on demand via the hotkey or the tray icon, and hidden again
//! with `Esc` (or the hotkey).
//!
//! GPUI does not provide a system-level hotkey API, so registration is
//! delegated to the `global-hotkey` crate and events are bridged into the
//! GPUI main loop through channels polled by a foreground task. Tray and
//! tray-menu events ride the same loop.
//!
//! Window visibility on Windows is driven through the native HWND
//! (`ShowWindow(SW_HIDE/SW_SHOW)`), because `App::hide` is a no-op on the
//! Windows backend of the pinned GPUI revision. Other platforms fall back to
//! `App::hide` / `App::activate` (see docs/architecture.md, M4 will polish
//! non-Windows behavior).

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{cell::RefCell, rc::Rc, time::Duration};

use anyhow::{Context as _, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gpui::{
    actions, div, prelude::*, px, rgb, size, AnyWindowHandle, App, AppContext, AsyncApp, Bounds,
    FocusHandle, FontWeight, KeyBinding, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions,
};
use gpui_platform::application;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};

actions!(steward, [HideWindow]);

const TOGGLE_HOTKEY_HINT: &str = "Ctrl+Alt+Space 呼出/隐藏 · Esc 隐藏";

/// The launcher bar is deliberately long and short: wide enough to hold a
/// search box plus quick-launch chips, short enough to sit unobtrusively in
/// the middle of the screen.
const LAUNCHER_WIDTH: f32 = 960.0;
const LAUNCHER_HEIGHT: f32 = 56.0;

const MENU_TOGGLE: &str = "toggle";
const MENU_QUIT: &str = "quit";

struct StewardApp {
    focus_handle: FocusHandle,
}

impl Render for StewardApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .on_action(|_: &HideWindow, window, cx| hide_window(window, cx))
            .flex()
            .flex_row()
            .size_full()
            .items_center()
            .gap_3()
            .px_4()
            .bg(rgb(0x181825))
            .text_color(rgb(0xcdd6f4))
            .child(
                div().flex().items_center().child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::BOLD)
                        .child("Steward"),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .px_3()
                    .bg(rgb(0x232332))
                    .rounded_md()
                    .text_sm()
                    .text_color(rgb(0x6c7086))
                    .child("搜索应用或输入命令…"),
            )
            .child(launcher_chip("计算器"))
            .child(launcher_chip("剪贴板历史"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x89b4fa))
                    .child(TOGGLE_HOTKEY_HINT),
            )
    }
}

fn launcher_chip(label: &'static str) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .bg(rgb(0x313244))
        .rounded_md()
        .text_sm()
        .child(label)
}

fn main() {
    application().run(|cx: &mut App| {
        cx.bind_keys([KeyBinding::new("escape", HideWindow, None)]);

        let window = open_launcher_window(cx);
        let window_handle = Rc::new(RefCell::new(Some(window)));

        // Closing the launcher (e.g. Alt+F4) must not kill the app: the tray
        // icon is the application shell, and the window is reopened on demand.
        let closed_handle = window_handle.clone();
        cx.on_window_closed(move |_cx, _window_id| {
            *closed_handle.borrow_mut() = None;
        })
        .detach();

        if let Err(error) = setup_global_hotkey(window_handle.clone(), cx) {
            eprintln!("failed to register global hotkey: {error:#}");
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Err(error) = setup_tray(cx) {
            eprintln!("failed to create tray icon: {error:#}");
        }
    });
}

fn open_launcher_window(cx: &mut App) -> AnyWindowHandle {
    let bounds = Bounds::centered(None, size(px(LAUNCHER_WIDTH), px(LAUNCHER_HEIGHT)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Hide the system titlebar so the bar is borderless.
            titlebar: Some(TitlebarOptions {
                appears_transparent: true,
                ..Default::default()
            }),
            // Silent startup: the window exists but stays hidden until the
            // hotkey or tray summons it.
            show: false,
            focus: false,
            // PopUp = tool window on Windows: no taskbar entry, always on top.
            kind: WindowKind::PopUp,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title("Steward");
            cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                focus_handle.focus(window, cx);
                StewardApp { focus_handle }
            })
        },
    )
    .expect("failed to open the launcher window")
    .into()
}

fn setup_global_hotkey(window: Rc<RefCell<Option<AnyWindowHandle>>>, cx: &mut App) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("create global hotkey manager")?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
    manager.register(hotkey).context("register global hotkey")?;
    // The hidden message window must stay alive for the app lifetime.
    Box::leak(Box::new(manager));

    let hotkey_events = GlobalHotKeyEvent::receiver();

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let tray_events = TrayIconEvent::receiver();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let menu_events = MenuEvent::receiver();

    cx.spawn(async move |cx| loop {
        while let Ok(event) = hotkey_events.try_recv() {
            if event.state == HotKeyState::Pressed {
                toggle_launcher(&window, cx);
            }
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        while let Ok(event) = tray_events.try_recv() {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                toggle_launcher(&window, cx);
            }
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        while let Ok(event) = menu_events.try_recv() {
            match event.id().as_ref() {
                MENU_TOGGLE => toggle_launcher(&window, cx),
                MENU_QUIT => cx.update(|cx| cx.quit()),
                _ => {}
            }
        }

        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    })
    .detach();

    Ok(())
}

/// Summon or dismiss the launcher bar. Reopens the window if it was closed.
fn toggle_launcher(window: &Rc<RefCell<Option<AnyWindowHandle>>>, cx: &mut AsyncApp) {
    let handle = window.borrow();
    match handle.as_ref() {
        Some(handle) => {
            let _ = (*handle).update(cx, |_, window, cx| {
                if platform::is_visible(window) {
                    hide_window(window, cx);
                } else {
                    show_window(window, cx);
                }
            });
        }
        None => {
            drop(handle);
            show_launcher(window, cx);
        }
    }
}

/// Show the launcher bar, reopening the window first if necessary.
fn show_launcher(window: &Rc<RefCell<Option<AnyWindowHandle>>>, cx: &mut AsyncApp) {
    if window.borrow().is_none() {
        *window.borrow_mut() = Some(cx.update(open_launcher_window));
    }
    let handle = window.borrow();
    if let Some(handle) = handle.as_ref() {
        let _ = (*handle).update(cx, |_, window, cx| show_window(window, cx));
    }
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn setup_tray(_cx: &mut App) -> Result<()> {
    let icon = load_tray_icon()?;
    let toggle = MenuItem::with_id(MENU_TOGGLE, "显示 / 隐藏 Steward", true, None);
    let quit = MenuItem::with_id(MENU_QUIT, "退出 Steward", true, None);
    let menu = Menu::new();
    menu.append(&toggle)?;
    menu.append(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("Steward")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()
        .context("build system tray icon")?;
    // The tray icon must outlive the event loop.
    Box::leak(Box::new(tray));

    Ok(())
}

#[cfg(target_os = "windows")]
fn load_tray_icon() -> Result<Icon> {
    Icon::from_resource(1, Some((32, 32))).context("load tray icon from embedded resources")
}

#[cfg(target_os = "macos")]
fn load_tray_icon() -> Result<Icon> {
    let png = include_bytes!("../../assets/steward.png");
    let image = image::load_from_memory(png).context("decode bundled steward.png")?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).context("create macOS tray icon")
}

#[cfg(target_os = "windows")]
mod platform {
    use crate::{LAUNCHER_HEIGHT, LAUNCHER_WIDTH};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::HiDpi::GetDpiForWindow,
        UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
            SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
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
            position_centered(hwnd);
            ShowWindow(hwnd, SW_SHOW);
            force_foreground(hwnd);
        }
    }

    /// A window opened with `show: false` never receives the placement GPUI
    /// computed for it, so the launcher keeps its creation-time (default)
    /// bounds. Apply the centered launcher rectangle ourselves before
    /// showing, in physical pixels.
    unsafe fn position_centered(hwnd: HWND) {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let scale = dpi as f32 / 96.0;
        let width = (LAUNCHER_WIDTH * scale).round() as i32;
        let height = (LAUNCHER_HEIGHT * scale).round() as i32;

        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - width) / 2;
        let y = work.top + ((work.bottom - work.top) - height) / 2;

        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
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

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
    actions, div, prelude::*, px, rgb, size, Animation, AnimationExt, AnyWindowHandle, App,
    AppContext, AsyncApp, Bounds, FocusHandle, KeyBinding, KeyDownEvent, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind, WindowOptions,
};
use gpui_platform::application;

mod i18n;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};

actions!(steward, [HideWindow]);

/// The launcher bar is deliberately long and short: wide enough to hold a
/// search box plus quick-launch chips, short enough to sit unobtrusively in
/// the middle of the screen.
const LAUNCHER_WIDTH: f32 = 800.0;
const LAUNCHER_HEIGHT: f32 = 56.0;

const MENU_TOGGLE: &str = "toggle";
const MENU_QUIT: &str = "quit";

struct StewardApp {
    focus_handle: FocusHandle,
    input: SearchInput,
    i18n: Rc<i18n::Localization>,
}

struct SearchInput {
    query: String,
    /// Cursor position measured in characters.
    cursor: usize,
}

/// Shared launcher state used by the foreground event loop: the (possibly
/// closed) window handle plus the focus handle that must be re-focused every
/// time the bar is summoned.
struct LauncherState {
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
}

impl SearchInput {
    fn char_count(&self) -> usize {
        self.query.chars().count()
    }

    fn byte_index(&self) -> usize {
        self.query
            .char_indices()
            .nth(self.cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len())
    }

    fn insert_char(&mut self, ch: char) {
        self.query.insert(self.byte_index(), ch);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let byte = self
                .query
                .char_indices()
                .nth(self.cursor - 1)
                .map(|(index, _)| index)
                .unwrap_or(0);
            self.query.remove(byte);
            self.cursor -= 1;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.char_count() {
            let byte = self.byte_index();
            self.query.remove(byte);
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        self.cursor = (self.cursor as i32 + delta).clamp(0, self.char_count() as i32) as usize;
    }

    fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.char_count());
    }

    fn before_cursor(&self) -> &str {
        &self.query[..self.byte_index()]
    }

    fn after_cursor(&self) -> &str {
        &self.query[self.byte_index()..]
    }
}

impl Render for StewardApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .on_action(|_: &HideWindow, window, cx| hide_window(window, cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .flex()
            .flex_row()
            .size_full()
            .items_center()
            .px_4()
            .bg(rgb(0x232332))
            .text_sm()
            .text_color(rgb(0xcdd6f4))
            .window_control_area(WindowControlArea::Drag)
            .child(if self.input.query.is_empty() {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(cursor())
                    .child(
                        div()
                            .text_color(rgb(0x6c7086))
                            .child(self.i18n.translate("search-placeholder")),
                    )
            } else {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(self.input.before_cursor().to_string())
                    .child(cursor())
                    .child(self.input.after_cursor().to_string())
            })
    }
}

impl StewardApp {
    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;

        // Insert the typed character (key_char also carries the shifted or
        // AltGr variant). Skip when ctrl/alt/win are held, e.g. shortcuts.
        if !modifiers.control && !modifiers.alt && !modifiers.platform {
            if let Some(ch) = keystroke.key_char.as_deref().and_then(|s| s.chars().next()) {
                if !ch.is_control() {
                    self.input.insert_char(ch);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
            }
        }

        match keystroke.key.as_str() {
            "space" => {
                self.input.insert_char(' ');
                cx.notify();
                cx.stop_propagation();
            }
            "backspace" => {
                self.input.backspace();
                cx.notify();
                cx.stop_propagation();
            }
            "delete" => {
                self.input.delete();
                cx.notify();
                cx.stop_propagation();
            }
            "left" => {
                self.input.move_cursor(-1);
                cx.notify();
                cx.stop_propagation();
            }
            "right" => {
                self.input.move_cursor(1);
                cx.notify();
                cx.stop_propagation();
            }
            "home" => {
                self.input.set_cursor(0);
                cx.notify();
                cx.stop_propagation();
            }
            "end" => {
                self.input.set_cursor(self.input.char_count());
                cx.notify();
                cx.stop_propagation();
            }
            // Hide directly at the key level (the keybinding is a fallback):
            // this is more robust than relying on action dispatch when the
            // window just went through a drag or was re-activated.
            "escape" => {
                hide_window(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

/// A blinking text cursor rendered as a thin vertical bar.
fn cursor() -> impl IntoElement {
    div()
        .w(px(2.0))
        .h(px(18.0))
        .bg(rgb(0x89b4fa))
        .with_animation(
            "cursor-blink",
            Animation::new(Duration::from_millis(530)).repeat_synced(),
            |this, delta| this.opacity(if delta < 0.5 { 1.0 } else { 0.0 }),
        )
}

fn main() {
    application().run(|cx: &mut App| {
        cx.bind_keys([KeyBinding::new("escape", HideWindow, None)]);

        let i18n = Rc::new(i18n::Localization::new().expect("failed to initialize localization"));
        let focus = cx.focus_handle();
        let window = open_launcher_window(cx, &focus, i18n.clone());
        let state = Rc::new(RefCell::new(LauncherState {
            window: Some(window),
            focus,
        }));

        // Closing the launcher (e.g. Alt+F4) must not kill the app: the tray
        // icon is the application shell, and the window is reopened on demand.
        let closed_state = state.clone();
        cx.on_window_closed(move |_cx, _window_id| {
            closed_state.borrow_mut().window = None;
        })
        .detach();

        if let Err(error) = setup_global_hotkey(state.clone(), i18n.clone(), cx) {
            eprintln!("failed to register global hotkey: {error:#}");
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Err(error) = setup_tray(cx, &i18n) {
            eprintln!("failed to create tray icon: {error:#}");
        }
    });
}

fn open_launcher_window(
    cx: &mut App,
    focus: &FocusHandle,
    i18n: Rc<i18n::Localization>,
) -> AnyWindowHandle {
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
                focus.focus(window, cx);
                StewardApp {
                    focus_handle: focus.clone(),
                    input: SearchInput {
                        query: String::new(),
                        cursor: 0,
                    },
                    i18n,
                }
            })
        },
    )
    .expect("failed to open the launcher window")
    .into()
}

fn setup_global_hotkey(
    state: Rc<RefCell<LauncherState>>,
    i18n: Rc<i18n::Localization>,
    cx: &mut App,
) -> Result<()> {
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
                toggle_launcher(&state, i18n.clone(), cx);
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
                toggle_launcher(&state, i18n.clone(), cx);
            }
        }

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        while let Ok(event) = menu_events.try_recv() {
            match event.id().as_ref() {
                MENU_TOGGLE => toggle_launcher(&state, i18n.clone(), cx),
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
fn toggle_launcher(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<i18n::Localization>,
    cx: &mut AsyncApp,
) {
    let state_ref = state.borrow();
    match state_ref.window.as_ref() {
        Some(handle) => {
            let focus = state_ref.focus.clone();
            let _ = (*handle).update(cx, |_, window, cx| {
                if platform::is_visible(window) {
                    hide_window(window, cx);
                } else {
                    focus.focus(window, cx);
                    show_window(window, cx);
                }
            });
        }
        None => {
            drop(state_ref);
            show_launcher(state, i18n, cx);
        }
    }
}

/// Show the launcher bar, reopening the window first if necessary.
fn show_launcher(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<i18n::Localization>,
    cx: &mut AsyncApp,
) {
    if state.borrow().window.is_none() {
        let focus = state.borrow().focus.clone();
        let handle = cx.update(|cx| open_launcher_window(cx, &focus, i18n.clone()));
        state.borrow_mut().window = Some(handle);
    }
    let state_ref = state.borrow();
    if let Some(handle) = state_ref.window.as_ref() {
        let focus = state_ref.focus.clone();
        let _ = (*handle).update(cx, |_, window, cx| {
            focus.focus(window, cx);
            show_window(window, cx);
        });
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
fn setup_tray(_cx: &mut App, i18n: &i18n::Localization) -> Result<()> {
    let icon = load_tray_icon()?;
    let toggle = MenuItem::with_id(MENU_TOGGLE, i18n.translate("app-toggle"), true, None);
    let quit = MenuItem::with_id(MENU_QUIT, i18n.translate("app-quit"), true, None);
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
        // Horizontally centered; vertically in the upper third of the screen.
        let y = work.top + ((work.bottom - work.top) - height) / 3;

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

//! Schedules platform, search, and UI events on the application loop.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use gpui::App;
use tray_icon::{menu::MenuEvent, MouseButton, MouseButtonState, TrayIconEvent};

use crate::config::{MENU_QUIT, MENU_SETTINGS};
use crate::handlers;
use crate::state::AppState;

pub(crate) fn spawn(state: Rc<RefCell<AppState>>, cx: &mut App) -> anyhow::Result<()> {
    let hotkey_events = GlobalHotKeyEvent::receiver();
    let tray_events = TrayIconEvent::receiver();
    let menu_events = MenuEvent::receiver();

    // Windows can leave a freshly shown launcher visible without making it the
    // foreground window, so observe the foreground while it is visible.
    #[cfg(target_os = "windows")]
    let mut cached_launcher_hwnd: Option<windows_sys::Win32::Foundation::HWND> = None;
    #[cfg(target_os = "windows")]
    let mut last_foreground_hwnd: Option<windows_sys::Win32::Foundation::HWND> = None;

    cx.spawn(async move |cx| loop {
        state.borrow_mut().search.borrow_mut().poll();
        handlers::drain_plugin_launches(cx, &state);
        handlers::drain_plugin_events(cx, &state);
        handlers::drain_clipboard(&state);
        handlers::drain_requests(&state);
        handlers::drain_commands(cx, &state);

        while let Ok(event) = hotkey_events.try_recv() {
            if event.state == HotKeyState::Pressed
                && state
                    .borrow()
                    .summon_hotkey
                    .is_some_and(|hotkey| hotkey.id() == event.id)
            {
                handlers::toggle_launcher(cx, &state);
            }
        }
        while let Ok(event) = tray_events.try_recv() {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                handlers::toggle_launcher(cx, &state);
            }
        }
        while let Ok(event) = menu_events.try_recv() {
            match event.id().as_ref() {
                MENU_SETTINGS => handlers::open_settings(cx, &state),
                MENU_QUIT => cx.update(|cx| cx.quit()),
                _ => {}
            }
        }

        #[cfg(target_os = "windows")]
        {
            let handle = state.borrow().window;
            match handle {
                None => cached_launcher_hwnd = None,
                Some(handle) if cached_launcher_hwnd.is_none() => {
                    cached_launcher_hwnd = handle
                        .update(cx, |_, window, _| crate::platform::hwnd(window))
                        .ok()
                        .flatten();
                }
                Some(_) => {}
            }

            if let Some(hwnd) = cached_launcher_hwnd {
                if crate::platform::is_hwnd_visible(hwnd) {
                    let foreground = crate::platform::foreground_hwnd();
                    match last_foreground_hwnd {
                        None => last_foreground_hwnd = Some(foreground),
                        Some(previous) if previous != foreground => {
                            last_foreground_hwnd = Some(foreground);
                            if foreground != hwnd && !crate::platform::cursor_hits_window(hwnd) {
                                if let Some(handle) = state.borrow().window {
                                    let _ = handle.update(cx, |_, window, cx| {
                                        crate::window::hide_window(window, cx)
                                    });
                                }
                            }
                        }
                        Some(_) => {}
                    }
                } else {
                    last_foreground_hwnd = None;
                }
            }
        }

        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    })
    .detach();
    Ok(())
}

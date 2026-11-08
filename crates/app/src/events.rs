//! Bridges tray, hotkey, clipboard, NSH UI requests, and search workers.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use gpui::{App, AsyncApp};
use tray_icon::{menu::MenuEvent, MouseButton, MouseButtonState, TrayIconEvent};

use crate::config::{
    LANGUAGE_SETTING, MENU_QUIT, MENU_SETTINGS, SUMMON_HOTKEY_SETTING, THEME_COLOR_SETTING,
};
use crate::services::{RowAction, WindowCommand};
use crate::state::AppState;

pub(crate) fn spawn(state: Rc<RefCell<AppState>>, cx: &mut App) -> anyhow::Result<()> {
    let hotkey_events = GlobalHotKeyEvent::receiver();
    let tray_events = TrayIconEvent::receiver();
    let menu_events = MenuEvent::receiver();

    // The activation observer handles the normal WM_ACTIVATE path. Windows can
    // still leave a freshly shown launcher visible without making it the
    // foreground window (for example when the foreground lock denies
    // SetForegroundWindow), so the observer never receives the later
    // deactivation. Watch the foreground as a fallback while the launcher is
    // visible, without round-tripping through GPUI on every idle tick.
    #[cfg(target_os = "windows")]
    let mut cached_launcher_hwnd: Option<windows_sys::Win32::Foundation::HWND> = None;
    #[cfg(target_os = "windows")]
    let mut last_foreground_hwnd: Option<windows_sys::Win32::Foundation::HWND> = None;

    cx.spawn(async move |cx| loop {
        state.borrow_mut().search.borrow_mut().poll();
        drain_clipboard(&state);
        drain_requests(&state);
        drain_commands(cx, &state);

        while let Ok(event) = hotkey_events.try_recv() {
            if event.state == HotKeyState::Pressed
                && state
                    .borrow()
                    .summon_hotkey
                    .is_some_and(|hotkey| hotkey.id() == event.id)
            {
                toggle_launcher(cx, &state);
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
                toggle_launcher(cx, &state);
            }
        }
        while let Ok(event) = menu_events.try_recv() {
            match event.id().as_ref() {
                MENU_SETTINGS => open_settings(cx, &state),
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

fn drain_requests(state: &Rc<RefCell<AppState>>) {
    let requests = {
        let state = state.borrow();
        state.requests.clone()
    };
    while let Ok(request) = requests.try_recv() {
        state.borrow().search.borrow_mut().handle(request);
    }
}

fn drain_commands(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    let commands = {
        let state = state.borrow();
        state.commands.clone()
    };
    while let Ok(command) = commands.try_recv() {
        match command {
            WindowCommand::Hide => {
                if let Some(handle) = state.borrow().window {
                    let _ =
                        handle.update(cx, |_, window, cx| crate::window::hide_window(window, cx));
                }
            }
            WindowCommand::OpenSettings => open_settings(cx, state),
            WindowCommand::Resize { height } => {
                state.borrow_mut().launcher_height = height;
                if let Some(handle) = state.borrow().window {
                    let _ = handle.update(cx, |_, window, _| {
                        crate::window::resize_launcher(window, height)
                    });
                }
            }
            WindowCommand::Activate(action) => activate(cx, state, action),
            WindowCommand::ClosePlugin(plugin_id) => {
                cx.update(|cx| crate::plugin_window::close(cx, state, &plugin_id));
            }
            WindowCommand::SetLanguage(locale) => {
                set_language(state, &locale);
            }
            WindowCommand::SetAccent(accent) => {
                set_accent(state, &accent);
            }
            WindowCommand::SetAutostart(enabled) => {
                let effective = crate::autostart::set_autostart(enabled);
                state.borrow().services.set_autostart(effective);
            }
            WindowCommand::SetSummonHotkey(requested) => {
                match crate::hotkeys::replace_summon_hotkey(state, &requested) {
                    Ok(hotkey) => {
                        let value = hotkey.to_string();
                        let storage = state.borrow().storage.clone();
                        if let Err(error) =
                            storage.borrow().set_setting(SUMMON_HOTKEY_SETTING, &value)
                        {
                            eprintln!("persist summon hotkey: {error:#}");
                        }
                        state.borrow().services.set_summon_hotkey(value);
                    }
                    Err(error) => eprintln!("change summon hotkey: {error:#}"),
                }
            }
        }
    }
}

fn set_language(state: &Rc<RefCell<AppState>>, locale: &str) {
    let state = state.borrow();
    if let Err(error) = state.storage.borrow().set_setting(LANGUAGE_SETTING, locale) {
        eprintln!("persist language: {error:#}");
        return;
    }
    state.i18n.select_language(locale);
    state.services.set_locale(locale);
}

fn set_accent(state: &Rc<RefCell<AppState>>, accent: &str) {
    let Some(accent) = normalize_accent(accent) else {
        eprintln!("ignore invalid accent '{accent}': expected #rrggbb");
        return;
    };
    let state_ref = state.borrow();
    if let Err(error) = state_ref
        .storage
        .borrow()
        .set_setting(THEME_COLOR_SETTING, &accent)
    {
        eprintln!("persist accent: {error:#}");
        return;
    }
    state_ref.services.set_accent(&accent);
    drop(state_ref);
    crate::plugin_window::broadcast_theme(state, &accent);
}

fn normalize_accent(value: &str) -> Option<String> {
    let hex = value.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

fn activate(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>, action: RowAction) {
    match action {
        RowAction::LaunchApp(path) => {
            let _ = state.borrow().storage.borrow().upsert_usage(&path);
            if let Err(error) = crate::launch::launch(&path) {
                eprintln!("launch {}: {error:#}", path.display());
            }
            if let Some(handle) = state.borrow().window {
                let _ = handle.update(cx, |_, window, cx| crate::window::hide_window(window, cx));
            }
        }
        RowAction::CopyText(text) => {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(text);
            }
        }
        RowAction::OpenUrl(url) => {
            if let Err(error) = crate::launch::open_url(&url) {
                eprintln!("open URL {url}: {error:#}");
            }
        }
        RowAction::OpenPlugin {
            plugin_id,
            command,
            title,
            input,
        } => {
            let result = cx.update(|cx| {
                crate::plugin_window::open_or_focus(
                    cx,
                    state.clone(),
                    plugin_id,
                    command,
                    title,
                    input,
                )
            });
            if let Err(error) = result {
                eprintln!("open plugin: {error:#}");
            }
        }
    }
}

fn toggle_launcher(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    if let Some(handle) = state.borrow().window {
        let height = state.borrow().launcher_height;
        let _ = handle.update(cx, |_, window, cx| {
            if crate::platform::is_visible(window) {
                crate::window::hide_window(window, cx);
            } else {
                crate::platform::show(window, crate::config::LAUNCHER_WIDTH, height);
                cx.activate(true);
            }
        });
        return;
    }
    let (services, data_dir) = {
        let state = state.borrow();
        (state.services.clone(), state.data_dir.clone())
    };
    let handle =
        cx.update(|cx| crate::window::open_launcher_window(cx, state.clone(), services, data_dir));
    state.borrow_mut().window = Some(handle);
    if let Some(handle) = state.borrow().window {
        let _ = handle.update(cx, |_, window, cx| {
            crate::platform::show(
                window,
                crate::config::LAUNCHER_WIDTH,
                crate::config::LAUNCHER_HEIGHT,
            );
            cx.activate(true);
        });
    }
}

fn open_settings(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    let (services, data_dir) = {
        let state = state.borrow();
        (state.services.clone(), state.data_dir.clone())
    };
    cx.update(|cx| {
        crate::window::open_settings_window(cx, state.clone(), services, data_dir);
    });
}

fn drain_clipboard(state: &Rc<RefCell<AppState>>) {
    let receiver = {
        let state = state.borrow();
        let receiver = state.clipboard_rx.borrow().clone();
        receiver
    };
    let Some(receiver) = receiver else {
        return;
    };
    let mut latest = None;
    while let Ok(entries) = receiver.try_recv() {
        latest = Some(entries);
    }
    if let Some(entries) = latest {
        if let Ok(mut current) = state.borrow().clipboard_entries.lock() {
            *current = entries;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_accent;

    #[test]
    fn accents_are_normalized_and_validated() {
        assert_eq!(normalize_accent("#863BFF").as_deref(), Some("#863bff"));
        assert_eq!(normalize_accent("a6e3a1").as_deref(), Some("#a6e3a1"));
        assert_eq!(normalize_accent("#12345"), None);
        assert_eq!(normalize_accent("#hello!"), None);
    }
}

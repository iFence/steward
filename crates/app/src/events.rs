//! Bridges native tray/hotkey events into the GPUI event loop and drains
//! background work (scan results, icon batches) on the foreground thread.

use std::{cell::RefCell, rc::Rc, time::Duration};

use anyhow::Result;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use gpui::{App, AsyncApp};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{
    menu::MenuEvent, MouseButton, MouseButtonState, TrayIconEvent,
};

use crate::config::{MENU_QUIT, MENU_SETTINGS};
use crate::i18n::Localization;
use crate::launcher::{LauncherState, StewardApp};
use crate::platform;
use crate::settings::toggle_settings_window;
use crate::window::{hide_window, toggle_launcher};

/// Drain finished background icon extractions into the shared cache. When the
/// batch belongs to the current search, re-run the search so every row picks
/// its icon up from the cache; a batch superseded by a newer query only fills
/// the cache for future searches.
fn drain_icon_batches(state: &Rc<RefCell<LauncherState>>, cx: &mut AsyncApp) {
    let Some(rx) = state.borrow().icon_rx.borrow().clone() else {
        return;
    };
    let batch = match rx.try_recv() {
        Ok(batch) => batch,
        Err(crossbeam_channel::TryRecvError::Empty) => return,
        Err(crossbeam_channel::TryRecvError::Disconnected) => {
            *state.borrow().icon_rx.borrow_mut() = None;
            return;
        }
    };
    let (gen, icons) = batch;
    let current_gen = state.borrow().icon_gen.get();
    {
        let state = state.borrow();
        let mut cache = state.icon_cache.borrow_mut();
        for (path, icon) in &icons {
            cache.insert(path.clone(), icon.clone());
        }
    }
    if gen != current_gen {
        return;
    }
    let Some(window) = state.borrow().window else {
        return;
    };
    let Some(app) = window.downcast::<StewardApp>() else {
        return;
    };
    let _ = app.update(cx, |app, window, cx| app.search(window, cx));
}

/// Bridge native tray/hotkey events into the GPUI event loop. Runs only after
/// GPUI started; the hotkey manager itself is registered by the caller
/// (boot closure).
pub(crate) fn spawn_event_poll_task(
    state: Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut App,
) -> Result<()> {
    let hotkey_events = GlobalHotKeyEvent::receiver();

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let tray_events = TrayIconEvent::receiver();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let menu_events = MenuEvent::receiver();

    // The activation observer hides the launcher on WM_ACTIVATE(WA_INACTIVE),
    // but Windows only delivers that message to a window that actually owns
    // activation. A summon can intermittently fail to take the foreground —
    // the OS foreground lock denies `SetForegroundWindow` (e.g. while the
    // previous foreground app runs elevated) — leaving the bar visible but
    // never active, so clicking elsewhere deactivates the *other* window and
    // the observer never fires. This foreground watch is the safety net: while
    // the launcher is visible, every time the foreground window *moves* to
    // something else (a click on or Alt+Tab to another window), hide it. The
    // cursor check keeps the bar up when an IME candidate window briefly takes
    // the foreground while the user is still typing into the launcher.
    #[cfg(target_os = "windows")]
    let mut cached_launcher_hwnd: Option<windows_sys::Win32::Foundation::HWND> = None;
    // The foreground HWND observed on the previous tick (None until the
    // launcher has been seen visible once, so the baseline is recorded without
    // hiding a freshly-shown bar while Windows transfers the foreground).
    #[cfg(target_os = "windows")]
    let mut last_foreground_hwnd: Option<windows_sys::Win32::Foundation::HWND> = None;

    cx.spawn(async move |cx| loop {
        // A background scan may finish at any time; both event loops drain it.
        state.borrow().apply_scan_results();
        // Background icon extractions for below-the-fold results finish
        // asynchronously; apply them as they arrive.
        drain_icon_batches(&state, cx);

        while let Ok(event) = hotkey_events.try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            // `HotKey::id` is derived from the modifier/key combination, so a
            // registered hotkey can be matched to its event by id alone. Only
            // the summon hotkey is registered globally; the settings hotkey is
            // launcher-scoped and never produces a `WM_HOTKEY`.
            if state
                .borrow()
                .summon_hotkey
                .is_some_and(|hotkey| hotkey.id() == event.id)
            {
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
                MENU_SETTINGS => toggle_settings_window(&state, i18n.clone(), cx),
                MENU_QUIT => cx.update(|cx| cx.quit()),
                _ => {}
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Only re-fetch the HWND when it is unknown (first run, or the
            // window was closed and recreated); otherwise reuse the cache so
            // the idle loop never round-trips through the main thread.
            let handle = state.borrow().window;
            match handle {
                None => cached_launcher_hwnd = None,
                Some(h) if cached_launcher_hwnd.is_none() => {
                    cached_launcher_hwnd = h
                        .update(cx, |_, window, _| platform::hwnd(window))
                        .ok()
                        .flatten();
                }
                Some(_) => {}
            }
            if let Some(hwnd) = cached_launcher_hwnd {
                if platform::is_hwnd_visible(hwnd) {
                    let foreground = platform::foreground_hwnd();
                    match last_foreground_hwnd {
                        // Baseline: the launcher was just shown (or re-shown);
                        // record the current foreground without hiding so a
                        // fresh bar is not mistaken for one the user clicked
                        // away from.
                        None => last_foreground_hwnd = Some(foreground),
                        Some(previous) if previous != foreground => {
                            last_foreground_hwnd = Some(foreground);
                            // The foreground moved away from the launcher while
                            // it is still visible — the user clicked or
                            // switched to another window. The cursor guard
                            // exempts IME candidate windows, which take the
                            // foreground while the user is still typing into
                            // the launcher.
                            if foreground != hwnd && !platform::cursor_hits_window(hwnd) {
                                if let Some(handle) = state.borrow().window {
                                    let _ =
                                        handle.update(cx, |_, window, cx| hide_window(window, cx));
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
            .timer(Duration::from_millis(10))
            .await;
    })
    .detach();

    Ok(())
}

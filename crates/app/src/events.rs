//! Bridges native tray/hotkey events into the GPUI event loop and drains
//! background work (scan results, icon batches) on the foreground thread.

use std::{cell::RefCell, rc::Rc, time::Duration};

use anyhow::Result;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use gpui::{App, AsyncApp};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{menu::MenuEvent, MouseButton, MouseButtonState, TrayIconEvent};

use crate::config::{MENU_QUIT, MENU_SETTINGS};
use crate::i18n::Localization;
use crate::launcher::{LauncherState, StewardApp};
use crate::platform;
use crate::settings::toggle_settings_window;
use crate::window::{hide_window, toggle_launcher};

/// Drain plugin-host events on the foreground thread:
///
/// - a `CommandResult` for the current query generation fills the matching
///   slot and re-renders the merged list; stale generations are dropped;
/// - toasts, crashes and restarts are logged (a real toast surface lands in
///   M3 with the UI framework).
fn drain_plugin_events(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut AsyncApp,
) {
    let events = state.borrow().plugin_host.borrow_mut().drain_events();
    let mut rerender = false;
    for event in events {
        match event {
            steward_plugin_host::HostEvent::CommandResult {
                gen,
                plugin_id,
                command,
                result,
            } => {
                let current_gen = state.borrow().plugin_gen.get();
                if gen != current_gen {
                    continue;
                }
                {
                    let state_ref = state.borrow();
                    let hits = state_ref.plugin_hits.borrow();
                    let Some(index) = hits
                        .iter()
                        .position(|hit| hit.plugin_id == plugin_id && hit.command == command)
                    else {
                        continue;
                    };
                    state_ref
                        .plugin_pending
                        .borrow_mut()
                        .remove(&(plugin_id.clone(), command.clone()));
                    match result {
                        Ok(view) => {
                            state_ref.plugin_views.borrow_mut()[index] = Some(view);
                        }
                        Err(error) => {
                            eprintln!(
                                "[steward] plugin {plugin_id} command {command} failed: {} ({})",
                                error.message, error.code
                            );
                        }
                    }
                }
                rerender = true;
            }
            steward_plugin_host::HostEvent::SearchResult {
                gen,
                plugin_id,
                command,
                query: _query,
                result,
            } => {
                let current_gen = state.borrow().search_gen.get();
                if gen != current_gen {
                    continue;
                }
                let view = match result {
                    Ok(view) => view,
                    Err(error) => {
                        eprintln!(
                            "[steward] plugin {plugin_id} search failed: {} ({})",
                            error.message, error.code
                        );
                        continue;
                    }
                };
                let state_clone = state.clone();
                let plugin_id_clone = plugin_id.clone();
                let command_clone = command.clone();
                {
                    let state_ref = state.borrow();
                    state_ref
                        .plugin_search_results
                        .borrow_mut()
                        .insert((plugin_id.clone(), command.clone()), view.clone());
                }
                // If the search view is open in a detached panel, feed the
                // result there (it owns the SearchBar); otherwise the launcher
                // re-renders the inline results.
                cx.update(|cx| {
                    crate::plugin_panel_window::apply_search_result_to_panel(
                        &state_clone,
                        &plugin_id_clone,
                        &command_clone,
                        gen,
                        &view,
                        cx,
                    );
                });
                rerender = true;
            }
            steward_plugin_host::HostEvent::Toast { params } => {
                let message = params["message"].as_str().unwrap_or("");
                eprintln!("[steward] plugin toast: {message}");
                // TODO(M3): render a real toast in the launcher UI.
            }
            steward_plugin_host::HostEvent::RuntimeCrashed { plugin_id } => {
                eprintln!(
                    "[steward] plugin runtime {} crashed; restart scheduled",
                    plugin_id.as_deref().unwrap_or("(shared pool)")
                );
            }
            steward_plugin_host::HostEvent::RuntimeRestarted { plugin_id } => {
                eprintln!(
                    "[steward] plugin runtime {} restarted",
                    plugin_id.as_deref().unwrap_or("(shared pool)")
                );
            }
            steward_plugin_host::HostEvent::ItemView {
                plugin_id,
                command,
                item_id,
                view,
            } => {
                // A list item selection returned a new view (e.g. `detail`):
                // store it on the plugin slot and, for a panel-hosting view,
                // pop it into the independent window so the drill-down is
                // visible without a second confirm.
                let state_ref = state.borrow();
                let hits = state_ref.plugin_hits.borrow();
                if let Some(index) = hits
                    .iter()
                    .position(|hit| hit.plugin_id == plugin_id && hit.command == command)
                {
                    let detachable = hits[index].detachable;
                    let panel_view = crate::launcher::is_detail_or_form_view(&view);
                    if panel_view {
                        let state_clone = state.clone();
                        let i18n_clone = i18n.clone();
                        let plugin_id_clone = plugin_id.clone();
                        let command_clone = command.clone();
                        cx.update(|cx| {
                            // Replace a previously-open panel (same command) so
                            // the new drill-down view is shown instead of stale.
                            crate::plugin_panel_window::dock_panel_back(
                                &state_clone,
                                &plugin_id_clone,
                                &command_clone,
                                cx,
                            );
                            let _ = crate::plugin_panel_window::open_plugin_panel(
                                &state_clone,
                                i18n_clone,
                                plugin_id_clone,
                                command_clone,
                                view,
                                detachable,
                                cx,
                            );
                        });
                    } else {
                        state_ref.plugin_views.borrow_mut()[index] = Some(view);
                        rerender = true;
                    }
                } else {
                    eprintln!(
                        "[steward] item {item_id} returned a view for an unknown command {plugin_id}/{command}"
                    );
                }
            }
        }
    }
    if rerender {
        let Some(window) = state.borrow().window else {
            return;
        };
        let Some(app) = window.downcast::<StewardApp>() else {
            return;
        };
        let _ = app.update(cx, |app, window, cx| app.apply_plugin_views(window, cx));
    }
}

/// Drain host-side clipboard-history snapshots into the plugin host, so the
/// latest entries are injected into `command.invoke` for permitted plugins.
fn drain_clipboard_events(state: &Rc<RefCell<LauncherState>>) {
    let Some(rx) = state.borrow().clipboard_rx.borrow().clone() else {
        return;
    };
    let mut latest: Option<Vec<steward_ipc_protocol::ClipboardEntry>> = None;
    loop {
        match rx.try_recv() {
            Ok(entries) => latest = Some(entries),
            Err(crossbeam_channel::TryRecvError::Empty) => break,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                *state.borrow().clipboard_rx.borrow_mut() = None;
                break;
            }
        }
    }
    if let Some(entries) = latest {
        state
            .borrow()
            .plugin_host
            .borrow_mut()
            .set_clipboard_history(entries);
    }
}

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
    // the foreground while the user is still typing into the launcher, and
    // the pinned check keeps a pinned calendar up (mirrors the activation
    // observer in `window.rs`).
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
        // Plugin reconcile: apply newly scanned/version-changed plugins.
        state.borrow().apply_plugin_scan();
        // Plugin command responses, toasts and runtime crashes/restarts.
        drain_plugin_events(&state, i18n.clone(), cx);
        // Host-side clipboard history, forwarded to the plugin host.
        drain_clipboard_events(&state);
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
                            // the launcher. Detached plugin-view windows are
                            // independent and are never hidden here.
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

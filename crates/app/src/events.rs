//! Bridges native tray/hotkey events into the GPUI event loop and drains
//! background work (scan results, icon batches) on the foreground thread.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

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
                let message = params["message"].as_str().unwrap_or("").to_string();
                if !message.is_empty() {
                    let kind = params["kind"].as_str().unwrap_or("info").to_string();
                    let duration = params["durationMs"].as_u64().unwrap_or(3000);
                    cx.update(move |cx| crate::overlay::show_toast(cx, message, kind, duration));
                }
            }
            steward_plugin_host::HostEvent::OpenUrl { url } => {
                if let Err(error) = crate::launch::open_url(&url) {
                    eprintln!("[steward] plugin open.url failed: {error:#}");
                }
            }
            steward_plugin_host::HostEvent::OpenPath { path } => {
                if let Err(error) = crate::launch::open_path(&path) {
                    eprintln!("[steward] plugin open.path failed: {error:#}");
                }
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
            steward_plugin_host::HostEvent::ViewUpdate {
                plugin_id,
                command,
                view,
            } => {
                // A `ui` tree element handler returned a new tree. Feed an open
                // detached panel first (it owns the rendered entity); an inline
                // slot is refreshed so the launcher's own render picks it up.
                {
                    let state_clone = state.clone();
                    let plugin_id_clone = plugin_id.clone();
                    let command_clone = command.clone();
                    let view_clone = view.clone();
                    cx.update(|cx| {
                        crate::plugin_panel_window::apply_view_update_to_panel(
                            &state_clone,
                            &plugin_id_clone,
                            &command_clone,
                            &view_clone,
                            cx,
                        );
                    });
                }
                let index = {
                    let state_ref = state.borrow();
                    let hits = state_ref.plugin_hits.borrow();
                    hits.iter()
                        .position(|hit| hit.plugin_id == plugin_id && hit.command == command)
                };
                if let Some(index) = index {
                    state.borrow().plugin_views.borrow_mut()[index] = Some(view);
                    rerender = true;
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

/// Fold application-index news into the launcher.
///
/// Two things happen here on every tick: a finished background scan is applied
/// to the shared `Engine`, and the Start Menu watcher / reconcile timer decides
/// whether the next scan is due. When the scan changed the app set, the visible
/// query is re-run so an install or uninstall shows up (or disappears) without
/// the user retyping.
fn drain_app_index(state: &Rc<RefCell<LauncherState>>, cx: &mut AsyncApp) {
    let changed = state.borrow().apply_scan_results();
    let due = {
        let launcher = state.borrow_mut();
        let scanning = launcher.scan_rx.borrow().is_some();
        let mut watch = launcher.app_watch.borrow_mut();
        match watch.as_mut() {
            Some(watch) => watch.take_scan_request(scanning, Instant::now()),
            None => false,
        }
    };
    if due {
        state.borrow().spawn_app_scan();
    }
    if !changed {
        return;
    }
    let Some(window) = state.borrow().window else {
        return;
    };
    let Some(app) = window.downcast::<StewardApp>() else {
        return;
    };
    let _ = app.update(cx, |app, window, cx| {
        app.search(window, cx);
    });
}

/// Fold file-index events into the launcher.
///
/// Two different kinds of news arrive on the same poll tick: index lifecycle
/// (a build finished, a USN pass applied) and the answer to the file search the
/// launcher asked for. Both end in the same place — the visible query is re-run
/// so the drop-down matches the current index.
///
/// The re-run is gated on [`RenderSignature`](crate::launcher::RenderSignature),
/// a summary of everything a render depends on. Without that gate this function
/// ran on all ten ticks a second and each run pushed a fresh row list, which
/// reset the highlighted row: the window flickered, and the arrow keys looked
/// broken because any selection the user made was overwritten before the next
/// repaint.
fn drain_file_index(
    state: &Rc<RefCell<LauncherState>>,
    cx: &mut AsyncApp,
    i18n: &Rc<Localization>,
) {
    let changed = state.borrow_mut().file_index.poll_events();
    // The real-time watcher lost changes (its kernel buffer overflowed), so the
    // index can no longer be trusted without a reconcile: replay the journal
    // when one exists (it holds the same changes and is cheap), otherwise
    // rebuild.
    {
        let mut launcher = state.borrow_mut();
        if launcher.file_index.take_reconcile_request() {
            if launcher.file_index.supports_journal() {
                eprintln!("file index: watcher lost changes; catching up from the journal");
                launcher.file_index.request_catch_up();
            } else {
                eprintln!("file index: watcher lost changes; rebuilding the index");
                launcher.file_index.request_build();
            }
        }
    }
    // When a volume's journal can never be replayed again (it was recreated or
    // wrapped), only a full rebuild can restore that volume's records; retrying
    // the catch-up would loop forever because the cursor can never become
    // usable again.
    {
        let mut launcher = state.borrow_mut();
        if launcher.file_index.take_rebuild_request() {
            eprintln!("file index: journal cannot be replayed; rebuilding the index");
            launcher.file_index.request_build();
        }
    }
    update_tray_status(state, i18n);

    let reply = {
        let index = state.borrow();
        match index.file_index.replies.try_recv() {
            Ok(reply) => Some(reply),
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(crossbeam_channel::TryRecvError::Disconnected) => None,
        }
    };
    let new_hits = match reply {
        Some(reply) => {
            let mut index = state.borrow_mut();
            let hits_changed = index.file_hits != reply.hits;
            // Late replies for a superseded query are dropped by generation.
            index.file_hits = reply.hits;
            index.file_hits_generation = reply.generation;
            index.file_search_ms = reply.elapsed.as_millis();
            hits_changed
        }
        None => false,
    };

    if !changed && !new_hits {
        return;
    }

    let Some(window) = state.borrow().window else {
        return;
    };
    let Some(app) = window.downcast::<StewardApp>() else {
        return;
    };
    let _ = app.update(cx, |app, window, cx| {
        // Nothing a render would show has changed: leave the rows — and the
        // selection the user is moving — exactly as they are.
        let before = crate::launcher::RenderSignature::capture(app);
        if *app.rendered.borrow() == before {
            return;
        }
        // `search` re-runs the app and plugin halves too, which are cheap; the
        // file half is served from the hits just stored, so this cannot loop.
        app.search(window, cx);
        *app.rendered.borrow_mut() = crate::launcher::RenderSignature::capture(app);
    });
}

/// Report the file index in the tray's status line.
///
/// A full-volume build is the one long-running thing the app does, and the only
/// place it can be watched from is the tray: the launcher bar is hidden most of
/// the time, and the build's own log goes to stderr, which a normal launch has no
/// console for.
fn update_tray_status(state: &Rc<RefCell<LauncherState>>, i18n: &Rc<Localization>) {
    // The shared handle is taken out first: the `Rc` clone (and the borrow that
    // follows) keep the item alive independently of the launcher state, so the
    // status can be read and the item updated without overlapping borrows.
    let handle = state.borrow().tray_status.clone();
    let borrowed = handle.borrow();
    let Some(item) = borrowed.as_ref() else {
        return;
    };
    let (building, ready, records) = {
        let state = state.borrow();
        (
            state.file_index.is_building(),
            state.file_index.is_ready(),
            state.file_index.records,
        )
    };
    let status = if building {
        crate::tray::TrayStatus::Indexing
    } else if ready {
        crate::tray::TrayStatus::Indexed(records)
    } else {
        crate::tray::TrayStatus::Idle
    };
    item.update(status, i18n);
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

    // How often the USN journal is replayed across all volumes. The real-time
    // watcher handles most changes first; this is the safety net for changes
    // made while Steward was not running (or while a watcher root was
    // unreadable), and it is cheap because only records after each stored
    // cursor are read.
    const LIVE_INDEX_REFRESH: Duration = Duration::from_secs(2);
    let mut last_live_tick = Instant::now();

    cx.spawn(async move |cx| loop {
        // Application-index news: a finished scan (applied to the shared index
        // and, when the set changed, re-run through the visible query) plus the
        // timing that starts the next scan.
        drain_app_index(&state, cx);
        // Plugin reconcile: apply newly scanned/version-changed plugins.
        state.borrow().apply_plugin_scan();
        // Plugin command responses, toasts and runtime crashes/restarts.
        drain_plugin_events(&state, i18n.clone(), cx);
        // Host-side clipboard history, forwarded to the plugin host.
        drain_clipboard_events(&state);
        // Background icon extractions for below-the-fold results finish
        // asynchronously; apply them as they arrive.
        drain_icon_batches(&state, cx);
        // File index: build progress, finished builds, USN catch-up, and the
        // answers to file searches. A finished build or a fresh set of file hits
        // re-runs the visible query so the drop-down reflects them.
        drain_file_index(&state, cx, &i18n);
        // Live maintenance: replay each volume's USN journal on a short timer so
        // changes made while Steward was closed (or while a watcher root was
        // unreadable) reach the index without a restart.
        if last_live_tick.elapsed() >= LIVE_INDEX_REFRESH {
            last_live_tick = Instant::now();
            let mut launcher = state.borrow_mut();
            // Only a `$MFT`-built index has journals to replay; a walk-built
            // one is maintained solely by the real-time watcher after its
            // startup rebuild.
            if launcher.file_index.is_ready() && launcher.file_index.supports_journal() {
                launcher.file_index.request_catch_up();
            }
        }
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

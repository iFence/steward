//! Launcher window lifecycle: bootstrap, opening the bar, and showing/hiding
//! it on summon.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    prelude::*, px, size, AnyWindowHandle, App, AsyncApp, Bounds, ClipboardItem, FocusHandle,
    KeyBinding, QuitMode, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions,
};
use steward_ui_components::{
    init_components, CalendarSelectCallback, CalendarView, PinToggleCallback, ResultItem,
    ResultList, ResultListDelegate,
};

use crate::config::{
    HideWindow, CLOSE_ON_HIDE, DEFAULT_ACCENT, LAUNCHER_HEIGHT, LAUNCHER_WIDTH, THEME_COLOR_SETTING,
};
use crate::i18n::Localization;
use crate::launch::launch;
use crate::launcher::{LauncherState, StewardApp};
use crate::platform;
use crate::search_input::SearchInput;
use crate::theme::{adaptive_scrim_alpha, apply_steward_theme, parse_hex_color};

/// Common GPUI bootstrap shared by every startup path: key bindings,
/// gpui-component init, theme, focus handle and the launcher-window close
/// subscription.
pub(crate) fn init_ui_common(cx: &mut App, state: &Rc<RefCell<LauncherState>>) -> FocusHandle {
    // The tray icon is the application shell: closing the launcher window
    // (Esc, activation loss, Alt+F4) must not terminate the process. GPUI
    // defaults to quitting when the last window closes on non-macOS.
    cx.set_quit_mode(QuitMode::Explicit);

    cx.bind_keys([KeyBinding::new("escape", HideWindow, None)]);

    // Initialize the gpui-component stack (theme, global state, root,
    // popover/menu layers, ...) before the first UI element renders; the
    // settings window's `Settings` widget depends on it.
    init_components(cx);

    // Match the component surfaces (settings window, ...) to the launcher's
    // palette and restore the persisted theme color.
    let accent = state
        .borrow()
        .storage
        .borrow()
        .get_setting(THEME_COLOR_SETTING)
        .and_then(|value| parse_hex_color(&value))
        .unwrap_or(DEFAULT_ACCENT);
    apply_steward_theme(cx, accent);

    let focus = cx.focus_handle();
    state.borrow_mut().focus = Some(focus.clone());

    // A background scan may have finished while the app was starting; apply it
    // before the first window opens.
    state.borrow().apply_scan_results();

    // Closing the launcher (e.g. Alt+F4) must not kill the app: the tray icon
    // is the application shell, and the window is reopened on demand. Filtered
    // by window id so settings-window close events don't clear the launcher
    // handle.
    let closed_state = state.clone();
    cx.on_window_closed(move |cx, window_id| {
        // A detached plugin-view window was closed (Esc / close button / test):
        // drop it from the registry and re-render the launcher so the view
        // reappears inline.
        crate::plugin_panel_window::panel_window_closed(&closed_state, window_id, cx);
        let launcher_id = closed_state.borrow().window.as_ref().map(|h| h.window_id());
        if launcher_id == Some(window_id) {
            closed_state.borrow_mut().window = None;
        }
    })
    .detach();

    focus
}

pub(crate) fn open_launcher_window(
    cx: &mut App,
    focus: &FocusHandle,
    i18n: Rc<Localization>,
    state: &Rc<RefCell<LauncherState>>,
) -> AnyWindowHandle {
    // Frosted-glass backdrop: the launcher composits a dark scrim (see
    // `render`; base `palette::SCRIM_ALPHA`, raised adaptively over bright
    // backdrops) over a blurred window background —
    // Windows Acrylic, macOS vibrancy. The launcher used to paint a
    // translucent tint over the Windows Mica / macOS vibrancy backdrop without
    // any blur, which followed the OS theme: in light mode the Mica turned
    // light gray, the light backdrop bled through as white edges, and the
    // white query text lost all contrast. Blurring the backdrop and keeping
    // our own fixed dark tint (instead of letting the OS theme drive it)
    // preserves the identical dark look in both system modes.
    let window_background = WindowBackgroundAppearance::Blurred;

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
            window_background,
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title("Steward");

            // The shared index was seeded in `ensure_app_index` at boot and is
            // refreshed in the background; the window only takes a handle to
            // it. Mirror the current entries into the delegate so Enter can
            // resolve a row to its action.
            let storage = state.borrow().storage.clone();
            let engine = state.borrow().engine.clone();
            let last_results = Rc::new(RefCell::new(
                engine
                    .borrow()
                    .entries()
                    .iter()
                    .cloned()
                    .map(ResultItem::App)
                    .collect::<Vec<_>>(),
            ));
            let results_for_cb = last_results.clone();
            let confirm_storage = storage.clone();
            let plugin_host = state.borrow().plugin_host.clone();
            let calendar_state = state.clone();
            let on_calendar_select: CalendarSelectCallback = Rc::new(move |date: String, _cx| {
                // Clicking a day confirms it: dispatch `item.invoke` to the
                // plugin that produced the calendar view and keep the launcher
                // open.
                let active = calendar_state.borrow().plugin_calendar.borrow().clone();
                if let Some(active) = active {
                    if calendar_state
                        .borrow()
                        .plugin_host
                        .borrow_mut()
                        .invoke_item(&active.plugin_id, &date)
                        .is_none()
                    {
                        eprintln!(
                            "[steward] plugin {} not ready for item.invoke",
                            active.plugin_id
                        );
                    }
                }
            });
            // Pin/detach the calendar: in the inline grid, the (unpinned)
            // control flips the shared state to "popped out" and opens the
            // independent window. A direct callback (same pattern as day
            // clicks) instead of an action dispatch: `App::dispatch_action`
            // routes through the platform's active window, which is unreliable
            // for a PopUp launcher.
            let pin_state = state.clone();
            let pin_i18n = i18n.clone();
            let on_toggle_pin: PinToggleCallback = Rc::new(move |pinned: bool, cx: &mut App| {
                if pinned {
                    crate::plugin_panel_window::open_plugin_panel_window(
                        &pin_state,
                        pin_i18n.clone(),
                        cx,
                    );
                }
            });
            let confirm_state = state.clone();
            let on_confirm = move |index: usize, cx: &mut App| -> bool {
                let item = results_for_cb.borrow().get(index).cloned();
                match item {
                    // An application: launch it and bump its usage frequency.
                    Some(ResultItem::App(app)) => {
                        let _ = confirm_storage.borrow().upsert_usage(&app.path);
                        if let Err(error) = launch(&app.path) {
                            eprintln!("failed to launch {}: {error:#}", app.path.display());
                        }
                        true
                    }
                    // A calculator result: put the answer on the clipboard. The
                    // window is hidden afterwards by `after_confirm` (Enter) or
                    // by the activation observer once focus is lost (click).
                    Some(ResultItem::Action { title, .. }) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(title));
                        true
                    }
                    // A link: open the URL in the default browser.
                    Some(ResultItem::Link { url, .. }) => {
                        if let Err(error) = crate::launch::open_url(&url) {
                            eprintln!("failed to open {url}: {error:#}");
                        }
                        true
                    }
                    // A plugin row: dispatch `item.invoke` and keep the
                    // launcher open so the plugin's toast/result is visible.
                    Some(ResultItem::Plugin {
                        plugin_id, item_id, ..
                    }) => {
                        if plugin_host
                            .borrow_mut()
                            .invoke_item(&plugin_id, &item_id)
                            .is_none()
                        {
                            eprintln!("[steward] plugin {plugin_id} is not ready for item.invoke");
                        }
                        false
                    }
                    // A calendar command row: reveal the stored calendar view
                    // (the grid replaces the results list) without leaving the
                    // launcher. The view must already have landed; a query
                    // change hides the grid again via `render_merged`.
                    Some(ResultItem::Calendar {
                        plugin_id, command, ..
                    }) => {
                        let (hit, view) = {
                            let state = confirm_state.borrow();
                            let hits = state.plugin_hits.borrow();
                            let views = state.plugin_views.borrow();
                            let Some((hit, view)) =
                                hits.iter().zip(views.iter()).find(|(hit, _)| {
                                    hit.plugin_id == plugin_id && hit.command == command
                                })
                            else {
                                return false;
                            };
                            (hit.clone(), view.clone())
                        };
                        let Some(view) = view else {
                            return false;
                        };
                        // If the calendar is already popped into its own
                        // window, just bring that window forward instead of
                        // re-populating the launcher grid.
                        if confirm_state
                            .borrow()
                            .plugin_window_open(&plugin_id, &command)
                        {
                            crate::plugin_panel_window::focus_plugin_panel_window(
                                &confirm_state,
                                &plugin_id,
                                &command,
                                cx,
                            );
                            return false;
                        }
                        if let Some(active) = crate::launcher::parse_calendar_view(
                            &view,
                            &plugin_id,
                            &hit.command,
                            hit.detachable,
                        ) {
                            *confirm_state.borrow_mut().plugin_calendar.borrow_mut() = Some(active);
                            // Reveal the grid after the current key-event
                            // dispatch completes: while an event is being
                            // handled the window is taken out of the app's
                            // window registry, so updating it synchronously
                            // fails with "window not found". Deferring
                            // mirrors how GPUI dispatches window actions.
                            let confirm_state = confirm_state.clone();
                            cx.defer(move |cx| {
                                // Copy the handle out so the borrow on the
                                // shared state ends before `app.update`
                                // re-enters it (a `RefCell` double borrow
                                // would panic).
                                let handle = confirm_state.borrow().window;
                                if let Some(handle) = handle {
                                    if let Some(app) = handle.downcast::<StewardApp>() {
                                        let _ = app.update(cx, |app, window, cx| {
                                            app.apply_plugin_views(window, cx)
                                        });
                                    }
                                }
                            });
                        }
                        false
                    }
                    None => false,
                }
            };
            let delegate = ResultListDelegate::new()
                .type_label(i18n.translate("application"))
                .on_confirm(on_confirm);

            cx.new(|cx| {
                focus.focus(window, cx);
                // Dismiss the launcher whenever another application takes
                // activation (e.g. the user clicks another window). Detached
                // plugin-view windows live in their own windows and are not
                // affected by the launcher's activation.
                let activation_subscription =
                    cx.observe_window_activation(window, move |_, window, cx| {
                        if !window.is_window_active() {
                            hide_window(window, cx);
                        }
                    });
                let results = ResultList::new(delegate, window, cx);
                let calendar =
                    CalendarView::new(Some(on_calendar_select), Some(on_toggle_pin), window, cx);
                let mut app = StewardApp {
                    focus_handle: focus.clone(),
                    input: SearchInput {
                        query: String::new(),
                        cursor: 0,
                        marked: None,
                        selection: None,
                    },
                    i18n,
                    engine,
                    storage: storage.clone(),
                    last_results,
                    results,
                    calendar,
                    calendar_selected: String::new(),
                    base_items: Vec::new(),
                    base_icons: Vec::new(),
                    builtin_count: 0,
                    state: state.clone(),
                    detachable_list_target: None,
                    _activation_subscription: activation_subscription,
                    mouse_selecting: false,
                    mouse_anchor: 0,
                };
                // Seed the first summon with the most-used applications (an
                // empty query sorts by usage frequency), so the launcher opens
                // with recents instead of an empty bar.
                app.search(window, cx);
                app
            })
        },
    )
    .expect("failed to open the launcher window")
    .into()
}

/// Summon or dismiss the launcher bar. Reopens the window if it was closed.
pub(crate) fn toggle_launcher(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut AsyncApp,
) {
    let mut state_ref = state.borrow_mut();
    match state_ref.window {
        Some(handle) => {
            let focus = state_ref
                .focus
                .clone()
                .expect("focus is initialized together with GPUI");
            let height = state_ref.height();
            state_ref.last_applied_height = height;
            // Drop the borrow before `handle.update`: the closure re-enters the
            // shared state through `show_window`, which adapts the scrim.
            drop(state_ref);
            let _ = handle.update(cx, |_, window, cx| {
                if platform::is_visible(window) {
                    hide_window(window, cx);
                } else {
                    // Re-apply the height so a freshly-created window matches
                    // the current result count (mirrors live sizing on search).
                    platform::resize(window, height);
                    focus.focus(window, cx);
                    show_window(window, cx, state);
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
pub(crate) fn show_launcher(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut AsyncApp,
) {
    if state.borrow().window.is_none() {
        let focus = state
            .borrow()
            .focus
            .clone()
            .expect("focus is initialized together with GPUI");
        let handle = cx.update(|cx| open_launcher_window(cx, &focus, i18n.clone(), state));
        state.borrow_mut().window = Some(handle);
    }
    let mut state_ref = state.borrow_mut();
    if let Some(handle) = state_ref.window {
        let focus = state_ref
            .focus
            .clone()
            .expect("focus is initialized together with GPUI");
        let height = state_ref.height();
        state_ref.last_applied_height = height;
        drop(state_ref);
        let _ = handle.update(cx, |_, window, cx| {
            platform::resize(window, height);
            focus.focus(window, cx);
            show_window(window, cx, state);
        });
    }
}

pub(crate) fn hide_window(window: &mut Window, _cx: &mut App) {
    #[cfg(target_os = "windows")]
    if CLOSE_ON_HIDE {
        // Remove the window entirely instead of hiding it: the per-window
        // renderer, swapchain and GPU atlas are reclaimed, and the next summon
        // recreates the window from the shared engine (no re-scan). The shared
        // handle is cleared by the `on_window_closed` subscription.
        window.remove_window();
        return;
    }
    platform::hide(window);
    #[cfg(not(target_os = "windows"))]
    _cx.hide();
}

fn show_window(window: &mut Window, _cx: &mut App, state: &Rc<RefCell<LauncherState>>) {
    #[cfg(not(target_os = "windows"))]
    _cx.activate(true);
    // Adapt the scrim to the backdrop while the window is still hidden (the
    // sample runs inside `platform::show`, before `ShowWindow`): over a bright
    // backdrop the bar darkens toward SCRIM_ALPHA_MAX so the white ink keeps
    // its contrast, while over a dark desktop it stays at the frosted-glass
    // SCRIM_ALPHA. The next paint picks up the new value.
    let height = state.borrow().height();
    if let Some(brightness) = platform::show(window, height) {
        state.borrow_mut().scrim_alpha = adaptive_scrim_alpha(brightness);
    }
    window.refresh();
}

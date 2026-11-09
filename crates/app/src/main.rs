//! Steward desktop entry point with an NSH Rust launcher shell.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#![allow(dead_code)]

mod app_icons;
mod autostart;
mod clipboard_history;
mod config;
mod events;
mod handlers;
mod hotkeys;
mod i18n;
mod launch;
mod logging;
mod nsh_surface;
mod platform;
mod plugin_window;
mod search;
mod services;
mod settings;
mod shell;
mod state;
mod theme;
mod tray;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui::App;
use gpui_platform::application;
use steward_storage::Storage;
use tracing::{error, info, warn};

use crate::config::{DEFAULT_ACCENT, LANGUAGE_SETTING, THEME_COLOR_SETTING};
use crate::i18n::Localization;
use crate::services::{AppServices, SearchState, UiSettings};
use crate::state::AppState;

fn main() {
    if std::env::args().any(|arg| arg == "--sync-autostart") {
        autostart::sync_autostart_path();
        return;
    }
    if std::env::args().any(|arg| arg == "--unregister-autostart") {
        autostart::unregister_autostart();
        return;
    }

    let data_dir = std::env::var_os("STEWARD_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("no OS data directory available")
                .join("Steward")
        });
    let log_guard = match logging::init(&data_dir) {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("failed to initialize file logging: {error:#}");
            None
        }
    };
    info!(data_dir = %data_dir.display(), "starting Steward");
    #[cfg(target_os = "windows")]
    platform::set_dpi_awareness();
    #[cfg(target_os = "windows")]
    platform::enable_dark_mode();
    autostart::repair_autostart_path();
    let storage = Rc::new(RefCell::new(
        Storage::open().expect("failed to open the Steward storage database"),
    ));
    if std::env::args().any(|arg| arg == "--print-summon-hotkey") {
        println!("{}", hotkeys::persisted_summon_hotkey(&storage.borrow()));
        return;
    }
    let language = storage.borrow().get_setting(LANGUAGE_SETTING);
    let i18n = Rc::new(
        Localization::new_with_language(language.as_deref())
            .expect("failed to initialize localization"),
    );
    let locale = language.clone().unwrap_or_else(|| i18n.language());
    let ui_strings = i18n.ui_strings();
    let accent = storage
        .borrow()
        .get_setting(THEME_COLOR_SETTING)
        .unwrap_or_else(|| format!("#{DEFAULT_ACCENT:06x}"));
    let autostart = autostart::autostart_enabled();
    let summon_hotkey = hotkeys::persisted_summon_hotkey(&storage.borrow()).to_string();

    let search = Rc::new(RefCell::new(
        SearchState::new(
            data_dir.clone(),
            storage.clone(),
            ui_strings.clone(),
            locale.clone(),
        )
        .expect("failed to initialize launcher search"),
    ));
    load_trust_roots(&search);
    search.borrow_mut().start();
    let search_revision = search.borrow().revision_handle();

    let (request_tx, request_rx) = crossbeam_channel::unbounded();
    let (command_tx, command_rx) = crossbeam_channel::unbounded();
    let (plugin_event_tx, plugin_event_rx) = crossbeam_channel::unbounded();
    let services = Arc::new(AppServices::new(
        request_tx,
        command_tx,
        UiSettings::new(locale, ui_strings, accent, autostart, summon_hotkey),
        search_revision,
    ));
    let (clipboard_tx, clipboard_rx) = crossbeam_channel::unbounded();
    let clipboard_watcher =
        clipboard_history::ClipboardWatcher::spawn(data_dir.clone(), clipboard_tx);
    let clipboard_entries = Arc::new(Mutex::new(Vec::new()));

    let state = Rc::new(RefCell::new(AppState {
        data_dir: data_dir.clone(),
        services: services.clone(),
        window: None,
        launcher_height: config::LAUNCHER_HEIGHT,
        settings_window: None,
        storage,
        i18n: i18n.clone(),
        search,
        requests: request_rx,
        commands: command_rx,
        clipboard_entries,
        clipboard_rx: RefCell::new(Some(clipboard_rx)),
        tray: RefCell::new(None),
        hotkey_manager: None,
        summon_hotkey: None,
        plugin_windows: Default::default(),
        plugin_events: plugin_event_tx,
        plugin_events_rx: plugin_event_rx,
        _clipboard_watcher: clipboard_watcher,
        _log_guard: log_guard,
    }));

    application().with_assets(gpui_kit::assets::Assets).run({
        let state = state.clone();
        let i18n = i18n.clone();
        let services = services.clone();
        move |cx: &mut App| {
            settings::init_ui(cx, &services.accent());
            window::init_common(cx);
            nsh_platform_gpui::set_ui_sets(vec![nsh_base::registry()]);
            let handle =
                window::open_launcher_window(cx, state.clone(), services.clone(), data_dir.clone());
            state.borrow_mut().window = Some(handle);
            if std::env::var_os("STEWARD_SHOW_SETTINGS").is_some() {
                window::open_settings_window(cx, state.clone(), services.clone(), data_dir.clone());
            }
            if std::env::var_os("STEWARD_SHOW_ON_START").is_some() {
                let _ = handle.update(cx, |_, window, cx| {
                    window::show_launcher(window, config::LAUNCHER_HEIGHT, &services);
                    cx.activate(true);
                });
            }

            match tray::setup_tray(&i18n) {
                Ok(tray) => *state.borrow().tray.borrow_mut() = Some(tray),
                Err(error) => error!(error = %format!("{error:#}"), "failed to create tray icon"),
            }
            if let Err(error) = hotkeys::setup(&state) {
                error!(error = %format!("{error:#}"), "failed to setup summon hotkey");
            }
            if let Err(error) = events::spawn(state.clone(), cx) {
                error!(error = %format!("{error:#}"), "failed to start event loop");
            }
            if let Ok(plugin_id) = std::env::var("STEWARD_SMOKE_PLUGIN") {
                let command =
                    std::env::var("STEWARD_SMOKE_COMMAND").unwrap_or_else(|_| plugin_id.clone());
                let title =
                    std::env::var("STEWARD_SMOKE_TITLE").unwrap_or_else(|_| command.clone());
                state.borrow().services.send_window_command(
                    crate::services::WindowCommand::Activate(
                        crate::services::RowAction::OpenPlugin {
                            plugin_id,
                            command,
                            title,
                            input: String::new(),
                        },
                    ),
                );
            }

            let closed_state = state.clone();
            cx.on_window_closed(move |_, window_id| {
                if closed_state
                    .borrow()
                    .window
                    .is_some_and(|handle| handle.window_id() == window_id)
                {
                    closed_state.borrow_mut().window = None;
                }
                if closed_state
                    .borrow()
                    .settings_window
                    .is_some_and(|handle| handle.window_id() == window_id)
                {
                    closed_state.borrow_mut().settings_window = None;
                }
                closed_state
                    .borrow_mut()
                    .plugin_windows
                    .retain(|_, window| window.handle.window_id() != window_id);
            })
            .detach();
        }
    });
}

fn load_trust_roots(search: &Rc<RefCell<SearchState>>) {
    let mut roots =
        serde_json::from_str::<Vec<String>>(include_str!("../../../assets/nsh/trust-roots.json"))
            .expect("assets/nsh/trust-roots.json must be a JSON string array");
    if let Ok(root) = std::env::var("STEWARD_NSH_TRUST_ROOT") {
        roots.push(root);
    }
    for root in roots {
        if let Err(error) = search.borrow_mut().add_trust_root(root.trim()) {
            warn!(error = %format!("{error:#}"), "ignoring invalid NSH trust root");
        }
    }
}

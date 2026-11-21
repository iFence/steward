//! Steward desktop entry point with an NSH Rust launcher shell.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app_icons;
mod autostart;
mod clipboard_history;
mod config;
mod diagnostics;
mod events;
mod handlers;
mod holiday_schedule;
mod hotkey_registry;
mod hotkeys;
mod i18n;
mod icons;
mod instance;
mod launch;
mod launcher_model;
mod logging;
mod metrics;
mod nsh_surface;
mod platform;
mod plugin_containment;
mod plugin_execution;
mod plugin_filesystem;
mod plugin_management;
mod plugin_network;
mod plugin_runtime;
mod plugin_system;
mod plugin_window;
mod search;
mod services;
mod settings;
#[allow(dead_code)] // Backup metadata is registered now and consumed by the backup feature.
mod settings_registry;
mod shell;
mod state;
mod theme;
mod tray;
mod updates;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::App;
use gpui_platform::application;
use steward_storage::Storage;
use tracing::{error, info, warn};

use crate::config::{CHECK_UPDATES_SETTING, DEFAULT_ACCENT, LANGUAGE_SETTING, THEME_COLOR_SETTING};
use crate::i18n::Localization;
use crate::services::{AppServices, SearchState, UiSettings};
use crate::state::AppState;

fn main() {
    let process_started = std::time::Instant::now();
    if plugin_window::is_plugin_host_process() {
        plugin_window::run_plugin_host();
        return;
    }
    let data_dir = std::env::var_os("STEWARD_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("no OS data directory available")
                .join("Steward")
        });
    if let Some(position) = std::env::args().position(|arg| arg == "--write-diagnostics") {
        let output = std::env::args()
            .nth(position + 1)
            .filter(|arg| !arg.starts_with("--"))
            .map(PathBuf::from);
        match diagnostics::write_report(&data_dir, output.as_deref()) {
            Ok(path) => println!("{}", path.display()),
            Err(error) => {
                eprintln!("failed to write Steward diagnostics: {error:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--sync-autostart") {
        autostart::sync_autostart_path();
        return;
    }
    if std::env::args().any(|arg| arg == "--unregister-autostart") {
        autostart::unregister_autostart();
        return;
    }

    // A secondary GUI launch only asks the existing process to summon the
    // launcher. Utility modes above remain independent for installer and
    // headless tooling.
    let instance = if std::env::args().any(|arg| arg == "--print-summon-hotkey") {
        None
    } else {
        match instance::claim() {
            Ok(instance::InstanceClaim::Primary(guard)) => Some(guard),
            Ok(instance::InstanceClaim::Secondary) => return,
            Err(error) => {
                eprintln!("failed to claim the Steward single-instance lock: {error}");
                return;
            }
        }
    };

    let log_guard = match logging::init(&data_dir) {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("failed to initialize file logging: {error:#}");
            None
        }
    };
    metrics::init();
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
    let check_updates_on_startup = storage
        .borrow()
        .get_setting(CHECK_UPDATES_SETTING)
        .map(|value| value != "false")
        .unwrap_or(true);

    let search = Rc::new(RefCell::new(
        SearchState::new(
            data_dir.clone(),
            storage.clone(),
            ui_strings.clone(),
            locale.clone(),
        )
        .expect("failed to initialize launcher search"),
    ));
    let trust_roots = load_trust_roots(&search);
    search.borrow_mut().start();
    let search_revision = search.borrow().revision_handle();

    let (request_tx, request_rx) = crossbeam_channel::unbounded();
    let (command_tx, command_rx) = crossbeam_channel::unbounded();
    let (plugin_event_tx, plugin_event_rx) = crossbeam_channel::unbounded();
    let services = Arc::new(AppServices::new(
        request_tx,
        command_tx,
        UiSettings::new(
            locale,
            ui_strings,
            accent,
            autostart,
            summon_hotkey,
            check_updates_on_startup,
        ),
        search_revision,
    ));
    let plugins_root = std::env::var_os("STEWARD_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("plugins"));
    let plugin_store = steward_plugin_store::PluginStore::open(
        steward_plugin_store::PluginStoreConfig::new(data_dir.clone(), plugins_root, trust_roots),
    )
    .expect("failed to initialize local plugin store");
    services.set_plugin_management(plugin_management::PluginManagementPort::spawn(plugin_store));
    if services.check_updates_on_startup() {
        services.update_port().check_for_updates(false);
    }
    let clipboard_watcher = clipboard_history::ClipboardWatcher::spawn(data_dir.clone());

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
        tray: RefCell::new(None),
        hotkey_manager: None,
        summon_hotkey: None,
        hotkey_registry: crate::hotkey_registry::HotkeyRegistry::default(),
        plugin_windows: Default::default(),
        plugin_events: plugin_event_tx,
        plugin_events_rx: plugin_event_rx,
        last_update_revision: 0,
        last_plugin_management_revision: 0,
        _clipboard_watcher: clipboard_watcher,
        _log_guard: log_guard,
        _instance: instance,
    }));

    application().with_assets(gpui_kit::assets::AllAssets).run({
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
                Ok(tray) => {
                    *state.borrow().tray.borrow_mut() = Some(tray);
                    metrics::value("tray_lifecycle", 1, serde_json::json!({ "phase": "ready" }));
                }
                Err(error) => {
                    metrics::value(
                        "component_failure",
                        1,
                        serde_json::json!({ "component": "tray" }),
                    );
                    error!(error = %format!("{error:#}"), "failed to create tray icon");
                }
            }
            match hotkeys::setup(&state) {
                Ok(()) => metrics::value(
                    "hotkey_lifecycle",
                    1,
                    serde_json::json!({ "phase": "ready" }),
                ),
                Err(error) => {
                    metrics::value(
                        "component_failure",
                        1,
                        serde_json::json!({ "component": "summon_hotkey" }),
                    );
                    error!(error = %format!("{error:#}"), "failed to setup summon hotkey");
                }
            }
            if let Err(error) = events::spawn(state.clone(), cx) {
                error!(error = %format!("{error:#}"), "failed to start event loop");
            }
            metrics::duration(
                "cold_start",
                process_started.elapsed(),
                serde_json::json!({
                    "cold": std::env::var_os("STEWARD_METRICS_COLD").is_none(),
                }),
            );
            metrics::value(
                "hidden_plugin_vm_count",
                0,
                serde_json::json!({ "phase": "tray_ready" }),
            );
            metrics::value(
                "plugin_host_process_count",
                state.borrow().plugin_windows.values().count() as u64,
                serde_json::json!({ "phase": "tray_ready" }),
            );
            let smoke_requests = smoke_plugin_requests();
            for request in &smoke_requests {
                state.borrow().services.send_window_command(
                    crate::services::WindowCommand::Activate(smoke_plugin_action(request)),
                );
            }
            if !smoke_requests.is_empty() && std::env::var_os("STEWARD_SMOKE_REPEAT").is_some() {
                let services = services.clone();
                let delay = std::env::var("STEWARD_SMOKE_REPEAT_DELAY_MS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(2_000);
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    for request in smoke_requests {
                        services.send_window_command(crate::services::WindowCommand::Activate(
                            smoke_plugin_action(&request),
                        ));
                    }
                });
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
            })
            .detach();
        }
    });
}

fn load_trust_roots(search: &Rc<RefCell<SearchState>>) -> Vec<String> {
    let mut roots =
        serde_json::from_str::<Vec<String>>(include_str!("../../../assets/nsh/trust-roots.json"))
            .expect("assets/nsh/trust-roots.json must be a JSON string array");
    if let Ok(root) = std::env::var("STEWARD_NSH_TRUST_ROOT") {
        roots.push(root);
    }
    roots
        .into_iter()
        .filter_map(|root| {
            let root = root.trim().to_owned();
            match search.borrow_mut().add_trust_root(&root) {
                Ok(()) => Some(root),
                Err(error) => {
                    warn!(error = %format!("{error:#}"), "ignoring invalid NSH trust root");
                    None
                }
            }
        })
        .collect()
}

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SmokePluginRequest {
    plugin_id: String,
    command: String,
    title: String,
}

fn smoke_plugin_requests() -> Vec<SmokePluginRequest> {
    if let Ok(value) = std::env::var("STEWARD_SMOKE_PLUGINS_JSON") {
        match serde_json::from_str(&value) {
            Ok(requests) => return requests,
            Err(error) => {
                error!(error = %error, "parse STEWARD_SMOKE_PLUGINS_JSON");
                return Vec::new();
            }
        }
    }

    let Ok(plugin_id) = std::env::var("STEWARD_SMOKE_PLUGIN") else {
        return Vec::new();
    };
    let command = std::env::var("STEWARD_SMOKE_COMMAND").unwrap_or_else(|_| plugin_id.clone());
    let title = std::env::var("STEWARD_SMOKE_TITLE").unwrap_or_else(|_| command.clone());
    vec![SmokePluginRequest {
        plugin_id,
        command,
        title,
    }]
}

fn smoke_plugin_action(request: &SmokePluginRequest) -> crate::services::RowAction {
    crate::services::RowAction::OpenPlugin {
        plugin_id: request.plugin_id.clone(),
        command: request.command.clone(),
        title: request.title.clone(),
        input: String::new(),
    }
}

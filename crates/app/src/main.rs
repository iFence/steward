//! Steward desktop entry point.
//!
//! Startup is silent: the app registers a system tray icon (Windows/macOS)
//! and one global hotkey — summon (`Ctrl+Alt+Space`), changeable in Settings —
//! but opens no window. The launcher bar — a wide, short, borderless popup
//! centered on the primary display — is summoned on demand via the hotkey or
//! the tray icon, and hidden again with `Esc` (or the hotkey), or
//! automatically when another application takes activation (clicking away).
//! The settings hotkey (`Ctrl+,`) is launcher-scoped: it is not a global
//! binding, so it only opens the settings window while the launcher is visible.
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
//!
//! This file is deliberately thin: every concern that used to live here was
//! split into focused modules (`config`, `theme`, `launcher`, `window`,
//! `settings`, `hotkeys`, `tray`, `events`, `platform`, ...), leaving only the
//! boot sequence, the module wiring, and the two leaf modules (`i18n`,
//! `app_icons`) that are too small to move.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod autostart;
mod clipboard_history;
mod config;
mod events;
mod hotkeys;
mod i18n;
mod launch;
mod launcher;
mod platform;
mod plugin_panel_window;
mod search_input;
mod settings;
mod theme;
mod window;

#[cfg(target_os = "windows")]
mod app_icons;

#[cfg(not(target_os = "windows"))]
mod app_icons {
    /// Icon extraction is Windows-only for M1; other platforms render rows
    /// without icons until a scanner provides them.
    pub fn app_icon_image(_path: &std::path::Path) -> Option<std::sync::Arc<gpui::Image>> {
        None
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod tray;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use global_hotkey::hotkey::HotKey;
use gpui::App;
use gpui_platform::application;
use steward_core_engine::Engine;
use steward_plugin_host::{HostConfig, PluginHost};
use steward_plugin_registry::Registry;

use crate::config::{LANGUAGE_SETTING, SUMMON_HOTKEY_SETTING};
use crate::events::spawn_event_poll_task;
use crate::hotkeys::{setup_global_hotkey, HotkeyField};
use crate::launcher::LauncherState;
use crate::window::{init_ui_common, open_launcher_window};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use crate::tray::setup_tray;

fn main() {
    // Headless query for the effective summon hotkey, used by
    // `scripts/bench-resident.ps1` to inject a matching synthetic WM_HOTKEY
    // (the id is derived from the modifier/key combo, so a mismatched binding
    // is silently ignored by the app). Prints the persisted value, or the
    // built-in default when nothing is stored, then exits before GPUI starts.
    if std::env::args().any(|arg| arg == "--print-summon-hotkey") {
        let storage =
            steward_storage::Storage::open().expect("failed to open the Steward storage database");
        let hotkey = storage
            .get_setting(SUMMON_HOTKEY_SETTING)
            .and_then(|value| value.parse::<HotKey>().ok())
            .unwrap_or_else(|| HotkeyField::Summon.default_hotkey());
        println!("{hotkey}");
        return;
    }

    // Opt into per-monitor DPI awareness before any window exists so GPUI and
    // the launcher size calculations see the real display scale (e.g. 2.0 at
    // 200%) instead of the 96-DPI virtualization the OS applies otherwise.
    #[cfg(target_os = "windows")]
    platform::set_dpi_awareness();
    // Opt into dark mode before any window or menu exists, so native surfaces
    // (tray context menu, settings title bar) match the app's dark theme.
    #[cfg(target_os = "windows")]
    platform::enable_dark_mode();

    let storage = Rc::new(RefCell::new(
        steward_storage::Storage::open().expect("failed to open the Steward storage database"),
    ));
    let language = storage.borrow().get_setting(LANGUAGE_SETTING);
    let i18n = Rc::new(
        i18n::Localization::new_with_language(language.as_deref())
            .expect("failed to initialize localization"),
    );
    // Plugin metadata cache. `STEWARD_PLUGINS_DIR` overrides the plugin root
    // (e.g. pointing at a repo's `packages/plugins` during development); the
    // SQLite cache itself always lives in the app data directory.
    let data_dir = dirs::data_dir()
        .expect("no OS data directory available")
        .join("Steward");
    // Host-side clipboard history: a background thread polls the system
    // clipboard and pushes newest entries; the foreground poll task forwards
    // them to the plugin host. The watcher runs for the app's lifetime.
    let (clipboard_tx, clipboard_rx) = crossbeam_channel::unbounded();
    let clipboard_watcher =
        crate::clipboard_history::ClipboardWatcher::spawn(data_dir.clone(), clipboard_tx);
    let registry = Rc::new(RefCell::new(
        if let Some(root) = std::env::var("STEWARD_PLUGINS_DIR").ok().map(PathBuf::from) {
            Registry::open_with_root(&data_dir, &root)
        } else {
            Registry::open_at(&data_dir)
        }
        .expect("failed to open the plugin registry database"),
    ));
    // Plugin host: resolves the `steward-plugin-runtime` binary (env override
    // `STEWARD_PLUGIN_RUNTIME_BIN` or the sibling of this executable). A
    // missing binary degrades to "no plugins" instead of failing startup.
    let plugin_host_config = match HostConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("plugin runtime binary not found: {error:#}; plugins disabled");
            HostConfig::default()
        }
    };
    let plugin_host = Rc::new(RefCell::new(PluginHost::new(plugin_host_config)));
    let state = Rc::new(RefCell::new(LauncherState {
        window: None,
        settings_window: None,
        focus: None,
        result_count: 0,
        scrim_alpha: steward_ui_components::palette::SCRIM_ALPHA,
        last_applied_height: 0.0,
        storage,
        engine: Rc::new(RefCell::new(Engine::new())),
        icon_cache: RefCell::new(HashMap::new()),
        plugin_icons: RefCell::new(HashMap::new()),
        icon_gen: Cell::new(0),
        icon_rx: RefCell::new(None),
        scan_rx: RefCell::new(None),
        plugin_host,
        plugin_registry: registry,
        plugin_scan_rx: RefCell::new(None),
        plugin_gen: Cell::new(0),
        plugin_hits: RefCell::new(Vec::new()),
        plugin_views: RefCell::new(Vec::new()),
        plugin_pending: RefCell::new(HashSet::new()),
        search_gen: Cell::new(0),
        plugin_search_results: RefCell::new(HashMap::new()),
        show_epoch: Cell::new(0),
        plugin_calendar: RefCell::new(None),
        panel_view_windows: RefCell::new(HashMap::new()),
        hotkey_manager: None,
        summon_hotkey: None,
        settings_hotkey: None,
        clipboard_rx: RefCell::new(Some(clipboard_rx)),
        _clipboard_watcher: Some(clipboard_watcher),
    }));

    // GPUI starts at boot and the launcher window is created hidden, so every
    // summon is instant. A lazy-loading variant was measured and reverted: it
    // deferred GPUI/DirectX/DirectWrite initialization to the first summon,
    // making that summon 0.5-4.5 s — unacceptable for a launcher (decision
    // record in docs/architecture.md).
    application().run(move |cx: &mut App| {
        let focus = init_ui_common(cx, &state);

        // Seed the index from the cache, and refresh it in the background when
        // the cache is stale, before the window opens.
        state.borrow().ensure_app_index();
        // Seed the plugin host from the metadata cache (cold path: SQLite
        // only); a background scan reconciles new/changed plugins.
        state.borrow().ensure_plugin_index();
        let window = open_launcher_window(cx, &focus, i18n.clone(), &state);
        state.borrow_mut().window = Some(window);

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Err(error) = setup_tray(&i18n) {
            eprintln!("failed to create tray icon: {error:#}");
        }
        if let Err(error) = setup_global_hotkey(&state) {
            eprintln!("failed to register global hotkey: {error:#}");
        }
        if let Err(error) = spawn_event_poll_task(state.clone(), i18n.clone(), cx) {
            eprintln!("failed to start event polling task: {error:#}");
        }
    });
}

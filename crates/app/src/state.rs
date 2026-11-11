use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use global_hotkey::{hotkey::HotKey, GlobalHotKeyManager};
use gpui::AnyWindowHandle;
use nsh_engine::HostChannel;
use nsh_package::PackageReceipt;
use steward_plugin_contract::{LaunchRequest, PluginErrorCode};

use crate::clipboard_history::ClipboardEntry;
use crate::i18n::Localization;
use crate::services::{SearchRequest, SearchState, WindowCommand};
use crate::tray::TrayHandle;

pub(crate) struct PluginWindow {
    pub handle: AnyWindowHandle,
    pub host: HostChannel,
}

#[derive(Default)]
pub(crate) struct PluginWindowManager {
    windows: HashMap<String, PluginWindow>,
}

impl PluginWindowManager {
    pub(crate) fn get(&self, plugin_id: &str) -> Option<&PluginWindow> {
        self.windows.get(plugin_id)
    }

    pub(crate) fn insert(&mut self, plugin_id: String, window: PluginWindow) {
        self.windows.insert(plugin_id, window);
    }

    pub(crate) fn remove(&mut self, plugin_id: &str) -> Option<PluginWindow> {
        self.windows.remove(plugin_id)
    }

    pub(crate) fn contains(&self, plugin_id: &str) -> bool {
        self.windows.contains_key(plugin_id)
    }

    pub(crate) fn retain(&mut self, f: impl FnMut(&String, &mut PluginWindow) -> bool) {
        self.windows.retain(f);
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &PluginWindow> {
        self.windows.values()
    }
}

#[derive(Debug)]
pub(crate) enum PluginRuntimeEvent {
    Healthy {
        plugin_id: String,
        receipt: PackageReceipt,
    },
    Failed {
        plugin_id: String,
        request: LaunchRequest,
        code: PluginErrorCode,
        error: String,
    },
}

pub(crate) struct AppState {
    pub data_dir: std::path::PathBuf,
    pub services: Arc<crate::services::AppServices>,
    pub window: Option<AnyWindowHandle>,
    pub launcher_height: f32,
    pub settings_window: Option<AnyWindowHandle>,
    pub storage: Rc<RefCell<steward_storage::Storage>>,
    pub i18n: Rc<Localization>,
    pub search: Rc<RefCell<SearchState>>,
    pub requests: crossbeam_channel::Receiver<SearchRequest>,
    pub commands: crossbeam_channel::Receiver<WindowCommand>,
    pub clipboard_entries: Arc<Mutex<Vec<ClipboardEntry>>>,
    pub clipboard_rx: RefCell<Option<crossbeam_channel::Receiver<Vec<ClipboardEntry>>>>,
    pub tray: RefCell<Option<TrayHandle>>,
    pub hotkey_manager: Option<GlobalHotKeyManager>,
    pub summon_hotkey: Option<HotKey>,
    pub plugin_windows: PluginWindowManager,
    pub plugin_events: crossbeam_channel::Sender<PluginRuntimeEvent>,
    pub plugin_events_rx: crossbeam_channel::Receiver<PluginRuntimeEvent>,
    pub _clipboard_watcher: crate::clipboard_history::ClipboardWatcher,
    pub _log_guard: Option<crate::logging::LoggingGuard>,
    pub _instance: Option<crate::instance::InstanceGuard>,
}

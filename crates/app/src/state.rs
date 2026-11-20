use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use global_hotkey::{hotkey::HotKey, GlobalHotKeyManager};
use gpui::AnyWindowHandle;
use nsh_package::PackageReceipt;
use steward_plugin_contract::{LaunchRequest, PluginErrorCode, PluginSessionId};

use crate::i18n::Localization;
use crate::plugin_window::PluginProcess;
use crate::services::{SearchRequest, SearchState, WindowCommand};
use crate::tray::TrayHandle;

#[derive(Default)]
pub(crate) struct PluginWindowManager {
    windows: HashMap<String, PluginProcess>,
}

impl PluginWindowManager {
    pub(crate) fn get_mut(&mut self, plugin_id: &str) -> Option<&mut PluginProcess> {
        self.windows.get_mut(plugin_id)
    }

    pub(crate) fn insert(&mut self, plugin_id: String, process: PluginProcess) {
        self.windows.insert(plugin_id, process);
    }

    pub(crate) fn remove(&mut self, plugin_id: &str) -> Option<PluginProcess> {
        self.windows.remove(plugin_id)
    }

    pub(crate) fn session_id(&self, plugin_id: &str) -> Option<PluginSessionId> {
        self.windows.get(plugin_id).map(PluginProcess::session_id)
    }

    pub(crate) fn retain(&mut self, f: impl FnMut(&String, &mut PluginProcess) -> bool) {
        self.windows.retain(f);
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &PluginProcess> {
        self.windows.values()
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut PluginProcess> {
        self.windows.values_mut()
    }
}

#[derive(Debug)]
pub(crate) enum PluginRuntimeEvent {
    Healthy {
        plugin_id: String,
        session_id: PluginSessionId,
        receipt: PackageReceipt,
        requested_at: std::time::Instant,
        first_frame_at: std::time::Instant,
    },
    Failed {
        plugin_id: String,
        session_id: PluginSessionId,
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
    pub tray: RefCell<Option<TrayHandle>>,
    pub hotkey_manager: Option<GlobalHotKeyManager>,
    pub summon_hotkey: Option<HotKey>,
    pub hotkey_registry: crate::hotkey_registry::HotkeyRegistry,
    pub plugin_windows: PluginWindowManager,
    pub plugin_events: crossbeam_channel::Sender<PluginRuntimeEvent>,
    pub plugin_events_rx: crossbeam_channel::Receiver<PluginRuntimeEvent>,
    pub last_update_revision: u64,
    pub _clipboard_watcher: crate::clipboard_history::ClipboardWatcher,
    pub _log_guard: Option<crate::logging::LoggingGuard>,
    pub _instance: Option<crate::instance::InstanceGuard>,
}

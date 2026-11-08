use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use global_hotkey::{hotkey::HotKey, GlobalHotKeyManager};
use gpui::AnyWindowHandle;
use nsh_engine::HostChannel;

use crate::clipboard_history::ClipboardEntry;
use crate::i18n::Localization;
use crate::services::{SearchRequest, SearchState, WindowCommand};
use crate::tray::TrayHandle;

pub(crate) struct PluginWindow {
    pub handle: AnyWindowHandle,
    pub host: HostChannel,
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
    pub plugin_windows: HashMap<String, PluginWindow>,
    pub _clipboard_watcher: crate::clipboard_history::ClipboardWatcher,
}

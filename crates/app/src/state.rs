use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use global_hotkey::{hotkey::HotKey, GlobalHotKeyManager};
use gpui::AnyWindowHandle;
use nsh_package::PackageReceipt;
use steward_plugin_contract::{
    LaunchRequest, PluginErrorCode, PluginNotice, PluginPresentationEvent, PluginSessionId,
};

use crate::i18n::Localization;
use crate::plugin_window::PluginProcess;
use crate::services::{SearchRequest, SearchState, WindowCommand};
use crate::tray::TrayHandle;

#[derive(Default)]
pub(crate) struct PluginWindowManager {
    windows: HashMap<String, PluginProcess>,
    retries: HashMap<String, PluginRetryState>,
}

impl PluginWindowManager {
    pub(crate) fn get(&self, plugin_id: &str) -> Option<&PluginProcess> {
        self.windows.get(plugin_id)
    }

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

    pub(crate) fn retry_after(&mut self, plugin_id: &str, now: Instant) -> Option<Duration> {
        self.retries
            .get_mut(plugin_id)
            .and_then(|retry| retry.remaining(now))
    }

    pub(crate) fn record_failure(&mut self, plugin_id: &str, now: Instant) -> Duration {
        self.retries
            .entry(plugin_id.into())
            .or_default()
            .record_failure(now)
    }

    pub(crate) fn record_healthy(&mut self, plugin_id: &str) {
        self.retries.remove(plugin_id);
    }
}

#[derive(Default)]
struct PluginRetryState {
    failures: u32,
    retry_not_before: Option<Instant>,
    last_failure: Option<Instant>,
}

impl PluginRetryState {
    const RESET_AFTER: Duration = Duration::from_secs(5 * 60);
    const MAX_DELAY: Duration = Duration::from_secs(30);

    fn remaining(&mut self, now: Instant) -> Option<Duration> {
        if self
            .last_failure
            .is_some_and(|last| now.saturating_duration_since(last) >= Self::RESET_AFTER)
        {
            *self = Self::default();
            return None;
        }
        self.retry_not_before
            .and_then(|retry_at| retry_at.checked_duration_since(now))
            .filter(|remaining| !remaining.is_zero())
    }

    fn record_failure(&mut self, now: Instant) -> Duration {
        if self
            .last_failure
            .is_some_and(|last| now.saturating_duration_since(last) >= Self::RESET_AFTER)
        {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        self.last_failure = Some(now);
        let delay = retry_delay(self.failures);
        self.retry_not_before = Some(now + delay);
        delay
    }
}

fn retry_delay(failures: u32) -> Duration {
    let seconds = match failures {
        0 | 1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => PluginRetryState::MAX_DELAY.as_secs(),
    };
    Duration::from_secs(seconds)
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
    Notice {
        plugin_id: String,
        session_id: PluginSessionId,
        notice: PluginNotice,
    },
    Presentation {
        plugin_id: String,
        session_id: PluginSessionId,
        event: PluginPresentationEvent,
    },
    Stopped {
        plugin_id: String,
        session_id: PluginSessionId,
        request: LaunchRequest,
    },
    Failed {
        plugin_id: String,
        session_id: PluginSessionId,
        request: LaunchRequest,
        code: PluginErrorCode,
        error: String,
    },
    Exited {
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
    pub last_plugin_management_revision: u64,
    pub last_developer_diagnostics_revision: u64,
    pub _clipboard_watcher: crate::clipboard_history::ClipboardWatcher,
    pub _log_guard: Option<crate::logging::LoggingGuard>,
    pub _instance: Option<crate::instance::InstanceGuard>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_grows_then_resets_after_a_healthy_session() {
        let started = Instant::now();
        let mut retry = PluginRetryState::default();

        assert_eq!(retry.record_failure(started), Duration::from_secs(2));
        assert_eq!(
            retry.remaining(started + Duration::from_millis(500)),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(retry.record_failure(started), Duration::from_secs(4));
        assert_eq!(retry.record_failure(started), Duration::from_secs(8));

        let mut manager = PluginWindowManager::default();
        manager.record_failure("com.example.plugin", started);
        manager.record_healthy("com.example.plugin");
        assert_eq!(manager.retry_after("com.example.plugin", started), None);
    }

    #[test]
    fn retry_state_expires_after_the_failure_window() {
        let started = Instant::now();
        let mut retry = PluginRetryState::default();
        retry.record_failure(started);

        assert_eq!(
            retry.remaining(started + PluginRetryState::RESET_AFTER),
            None
        );
        assert_eq!(retry.failures, 0);
    }
}

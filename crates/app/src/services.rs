//! Thread boundary between the NSH launcher root and Steward's host services.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Arc, RwLock,
};

use nsh_core::element::engine_context;
use steward_core_engine::{AppEntry, Engine};
use steward_plugin_contract::{LaunchRequest, PluginNotice};
use steward_storage::Storage;
use steward_ui_components::palette;
use tokio::sync::oneshot;

use crate::developer_diagnostics::DeveloperDiagnosticsPort;
use crate::i18n::UiStrings;
pub(crate) use crate::launcher_model::RowAction;
use crate::plugin_management::PluginManagementPort;
pub(crate) use crate::plugin_runtime::PluginLaunchOutcome;
use crate::plugin_runtime::PluginRuntimeState;
use crate::search::{
    ApplicationSource, BuiltinSource, FileSearchCache, FileSearchSource, PluginSource,
    SearchSource, SettingsIndex, SettingsSource,
};
pub use crate::search::{SearchReply, SearchRequest};
use crate::updates::UpdatePort;

#[derive(Debug, Clone)]
pub struct UiSettings {
    locale: String,
    strings: UiStrings,
    accent: String,
    autostart: bool,
    summon_hotkey: String,
    settings_hotkey: String,
    check_updates_on_startup: bool,
}

impl UiSettings {
    pub fn new(
        locale: String,
        strings: UiStrings,
        accent: String,
        autostart: bool,
        summon_hotkey: String,
        settings_hotkey: String,
        check_updates_on_startup: bool,
    ) -> Self {
        Self {
            locale,
            strings,
            accent,
            autostart,
            summon_hotkey,
            settings_hotkey,
            check_updates_on_startup,
        }
    }
}

#[derive(Clone)]
pub struct LauncherQueryPort {
    requests: crossbeam_channel::Sender<SearchRequest>,
    search_revision: Arc<AtomicU64>,
    locale_revision: Arc<AtomicU64>,
    launcher_reset_revision: Arc<AtomicU64>,
    launcher_focus_revision: Arc<AtomicU64>,
    launcher_scrim_alpha: Arc<AtomicU32>,
}

#[derive(Clone)]
pub struct WindowCommandPort {
    window_commands: crossbeam_channel::Sender<WindowCommand>,
}

#[derive(Clone)]
pub struct UiSettingsPort {
    settings: Arc<RwLock<UiSettings>>,
}

#[derive(Clone)]
pub struct NoticePort {
    notice: Arc<RwLock<Option<PluginNotice>>>,
    search_revision: Arc<AtomicU64>,
}

impl NoticePort {
    pub fn notice(&self) -> Option<PluginNotice> {
        self.notice.read().ok().and_then(|notice| notice.clone())
    }
}

#[derive(Clone)]
pub struct AppServices {
    query: LauncherQueryPort,
    commands: WindowCommandPort,
    settings: UiSettingsPort,
    notices: NoticePort,
    updates: UpdatePort,
    plugin_management: Arc<RwLock<PluginManagementPort>>,
    developer_diagnostics: DeveloperDiagnosticsPort,
}

#[derive(Clone)]
pub struct LauncherServices {
    query: LauncherQueryPort,
    commands: WindowCommandPort,
    settings: UiSettingsPort,
    notices: NoticePort,
}

impl AppServices {
    pub fn new(
        requests: crossbeam_channel::Sender<SearchRequest>,
        window_commands: crossbeam_channel::Sender<WindowCommand>,
        settings: UiSettings,
        search_revision: Arc<AtomicU64>,
    ) -> Self {
        let locale_revision = Arc::new(AtomicU64::new(1));
        let launcher_reset_revision = Arc::new(AtomicU64::new(0));
        let launcher_focus_revision = Arc::new(AtomicU64::new(0));
        let launcher_scrim_alpha = Arc::new(AtomicU32::new(palette::SCRIM_ALPHA.to_bits()));
        Self {
            query: LauncherQueryPort {
                requests,
                search_revision: search_revision.clone(),
                locale_revision,
                launcher_reset_revision,
                launcher_focus_revision,
                launcher_scrim_alpha,
            },
            commands: WindowCommandPort { window_commands },
            settings: UiSettingsPort {
                settings: Arc::new(RwLock::new(settings)),
            },
            notices: NoticePort {
                notice: Arc::new(RwLock::new(None)),
                search_revision,
            },
            updates: UpdatePort::default(),
            plugin_management: Arc::new(RwLock::new(PluginManagementPort::default())),
            developer_diagnostics: DeveloperDiagnosticsPort::default(),
        }
    }

    #[cfg(test)]
    pub fn notice_port(&self) -> NoticePort {
        self.notices.clone()
    }

    pub fn update_port(&self) -> UpdatePort {
        self.updates.clone()
    }

    pub fn plugin_management(&self) -> PluginManagementPort {
        self.plugin_management
            .read()
            .map(|port| port.clone())
            .unwrap_or_default()
    }

    pub fn set_plugin_management(&self, port: PluginManagementPort) {
        if let Ok(mut current) = self.plugin_management.write() {
            *current = port;
        }
    }

    pub fn developer_diagnostics(&self) -> DeveloperDiagnosticsPort {
        self.developer_diagnostics.clone()
    }

    pub fn configure_developer_diagnostics(&self, enabled: bool, data_dir: &std::path::Path) {
        self.developer_diagnostics.configure(enabled, data_dir);
    }

    pub fn developer_diagnostics_revision(&self) -> u64 {
        self.developer_diagnostics.revision()
    }

    pub fn launcher(&self) -> LauncherServices {
        LauncherServices {
            query: self.query.clone(),
            commands: self.commands.clone(),
            settings: self.settings.clone(),
            notices: self.notices.clone(),
        }
    }

    pub fn send_window_command(&self, command: WindowCommand) {
        let _ = self.commands.window_commands.send(command);
    }

    pub fn locale(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.locale.clone())
            .unwrap_or_else(|_| "en".into())
    }

    pub fn accent(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.accent.clone())
            .unwrap_or_else(|_| "#863bff".into())
    }

    pub fn autostart(&self) -> bool {
        self.settings
            .settings
            .read()
            .map(|settings| settings.autostart)
            .unwrap_or(false)
    }

    pub fn summon_hotkey(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.summon_hotkey.clone())
            .unwrap_or_else(|_| "control+alt+Space".into())
    }

    pub fn settings_hotkey(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.settings_hotkey.clone())
            .unwrap_or_else(|_| "control+Comma".into())
    }

    pub fn check_updates_on_startup(&self) -> bool {
        self.settings
            .settings
            .read()
            .map(|settings| settings.check_updates_on_startup)
            .unwrap_or(true)
    }

    pub fn set_locale(&self, locale: impl Into<String>, strings: UiStrings) {
        if let Ok(mut settings) = self.settings.settings.write() {
            settings.locale = locale.into();
            settings.strings = strings;
        }
        self.query.locale_revision.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_accent(&self, accent: impl Into<String>) {
        if let Ok(mut settings) = self.settings.settings.write() {
            settings.accent = accent.into();
        }
    }

    pub fn set_autostart(&self, enabled: bool) {
        if let Ok(mut settings) = self.settings.settings.write() {
            settings.autostart = enabled;
        }
    }

    pub fn set_summon_hotkey(&self, hotkey: impl Into<String>) {
        if let Ok(mut settings) = self.settings.settings.write() {
            settings.summon_hotkey = hotkey.into();
        }
    }

    pub fn set_settings_hotkey(&self, hotkey: impl Into<String>) {
        if let Ok(mut settings) = self.settings.settings.write() {
            settings.settings_hotkey = hotkey.into();
        }
    }

    pub fn set_check_updates_on_startup(&self, enabled: bool) {
        if let Ok(mut settings) = self.settings.settings.write() {
            settings.check_updates_on_startup = enabled;
        }
    }

    #[cfg(test)]
    pub fn search_revision(&self) -> u64 {
        self.query.search_revision.load(Ordering::Relaxed)
    }

    pub fn request_launcher_reset(&self) {
        self.query
            .launcher_reset_revision
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn request_launcher_focus(&self) {
        self.query
            .launcher_focus_revision
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_launcher_scrim_alpha(&self, alpha: f32) {
        let alpha = if alpha.is_finite() {
            alpha.clamp(palette::SCRIM_ALPHA, palette::SCRIM_ALPHA_MAX)
        } else {
            palette::SCRIM_ALPHA
        };
        self.query
            .launcher_scrim_alpha
            .store(alpha.to_bits(), Ordering::Relaxed);
    }

    pub fn notice(&self) -> Option<PluginNotice> {
        self.notices
            .notice
            .read()
            .ok()
            .and_then(|notice| notice.clone())
    }

    pub fn publish_notice(&self, notice: PluginNotice) {
        if let Ok(mut current) = self.notices.notice.write() {
            *current = Some(notice);
        }
        self.notices.search_revision.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear_notice(&self) {
        if let Ok(mut current) = self.notices.notice.write() {
            *current = None;
        }
        self.notices.search_revision.fetch_add(1, Ordering::Relaxed);
    }
}

impl LauncherServices {
    pub async fn search(&self, generation: u64, query: String) -> Option<SearchReply> {
        let (tx, rx) = oneshot::channel();
        self.query
            .requests
            .send(SearchRequest {
                generation,
                query,
                reply: tx,
            })
            .ok()?;
        rx.await.ok()
    }

    pub fn send_window_command(&self, command: WindowCommand) {
        let _ = self.commands.window_commands.send(command);
    }

    pub fn search_placeholder(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.strings.search_placeholder.clone())
            .unwrap_or_else(|_| "Search apps, files, settings, or type a command...".into())
    }

    pub fn search_revision(&self) -> u64 {
        self.query.search_revision.load(Ordering::Relaxed)
    }

    pub fn settings_hotkey(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.settings_hotkey.clone())
            .unwrap_or_else(|_| "control+Comma".into())
    }

    pub fn ui_strings(&self) -> UiStrings {
        self.settings
            .settings
            .read()
            .map(|settings| settings.strings.clone())
            .unwrap_or_else(|_| UiStrings {
                search_placeholder: "Search apps, files, settings, or type a command...".into(),
                application: "Application".into(),
                file_search_unavailable: "Windows Search is unavailable".into(),
                file_search_repair: "Open Windows Search settings".into(),
                settings: "Windows Settings".into(),
                plugin: "Plugin".into(),
                calculator: "Calculator".into(),
                open_url: "Open URL".into(),
                launcher_navigate: "Navigate".into(),
                launcher_open: "Open".into(),
                launcher_close: "Close".into(),
                launcher_confirm: "Confirm".into(),
                launcher_actions: "Actions".into(),
                launcher_copy: "Copy".into(),
                launcher_open_application: "Open Application".into(),
                launcher_open_file: "Open File".into(),
                launcher_open_settings: "Open Settings".into(),
                launcher_open_plugin: "Open Plugin".into(),
                launcher_open_location: "Open file location".into(),
                launcher_copy_path: "Copy path".into(),
                launcher_properties: "Properties".into(),
                launcher_action_application_window: "Application Window".into(),
                launcher_action_recent_files: "Open Files... / Recent".into(),
                launcher_action_run_as_administrator: "Run as Administrator".into(),
                launcher_action_run_as_different_user: "Run as Different User".into(),
                launcher_action_show_in_file_explorer: "Show in File Explorer".into(),
                launcher_action_search: "Search for actions...".into(),
                launcher_action_no_results: "No matching actions".into(),
            })
    }

    pub fn locale_revision(&self) -> u64 {
        self.query.locale_revision.load(Ordering::Relaxed)
    }

    pub fn launcher_reset_revision(&self) -> u64 {
        self.query.launcher_reset_revision.load(Ordering::Relaxed)
    }

    pub fn launcher_focus_revision(&self) -> u64 {
        self.query.launcher_focus_revision.load(Ordering::Relaxed)
    }

    pub fn launcher_scrim_alpha(&self) -> f32 {
        f32::from_bits(self.query.launcher_scrim_alpha.load(Ordering::Relaxed))
    }

    pub fn notice(&self) -> Option<PluginNotice> {
        self.notices.notice()
    }
}

#[derive(Debug, Clone)]
pub enum WindowCommand {
    Hide,
    FocusLauncher,
    Resize { height: f32 },
    Activate(RowAction),
    SetLanguage(String),
    SetAccent(String),
    SetAutostart(bool),
    SetSummonHotkey(String),
    SetSettingsHotkey(String),
    SetCheckUpdatesOnStartup(bool),
    OpenSettings,
    Quit,
}

pub fn services() -> Option<Arc<LauncherServices>> {
    engine_context::<Arc<LauncherServices>>().map(|services| services.as_ref().clone())
}

pub struct SearchState {
    engine: Engine,
    strings: UiStrings,
    locale: String,
    settings: SettingsIndex,
    file_search: FileSearchCache,
    storage: std::rc::Rc<std::cell::RefCell<Storage>>,
    icon_cache: HashMap<PathBuf, Option<String>>,
    plugins: PluginRuntimeState,
    app_scan: Option<crossbeam_channel::Receiver<Vec<AppEntry>>>,
    search_revision: Arc<AtomicU64>,
    next_app_scan: std::time::Instant,
}

impl SearchState {
    pub fn new(
        data_dir: PathBuf,
        storage: std::rc::Rc<std::cell::RefCell<Storage>>,
        strings: UiStrings,
        locale: String,
    ) -> anyhow::Result<Self> {
        let cached = storage
            .borrow()
            .cached_apps()
            .unwrap_or_default()
            .into_iter()
            .map(|app| AppEntry {
                name: app.name,
                path: app.path,
            })
            .collect();
        let mut engine = Engine::new();
        engine.set_entries(cached);
        let plugins = PluginRuntimeState::new(&data_dir)?;
        Ok(Self {
            engine,
            strings,
            locale,
            settings: SettingsIndex::new(),
            file_search: FileSearchCache::new(),
            storage,
            icon_cache: HashMap::new(),
            plugins,
            app_scan: None,
            search_revision: Arc::new(AtomicU64::new(0)),
            next_app_scan: std::time::Instant::now(),
        })
    }

    pub fn start(&mut self) {
        self.spawn_app_scan();
        self.plugins.start();
    }

    pub fn add_trust_root(&mut self, key: &str) -> anyhow::Result<()> {
        self.plugins.add_trust_root(key)
    }

    pub fn revision_handle(&self) -> Arc<AtomicU64> {
        self.search_revision.clone()
    }

    pub fn set_ui_strings(&mut self, locale: impl Into<String>, strings: UiStrings) {
        self.locale = locale.into();
        self.strings = strings;
    }

    pub fn start_launch(
        &mut self,
        request: LaunchRequest,
    ) -> Result<(), steward_plugin_host::PluginHostError> {
        self.plugins.start_launch(request)
    }

    pub fn refresh_plugins(&mut self) {
        self.plugins.refresh_now();
    }

    pub fn refresh_plugins_blocking(&mut self) -> anyhow::Result<()> {
        self.plugins.refresh_blocking()
    }

    pub fn accept_receipt(&mut self, receipt: &nsh_package::PackageReceipt) -> anyhow::Result<()> {
        self.plugins.accept_receipt(receipt)
    }

    pub fn poll_launch(&mut self) -> Option<PluginLaunchOutcome> {
        self.plugins.poll_launch()
    }

    pub fn handle(&mut self, request: SearchRequest) {
        self.file_search.set_query(request.query.trim());
        let items = self.local_items(&request.query);
        let _ = request.reply.send(SearchReply {
            generation: request.generation,
            items,
        });
    }

    pub fn poll(&mut self) {
        if self.file_search.poll() {
            self.search_revision.fetch_add(1, Ordering::Relaxed);
        }
        let app_scan = self.app_scan.clone();
        if let Some(rx) = app_scan {
            let mut finished = false;
            loop {
                match rx.try_recv() {
                    Ok(apps) => {
                        if self.engine.entries() != apps.as_slice() {
                            let cached = apps
                                .iter()
                                .map(|app| steward_storage::CachedApp {
                                    name: app.name.clone(),
                                    path: app.path.clone(),
                                })
                                .collect::<Vec<_>>();
                            let _ = self.storage.borrow_mut().mark_seen(&cached);
                            self.engine.set_entries(apps);
                            self.search_revision.fetch_add(1, Ordering::Relaxed);
                        }
                        finished = true;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
            if finished {
                self.app_scan = None;
                self.next_app_scan = std::time::Instant::now() + std::time::Duration::from_secs(30);
            }
        }
        if self.plugins.poll() {
            self.search_revision.fetch_add(1, Ordering::Relaxed);
        }
        if self.app_scan.is_none() && std::time::Instant::now() >= self.next_app_scan {
            self.spawn_app_scan();
        }
    }

    fn local_items(&mut self, query: &str) -> Vec<crate::launcher_model::LauncherItem> {
        let trimmed = query.trim();
        let mut builtin = BuiltinSource {
            strings: &self.strings,
        };
        let mut applications = ApplicationSource {
            engine: &self.engine,
            storage: &self.storage,
            icon_cache: &mut self.icon_cache,
            label: &self.strings.application,
        };
        let mut first_sources: [&mut dyn SearchSource; 2] = [&mut builtin, &mut applications];
        let mut contribution = crate::search::collect(&mut first_sources, trimmed);

        let mut files = FileSearchSource {
            query: trimmed,
            cache: &self.file_search,
            icon_cache: &mut self.icon_cache,
            unavailable_title: &self.strings.file_search_unavailable,
            unavailable_subtitle: &self.strings.file_search_repair,
        };
        let mut plugins = PluginSource {
            host: self.plugins.host(),
            label: &self.strings.plugin,
        };
        let mut settings = SettingsSource {
            index: &self.settings,
            label: &self.strings.settings,
            locale: &self.locale,
        };
        let mut remaining_sources: [&mut dyn SearchSource; 3] =
            [&mut plugins, &mut settings, &mut files];
        let mut remaining = crate::search::collect(&mut remaining_sources, trimmed);
        contribution.items.append(&mut remaining.items);
        contribution.sort_for_query(trimmed);
        contribution.items
    }

    fn spawn_app_scan(&mut self) {
        if self.app_scan.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(steward_core_engine::platform_scanner().scan());
        });
        self.app_scan = Some(rx);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::AtomicU64;

    use steward_plugin_contract::{
        LaunchRequest, PluginErrorCode, PluginNotice, PluginNoticeKind, PluginNoticeLevel,
    };

    use super::*;

    fn request(command: &str) -> LaunchRequest {
        LaunchRequest {
            plugin_id: "com.example.test".into(),
            command: command.into(),
            title: "Test".into(),
            input: command.into(),
            locale: "en".into(),
            accent: "#863bff".into(),
            dark: true,
            presentation: steward_plugin_contract::PresentationMode::Windowed,
        }
    }

    fn ui_strings() -> UiStrings {
        UiStrings {
            search_placeholder: "Search".into(),
            application: "Application".into(),
            file_search_unavailable: "Windows Search is unavailable".into(),
            file_search_repair: "Open Windows Search settings".into(),
            settings: "Windows Settings".into(),
            plugin: "Plugin".into(),
            calculator: "Calculator".into(),
            open_url: "Open URL".into(),
            launcher_navigate: "Navigate".into(),
            launcher_open: "Open".into(),
            launcher_close: "Close".into(),
            launcher_confirm: "Confirm".into(),
            launcher_actions: "Actions".into(),
            launcher_copy: "Copy".into(),
            launcher_open_application: "Open Application".into(),
            launcher_open_file: "Open File".into(),
            launcher_open_settings: "Open Settings".into(),
            launcher_open_plugin: "Open Plugin".into(),
            launcher_open_location: "Open file location".into(),
            launcher_copy_path: "Copy path".into(),
            launcher_properties: "Properties".into(),
            launcher_action_application_window: "Application Window".into(),
            launcher_action_recent_files: "Open Files... / Recent".into(),
            launcher_action_run_as_administrator: "Run as Administrator".into(),
            launcher_action_run_as_different_user: "Run as Different User".into(),
            launcher_action_show_in_file_explorer: "Show in File Explorer".into(),
            launcher_action_search: "Search for actions...".into(),
            launcher_action_no_results: "No matching actions".into(),
        }
    }

    fn services() -> AppServices {
        let (request_tx, _request_rx) = crossbeam_channel::unbounded();
        let (command_tx, _command_rx) = crossbeam_channel::unbounded();
        AppServices::new(
            request_tx,
            command_tx,
            UiSettings::new(
                "en".into(),
                ui_strings(),
                "#863bff".into(),
                false,
                "control+alt+Space".into(),
                "control+Comma".into(),
                false,
            ),
            Arc::new(AtomicU64::new(0)),
        )
    }

    #[test]
    fn notices_are_exposed_through_their_port_and_advance_search() {
        let services = services();
        assert_eq!(services.search_revision(), 0);
        services.publish_notice(PluginNotice {
            title: "Failed".into(),
            message: "load_failed".into(),
            plugin_id: Some("com.example.test".into()),
            code: Some(PluginErrorCode::LoadFailed),
            level: PluginNoticeLevel::Error,
            kind: PluginNoticeKind::Toast,
        });
        assert_eq!(services.notice_port().notice().unwrap().title, "Failed");
        assert_eq!(services.search_revision(), 1);
        services.clear_notice();
        assert!(services.notice_port().notice().is_none());
        assert_eq!(services.search_revision(), 2);
    }

    #[test]
    fn update_startup_preference_is_exposed_and_mutable() {
        let services = services();
        assert!(!services.check_updates_on_startup());
        services.set_check_updates_on_startup(true);
        assert!(services.check_updates_on_startup());
    }

    #[test]
    fn launcher_focus_requests_reach_the_surface() {
        let services = services();
        let launcher = services.launcher();

        assert_eq!(launcher.launcher_focus_revision(), 0);
        services.request_launcher_focus();
        assert_eq!(launcher.launcher_focus_revision(), 1);
    }

    #[test]
    fn repeated_requests_update_the_pending_launch() {
        let dir = std::env::temp_dir().join(format!(
            "steward-pending-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Rc::new(RefCell::new(Storage::open_at(&dir).unwrap()));
        let mut state = SearchState::new(dir.clone(), storage, ui_strings(), "en".into()).unwrap();
        let (tx, rx) = crossbeam_channel::bounded::<
            Result<steward_plugin_host::PluginLaunch, steward_plugin_host::PluginHostError>,
        >(1);
        drop(tx);
        state
            .plugins
            .insert_pending_launch_for_test("com.example.test", request("first"), rx);
        state.start_launch(request("second")).unwrap();
        assert_eq!(
            state.plugins.pending_launch_command("com.example.test"),
            Some("second")
        );
        assert!(matches!(
            state.poll_launch(),
            Some(PluginLaunchOutcome::Failed {
                code: PluginErrorCode::RuntimeError,
                ..
            })
        ));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn windows_settings_are_part_of_local_launcher_results() {
        let dir = std::env::temp_dir().join(format!(
            "steward-settings-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Rc::new(RefCell::new(Storage::open_at(&dir).unwrap()));
        let mut state = SearchState::new(dir.clone(), storage, ui_strings(), "zh".into()).unwrap();

        #[cfg(target_os = "windows")]
        let _icon_guard = crate::app_icons::ICON_TEST_LOCK.lock().unwrap();
        let items = state.local_items("lanya");

        assert!(items.iter().any(|item| {
            item.id == "settings:ms-settings:bluetooth"
                && item.action_id.as_str() == "settings:ms-settings:bluetooth"
        }));
        assert!(items.iter().any(|item| matches!(
            &item.action,
            RowAction::OpenSettings(uri) if uri == "ms-settings:bluetooth"
        )));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn builtin_commands_always_have_icons() {
        let dir = std::env::temp_dir().join(format!(
            "steward-command-icons-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Rc::new(RefCell::new(Storage::open_at(&dir).unwrap()));
        let mut state = SearchState::new(dir.clone(), storage, ui_strings(), "en".into()).unwrap();

        let calculator = state.local_items("1+1");
        assert_eq!(
            calculator[0].icon.source(),
            Some(crate::icons::CALCULATOR.to_owned())
        );

        let browser = state.local_items("https://example.com");
        assert_eq!(
            browser[0].icon.source(),
            Some(crate::icons::OPEN_URL.to_owned())
        );

        std::fs::remove_dir_all(dir).ok();
    }
}

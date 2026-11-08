//! Thread boundary between the NSH launcher root and Steward's host services.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Arc, RwLock,
};

use nsh_core::ui::engine_context;
use steward_core_engine::{AppEntry, Engine};
use steward_plugin_contract::{LaunchRequest, PluginErrorCode, PluginNotice};
use steward_plugin_host::PluginHost;
use steward_plugin_registry::{PluginMeta, Registry};
use steward_storage::Storage;
use steward_ui_components::palette;
use tokio::sync::oneshot;

use crate::i18n::UiStrings;
use crate::search::{ApplicationSource, BuiltinSource, PluginSource, SearchSource};
pub use crate::search::{RowAction, SearchReply, SearchRequest};

#[derive(Debug, Clone)]
pub struct UiSettings {
    locale: String,
    strings: UiStrings,
    accent: String,
    autostart: bool,
    summon_hotkey: String,
}

impl UiSettings {
    pub fn new(
        locale: String,
        strings: UiStrings,
        accent: String,
        autostart: bool,
        summon_hotkey: String,
    ) -> Self {
        Self {
            locale,
            strings,
            accent,
            autostart,
            summon_hotkey,
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct LauncherQueryPort {
    requests: crossbeam_channel::Sender<SearchRequest>,
    search_revision: Arc<AtomicU64>,
    locale_revision: Arc<AtomicU64>,
    launcher_reset_revision: Arc<AtomicU64>,
    launcher_focus_revision: Arc<AtomicU64>,
    launcher_scrim_alpha: Arc<AtomicU32>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct WindowCommandPort {
    window_commands: crossbeam_channel::Sender<WindowCommand>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct UiSettingsPort {
    settings: Arc<RwLock<UiSettings>>,
}

#[derive(Clone)]
#[allow(dead_code)]
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
        }
    }

    pub fn query_port(&self) -> LauncherQueryPort {
        self.query.clone()
    }

    pub fn command_port(&self) -> WindowCommandPort {
        self.commands.clone()
    }

    pub fn settings_port(&self) -> UiSettingsPort {
        self.settings.clone()
    }

    pub fn notice_port(&self) -> NoticePort {
        self.notices.clone()
    }

    pub fn launcher(&self) -> LauncherServices {
        LauncherServices {
            query: self.query.clone(),
            commands: self.commands.clone(),
            settings: self.settings.clone(),
            notices: self.notices.clone(),
        }
    }

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

    pub fn locale(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.locale.clone())
            .unwrap_or_else(|_| "en".into())
    }

    pub fn search_placeholder(&self) -> String {
        self.settings
            .settings
            .read()
            .map(|settings| settings.strings.search_placeholder.clone())
            .unwrap_or_else(|_| "Search apps or type a command...".into())
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

    pub fn search_revision(&self) -> u64 {
        self.query.search_revision.load(Ordering::Relaxed)
    }

    pub fn locale_revision(&self) -> u64 {
        self.query.locale_revision.load(Ordering::Relaxed)
    }

    pub fn request_launcher_reset(&self) {
        self.query
            .launcher_reset_revision
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn launcher_reset_revision(&self) -> u64 {
        self.query.launcher_reset_revision.load(Ordering::Relaxed)
    }

    pub fn request_launcher_focus(&self) {
        self.query
            .launcher_focus_revision
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn launcher_focus_revision(&self) -> u64 {
        self.query.launcher_focus_revision.load(Ordering::Relaxed)
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

    pub fn launcher_scrim_alpha(&self) -> f32 {
        f32::from_bits(self.query.launcher_scrim_alpha.load(Ordering::Relaxed))
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
            .unwrap_or_else(|_| "Search apps or type a command...".into())
    }

    pub fn search_revision(&self) -> u64 {
        self.query.search_revision.load(Ordering::Relaxed)
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
    OpenSettings,
    Resize { height: f32 },
    Activate(RowAction),
    ClosePlugin(String),
    SetLanguage(String),
    SetAccent(String),
    SetAutostart(bool),
    SetSummonHotkey(String),
}

pub fn services() -> Option<Arc<LauncherServices>> {
    engine_context::<Arc<LauncherServices>>().map(|services| services.as_ref().clone())
}

pub struct SearchState {
    engine: Engine,
    strings: UiStrings,
    storage: std::rc::Rc<std::cell::RefCell<Storage>>,
    icon_cache: HashMap<PathBuf, Option<String>>,
    plugin_host: PluginHost,
    registry: Registry,
    app_scan: Option<crossbeam_channel::Receiver<Vec<AppEntry>>>,
    plugin_scan: Option<crossbeam_channel::Receiver<Vec<PluginMeta>>>,
    pending_launches: HashMap<String, PendingLaunch>,
    search_revision: Arc<AtomicU64>,
    next_app_scan: std::time::Instant,
    next_plugin_scan: std::time::Instant,
}

impl SearchState {
    pub fn new(
        data_dir: PathBuf,
        storage: std::rc::Rc<std::cell::RefCell<Storage>>,
        strings: UiStrings,
    ) -> anyhow::Result<Self> {
        let cached = storage.borrow().cached_apps().unwrap_or_default();
        let mut engine = Engine::new();
        engine.set_entries(cached);
        let mut plugin_host =
            PluginHost::new(steward_plugin_host::HostConfig::new(data_dir.clone()))?;
        let registry = Registry::open_with_root(
            &data_dir,
            &std::env::var_os("STEWARD_PLUGINS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("plugins")),
        )?;
        let cached_plugins = registry.cached_plugins().unwrap_or_default();
        plugin_host.set_plugins(&cached_plugins);
        Ok(Self {
            engine,
            strings,
            storage,
            icon_cache: HashMap::new(),
            plugin_host,
            registry,
            app_scan: None,
            plugin_scan: None,
            pending_launches: HashMap::new(),
            search_revision: Arc::new(AtomicU64::new(0)),
            next_app_scan: std::time::Instant::now(),
            next_plugin_scan: std::time::Instant::now(),
        })
    }

    pub fn start(&mut self) {
        self.spawn_app_scan();
        self.spawn_plugin_scan();
    }

    pub fn add_trust_root(&mut self, key: &str) -> anyhow::Result<()> {
        self.plugin_host.add_trust_root_base64(key)
    }

    pub fn revision_handle(&self) -> Arc<AtomicU64> {
        self.search_revision.clone()
    }

    pub fn set_ui_strings(&mut self, strings: UiStrings) {
        self.strings = strings;
    }

    pub fn start_launch(
        &mut self,
        request: LaunchRequest,
    ) -> Result<(), steward_plugin_host::PluginHostError> {
        if let Some(pending) = self.pending_launches.get_mut(&request.plugin_id) {
            pending.latest = request;
            return Ok(());
        }
        let receiver = self.plugin_host.start_launch(request.clone())?;
        self.pending_launches.insert(
            request.plugin_id.clone(),
            PendingLaunch {
                latest: request.clone(),
                initial: request,
                receiver,
            },
        );
        Ok(())
    }

    pub fn accept_receipt(&mut self, receipt: &nsh_package::PackageReceipt) -> anyhow::Result<()> {
        self.plugin_host.accept_receipt(receipt)
    }

    pub fn poll_launch(&mut self) -> Option<PluginLaunchOutcome> {
        let mut finished = None;
        for (plugin_id, pending) in &self.pending_launches {
            let result = match pending.receiver.try_recv() {
                Ok(result) => result,
                Err(crossbeam_channel::TryRecvError::Empty) => continue,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    Err(steward_plugin_host::PluginHostError::WorkerExited)
                }
            };
            finished = Some((
                plugin_id.clone(),
                pending.initial.clone(),
                pending.latest.clone(),
                result,
            ));
            break;
        }
        let (plugin_id, initial, latest, result) = finished?;
        self.pending_launches.remove(&plugin_id);
        Some(match result {
            Ok(launch) => PluginLaunchOutcome::Ready {
                initial,
                latest,
                launch: Box::new(launch),
            },
            Err(error) => PluginLaunchOutcome::Failed {
                request: latest,
                code: error.code(),
                error: error.to_string(),
            },
        })
    }

    pub fn handle(&mut self, request: SearchRequest) {
        let (rows, actions) = self.local_rows(&request.query);
        let _ = request.reply.send(SearchReply {
            generation: request.generation,
            rows,
            actions,
        });
    }

    pub fn poll(&mut self) {
        let app_scan = self.app_scan.clone();
        if let Some(rx) = app_scan {
            while let Ok(apps) = rx.try_recv() {
                let _ = self.storage.borrow_mut().mark_seen(&apps);
                self.engine.set_entries(apps);
                self.search_revision.fetch_add(1, Ordering::Relaxed);
            }
            self.app_scan = None;
            self.next_app_scan = std::time::Instant::now() + std::time::Duration::from_secs(30);
        }
        let plugin_scan = self.plugin_scan.clone();
        if let Some(rx) = plugin_scan {
            while let Ok(plugins) = rx.try_recv() {
                self.plugin_host.set_plugins(&plugins);
                self.search_revision.fetch_add(1, Ordering::Relaxed);
            }
            self.plugin_scan = None;
            self.next_plugin_scan = std::time::Instant::now() + std::time::Duration::from_secs(30);
        }
        if self.app_scan.is_none() && std::time::Instant::now() >= self.next_app_scan {
            self.spawn_app_scan();
        }
        if self.plugin_scan.is_none() && std::time::Instant::now() >= self.next_plugin_scan {
            self.spawn_plugin_scan();
        }
    }

    fn local_rows(
        &mut self,
        query: &str,
    ) -> (Vec<steward_ui_components::LauncherRow>, Vec<RowAction>) {
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
        let mut plugins = PluginSource {
            host: &self.plugin_host,
            label: &self.strings.plugin,
        };
        let mut sources: [&mut dyn SearchSource; 3] =
            [&mut builtin, &mut applications, &mut plugins];
        let contribution = crate::search::collect(&mut sources, trimmed);
        (contribution.rows, contribution.actions)
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

    fn spawn_plugin_scan(&mut self) {
        if self.plugin_scan.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let data_dir = self.registry.data_dir().to_path_buf();
        let root = self.registry.plugins_root().to_path_buf();
        std::thread::spawn(move || {
            if let Ok(mut registry) = Registry::open_with_root(&data_dir, &root) {
                let _ = registry.scan();
                let _ = tx.send(registry.cached_plugins().unwrap_or_default());
            }
        });
        self.plugin_scan = Some(rx);
    }
}

struct PendingLaunch {
    initial: LaunchRequest,
    latest: LaunchRequest,
    receiver: crossbeam_channel::Receiver<
        Result<steward_plugin_host::PluginLaunch, steward_plugin_host::PluginHostError>,
    >,
}

pub enum PluginLaunchOutcome {
    Ready {
        initial: LaunchRequest,
        latest: LaunchRequest,
        launch: Box<steward_plugin_host::PluginLaunch>,
    },
    Failed {
        request: LaunchRequest,
        code: PluginErrorCode,
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::AtomicU64;

    use steward_plugin_contract::{LaunchRequest, PluginErrorCode, PluginNotice};

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
                UiStrings {
                    search_placeholder: "Search".into(),
                    application: "Application".into(),
                    plugin: "Plugin".into(),
                    calculator: "Calculator".into(),
                    open_url: "Open URL".into(),
                },
                "#863bff".into(),
                false,
                "control+alt+Space".into(),
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
            code: PluginErrorCode::LoadFailed,
        });
        assert_eq!(services.notice_port().notice().unwrap().title, "Failed");
        assert_eq!(services.search_revision(), 1);
        services.clear_notice();
        assert!(services.notice_port().notice().is_none());
        assert_eq!(services.search_revision(), 2);
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
        let mut state = SearchState::new(
            dir.clone(),
            storage,
            UiStrings {
                search_placeholder: "Search".into(),
                application: "Application".into(),
                plugin: "Plugin".into(),
                calculator: "Calculator".into(),
                open_url: "Open URL".into(),
            },
        )
        .unwrap();
        let (tx, rx) = crossbeam_channel::bounded::<
            Result<steward_plugin_host::PluginLaunch, steward_plugin_host::PluginHostError>,
        >(1);
        drop(tx);
        state.pending_launches.insert(
            "com.example.test".into(),
            PendingLaunch {
                initial: request("first"),
                latest: request("first"),
                receiver: rx,
            },
        );
        state.start_launch(request("second")).unwrap();
        assert_eq!(
            state.pending_launches["com.example.test"].latest.command,
            "second"
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
}

//! Thread boundary between the NSH launcher root and Steward's host services.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};

use nsh_core::ui::engine_context;
use steward_core_engine::{AppEntry, Engine};
use steward_plugin_host::{PluginHost, RouteHit};
use steward_plugin_registry::{PluginMeta, Registry};
use steward_storage::Storage;
use tokio::sync::oneshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAction {
    LaunchApp(PathBuf),
    CopyText(String),
    OpenUrl(String),
    OpenPlugin {
        plugin_id: String,
        command: String,
        title: String,
        input: String,
    },
}

#[derive(Debug)]
pub struct SearchReply {
    pub generation: u64,
    pub rows: Vec<steward_ui_components::LauncherRow>,
    pub actions: Vec<RowAction>,
}

pub struct SearchRequest {
    pub generation: u64,
    pub query: String,
    pub reply: oneshot::Sender<SearchReply>,
}

#[derive(Debug, Clone)]
struct UiSettings {
    locale: String,
    accent: String,
    autostart: bool,
    summon_hotkey: String,
}

#[derive(Clone)]
pub struct AppServices {
    requests: crossbeam_channel::Sender<SearchRequest>,
    window_commands: crossbeam_channel::Sender<WindowCommand>,
    settings: Arc<RwLock<UiSettings>>,
    search_revision: Arc<AtomicU64>,
}

impl AppServices {
    pub fn new(
        requests: crossbeam_channel::Sender<SearchRequest>,
        window_commands: crossbeam_channel::Sender<WindowCommand>,
        locale: String,
        accent: String,
        autostart: bool,
        summon_hotkey: String,
        search_revision: Arc<AtomicU64>,
    ) -> Self {
        Self {
            requests,
            window_commands,
            settings: Arc::new(RwLock::new(UiSettings {
                locale,
                accent,
                autostart,
                summon_hotkey,
            })),
            search_revision,
        }
    }

    pub async fn search(&self, generation: u64, query: String) -> Option<SearchReply> {
        let (tx, rx) = oneshot::channel();
        self.requests
            .send(SearchRequest {
                generation,
                query,
                reply: tx,
            })
            .ok()?;
        rx.await.ok()
    }

    pub fn send_window_command(&self, command: WindowCommand) {
        let _ = self.window_commands.send(command);
    }

    pub fn locale(&self) -> String {
        self.settings
            .read()
            .map(|settings| settings.locale.clone())
            .unwrap_or_else(|_| "en".into())
    }

    pub fn accent(&self) -> String {
        self.settings
            .read()
            .map(|settings| settings.accent.clone())
            .unwrap_or_else(|_| "#863bff".into())
    }

    pub fn autostart(&self) -> bool {
        self.settings
            .read()
            .map(|settings| settings.autostart)
            .unwrap_or(false)
    }

    pub fn summon_hotkey(&self) -> String {
        self.settings
            .read()
            .map(|settings| settings.summon_hotkey.clone())
            .unwrap_or_else(|_| "control+alt+Space".into())
    }

    pub fn set_locale(&self, locale: impl Into<String>) {
        if let Ok(mut settings) = self.settings.write() {
            settings.locale = locale.into();
        }
    }

    pub fn set_accent(&self, accent: impl Into<String>) {
        if let Ok(mut settings) = self.settings.write() {
            settings.accent = accent.into();
        }
    }

    pub fn set_autostart(&self, enabled: bool) {
        if let Ok(mut settings) = self.settings.write() {
            settings.autostart = enabled;
        }
    }

    pub fn set_summon_hotkey(&self, hotkey: impl Into<String>) {
        if let Ok(mut settings) = self.settings.write() {
            settings.summon_hotkey = hotkey.into();
        }
    }

    pub fn search_revision(&self) -> u64 {
        self.search_revision.load(Ordering::Relaxed)
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

pub fn services() -> Option<Arc<AppServices>> {
    engine_context::<Arc<AppServices>>().map(|services| services.as_ref().clone())
}

pub struct SearchState {
    engine: Engine,
    storage: std::rc::Rc<std::cell::RefCell<Storage>>,
    icon_cache: HashMap<PathBuf, Option<String>>,
    plugin_host: PluginHost,
    registry: Registry,
    app_scan: Option<crossbeam_channel::Receiver<Vec<AppEntry>>>,
    plugin_scan: Option<crossbeam_channel::Receiver<Vec<PluginMeta>>>,
    search_revision: Arc<AtomicU64>,
    next_app_scan: std::time::Instant,
    next_plugin_scan: std::time::Instant,
}

impl SearchState {
    pub fn new(
        data_dir: PathBuf,
        storage: std::rc::Rc<std::cell::RefCell<Storage>>,
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
            storage,
            icon_cache: HashMap::new(),
            plugin_host,
            registry,
            app_scan: None,
            plugin_scan: None,
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

    pub fn prepare_launch(
        &self,
        plugin_id: &str,
        command: &str,
        title: &str,
        input: &str,
        locale: &str,
        accent: &str,
    ) -> Result<steward_plugin_host::PluginLaunch, steward_plugin_host::PluginHostError> {
        let _ = title;
        self.plugin_host
            .prepare_launch(plugin_id, command, input, locale, accent, true)
    }

    pub fn accept_launch(
        &mut self,
        launch: &steward_plugin_host::PluginLaunch,
    ) -> anyhow::Result<()> {
        self.plugin_host.accept_launch(launch)
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
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        let trimmed = query.trim();

        if !trimmed.is_empty() {
            if let Some(value) = steward_core_engine::try_evaluate(trimmed) {
                let text = steward_core_engine::format_value(value);
                rows.push(row("calc", "Calculator", &text, None));
                actions.push(RowAction::CopyText(text));
            } else if steward_core_engine::try_openable(trimmed).is_some() {
                rows.push(row("url", "Open URL", trimmed, None));
                actions.push(RowAction::OpenUrl(trimmed.to_owned()));
            }
        }

        let app_hits = {
            let storage = self.storage.borrow();
            self.engine
                .query_scored(trimmed, &|path| storage.frequency_str(path))
        };
        for hit in app_hits {
            let icon = self.app_icon(&hit.app.path);
            let id = format!("app:{}", hit.app.path.display());
            rows.push(row(&id, &hit.app.name, "Application", icon));
            actions.push(RowAction::LaunchApp(hit.app.path));
        }

        for RouteHit {
            plugin_id,
            command,
            title,
            input,
            ..
        } in self.plugin_host.query(trimmed)
        {
            rows.push(row(
                &format!("plugin:{plugin_id}:{command}"),
                &title,
                "Plugin",
                None,
            ));
            actions.push(RowAction::OpenPlugin {
                plugin_id,
                command,
                title,
                input,
            });
        }
        (rows, actions)
    }

    fn app_icon(&mut self, path: &std::path::Path) -> Option<String> {
        if let Some(icon) = self.icon_cache.get(path) {
            return icon.clone();
        }
        #[cfg(target_os = "windows")]
        let icon = crate::app_icons::app_icon_data_url(path);
        #[cfg(not(target_os = "windows"))]
        let icon = None;
        self.icon_cache.insert(path.to_path_buf(), icon.clone());
        icon
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

fn row(
    id: &str,
    title: &str,
    subtitle: &str,
    icon: Option<String>,
) -> steward_ui_components::LauncherRow {
    steward_ui_components::LauncherRow {
        id: id.to_owned(),
        title: title.to_owned(),
        subtitle: subtitle.to_owned(),
        icon,
    }
}

//! Plugin host process lifecycle and the isolated NSH plugin window child.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    px, size, App, AppContext, Bounds, Context, Entity, Focusable, IntoElement, Render,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use nsh_engine::{HostCall, HostChannel, HostReply};
use nsh_platform_gpui::{AppOptions, AppView};
use steward_plugin_contract::{
    LaunchRequest, PluginErrorCode, PluginProcessAction, PluginProcessCommand, PluginProcessEvent,
    PluginProcessStart, PluginSessionId, PROTOCOL_VERSION,
};
use steward_plugin_host::{HostConfig, PluginHost, PluginLaunch};
use steward_plugin_registry::Registry;

use crate::nsh_surface::{new_view, plugin_limits};
use crate::state::{AppState, PluginRuntimeEvent};

const PLUGIN_PROCESS_ARG: &str = "--plugin-host-process";
const PLUGIN_WINDOW_WIDTH: f32 = 900.0;
const PLUGIN_WINDOW_HEIGHT: f32 = 660.0;
const PLUGIN_WINDOW_MIN_WIDTH: f32 = 720.0;
const PLUGIN_WINDOW_MIN_HEIGHT: f32 = 600.0;
const FIRST_FRAME_GRACE: Duration = Duration::from_millis(250);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(2);
const HOLIDAY_QUEUE_DEPTH: usize = 16;
static NEXT_PLUGIN_SESSION: AtomicU64 = AtomicU64::new(1);

/// The shell's handle to one isolated plugin host process.
pub(crate) struct PluginProcess {
    child: Child,
    input: ChildStdin,
    session_id: PluginSessionId,
    healthy: Arc<AtomicBool>,
    failure_reported: Arc<AtomicBool>,
    request: LaunchRequest,
}

enum PluginProcessExit {
    Running,
    Clean,
    Failed {
        code: PluginErrorCode,
        error: String,
    },
}

impl PluginProcess {
    pub(crate) fn session_id(&self) -> PluginSessionId {
        self.session_id
    }

    fn send(&mut self, action: PluginProcessAction) -> std::io::Result<()> {
        let command = PluginProcessCommand {
            protocol_version: PROTOCOL_VERSION,
            session_id: self.session_id,
            action,
        };
        write_json_line(&mut self.input, &command)
    }

    fn poll_exit(&mut self) -> PluginProcessExit {
        let status = match self.child.try_wait() {
            Ok(None) => return PluginProcessExit::Running,
            Ok(Some(status)) => status,
            Err(error) => {
                if !self.failure_reported.swap(true, Ordering::AcqRel) {
                    return PluginProcessExit::Failed {
                        code: PluginErrorCode::RuntimeError,
                        error: format!("inspect plugin host process: {error}"),
                    };
                }
                return PluginProcessExit::Clean;
            }
        };
        if self.healthy.load(Ordering::Acquire) {
            if status.success() || self.failure_reported.load(Ordering::Acquire) {
                return PluginProcessExit::Clean;
            }
            if !self.failure_reported.swap(true, Ordering::AcqRel) {
                return PluginProcessExit::Failed {
                    code: PluginErrorCode::RuntimeError,
                    error: format!("plugin host process exited unexpectedly with {status}"),
                };
            }
            return PluginProcessExit::Clean;
        }
        if !self.failure_reported.swap(true, Ordering::AcqRel) {
            return PluginProcessExit::Failed {
                code: PluginErrorCode::FirstFrameFailed,
                error: format!(
                    "plugin host process exited before reporting a healthy first frame ({status})"
                ),
            };
        }
        PluginProcessExit::Clean
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        let _ = self.send(PluginProcessAction::Close);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn is_plugin_host_process() -> bool {
    std::env::args_os().any(|argument| argument.to_string_lossy() == PLUGIN_PROCESS_ARG)
}

/// Entry point for `steward-app --plugin-host-process`.
///
/// This function never claims the shell's single-instance mutex, creates a
/// tray, or starts launcher services. It owns exactly one NSH window and exits
/// when that window closes or its parent closes the protocol pipe.
pub(crate) fn run_plugin_host() {
    let events = ChildEventSink::new();
    let start = match read_start() {
        Ok(start) if start.protocol_version == PROTOCOL_VERSION => start,
        Ok(start) => {
            events.failed_and_flush(
                start.session_id,
                PluginErrorCode::LoadFailed,
                format!(
                    "plugin host protocol version {} is unsupported; expected {PROTOCOL_VERSION}",
                    start.protocol_version
                ),
            );
            return;
        }
        Err(error) => {
            eprintln!("failed to read plugin host start message: {error}");
            return;
        }
    };

    let data_dir = data_dir();
    let _log_guard = match crate::logging::init(&data_dir) {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("failed to initialize plugin host logging: {error:#}");
            None
        }
    };
    #[cfg(target_os = "windows")]
    crate::platform::set_dpi_awareness();
    #[cfg(target_os = "windows")]
    crate::platform::enable_dark_mode();

    let launch = match prepare_launch(&start.request, &data_dir) {
        Ok(launch) => launch,
        Err((code, error)) => {
            events.failed_and_flush(start.session_id, code, error);
            return;
        }
    };
    run_child_window(start, launch, data_dir, events);
}

fn read_start() -> Result<PluginProcessStart, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if line.is_empty() {
        return Err("plugin host stdin closed before the start message".into());
    }
    serde_json::from_str(&line).map_err(|error| format!("parse plugin host start message: {error}"))
}

fn prepare_launch(
    request: &LaunchRequest,
    data_dir: &Path,
) -> Result<PluginLaunch, (PluginErrorCode, String)> {
    let plugins_root = std::env::var_os("STEWARD_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("plugins"));
    let mut registry = Registry::open_with_root(data_dir, &plugins_root)
        .map_err(|error| (PluginErrorCode::LoadFailed, format!("{error:#}")))?;
    let mut plugins = registry
        .cached_plugins()
        .map_err(|error| (PluginErrorCode::LoadFailed, format!("{error:#}")))?;
    if !plugins
        .iter()
        .any(|plugin| plugin.manifest.id == request.plugin_id)
    {
        registry
            .scan()
            .map_err(|error| (PluginErrorCode::LoadFailed, format!("{error:#}")))?;
        plugins = registry
            .cached_plugins()
            .map_err(|error| (PluginErrorCode::LoadFailed, format!("{error:#}")))?;
    }
    let mut host = PluginHost::new(HostConfig::new(data_dir.to_path_buf()))
        .map_err(|error| (PluginErrorCode::LoadFailed, format!("{error:#}")))?;
    for root in trust_roots() {
        host.add_trust_root_base64(root.trim())
            .map_err(|error| (PluginErrorCode::TrustFailure, format!("{error:#}")))?;
    }
    host.set_plugins(&plugins);
    host.prepare_launch(request.clone())
        .map_err(|error| (error.code(), error.to_string()))
}

fn data_dir() -> PathBuf {
    std::env::var_os("STEWARD_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("no OS data directory available")
                .join("Steward")
        })
}

fn trust_roots() -> Vec<String> {
    let mut roots =
        serde_json::from_str::<Vec<String>>(include_str!("../../../assets/nsh/trust-roots.json"))
            .expect("assets/nsh/trust-roots.json must be a JSON string array");
    if let Ok(root) = std::env::var("STEWARD_NSH_TRUST_ROOT") {
        roots.push(root);
    }
    roots
}

#[derive(Clone)]
struct ChildEventSink {
    events: crossbeam_channel::Sender<ChildEventMessage>,
}

struct ChildEventMessage {
    event: PluginProcessEvent,
    flushed: Option<crossbeam_channel::Sender<()>>,
}

impl ChildEventSink {
    fn new() -> Self {
        let (events, receiver) = crossbeam_channel::unbounded::<ChildEventMessage>();
        std::thread::Builder::new()
            .name("steward-plugin-protocol".into())
            .spawn(move || {
                let stdout = std::io::stdout();
                let mut output = stdout.lock();
                for message in receiver {
                    let result = write_json_line(&mut output, &message.event);
                    if let Some(flushed) = message.flushed {
                        let _ = flushed.send(());
                    }
                    if result.is_err() {
                        break;
                    }
                }
            })
            .expect("spawn plugin protocol writer");
        Self { events }
    }

    fn send(&self, event: PluginProcessEvent) {
        let _ = self.events.send(ChildEventMessage {
            event,
            flushed: None,
        });
    }

    fn send_and_flush(&self, event: PluginProcessEvent) {
        let (flushed, receiver) = crossbeam_channel::bounded(1);
        if self
            .events
            .send(ChildEventMessage {
                event,
                flushed: Some(flushed),
            })
            .is_ok()
        {
            let _ = receiver.recv_timeout(Duration::from_secs(1));
        }
    }

    fn rendered(&self, session_id: PluginSessionId) {
        self.send(PluginProcessEvent::Rendered {
            protocol_version: PROTOCOL_VERSION,
            session_id,
        });
    }

    fn healthy(&self, session_id: PluginSessionId) {
        self.send(PluginProcessEvent::Healthy {
            protocol_version: PROTOCOL_VERSION,
            session_id,
        });
    }

    fn failed(&self, session_id: PluginSessionId, code: PluginErrorCode, error: impl Into<String>) {
        self.send(PluginProcessEvent::Failed {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            code,
            error: error.into(),
        });
    }

    fn failed_and_flush(
        &self,
        session_id: PluginSessionId,
        code: PluginErrorCode,
        error: impl Into<String>,
    ) {
        self.send_and_flush(PluginProcessEvent::Failed {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            code,
            error: error.into(),
        });
    }
}

fn write_json_line(writer: &mut impl Write, value: &impl serde::Serialize) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

struct PluginFrameView {
    app: Entity<AppView>,
    session_id: PluginSessionId,
    events: ChildEventSink,
    fatal_error: Arc<Mutex<Option<String>>>,
    first_frame_reported: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
}

impl Render for PluginFrameView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_reported.swap(true, Ordering::AcqRel) {
            self.events.rendered(self.session_id);
            let session_id = self.session_id;
            let events = self.events.clone();
            let fatal_error = self.fatal_error.clone();
            let healthy = self.healthy.clone();
            cx.spawn(async move |_view, cx| {
                cx.background_executor().timer(FIRST_FRAME_GRACE).await;
                let no_fatal_error = fatal_error
                    .lock()
                    .map(|fatal| fatal.is_none())
                    .unwrap_or(false);
                if no_fatal_error {
                    healthy.store(true, Ordering::Release);
                    events.healthy(session_id);
                }
            })
            .detach();
        }
        self.app.clone()
    }
}

struct HolidayRequest {
    year: u16,
    reply: HostReply,
}

fn start_holiday_worker(cache_dir: PathBuf) -> crossbeam_channel::Sender<HolidayRequest> {
    let (tx, rx) = crossbeam_channel::bounded::<HolidayRequest>(HOLIDAY_QUEUE_DEPTH);
    std::thread::Builder::new()
        .name("steward-holiday-host".into())
        .spawn(move || {
            while let Ok(request) = rx.recv() {
                match crate::holiday_schedule::load_year(request.year, &cache_dir) {
                    Ok(schedule) => {
                        request
                            .reply
                            .resolve(serde_json::to_value(schedule).unwrap_or_default());
                    }
                    Err(error) => {
                        tracing::warn!(
                            year = request.year,
                            %error,
                            "calendar holiday schedule is unavailable"
                        );
                        request.reply.resolve(serde_json::json!({}));
                    }
                }
            }
        })
        .expect("spawn holiday host worker");
    tx
}

fn run_child_window(
    start: PluginProcessStart,
    launch: PluginLaunch,
    data_dir: PathBuf,
    events: ChildEventSink,
) {
    let session_id = start.session_id;
    let initial = start.request;
    let (control_tx, control_rx) =
        crossbeam_channel::bounded::<PluginProcessCommand>(HOLIDAY_QUEUE_DEPTH);
    let reader_control = control_tx.clone();
    std::thread::Builder::new()
        .name("steward-plugin-control".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in BufReader::new(stdin.lock()).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(command) = serde_json::from_str::<PluginProcessCommand>(&line) else {
                    tracing::warn!("ignore malformed plugin process command");
                    continue;
                };
                if command.protocol_version != PROTOCOL_VERSION || command.session_id != session_id
                {
                    tracing::warn!("ignore plugin process command for another session");
                    continue;
                }
                if reader_control.send(command).is_err() {
                    return;
                }
            }
            let _ = reader_control.send(PluginProcessCommand {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                action: PluginProcessAction::Close,
            });
        })
        .expect("spawn plugin control reader");

    let nsh_data_dir = data_dir.join("nsh-data");
    let storage_path = nsh_data_dir
        .join(&initial.plugin_id)
        .join("steward-host-storage.json");
    let plugin_storage = Arc::new(Mutex::new(load_plugin_storage(&storage_path)));
    let host = HostChannel::new();
    host.set_params(launch.params.clone());
    host.handle("steward.clipboard.history", {
        let data_dir = data_dir.clone();
        move |call| {
            let entries = crate::clipboard_history::entries_for_plugin(&data_dir);
            call.resolve(serde_json::to_value(entries).unwrap_or_default());
        }
    });
    host.handle("steward.clipboard.write", |call: HostCall| {
        let Some(text) = call
            .data()
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
        else {
            call.reject("steward.clipboard.write requires a text string");
            return;
        };
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
            Ok(()) => call.resolve(serde_json::json!({})),
            Err(error) => call.reject(format!("write clipboard text: {error}")),
        }
    });
    host.handle("steward.storage.get", {
        let storage = plugin_storage.clone();
        move |call| {
            let Some(key) = call
                .data()
                .get("key")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
            else {
                call.reject("steward.storage.get requires a key string");
                return;
            };
            let value = storage
                .lock()
                .ok()
                .and_then(|storage| storage.get(&key).cloned());
            call.resolve(value.map_or(serde_json::Value::Null, serde_json::Value::String));
        }
    });
    host.handle("steward.storage.set", {
        let storage = plugin_storage.clone();
        let storage_path = storage_path.clone();
        move |call| {
            let Some(key) = call
                .data()
                .get("key")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
            else {
                call.reject("steward.storage.set requires a key string");
                return;
            };
            let Some(value) = call
                .data()
                .get("value")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
            else {
                call.reject("steward.storage.set requires a value string");
                return;
            };
            if key.is_empty() || key.len() > 256 || value.len() > 1024 * 1024 {
                call.reject("plugin storage key or value exceeds its limit");
                return;
            }
            let result = storage
                .lock()
                .map_err(|_| "plugin storage lock poisoned".to_owned())
                .and_then(|mut storage| {
                    storage.insert(key, value);
                    persist_plugin_storage(&storage_path, &storage)
                });
            match result {
                Ok(()) => call.resolve(serde_json::json!({})),
                Err(error) => call.reject(format!("write plugin storage: {error}")),
            }
        }
    });
    host.handle("steward.calendar.holidays", {
        let cache_dir = data_dir.join("calendar-holidays");
        let worker = Arc::new(OnceLock::new());
        move |call| {
            let Some(year) = call.data().get("year").and_then(|value| value.as_u64()) else {
                call.reject("steward.calendar.holidays requires an integer year");
                return;
            };
            let Ok(year) = u16::try_from(year) else {
                call.reject("steward.calendar.holidays year is out of range");
                return;
            };
            let Some(reply) = call.into_reply() else {
                return;
            };
            let sender = worker.get_or_init(|| start_holiday_worker(cache_dir.clone()));
            let request = HolidayRequest { year, reply };
            match sender.try_send(request) {
                Ok(()) => {}
                Err(crossbeam_channel::TrySendError::Full(request)) => {
                    request
                        .reply
                        .reject("steward.calendar.holidays is busy; retry later");
                }
                Err(crossbeam_channel::TrySendError::Disconnected(request)) => {
                    request
                        .reply
                        .reject("steward.calendar.holidays host worker is unavailable");
                }
            }
        }
    });
    host.handle("steward.window.close", {
        let control = control_tx.clone();
        move |call| {
            let _ = control.try_send(PluginProcessCommand {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                action: PluginProcessAction::Close,
            });
            call.resolve(serde_json::json!({}));
        }
    });

    gpui_platform::application()
        .with_assets(gpui_kit::assets::Assets)
        .run({
            let host = host.clone();
            move |cx: &mut App| {
                nsh_platform_gpui::set_ui_sets(vec![nsh_base::registry()]);
                let plugin_id = initial.plugin_id.clone();
                let title = initial.title.clone();
                let app_id = launch.app_id.clone();
                let package = launch.package;
                let bounds = Bounds::centered(
                    None,
                    size(px(PLUGIN_WINDOW_WIDTH), px(PLUGIN_WINDOW_HEIGHT)),
                    cx,
                );
                let fatal_error = Arc::new(Mutex::new(None::<String>));
                let healthy = Arc::new(AtomicBool::new(false));
                let first_frame_reported = Arc::new(AtomicBool::new(false));
                let request = initial.clone();
                let handle = cx
                    .open_window(
                        WindowOptions {
                            window_bounds: Some(WindowBounds::Windowed(bounds)),
                            titlebar: Some(TitlebarOptions {
                                title: Some(title.clone().into()),
                                ..Default::default()
                            }),
                            is_resizable: true,
                            window_min_size: Some(size(
                                px(PLUGIN_WINDOW_MIN_WIDTH),
                                px(PLUGIN_WINDOW_MIN_HEIGHT),
                            )),
                            show: true,
                            focus: true,
                            ..Default::default()
                        },
                        {
                            let host = host.clone();
                            let events = events.clone();
                            let fatal_error = fatal_error.clone();
                            let healthy = healthy.clone();
                            let first_frame_reported = first_frame_reported.clone();
                            move |window, cx| {
                                let app = cx.new(|cx| {
                                    let options = AppOptions {
                                        app_id,
                                        title: plugin_id.clone(),
                                        data_dir: Some(
                                            nsh_data_dir.clone().join(&request.plugin_id),
                                        ),
                                        limits: plugin_limits(),
                                        host: host.clone(),
                                        ..AppOptions::default()
                                    };
                                    let view = new_view(options, window, cx);
                                    view.on_app_error({
                                        let events = events.clone();
                                        let fatal_error = fatal_error.clone();
                                        let healthy = healthy.clone();
                                        let request = request.clone();
                                        move |error| {
                                            tracing::error!(
                                                plugin_id = %request.plugin_id,
                                                error = %error,
                                                fatal = error.is_fatal(),
                                                "plugin application error"
                                            );
                                            if error.is_fatal() {
                                                let message = error.to_string();
                                                if let Ok(mut fatal) = fatal_error.lock() {
                                                    if fatal.is_none() {
                                                        *fatal = Some(message.clone());
                                                        events.failed(
                                                            session_id,
                                                            if healthy.load(Ordering::Relaxed) {
                                                                PluginErrorCode::RuntimeError
                                                            } else {
                                                                PluginErrorCode::FirstFrameFailed
                                                            },
                                                            message,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    });
                                    view.run_package(&package);
                                    view.focus_handle(cx).focus(window, cx);
                                    view
                                });
                                cx.new(|_| PluginFrameView {
                                    app,
                                    session_id,
                                    events: events.clone(),
                                    fatal_error: fatal_error.clone(),
                                    first_frame_reported: first_frame_reported.clone(),
                                    healthy: healthy.clone(),
                                })
                            }
                        },
                    )
                    .expect("open plugin window");

                let timeout_events = events.clone();
                let timeout_first_frame = first_frame_reported.clone();
                let timeout_fatal = fatal_error.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(FIRST_FRAME_TIMEOUT).await;
                    if !timeout_first_frame.load(Ordering::Acquire)
                        && timeout_fatal
                            .lock()
                            .map(|fatal| fatal.is_none())
                            .unwrap_or(false)
                    {
                        timeout_events.failed(
                            session_id,
                            PluginErrorCode::FirstFrameFailed,
                            "plugin did not render its first frame within 2 seconds",
                        );
                    }
                })
                .detach();

                let control_host = host.clone();
                cx.spawn(async move |cx| loop {
                    while let Ok(command) = control_rx.try_recv() {
                        match command.action {
                            PluginProcessAction::Command { command, input } => {
                                let _ = control_host.send(
                                    "steward.command",
                                    serde_json::json!({
                                        "command": command,
                                        "input": input,
                                    }),
                                );
                            }
                            PluginProcessAction::Theme { accent, dark } => {
                                let _ = control_host.send(
                                    "steward.theme",
                                    serde_json::json!({ "accent": accent, "dark": dark }),
                                );
                            }
                            PluginProcessAction::Focus => {
                                let _ = handle.update(cx, |_, window, _| {
                                    crate::platform::show(
                                        window,
                                        PLUGIN_WINDOW_WIDTH,
                                        PLUGIN_WINDOW_HEIGHT,
                                    );
                                });
                                cx.update(|cx| cx.activate(true));
                            }
                            PluginProcessAction::Close => {
                                let _ = handle.update(cx, |_, window, _| {
                                    window.remove_window();
                                });
                                cx.update(|cx| cx.quit());
                                return;
                            }
                        }
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                })
                .detach();
            }
        });
}

pub(crate) fn focus_and_send(
    _cx: &mut App,
    state: &Rc<RefCell<AppState>>,
    request: &LaunchRequest,
) -> bool {
    let mut state = state.borrow_mut();
    let Some(process) = state.plugin_windows.get_mut(&request.plugin_id) else {
        return false;
    };
    process
        .send(PluginProcessAction::Command {
            command: request.command.clone(),
            input: request.input.clone(),
        })
        .and_then(|()| process.send(PluginProcessAction::Focus))
        .is_ok()
}

pub(crate) fn open_launch(
    cx: &mut App,
    state: Rc<RefCell<AppState>>,
    requested_at: Instant,
    initial: LaunchRequest,
    latest: LaunchRequest,
    launch: PluginLaunch,
) -> anyhow::Result<()> {
    if focus_and_send(cx, &state, &latest) {
        return Ok(());
    }

    let events = state.borrow().plugin_events.clone();
    let session_id = NEXT_PLUGIN_SESSION.fetch_add(1, Ordering::Relaxed);
    let process = spawn_process(
        session_id,
        requested_at,
        initial.clone(),
        launch.package.receipt.clone(),
        events,
    )?;
    state
        .borrow_mut()
        .plugin_windows
        .insert(initial.plugin_id.clone(), process);
    crate::metrics::value(
        "plugin_host_process_count",
        state.borrow().plugin_windows.values().count() as u64,
        serde_json::json!({ "plugin_id": initial.plugin_id.clone() }),
    );
    crate::metrics::value(
        "hidden_plugin_vm_count",
        0,
        serde_json::json!({ "phase": "plugin_opened" }),
    );

    if latest.command != initial.command || latest.input != initial.input {
        let _ = focus_and_send(cx, &state, &latest);
    }
    Ok(())
}

fn spawn_process(
    session_id: PluginSessionId,
    requested_at: Instant,
    request: LaunchRequest,
    receipt: nsh_package::PackageReceipt,
    events: crossbeam_channel::Sender<PluginRuntimeEvent>,
) -> anyhow::Result<PluginProcess> {
    let executable = std::env::current_exe()?;
    let mut child = Command::new(executable)
        .arg(PLUGIN_PROCESS_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("plugin host process did not expose a control pipe"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("plugin host process did not expose an event pipe"))?;
    write_json_line(
        &mut input,
        &PluginProcessStart {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            request: request.clone(),
        },
    )?;

    let healthy = Arc::new(AtomicBool::new(false));
    let failure_reported = Arc::new(AtomicBool::new(false));
    spawn_event_reader(
        output,
        ProcessEventContext {
            plugin_id: request.plugin_id.clone(),
            request: request.clone(),
            session_id,
            requested_at,
            receipt,
            events,
            healthy: healthy.clone(),
            failure_reported: failure_reported.clone(),
        },
    );
    Ok(PluginProcess {
        child,
        input,
        session_id,
        healthy,
        failure_reported,
        request,
    })
}

struct ProcessEventContext {
    plugin_id: String,
    request: LaunchRequest,
    session_id: PluginSessionId,
    requested_at: Instant,
    receipt: nsh_package::PackageReceipt,
    events: crossbeam_channel::Sender<PluginRuntimeEvent>,
    healthy: Arc<AtomicBool>,
    failure_reported: Arc<AtomicBool>,
}

fn spawn_event_reader(output: std::process::ChildStdout, context: ProcessEventContext) {
    let ProcessEventContext {
        plugin_id,
        request,
        session_id,
        requested_at,
        receipt,
        events,
        healthy,
        failure_reported,
    } = context;
    std::thread::Builder::new()
        .name(format!("steward-plugin-events-{session_id}"))
        .spawn(move || {
            let mut rendered_at = None;
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                let event = match serde_json::from_str::<PluginProcessEvent>(&line) {
                    Ok(event) => event,
                    Err(error) => {
                        report_protocol_failure(
                            &events,
                            &plugin_id,
                            &request,
                            session_id,
                            &failure_reported,
                            PluginErrorCode::RuntimeError,
                            format!("parse plugin host event: {error}"),
                        );
                        return;
                    }
                };
                match event {
                    PluginProcessEvent::Rendered {
                        protocol_version,
                        session_id: event_session,
                    } => {
                        if protocol_version != PROTOCOL_VERSION || event_session != session_id {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host sent a rendered event for another session".into(),
                            );
                            return;
                        }
                        rendered_at = Some(Instant::now());
                    }
                    PluginProcessEvent::Healthy {
                        protocol_version,
                        session_id: event_session,
                    } => {
                        let Some(first_frame_at) = rendered_at else {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host reported healthy before rendering a frame".into(),
                            );
                            return;
                        };
                        if protocol_version != PROTOCOL_VERSION || event_session != session_id {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host sent a healthy event for another session".into(),
                            );
                            return;
                        }
                        healthy.store(true, Ordering::Release);
                        let _ = events.send(PluginRuntimeEvent::Healthy {
                            plugin_id: plugin_id.clone(),
                            session_id,
                            receipt: receipt.clone(),
                            requested_at,
                            first_frame_at,
                        });
                    }
                    PluginProcessEvent::Failed {
                        protocol_version,
                        session_id: event_session,
                        code,
                        error,
                    } => {
                        if protocol_version != PROTOCOL_VERSION || event_session != session_id {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host sent a failure event for another session".into(),
                            );
                            return;
                        }
                        if !failure_reported.swap(true, Ordering::AcqRel) {
                            let _ = events.send(PluginRuntimeEvent::Failed {
                                plugin_id: plugin_id.clone(),
                                session_id,
                                request: request.clone(),
                                code,
                                error,
                            });
                        }
                    }
                }
            }
            if !healthy.load(Ordering::Acquire) && !failure_reported.load(Ordering::Acquire) {
                let error = if rendered_at.is_some() {
                    "plugin host exited after rendering but before becoming healthy"
                } else {
                    "plugin host exited before rendering its first frame"
                };
                report_protocol_failure(
                    &events,
                    &plugin_id,
                    &request,
                    session_id,
                    &failure_reported,
                    PluginErrorCode::FirstFrameFailed,
                    error.into(),
                );
            }
        })
        .expect("spawn plugin event reader");
}

fn report_protocol_failure(
    events: &crossbeam_channel::Sender<PluginRuntimeEvent>,
    plugin_id: &str,
    request: &LaunchRequest,
    session_id: PluginSessionId,
    failure_reported: &AtomicBool,
    code: PluginErrorCode,
    error: String,
) {
    if !failure_reported.swap(true, Ordering::AcqRel) {
        let _ = events.send(PluginRuntimeEvent::Failed {
            plugin_id: plugin_id.into(),
            session_id,
            request: request.clone(),
            code,
            error,
        });
    }
}

pub(crate) fn poll_processes(state: &Rc<RefCell<AppState>>) {
    let mut exited = Vec::new();
    state
        .borrow_mut()
        .plugin_windows
        .retain(|_, process| match process.poll_exit() {
            PluginProcessExit::Running => true,
            PluginProcessExit::Clean => false,
            PluginProcessExit::Failed { code, error } => {
                exited.push((process.request.clone(), process.session_id, code, error));
                false
            }
        });
    if exited.is_empty() {
        return;
    }
    let event_sender = state.borrow().plugin_events.clone();
    for (request, session_id, code, error) in exited {
        let _ = event_sender.send(PluginRuntimeEvent::Failed {
            plugin_id: request.plugin_id.clone(),
            session_id,
            request,
            code,
            error,
        });
    }
    crate::metrics::value(
        "plugin_host_process_count",
        state.borrow().plugin_windows.values().count() as u64,
        serde_json::json!({ "phase": "process_exited" }),
    );
}

fn load_plugin_storage(path: &Path) -> BTreeMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn persist_plugin_storage(path: &Path, storage: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec(storage).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

pub(crate) fn close(_cx: &mut App, state: &Rc<RefCell<AppState>>, plugin_id: &str) {
    let process = state.borrow_mut().plugin_windows.remove(plugin_id);
    drop(process);
    crate::metrics::value(
        "plugin_host_process_count",
        state.borrow().plugin_windows.values().count() as u64,
        serde_json::json!({ "plugin_id": plugin_id, "phase": "closed" }),
    );
}

pub(crate) fn broadcast_theme(state: &Rc<RefCell<AppState>>, accent: &str) {
    let mut state = state.borrow_mut();
    for process in state.plugin_windows.values_mut() {
        let _ = process.send(PluginProcessAction::Theme {
            accent: accent.into(),
            dark: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_private_storage_roundtrips_through_one_json_file() {
        let dir = std::env::temp_dir().join(format!(
            "steward-plugin-storage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("steward-host-storage.json");
        let values = BTreeMap::from([
            ("pinned".into(), r#"["one","two"]"#.into()),
            ("theme".into(), "dark".into()),
        ]);
        persist_plugin_storage(&path, &values).unwrap();
        assert_eq!(load_plugin_storage(&path), values);
        std::fs::remove_dir_all(dir).ok();
    }
}

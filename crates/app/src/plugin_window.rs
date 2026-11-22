//! Plugin host process lifecycle and the isolated NSH plugin window child.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    div, px, size, App, AppContext, Bounds, Context, Entity, Focusable, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, Render, Styled, TitlebarOptions, Window,
    WindowBounds, WindowKind, WindowOptions,
};
use nsh_engine::{HostCall, HostChannel, HostReply};
use nsh_platform_gpui::{AppOptions, AppView};
use steward_plugin_contract::capabilities::{
    EXEC_CANCEL, EXEC_RUN, FS_CANCEL_WALK, FS_GRANTS, FS_PICK, FS_READ_TEXT, FS_WALK,
    FS_WRITE_TEXT, NAVIGATION_BACK, NAVIGATION_CLOSE, NAVIGATION_SET_TITLE, NET_FETCH,
    SYSTEM_APPLICATIONS, SYSTEM_OPEN_PATH, SYSTEM_OPEN_WITH, SYSTEM_REVEAL_PATH,
};
use steward_plugin_contract::host_api::{
    parse_call_data, require_null_payload, CalendarHolidaysRequest, CancelWalkRequest,
    ClipboardWriteRequest, ConfirmRequest, ExecutionCancelRequest, HostCallError, HostErrorCode,
    PathPickMode, PathPickRequest, PreferencesGetRequest, ProgressRequest, ReadTextRequest,
    SetTitleRequest, StorageGetRequest, StorageSetRequest, SystemOpenWithRequest,
    SystemPathRequest, ToastRequest, WalkRequest, WriteTextRequest,
};
use steward_plugin_contract::{
    LaunchRequest, PluginErrorCode, PluginExecutionRequest, PluginExecutionResult, PluginNotice,
    PluginNoticeKind, PluginNoticeLevel, PluginPresentationEvent, PluginProcessAction,
    PluginProcessCommand, PluginProcessEvent, PluginProcessStart, PluginSessionId,
    PresentationMode, PROTOCOL_VERSION,
};
use steward_plugin_host::{HostConfig, PluginAuthorization, PluginHost, PluginLaunch};
use steward_plugin_registry::Registry;
use steward_storage::{NewCapabilityAuditEntry, Storage};

use crate::host_errors::RejectHostError;
use crate::nsh_surface::{new_view, plugin_limits};
use crate::plugin_containment::PluginProcessContainment;
use crate::plugin_execution::{ExecutionAudit, ExecutionBroker, ExecutionResultSink};
use crate::plugin_filesystem::{display_path, FileAccess, FileService, PickerMode};
use crate::plugin_network::{NetworkAudit, NetworkRequest, NetworkService};
use crate::plugin_system::SystemService;
use crate::state::{AppState, PluginRuntimeEvent};

const PLUGIN_PROCESS_ARG: &str = "--plugin-host-process";
const PLUGIN_WINDOW_WIDTH: f32 = 900.0;
const PLUGIN_WINDOW_HEIGHT: f32 = 660.0;
const PLUGIN_EMBEDDED_WIDTH: f32 = 760.0;
const PLUGIN_EMBEDDED_HEIGHT: f32 = 660.0;
const PLUGIN_WINDOW_MIN_WIDTH: f32 = 720.0;
const PLUGIN_WINDOW_MIN_HEIGHT: f32 = 600.0;
const FIRST_FRAME_GRACE: Duration = Duration::from_millis(250);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(2);
const HOLIDAY_QUEUE_DEPTH: usize = 16;
const DEFAULT_WALK_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_WALK_RESULTS: u64 = 2_000;
const DEFAULT_WALK_DEPTH: u64 = 32;
const DEFAULT_READ_BYTES: u64 = 1024 * 1024;
static NEXT_PLUGIN_SESSION: AtomicU64 = AtomicU64::new(1);
static NEXT_CONFIRMATION_ID: AtomicU64 = AtomicU64::new(1);

/// The shell's handle to one isolated plugin host process.
pub(crate) struct PluginProcess {
    child: Child,
    input: Arc<Mutex<ChildStdin>>,
    containment: PluginProcessContainment,
    session_id: PluginSessionId,
    healthy: Arc<AtomicBool>,
    failure_reported: Arc<AtomicBool>,
    pipe_closed: Arc<AtomicBool>,
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

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn request(&self) -> &LaunchRequest {
        &self.request
    }

    fn send(&mut self, action: PluginProcessAction) -> std::io::Result<()> {
        send_process_action(&self.input, self.session_id, action)
    }

    fn poll_exit(&mut self) -> PluginProcessExit {
        if let Some(error) = self.containment.poll_limit_violation() {
            crate::metrics::value(
                "plugin_host_resource_violation",
                1,
                serde_json::json!({
                    "plugin_id": self.request.plugin_id.clone(),
                    "child_pid": self.pid(),
                    "error": error,
                }),
            );
            return self.fail_once(PluginErrorCode::ResourceLimit, error);
        }
        if self.pipe_closed.load(Ordering::Acquire)
            && !self.failure_reported.load(Ordering::Acquire)
        {
            let _ = self.child.kill();
            let status = self.child.wait().ok();
            let (resource_limit, error) =
                self.containment.describe_exit(status, "event pipe closed");
            let code = if resource_limit {
                PluginErrorCode::ResourceLimit
            } else if self.healthy.load(Ordering::Acquire) {
                PluginErrorCode::RuntimeError
            } else {
                PluginErrorCode::FirstFrameFailed
            };
            return self.fail_once(code, error);
        }
        let status = match self.child.try_wait() {
            Ok(None) => return PluginProcessExit::Running,
            Ok(Some(status)) => status,
            Err(error) => {
                return self.fail_once(
                    PluginErrorCode::RuntimeError,
                    format!("inspect plugin host process: {error}"),
                );
            }
        };
        if self.failure_reported.load(Ordering::Acquire) {
            return PluginProcessExit::Clean;
        }
        if self.healthy.load(Ordering::Acquire) {
            if status.success() {
                return PluginProcessExit::Clean;
            }
            let (resource_limit, error) =
                self.containment.describe_exit(Some(status), "process exit");
            let code = if resource_limit {
                PluginErrorCode::ResourceLimit
            } else {
                PluginErrorCode::RuntimeError
            };
            return self.fail_once(code, error);
        }
        let (resource_limit, error) = self.containment.describe_exit(Some(status), "process exit");
        let code = if resource_limit {
            PluginErrorCode::ResourceLimit
        } else {
            PluginErrorCode::FirstFrameFailed
        };
        self.fail_once(code, error)
    }

    fn fail_once(&self, code: PluginErrorCode, error: String) -> PluginProcessExit {
        if self.failure_reported.swap(true, Ordering::AcqRel) {
            PluginProcessExit::Clean
        } else {
            PluginProcessExit::Failed { code, error }
        }
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
    if let Ok(roots_json) = std::env::var("STEWARD_NSH_TRUST_ROOTS_JSON") {
        if let Ok(mut additional) = serde_json::from_str::<Vec<String>>(&roots_json) {
            roots.append(&mut additional);
        }
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

    fn notice(&self, session_id: PluginSessionId, notice: PluginNotice) {
        self.send(PluginProcessEvent::Notice {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            notice,
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

    fn execution_request(&self, session_id: PluginSessionId, request: PluginExecutionRequest) {
        self.send(PluginProcessEvent::ExecutionRequest {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            request,
        });
    }

    fn execution_cancel(&self, session_id: PluginSessionId, request_id: u64) {
        self.send(PluginProcessEvent::ExecutionCancel {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            request_id,
        });
    }

    fn presentation(&self, session_id: PluginSessionId, event: PluginPresentationEvent) {
        self.send(PluginProcessEvent::Presentation {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            event,
        });
    }

    fn presentation_and_flush(&self, session_id: PluginSessionId, event: PluginPresentationEvent) {
        self.send_and_flush(PluginProcessEvent::Presentation {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            event,
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

fn send_process_action(
    input: &Arc<Mutex<ChildStdin>>,
    session_id: PluginSessionId,
    action: PluginProcessAction,
) -> std::io::Result<()> {
    let command = PluginProcessCommand {
        protocol_version: PROTOCOL_VERSION,
        session_id,
        action,
    };
    let mut input = input
        .lock()
        .map_err(|_| std::io::Error::other("plugin control pipe lock is poisoned"))?;
    write_json_line(&mut *input, &command)
}

struct PluginFrameView {
    app: Entity<AppView>,
    session_id: PluginSessionId,
    events: ChildEventSink,
    embedded: bool,
    escape_requested: Arc<AtomicBool>,
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
        let escape_requested = self.escape_requested.clone();
        let embedded = self.embedded;
        div()
            .size_full()
            .child(self.app.clone())
            .on_key_down(move |event, _window, _cx| {
                if embedded && event.keystroke.key.eq_ignore_ascii_case("escape") {
                    escape_requested.store(true, Ordering::Release);
                }
            })
    }
}

struct HolidayRequest {
    year: u16,
    reply: HostReply,
}

struct PathPickerRequest {
    mode: PickerMode,
    reply: HostReply,
}

fn parse_execution_request(data: &serde_json::Value) -> Result<PluginExecutionRequest, String> {
    let request_id = data
        .get("requestId")
        .and_then(serde_json::Value::as_u64)
        .filter(|request_id| *request_id > 0)
        .ok_or_else(|| "runProcess requires a positive requestId".to_owned())?;
    let executable = data
        .get("executable")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 260)
        .ok_or_else(|| "runProcess requires a bounded executable string".to_owned())?
        .to_owned();
    let arguments =
        data.get("arguments")
            .map(|value| {
                value
                    .as_array()
                    .ok_or_else(|| "runProcess arguments must be an array".to_owned())?
                    .iter()
                    .map(|argument| {
                        argument.as_str().map(str::to_owned).ok_or_else(|| {
                            "runProcess arguments must contain only strings".to_owned()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
    if arguments.len() > 256 {
        return Err("runProcess accepts at most 256 arguments".into());
    }
    let working_directory = data
        .get("workingDirectory")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "runProcess requires a workingDirectory string".to_owned())?
        .to_owned();
    let timeout_ms = data
        .get("timeoutMs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(30_000);
    let max_output_bytes = data
        .get("maxOutputBytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1024 * 1024);
    Ok(PluginExecutionRequest {
        request_id,
        executable,
        arguments,
        working_directory,
        timeout_ms,
        max_output_bytes,
    })
}

fn resolve_execution_reply(reply: HostReply, result: PluginExecutionResult) {
    if let Some(code) = result.error_code {
        reply.reject_host_error(HostCallError::new(
            public_error_code(code),
            result
                .error
                .as_deref()
                .unwrap_or("controlled process failed"),
        ));
    } else {
        reply.resolve(serde_json::to_value(result).unwrap_or_default());
    }
}

fn public_error_code(code: PluginErrorCode) -> HostErrorCode {
    match code {
        PluginErrorCode::PermissionDenied => HostErrorCode::PermissionDenied,
        PluginErrorCode::FeatureUnavailable => HostErrorCode::FeatureUnavailable,
        PluginErrorCode::Timeout => HostErrorCode::Timeout,
        PluginErrorCode::ResourceLimit | PluginErrorCode::ResourceExhausted => {
            HostErrorCode::ResourceExhausted
        }
        PluginErrorCode::InvalidInput => HostErrorCode::InvalidInput,
        PluginErrorCode::Cancelled => HostErrorCode::Cancelled,
        PluginErrorCode::ConfirmationDenied => HostErrorCode::ConfirmationDenied,
        PluginErrorCode::IoFailure
        | PluginErrorCode::NotFound
        | PluginErrorCode::CommandNotFound
        | PluginErrorCode::ReadFailed => HostErrorCode::IoFailure,
        PluginErrorCode::TrustFailure
        | PluginErrorCode::IdMismatch
        | PluginErrorCode::VersionMismatch
        | PluginErrorCode::LoadFailed
        | PluginErrorCode::FirstFrameFailed
        | PluginErrorCode::RuntimeError
        | PluginErrorCode::HostFailure => HostErrorCode::HostFailure,
    }
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
    let host_plugin_id = initial.plugin_id.clone();
    let audit = CapabilityAuditSink::new(&data_dir, &launch.authorization);
    let file_access = FileAccess {
        data_dir: data_dir.clone(),
        plugin_id: launch.authorization.plugin_id.clone(),
        publisher_fingerprint: launch.authorization.publisher_fingerprint.clone(),
        version: launch.authorization.version.clone(),
    };
    let files = FileService::new(file_access.clone());
    let system = SystemService::new(file_access);
    let network_audit: NetworkAudit = {
        let audit = audit.clone();
        Arc::new(move |capability, scope, outcome, detail| {
            audit.record_outcome(capability, scope, outcome, detail);
        })
    };
    let network = NetworkService::new(
        data_dir.clone(),
        launch.authorization.clone(),
        network_audit,
    );
    let (control_tx, control_rx) =
        crossbeam_channel::bounded::<PluginProcessCommand>(HOLIDAY_QUEUE_DEPTH);
    let (picker_tx, picker_rx) =
        crossbeam_channel::bounded::<PathPickerRequest>(HOLIDAY_QUEUE_DEPTH);
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
    let pending_confirmations = Arc::new(Mutex::new(HashMap::<u64, HostReply>::new()));
    let pending_executions = Arc::new(Mutex::new(HashMap::<u64, HostReply>::new()));
    let host = HostChannel::new();
    host.set_params(launch.params.clone());
    host.handle("steward.preferences.get", {
        let preferences = launch.preferences.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record("steward.preferences.get");
            let request = match parse_call_data::<PreferencesGetRequest>(
                "steward.preferences.get",
                call.data(),
            ) {
                Ok(request) if !request.key.is_empty() && request.key.len() <= 256 => request,
                Ok(_) => {
                    call.reject_code(
                        HostErrorCode::InvalidInput,
                        "steward.preferences.get requires a key of at most 256 bytes",
                    );
                    return;
                }
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            let value = preferences.get(&request.key).cloned().unwrap_or_default();
            call.resolve(value);
        }
    });
    host.handle("steward.preferences.all", {
        let preferences = launch.preferences.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record("steward.preferences.all");
            if let Err(error) = require_null_payload("steward.preferences.all", call.data()) {
                call.reject_host_error(error);
                return;
            }
            call.resolve(serde_json::to_value(&preferences).unwrap_or_default());
        }
    });
    host.handle("steward.feedback.toast", {
        let events = events.clone();
        let audit = audit.clone();
        let plugin_id = host_plugin_id.clone();
        move |call: HostCall| {
            audit.record("steward.feedback.toast");
            let request =
                match parse_call_data::<ToastRequest>("steward.feedback.toast", call.data()) {
                    Ok(request) => request,
                    Err(error) => {
                        call.reject_host_error(error);
                        return;
                    }
                };
            if request.title.trim().is_empty() || request.message.trim().is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.feedback.toast requires title and message strings",
                );
                return;
            };
            if request.title.len() > 128 || request.message.len() > 1024 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.feedback.toast content exceeds its limit",
                );
                return;
            }
            events.notice(
                session_id,
                PluginNotice {
                    title: request.title,
                    message: request.message,
                    plugin_id: Some(plugin_id.clone()),
                    code: None,
                    level: PluginNoticeLevel::Info,
                    kind: PluginNoticeKind::Toast,
                },
            );
            call.resolve(serde_json::json!({}));
        }
    });
    host.handle("steward.feedback.confirm", {
        let events = events.clone();
        let pending = pending_confirmations.clone();
        let audit = audit.clone();
        let plugin_id = host_plugin_id.clone();
        move |call: HostCall| {
            audit.record("steward.feedback.confirm");
            let request =
                match parse_call_data::<ConfirmRequest>("steward.feedback.confirm", call.data()) {
                    Ok(request) => request,
                    Err(error) => {
                        call.reject_host_error(error);
                        return;
                    }
                };
            if request.title.trim().is_empty() || request.message.trim().is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.feedback.confirm requires title and message strings",
                );
                return;
            };
            if request.title.len() > 128 || request.message.len() > 4096 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.feedback.confirm content exceeds its limit",
                );
                return;
            }
            let confirm_label = request.confirm_label.unwrap_or_else(|| "Confirm".into());
            let cancel_label = request.cancel_label.unwrap_or_else(|| "Cancel".into());
            if confirm_label.len() > 64 || cancel_label.len() > 64 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.feedback.confirm labels exceed their limit",
                );
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            let id = NEXT_CONFIRMATION_ID.fetch_add(1, Ordering::Relaxed);
            let Ok(mut pending) = pending.lock() else {
                reply.reject_code(
                    HostErrorCode::HostFailure,
                    "steward.feedback.confirm state is unavailable",
                );
                return;
            };
            pending.insert(id, reply);
            events.notice(
                session_id,
                PluginNotice {
                    title: request.title,
                    message: request.message,
                    plugin_id: Some(plugin_id.clone()),
                    code: None,
                    level: PluginNoticeLevel::Warning,
                    kind: PluginNoticeKind::Confirmation {
                        id,
                        confirm_label,
                        cancel_label,
                        destructive: request.destructive,
                    },
                },
            );
        }
    });
    host.handle("steward.feedback.progress", {
        let progress_host = host.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record("steward.feedback.progress");
            let request = match parse_call_data::<ProgressRequest>(
                "steward.feedback.progress",
                call.data(),
            ) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            if request.id.is_empty() || request.id.len() > 128 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.feedback.progress requires a bounded id string",
                );
                return;
            }
            if request.message.len() > 1024 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.feedback.progress requires a bounded message string",
                );
                return;
            }
            if request
                .current
                .zip(request.total)
                .is_some_and(|(current, total)| current > total)
            {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "progress current must not exceed total",
                );
                return;
            }
            let payload = serde_json::json!({
                "id": request.id,
                "message": request.message,
                "current": request.current,
                "total": request.total,
            });
            let _ = progress_host.send("steward.progress", payload);
            call.resolve(serde_json::json!({}));
        }
    });
    host.handle("steward.clipboard.history", {
        let data_dir = data_dir.clone();
        let audit = audit.clone();
        move |call| {
            audit.record("steward.clipboard.history");
            if let Err(error) = require_null_payload("steward.clipboard.history", call.data()) {
                call.reject_host_error(error);
                return;
            }
            let entries = crate::clipboard_history::entries_for_plugin(&data_dir);
            call.resolve(serde_json::to_value(entries).unwrap_or_default());
        }
    });
    host.handle("steward.clipboard.write", {
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record("steward.clipboard.write");
            let request = match parse_call_data::<ClipboardWriteRequest>(
                "steward.clipboard.write",
                call.data(),
            ) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            if request.text.len() > 100_000 {
                call.reject_code(
                    HostErrorCode::ResourceExhausted,
                    "clipboard text exceeds the 100000 byte limit",
                );
                return;
            }
            match arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(request.text))
            {
                Ok(()) => call.resolve(serde_json::json!({})),
                Err(error) => call.reject_code(
                    HostErrorCode::HostFailure,
                    format!("write clipboard text: {error}"),
                ),
            }
        }
    });
    host.handle("steward.storage.get", {
        let storage = plugin_storage.clone();
        let audit = audit.clone();
        move |call| {
            audit.record("steward.storage.get");
            let request =
                match parse_call_data::<StorageGetRequest>("steward.storage.get", call.data()) {
                    Ok(request) => request,
                    Err(error) => {
                        call.reject_host_error(error);
                        return;
                    }
                };
            if request.key.is_empty() || request.key.len() > 256 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "steward.storage.get requires a bounded key string",
                );
                return;
            };
            let value = storage
                .lock()
                .ok()
                .and_then(|storage| storage.get(&request.key).cloned());
            call.resolve(value.map_or(serde_json::Value::Null, serde_json::Value::String));
        }
    });
    host.handle("steward.storage.set", {
        let storage = plugin_storage.clone();
        let storage_path = storage_path.clone();
        let audit = audit.clone();
        move |call| {
            audit.record("steward.storage.set");
            let request =
                match parse_call_data::<StorageSetRequest>("steward.storage.set", call.data()) {
                    Ok(request) => request,
                    Err(error) => {
                        call.reject_host_error(error);
                        return;
                    }
                };
            if request.key.is_empty() || request.key.len() > 256 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "plugin storage key must contain 1 to 256 bytes",
                );
                return;
            }
            if request.value.len() > 1024 * 1024 {
                call.reject_code(
                    HostErrorCode::ResourceExhausted,
                    "plugin storage value exceeds the 1 MiB limit",
                );
                return;
            }
            let result = storage
                .lock()
                .map_err(|_| "plugin storage lock poisoned".to_owned())
                .and_then(|mut storage| {
                    storage.insert(request.key, request.value);
                    persist_plugin_storage(&storage_path, &storage)
                });
            match result {
                Ok(()) => call.resolve(serde_json::json!({})),
                Err(error) => call.reject_code(
                    HostErrorCode::HostFailure,
                    format!("write plugin storage: {error}"),
                ),
            }
        }
    });
    host.handle("steward.calendar.holidays", {
        let cache_dir = data_dir.join("calendar-holidays");
        let worker = Arc::new(OnceLock::new());
        let audit = audit.clone();
        move |call| {
            audit.record("steward.calendar.holidays");
            let request = match parse_call_data::<CalendarHolidaysRequest>(
                "steward.calendar.holidays",
                call.data(),
            ) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            let Some(reply) = call.into_reply() else {
                return;
            };
            let sender = worker.get_or_init(|| start_holiday_worker(cache_dir.clone()));
            let request = HolidayRequest {
                year: request.year,
                reply,
            };
            match sender.try_send(request) {
                Ok(()) => {}
                Err(crossbeam_channel::TrySendError::Full(request)) => {
                    request.reply.reject_code(
                        HostErrorCode::ResourceExhausted,
                        "steward.calendar.holidays is busy; retry later",
                    );
                }
                Err(crossbeam_channel::TrySendError::Disconnected(request)) => {
                    request.reply.reject_code(
                        HostErrorCode::HostFailure,
                        "steward.calendar.holidays host worker is unavailable",
                    );
                }
            }
        }
    });
    host.handle(FS_PICK, {
        let picker = picker_tx.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(FS_PICK);
            let request = match parse_call_data::<PathPickRequest>(FS_PICK, call.data()) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            let mode = match request.mode {
                PathPickMode::FolderRead => PickerMode::FolderRead,
                PathPickMode::FolderWrite => PickerMode::FolderWrite,
                PathPickMode::FileRead => PickerMode::FileRead,
            };
            let Some(reply) = call.into_reply() else {
                return;
            };
            match picker.try_send(PathPickerRequest { mode, reply }) {
                Ok(()) => {}
                Err(crossbeam_channel::TrySendError::Full(request))
                | Err(crossbeam_channel::TrySendError::Disconnected(request)) => {
                    request
                        .reply
                        .reject_code(HostErrorCode::ResourceExhausted, "the picker is busy");
                }
            }
        }
    });
    host.handle(FS_CANCEL_WALK, {
        let files = files.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(FS_CANCEL_WALK);
            let request = match parse_call_data::<CancelWalkRequest>(FS_CANCEL_WALK, call.data()) {
                Ok(request) if !request.id.is_empty() && request.id.len() <= 128 => request,
                Ok(_) => {
                    call.reject_code(
                        HostErrorCode::InvalidInput,
                        "cancelWalk requires a bounded id string",
                    );
                    return;
                }
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            let cancelled = files.cancel_walk(&request.id);
            call.resolve(serde_json::json!({ "cancelled": cancelled }));
        }
    });
    host.handle(FS_WALK, {
        let files = files.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(FS_WALK);
            let request = match parse_call_data::<WalkRequest>(FS_WALK, call.data()) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            if request.id.is_empty() || request.root.is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "walk requires id and root strings",
                );
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            files.enqueue_walk(
                request.id,
                request.root,
                request.max_results.unwrap_or(DEFAULT_WALK_RESULTS),
                request.max_depth.unwrap_or(DEFAULT_WALK_DEPTH),
                request.timeout_ms.unwrap_or(DEFAULT_WALK_TIMEOUT_MS),
                reply,
            );
        }
    });
    host.handle(FS_READ_TEXT, {
        let files = files.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(FS_READ_TEXT);
            let request = match parse_call_data::<ReadTextRequest>(FS_READ_TEXT, call.data()) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            if request.path.is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "readText requires a path string",
                );
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            files.enqueue_read(
                request.path,
                request.max_bytes.unwrap_or(DEFAULT_READ_BYTES),
                reply,
            );
        }
    });
    host.handle(FS_WRITE_TEXT, {
        let files = files.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(FS_WRITE_TEXT);
            let request = match parse_call_data::<WriteTextRequest>(FS_WRITE_TEXT, call.data()) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            if request.path.is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "writeText requires a path string",
                );
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            files.enqueue_write(request.path, request.contents, reply);
        }
    });
    host.handle(FS_GRANTS, {
        let files = files.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(FS_GRANTS);
            if let Err(error) = require_null_payload(FS_GRANTS, call.data()) {
                call.reject_host_error(error);
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            files.enqueue_grants(reply);
        }
    });
    host.handle(SYSTEM_OPEN_PATH, {
        let system = system.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(SYSTEM_OPEN_PATH);
            let request = match parse_call_data::<SystemPathRequest>(SYSTEM_OPEN_PATH, call.data())
            {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            if request.path.is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "openPath requires a path string",
                );
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            system.enqueue_open_path(request.path, reply);
        }
    });
    host.handle(SYSTEM_REVEAL_PATH, {
        let system = system.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(SYSTEM_REVEAL_PATH);
            let request =
                match parse_call_data::<SystemPathRequest>(SYSTEM_REVEAL_PATH, call.data()) {
                    Ok(request) => request,
                    Err(error) => {
                        call.reject_host_error(error);
                        return;
                    }
                };
            if request.path.is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "revealPath requires a path string",
                );
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            system.enqueue_reveal_path(request.path, reply);
        }
    });
    host.handle(SYSTEM_APPLICATIONS, {
        let system = system.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(SYSTEM_APPLICATIONS);
            if let Err(error) = require_null_payload(SYSTEM_APPLICATIONS, call.data()) {
                call.reject_host_error(error);
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            system.enqueue_list_applications(reply);
        }
    });
    host.handle(SYSTEM_OPEN_WITH, {
        let system = system.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(SYSTEM_OPEN_WITH);
            let request =
                match parse_call_data::<SystemOpenWithRequest>(SYSTEM_OPEN_WITH, call.data()) {
                    Ok(request) => request,
                    Err(error) => {
                        call.reject_host_error(error);
                        return;
                    }
                };
            if request.application_id.is_empty() || request.path.is_empty() {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "openWith requires applicationId and path strings",
                );
                return;
            }
            let Some(reply) = call.into_reply() else {
                return;
            };
            system.enqueue_open_with(request.application_id, request.path, reply);
        }
    });
    host.handle(NET_FETCH, {
        let network = network.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(NET_FETCH);
            let request = match NetworkRequest::from_value(call.data()) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_host_error(error.into_host_error());
                    return;
                }
            };
            let Some(reply) = call.into_reply() else {
                return;
            };
            network.enqueue(request, reply);
        }
    });
    host.handle(EXEC_RUN, {
        let events = events.clone();
        let pending = pending_executions.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(EXEC_RUN);
            let request = match parse_execution_request(call.data()) {
                Ok(request) => request,
                Err(error) => {
                    call.reject_code(HostErrorCode::InvalidInput, error);
                    return;
                }
            };
            let request_id = request.request_id;
            let Some(reply) = call.into_reply() else {
                return;
            };
            let Ok(mut pending) = pending.lock() else {
                reply.reject_code(
                    HostErrorCode::HostFailure,
                    "controlled process state is unavailable",
                );
                return;
            };
            if pending.len() >= 64 {
                reply.reject_code(
                    HostErrorCode::ResourceExhausted,
                    "too many controlled processes are pending",
                );
                return;
            }
            if pending.contains_key(&request_id) {
                reply.reject_code(
                    HostErrorCode::InvalidInput,
                    "process request id is already active",
                );
                return;
            }
            pending.insert(request_id, reply);
            drop(pending);
            events.execution_request(session_id, request);
        }
    });
    host.handle(EXEC_CANCEL, {
        let events = events.clone();
        let pending = pending_executions.clone();
        let audit = audit.clone();
        move |call: HostCall| {
            audit.record(EXEC_CANCEL);
            let request = match parse_call_data::<ExecutionCancelRequest>(EXEC_CANCEL, call.data())
            {
                Ok(request) if request.request_id > 0 => request,
                Ok(_) => {
                    call.reject_code(
                        HostErrorCode::InvalidInput,
                        "cancelProcess requires a positive requestId",
                    );
                    return;
                }
                Err(error) => {
                    call.reject_host_error(error);
                    return;
                }
            };
            let exists = pending
                .lock()
                .map(|pending| pending.contains_key(&request.request_id))
                .unwrap_or(false);
            if exists {
                events.execution_cancel(session_id, request.request_id);
            }
            call.resolve(serde_json::json!({ "cancelled": exists }));
        }
    });
    host.handle(NAVIGATION_BACK, {
        let control = control_tx.clone();
        let audit = audit.clone();
        move |call| {
            audit.record(NAVIGATION_BACK);
            if let Err(error) = require_null_payload(NAVIGATION_BACK, call.data()) {
                call.reject_host_error(error);
                return;
            }
            let _ = control.try_send(PluginProcessCommand {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                action: PluginProcessAction::Back,
            });
            call.resolve(serde_json::json!({}));
        }
    });
    host.handle(NAVIGATION_CLOSE, {
        let control = control_tx.clone();
        let audit = audit.clone();
        move |call| {
            audit.record(NAVIGATION_CLOSE);
            if let Err(error) = require_null_payload(NAVIGATION_CLOSE, call.data()) {
                call.reject_host_error(error);
                return;
            }
            let _ = control.try_send(PluginProcessCommand {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                action: PluginProcessAction::Close,
            });
            call.resolve(serde_json::json!({}));
        }
    });
    host.handle("steward.window.close", {
        let control = control_tx.clone();
        let audit = audit.clone();
        move |call| {
            audit.record("steward.window.close");
            if let Err(error) = require_null_payload("steward.window.close", call.data()) {
                call.reject_host_error(error);
                return;
            }
            let _ = control.try_send(PluginProcessCommand {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                action: PluginProcessAction::Back,
            });
            call.resolve(serde_json::json!({}));
        }
    });
    host.handle(NAVIGATION_SET_TITLE, {
        let control = control_tx.clone();
        let audit = audit.clone();
        move |call| {
            audit.record(NAVIGATION_SET_TITLE);
            let request =
                match parse_call_data::<SetTitleRequest>(NAVIGATION_SET_TITLE, call.data()) {
                    Ok(request) => request,
                    Err(error) => {
                        call.reject_host_error(error);
                        return;
                    }
                };
            let title = request.title.trim();
            if title.is_empty() || title.len() > 128 {
                call.reject_code(
                    HostErrorCode::InvalidInput,
                    "setTitle requires a non-empty title of at most 128 bytes",
                );
                return;
            }
            let _ = control.try_send(PluginProcessCommand {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                action: PluginProcessAction::SetTitle {
                    title: title.to_owned(),
                },
            });
            call.resolve(serde_json::json!({}));
        }
    });

    let ui_pending_confirmations = pending_confirmations.clone();
    let ui_pending_executions = pending_executions.clone();
    gpui_platform::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run({
            let host = host.clone();
            move |cx: &mut App| {
                nsh_platform_gpui::set_ui_sets(vec![nsh_base::registry()]);
                let plugin_id = initial.plugin_id.clone();
                let title = initial.title.clone();
                let app_id = launch.app_id.clone();
                let package = launch.package;
                let presentation = initial.presentation;
                let embedded = presentation == PresentationMode::Embedded;
                let width = if embedded {
                    PLUGIN_EMBEDDED_WIDTH
                } else {
                    PLUGIN_WINDOW_WIDTH
                };
                let height = if embedded {
                    PLUGIN_EMBEDDED_HEIGHT
                } else {
                    PLUGIN_WINDOW_HEIGHT
                };
                let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
                let fatal_error = Arc::new(Mutex::new(None::<String>));
                let healthy = Arc::new(AtomicBool::new(false));
                let first_frame_reported = Arc::new(AtomicBool::new(false));
                let escape_requested = Arc::new(AtomicBool::new(false));
                let request = initial.clone();
                let handle = cx
                    .open_window(
                        WindowOptions {
                            window_bounds: Some(WindowBounds::Windowed(bounds)),
                            titlebar: Some(TitlebarOptions {
                                title: Some(title.clone().into()),
                                appears_transparent: embedded,
                                ..Default::default()
                            }),
                            is_resizable: !embedded,
                            is_minimizable: !embedded,
                            window_min_size: (!embedded).then_some(size(
                                px(PLUGIN_WINDOW_MIN_WIDTH),
                                px(PLUGIN_WINDOW_MIN_HEIGHT),
                            )),
                            kind: if embedded {
                                WindowKind::PopUp
                            } else {
                                WindowKind::Normal
                            },
                            show: !embedded,
                            focus: !embedded,
                            ..Default::default()
                        },
                        {
                            let host = host.clone();
                            let events = events.clone();
                            let fatal_error = fatal_error.clone();
                            let healthy = healthy.clone();
                            let first_frame_reported = first_frame_reported.clone();
                            let escape_requested = escape_requested.clone();
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
                                    let _ = host.send(
                                        "steward.visibility",
                                        serde_json::json!({ "visible": true }),
                                    );
                                    let visibility_host = host.clone();
                                    let visibility_events = events.clone();
                                    cx.observe_window_activation(window, move |_, window, _| {
                                        let visible = window.is_window_active();
                                        let _ = visibility_host.send(
                                            "steward.visibility",
                                            serde_json::json!({ "visible": visible }),
                                        );
                                        visibility_events.presentation(
                                            session_id,
                                            PluginPresentationEvent::VisibilityChanged { visible },
                                        );
                                    })
                                    .detach();
                                    view
                                });
                                cx.new(|_| PluginFrameView {
                                    app,
                                    session_id,
                                    events: events.clone(),
                                    embedded,
                                    escape_requested: escape_requested.clone(),
                                    fatal_error: fatal_error.clone(),
                                    first_frame_reported: first_frame_reported.clone(),
                                    healthy: healthy.clone(),
                                })
                            }
                        },
                    )
                    .expect("open plugin window");

                if embedded {
                    let shown = handle.update(cx, |_, window, _| {
                        crate::platform::show(window, width, height)
                    });
                    if shown.is_ok() {
                        cx.activate(true);
                    }
                    let close_events = events.clone();
                    cx.on_window_closed(move |_, _| {
                        close_events
                            .presentation_and_flush(session_id, PluginPresentationEvent::Close);
                    })
                    .detach();
                }

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
                let control_confirmations = ui_pending_confirmations.clone();
                let control_executions = ui_pending_executions.clone();
                let control_events = events.clone();
                cx.spawn(async move |cx| loop {
                    if embedded && escape_requested.swap(false, Ordering::AcqRel) {
                        control_events
                            .presentation_and_flush(session_id, PluginPresentationEvent::Back);
                        let _ = handle.update(cx, |_, window, _| {
                            window.remove_window();
                        });
                        cx.update(|cx| cx.quit());
                        return;
                    }
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
                                    crate::platform::show(window, width, height);
                                });
                                cx.update(|cx| cx.activate(true));
                            }
                            PluginProcessAction::Back => {
                                control_events.presentation_and_flush(
                                    session_id,
                                    PluginPresentationEvent::Back,
                                );
                                let _ = handle.update(cx, |_, window, _| {
                                    window.remove_window();
                                });
                                cx.update(|cx| cx.quit());
                                return;
                            }
                            PluginProcessAction::Close => {
                                control_events.presentation_and_flush(
                                    session_id,
                                    PluginPresentationEvent::Close,
                                );
                                let _ = handle.update(cx, |_, window, _| {
                                    window.remove_window();
                                });
                                cx.update(|cx| cx.quit());
                                return;
                            }
                            PluginProcessAction::SetTitle { title } => {
                                let _ = handle.update(cx, |_, window, _| {
                                    window.set_window_title(&title);
                                });
                                control_events.presentation(
                                    session_id,
                                    PluginPresentationEvent::TitleChanged { title },
                                );
                            }
                            PluginProcessAction::ResolveConfirmation { id, confirmed } => {
                                if let Ok(mut pending) = control_confirmations.lock() {
                                    if let Some(reply) = pending.remove(&id) {
                                        if confirmed {
                                            reply.resolve(serde_json::json!({ "confirmed": true }));
                                        } else {
                                            reply.reject_code(
                                                HostErrorCode::ConfirmationDenied,
                                                "confirmation denied",
                                            );
                                        }
                                    }
                                }
                            }
                            PluginProcessAction::ResolveExecution { request_id, result } => {
                                if let Ok(mut pending) = control_executions.lock() {
                                    if let Some(reply) = pending.remove(&request_id) {
                                        resolve_execution_reply(reply, result);
                                    }
                                }
                            }
                        }
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                })
                .detach();

                let picker_files = files.clone();
                cx.spawn(async move |cx| loop {
                    while let Ok(request) = picker_rx.try_recv() {
                        let files = picker_files.clone();
                        let paths = cx.update(|cx| {
                            cx.prompt_for_paths(PathPromptOptions {
                                files: request.mode.selects_files(),
                                directories: !request.mode.selects_files(),
                                multiple: false,
                                prompt: Some("Select".into()),
                            })
                        });
                        cx.spawn(async move |_| {
                            let selected = match paths.await {
                                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                                Ok(Ok(None)) => None,
                                Ok(Err(error)) => {
                                    request.reply.reject_code(
                                        HostErrorCode::HostFailure,
                                        format!("open the system picker: {error}"),
                                    );
                                    return;
                                }
                                Err(_) => {
                                    request.reply.reject_code(
                                        HostErrorCode::HostFailure,
                                        "the system picker closed unexpectedly",
                                    );
                                    return;
                                }
                            };
                            let Some(selected) = selected else {
                                request.reply.reject_code(
                                    HostErrorCode::Cancelled,
                                    "path selection was cancelled",
                                );
                                return;
                            };
                            match files.picker_grant(&selected, request.mode) {
                                Ok(path) => request.reply.resolve(serde_json::json!({
                                    "path": display_path(&path),
                                })),
                                Err(error) => {
                                    request.reply.reject_host_error(error.into_host_error())
                                }
                            }
                        })
                        .detach();
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                })
                .detach();
            }
        });
}

#[derive(Clone)]
struct CapabilityAuditSink {
    entries: crossbeam_channel::Sender<NewCapabilityAuditEntry>,
    plugin_id: String,
}

impl CapabilityAuditSink {
    fn new(data_dir: &Path, authorization: &steward_plugin_host::PluginAuthorization) -> Self {
        let (entries, receiver) = crossbeam_channel::unbounded::<NewCapabilityAuditEntry>();
        let plugin_id = authorization.plugin_id.clone();
        let publisher_fingerprint = authorization.publisher_fingerprint.clone();
        let version = authorization.version.clone();
        let data_dir = data_dir.to_path_buf();
        std::thread::Builder::new()
            .name("steward-capability-audit".into())
            .spawn(move || {
                let storage = match Storage::open_at(&data_dir) {
                    Ok(storage) => storage,
                    Err(error) => {
                        tracing::warn!(
                            error = %format!("{error:#}"),
                            "capability audit worker could not open storage"
                        );
                        return;
                    }
                };
                for mut entry in receiver {
                    entry.plugin_id = plugin_id.clone();
                    entry.publisher_fingerprint = publisher_fingerprint.clone();
                    entry.detail = Some(match entry.detail.take() {
                        Some(detail) if !detail.is_empty() => {
                            format!("origin=host_call;version={version};{detail}")
                        }
                        _ => format!("origin=host_call;version={version}"),
                    });
                    if let Err(error) = storage.append_capability_audit(&entry) {
                        tracing::warn!(
                            error = %format!("{error:#}"),
                            "failed to append capability host-call audit"
                        );
                    }
                }
            })
            .expect("spawn capability audit worker");
        Self {
            entries,
            plugin_id: authorization.plugin_id.clone(),
        }
    }

    fn record(&self, capability: &str) {
        tracing::info!(
            plugin_id = %self.plugin_id,
            capability,
            outcome = "requested",
            "plugin capability trace"
        );
        self.record_outcome(capability, "host=steward", "allowed", "origin=host_call");
    }

    fn record_outcome(&self, capability: &str, scope: &str, outcome: &str, detail: &str) {
        tracing::info!(
            plugin_id = %self.plugin_id,
            capability,
            outcome,
            "plugin capability trace"
        );
        let occurred_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();
        let _ = self.entries.send(NewCapabilityAuditEntry {
            plugin_id: String::new(),
            publisher_fingerprint: String::new(),
            capability: capability.into(),
            scope: scope.into(),
            outcome: outcome.into(),
            occurred_at,
            detail: Some(detail.into()),
        });
    }
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
    if process.request().presentation != request.presentation {
        return false;
    }
    process
        .send(PluginProcessAction::Command {
            command: request.command.clone(),
            input: request.input.clone(),
        })
        .and_then(|()| process.send(PluginProcessAction::Focus))
        .is_ok()
}

fn append_runtime_audit(
    data_dir: &Path,
    authorization: &PluginAuthorization,
    capability: &str,
    scope: &str,
    outcome: &str,
    detail: &str,
) {
    let Ok(storage) = Storage::open_at(data_dir) else {
        return;
    };
    let occurred_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let _ = storage.append_capability_audit(&NewCapabilityAuditEntry {
        plugin_id: authorization.plugin_id.clone(),
        publisher_fingerprint: authorization.publisher_fingerprint.clone(),
        capability: capability.into(),
        scope: scope.into(),
        outcome: outcome.into(),
        occurred_at,
        detail: Some(format!(
            "origin=broker;version={};{detail}",
            authorization.version
        )),
    });
}

pub(crate) fn resolve_confirmation(
    state: &Rc<RefCell<AppState>>,
    plugin_id: &str,
    id: u64,
    confirmed: bool,
) -> bool {
    let mut state = state.borrow_mut();
    let Some(process) = state.plugin_windows.get_mut(plugin_id) else {
        return false;
    };
    process
        .send(PluginProcessAction::ResolveConfirmation { id, confirmed })
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
    let data_dir = state.borrow().data_dir.clone();
    let authorization = launch.authorization.clone();
    let receipt = launch.package.receipt.clone();
    let session_id = NEXT_PLUGIN_SESSION.fetch_add(1, Ordering::Relaxed);
    let process = spawn_process(
        session_id,
        requested_at,
        initial.clone(),
        receipt,
        authorization,
        data_dir,
        events,
    )?;
    let child_pid = process.pid();
    state
        .borrow_mut()
        .plugin_windows
        .insert(initial.plugin_id.clone(), process);
    crate::metrics::value(
        "plugin_host_process_count",
        state.borrow().plugin_windows.values().count() as u64,
        serde_json::json!({
            "plugin_id": initial.plugin_id.clone(),
            "child_pid": child_pid,
            "phase": "opened",
        }),
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
    authorization: PluginAuthorization,
    data_dir: PathBuf,
    events: crossbeam_channel::Sender<PluginRuntimeEvent>,
) -> anyhow::Result<PluginProcess> {
    let executable = std::env::current_exe()?;
    let mut child = Command::new(executable)
        .arg(PLUGIN_PROCESS_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let child_pid = child.id();
    let containment = match PluginProcessContainment::attach(child_pid) {
        Ok(containment) => containment,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
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
    let input = Arc::new(Mutex::new(input));

    let healthy = Arc::new(AtomicBool::new(false));
    let failure_reported = Arc::new(AtomicBool::new(false));
    let pipe_closed = Arc::new(AtomicBool::new(false));
    let policy = containment.policy();
    crate::metrics::value(
        "plugin_host_containment",
        child_pid as u64,
        serde_json::json!({
            "plugin_id": request.plugin_id.clone(),
            "backend": containment.backend(),
            "memory_limit_bytes": policy.memory_limit_bytes,
            "cpu_rate_percent": policy.cpu_rate_percent,
            "active_process_limit": policy.active_process_limit,
            "handle_limit": policy.handle_limit,
            "handle_poll_interval_ms": policy.handle_poll_interval_ms,
        }),
    );
    spawn_event_reader(
        output,
        ProcessEventContext {
            plugin_id: request.plugin_id.clone(),
            request: request.clone(),
            session_id,
            requested_at,
            receipt,
            authorization,
            data_dir,
            command_input: input.clone(),
            events,
            healthy: healthy.clone(),
            failure_reported: failure_reported.clone(),
            pipe_closed: pipe_closed.clone(),
        },
    );
    Ok(PluginProcess {
        child,
        input,
        containment,
        session_id,
        healthy,
        failure_reported,
        pipe_closed,
        request,
    })
}

struct ProcessEventContext {
    plugin_id: String,
    request: LaunchRequest,
    session_id: PluginSessionId,
    requested_at: Instant,
    receipt: nsh_package::PackageReceipt,
    authorization: PluginAuthorization,
    data_dir: PathBuf,
    command_input: Arc<Mutex<ChildStdin>>,
    events: crossbeam_channel::Sender<PluginRuntimeEvent>,
    healthy: Arc<AtomicBool>,
    failure_reported: Arc<AtomicBool>,
    pipe_closed: Arc<AtomicBool>,
}

fn spawn_event_reader(output: std::process::ChildStdout, context: ProcessEventContext) {
    let ProcessEventContext {
        plugin_id,
        request,
        session_id,
        requested_at,
        receipt,
        authorization,
        data_dir,
        command_input,
        events,
        healthy,
        failure_reported,
        pipe_closed,
    } = context;
    let execution_results: ExecutionResultSink = {
        let command_input = command_input.clone();
        Arc::new(move |request_id, result| {
            let _ = send_process_action(
                &command_input,
                session_id,
                PluginProcessAction::ResolveExecution { request_id, result },
            );
        })
    };
    let execution_audit: ExecutionAudit = {
        let data_dir = data_dir.clone();
        let authorization = authorization.clone();
        Arc::new(move |capability, scope, outcome, detail| {
            append_runtime_audit(
                &data_dir,
                &authorization,
                capability,
                scope,
                outcome,
                detail,
            );
        })
    };
    let broker = ExecutionBroker::new(
        data_dir,
        authorization,
        session_id,
        execution_results,
        execution_audit,
    );
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
                    PluginProcessEvent::Notice {
                        protocol_version,
                        session_id: event_session,
                        notice,
                    } => {
                        if protocol_version != PROTOCOL_VERSION || event_session != session_id {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host sent a notice event for another session".into(),
                            );
                            return;
                        }
                        let _ = events.send(PluginRuntimeEvent::Notice {
                            plugin_id: plugin_id.clone(),
                            session_id,
                            notice,
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
                    PluginProcessEvent::ExecutionRequest {
                        protocol_version,
                        session_id: event_session,
                        request: execution_request,
                    } => {
                        if protocol_version != PROTOCOL_VERSION || event_session != session_id {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host sent an execution request for another session".into(),
                            );
                            return;
                        }
                        broker.submit(execution_request);
                    }
                    PluginProcessEvent::ExecutionCancel {
                        protocol_version,
                        session_id: event_session,
                        request_id,
                    } => {
                        if protocol_version != PROTOCOL_VERSION || event_session != session_id {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host sent an execution cancellation for another session"
                                    .into(),
                            );
                            return;
                        }
                        broker.cancel(request_id);
                    }
                    PluginProcessEvent::Presentation {
                        protocol_version,
                        session_id: event_session,
                        event,
                    } => {
                        if protocol_version != PROTOCOL_VERSION || event_session != session_id {
                            report_protocol_failure(
                                &events,
                                &plugin_id,
                                &request,
                                session_id,
                                &failure_reported,
                                PluginErrorCode::RuntimeError,
                                "plugin host sent a presentation event for another session".into(),
                            );
                            return;
                        }
                        let _ = events.send(PluginRuntimeEvent::Presentation {
                            plugin_id: plugin_id.clone(),
                            session_id,
                            event,
                        });
                    }
                }
            }
            broker.cancel_all();
            if !failure_reported.load(Ordering::Acquire) {
                pipe_closed.store(true, Ordering::Release);
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
    let mut stopped = Vec::new();
    state
        .borrow_mut()
        .plugin_windows
        .retain(|_, process| match process.poll_exit() {
            PluginProcessExit::Running => true,
            PluginProcessExit::Clean => {
                stopped.push((process.request().clone(), process.session_id));
                false
            }
            PluginProcessExit::Failed { code, error } => {
                exited.push((process.request.clone(), process.session_id, code, error));
                false
            }
        });
    if exited.is_empty() && stopped.is_empty() {
        return;
    }
    let event_sender = state.borrow().plugin_events.clone();
    for (request, session_id) in stopped {
        let _ = event_sender.send(PluginRuntimeEvent::Stopped {
            plugin_id: request.plugin_id.clone(),
            session_id,
            request,
        });
    }
    for (request, session_id, code, error) in exited {
        let _ = event_sender.send(PluginRuntimeEvent::Exited {
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
    fn rust_host_parsers_accept_every_committed_sdk_fixture() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../contracts/plugin-api-v1.json")).unwrap();
        for call in contract["calls"].as_array().unwrap() {
            let name = call["name"].as_str().unwrap();
            let data = &call["request"]["examples"][0];
            match name {
                "steward.preferences.get" => {
                    parse_call_data::<PreferencesGetRequest>(name, data).unwrap();
                }
                "steward.feedback.toast" => {
                    parse_call_data::<ToastRequest>(name, data).unwrap();
                }
                "steward.feedback.confirm" => {
                    parse_call_data::<ConfirmRequest>(name, data).unwrap();
                }
                "steward.feedback.progress" => {
                    parse_call_data::<ProgressRequest>(name, data).unwrap();
                }
                "steward.clipboard.write" => {
                    parse_call_data::<ClipboardWriteRequest>(name, data).unwrap();
                }
                "steward.storage.get" => {
                    parse_call_data::<StorageGetRequest>(name, data).unwrap();
                }
                "steward.storage.set" => {
                    parse_call_data::<StorageSetRequest>(name, data).unwrap();
                }
                "steward.calendar.holidays" => {
                    parse_call_data::<CalendarHolidaysRequest>(name, data).unwrap();
                }
                "steward.fs.pick" => {
                    parse_call_data::<PathPickRequest>(name, data).unwrap();
                }
                "steward.fs.cancelWalk" => {
                    parse_call_data::<CancelWalkRequest>(name, data).unwrap();
                }
                "steward.fs.walk" => {
                    parse_call_data::<WalkRequest>(name, data).unwrap();
                }
                "steward.fs.readText" => {
                    parse_call_data::<ReadTextRequest>(name, data).unwrap();
                }
                "steward.fs.writeText" => {
                    parse_call_data::<WriteTextRequest>(name, data).unwrap();
                }
                "steward.system.openPath" | "steward.system.revealPath" => {
                    parse_call_data::<SystemPathRequest>(name, data).unwrap();
                }
                "steward.system.openWith" => {
                    parse_call_data::<SystemOpenWithRequest>(name, data).unwrap();
                }
                "steward.net.fetch" => {
                    NetworkRequest::from_value(data).unwrap();
                }
                "steward.exec.run" => {
                    parse_execution_request(data).unwrap();
                }
                "steward.exec.cancel" => {
                    parse_call_data::<ExecutionCancelRequest>(name, data).unwrap();
                }
                "steward.navigation.setTitle" => {
                    parse_call_data::<SetTitleRequest>(name, data).unwrap();
                }
                "steward.preferences.all"
                | "steward.clipboard.history"
                | "steward.window.close"
                | "steward.navigation.back"
                | "steward.navigation.close"
                | "steward.fs.grants"
                | "steward.system.applications" => {
                    require_null_payload(name, data).unwrap();
                }
                other => panic!("contract call {other} has no Rust parser"),
            }
        }
    }

    #[test]
    fn execution_parser_accepts_the_sdk_numeric_request_id_shape() {
        let request = parse_execution_request(&serde_json::json!({
            "requestId": 9_007_199_254_740_991_u64,
            "executable": "git.exe",
            "arguments": ["status", "--short"],
            "workingDirectory": r"D:\projects\steward",
            "timeoutMs": 30_000,
            "maxOutputBytes": 1_048_576
        }))
        .unwrap();

        assert_eq!(request.request_id, 9_007_199_254_740_991);
        assert_eq!(request.executable, "git.exe");
    }

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

    #[test]
    fn runtime_broker_audits_scoped_network_and_process_outcomes() {
        let dir = std::env::temp_dir().join(format!(
            "steward-plugin-runtime-audit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = Storage::open_at(&dir).unwrap();
        drop(storage);
        let authorization = PluginAuthorization {
            plugin_id: "com.example.audit".into(),
            publisher_fingerprint: "SHA256:publisher".into(),
            version: "1.0.0".into(),
            features: Vec::new(),
            legacy_compatibility: false,
            declared_capabilities: Default::default(),
            granted_capabilities: Default::default(),
        };
        append_runtime_audit(
            &dir,
            &authorization,
            NET_FETCH,
            "host=api.example.com;method=GET;path=/v1/items",
            "allowed",
            "status=200",
        );
        append_runtime_audit(
            &dir,
            &authorization,
            EXEC_RUN,
            "executable=git.exe;cwd=D:\\projects",
            "denied",
            "permission revoked",
        );
        let audit = Storage::open_at(&dir)
            .unwrap()
            .capability_audit("com.example.audit")
            .unwrap();
        assert!(audit.iter().any(|entry| {
            entry.capability == NET_FETCH
                && entry.scope.contains("method=GET")
                && entry.outcome == "allowed"
        }));
        assert!(audit.iter().any(|entry| {
            entry.capability == EXEC_RUN
                && entry.scope == "executable=git.exe;cwd=D:\\projects"
                && entry.outcome == "denied"
        }));
        std::fs::remove_dir_all(dir).ok();
    }
}

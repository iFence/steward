//! One full NSH application window per plugin.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{
    px, size, App, AppContext, Bounds, Context, Entity, Focusable, IntoElement, Render,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use nsh_engine::{HostChannel, HostReply};
use nsh_package::PackageReceipt;
use nsh_platform_gpui::{AppOptions, AppView};
use steward_plugin_contract::{LaunchRequest, PluginSessionId};
use steward_plugin_host::PluginLaunch;

use crate::nsh_surface::{new_view, plugin_limits};
use crate::services::WindowCommand;
use crate::state::{AppState, PluginRuntimeEvent, PluginWindow};

const PLUGIN_WINDOW_WIDTH: f32 = 900.0;
const PLUGIN_WINDOW_HEIGHT: f32 = 660.0;
const PLUGIN_WINDOW_MIN_WIDTH: f32 = 720.0;
const PLUGIN_WINDOW_MIN_HEIGHT: f32 = 600.0;
const FIRST_FRAME_GRACE: Duration = Duration::from_millis(250);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(2);
const HOLIDAY_QUEUE_DEPTH: usize = 16;
static NEXT_PLUGIN_SESSION: AtomicU64 = AtomicU64::new(1);

struct PluginFrameView {
    app: Entity<AppView>,
    plugin_id: String,
    session_id: PluginSessionId,
    receipt: PackageReceipt,
    requested_at: std::time::Instant,
    events: crossbeam_channel::Sender<PluginRuntimeEvent>,
    fatal_error: Arc<Mutex<Option<String>>>,
    first_frame_reported: Arc<AtomicBool>,
    cancel_health: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
}

impl Render for PluginFrameView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_reported.swap(true, Ordering::AcqRel) {
            let first_frame_at = std::time::Instant::now();
            let plugin_id = self.plugin_id.clone();
            let session_id = self.session_id;
            let receipt = self.receipt.clone();
            let requested_at = self.requested_at;
            let events = self.events.clone();
            let fatal_error = self.fatal_error.clone();
            let cancel_health = self.cancel_health.clone();
            let healthy = self.healthy.clone();
            cx.spawn(async move |_view, cx| {
                cx.background_executor().timer(FIRST_FRAME_GRACE).await;
                if cancel_health.load(Ordering::Acquire) {
                    return;
                }
                let no_fatal_error = fatal_error
                    .lock()
                    .map(|fatal| fatal.is_none())
                    .unwrap_or(false);
                if no_fatal_error {
                    healthy.store(true, Ordering::Release);
                    let _ = events.send(PluginRuntimeEvent::Healthy {
                        plugin_id,
                        session_id,
                        receipt,
                        requested_at,
                        first_frame_at,
                    });
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

fn start_holiday_worker(
    cache_dir: std::path::PathBuf,
) -> crossbeam_channel::Sender<HolidayRequest> {
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

pub(crate) fn focus_and_send(
    cx: &mut App,
    state: &Rc<RefCell<AppState>>,
    request: &LaunchRequest,
) -> bool {
    let existing = {
        let state_ref = state.borrow();
        state_ref
            .plugin_windows
            .get(&request.plugin_id)
            .map(|window| (window.host.clone(), window.handle))
    };
    let Some((host, handle)) = existing else {
        return false;
    };
    let _ = host.send(
        "steward.command",
        serde_json::json!({
            "command": request.command,
            "input": request.input,
        }),
    );
    let _ = handle.update(cx, |_, window, cx| {
        crate::platform::show(window, PLUGIN_WINDOW_WIDTH, PLUGIN_WINDOW_HEIGHT);
        cx.activate(true);
    });
    true
}

pub(crate) fn open_launch(
    cx: &mut App,
    state: Rc<RefCell<AppState>>,
    requested_at: std::time::Instant,
    initial: LaunchRequest,
    latest: LaunchRequest,
    launch: PluginLaunch,
) -> anyhow::Result<()> {
    if focus_and_send(cx, &state, &latest) {
        return Ok(());
    }

    let (services, data_dir, entries, events) = {
        let state_ref = state.borrow();
        (
            state_ref.services.clone(),
            state_ref.data_dir.join("nsh-data"),
            state_ref.clipboard_entries.clone(),
            state_ref.plugin_events.clone(),
        )
    };

    let host = HostChannel::new();
    host.set_params(launch.params.clone());
    host.handle("steward.clipboard.history", {
        let entries = entries.clone();
        move |call| {
            let entries = entries
                .lock()
                .map(|entries| entries.clone())
                .unwrap_or_default();
            call.resolve(serde_json::to_value(entries).unwrap_or_default());
        }
    });
    host.handle("steward.clipboard.write", |call| {
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

    let storage_path = data_dir
        .join(&initial.plugin_id)
        .join("steward-host-storage.json");
    let plugin_storage = Arc::new(Mutex::new(load_plugin_storage(&storage_path)));
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
        let request = initial.clone();
        let services = services.clone();
        move |call| {
            services.send_window_command(WindowCommand::ClosePlugin(request.plugin_id.clone()));
            call.resolve(serde_json::json!({}));
        }
    });

    let plugin_id = initial.plugin_id.clone();
    let window_plugin_id = plugin_id.clone();
    let app_id = launch.app_id.clone();
    let package = launch.package;
    let receipt = package.receipt.clone();
    let data_plugin_id = plugin_id.clone();
    let title = initial.title.clone();
    let bounds = Bounds::centered(
        None,
        size(px(PLUGIN_WINDOW_WIDTH), px(PLUGIN_WINDOW_HEIGHT)),
        cx,
    );
    let fatal_error = Arc::new(Mutex::new(None::<String>));
    let healthy = Arc::new(AtomicBool::new(false));
    let first_frame_reported = Arc::new(AtomicBool::new(false));
    let cancel_health = Arc::new(AtomicBool::new(false));
    let session_id = NEXT_PLUGIN_SESSION.fetch_add(1, Ordering::Relaxed);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
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
            let cancel_health = cancel_health.clone();
            let error_request = initial.clone();
            move |window, cx| {
                let app = cx.new(|cx| {
                    let options = AppOptions {
                        app_id,
                        title: window_plugin_id.clone(),
                        data_dir: Some(data_dir.join(&data_plugin_id)),
                        limits: plugin_limits(),
                        host: host.clone(),
                        ..AppOptions::default()
                    };
                    let view = new_view(options, window, cx);
                    view.on_app_error({
                        let events = events.clone();
                        let fatal_error = fatal_error.clone();
                        let healthy = healthy.clone();
                        let request = error_request.clone();
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
                                        let _ = events.send(PluginRuntimeEvent::Failed {
                                            plugin_id: request.plugin_id.clone(),
                                            session_id,
                                            request: request.clone(),
                                            code: if healthy.load(Ordering::Relaxed) {
                                                steward_plugin_contract::PluginErrorCode::RuntimeError
                                            } else {
                                                steward_plugin_contract::PluginErrorCode::FirstFrameFailed
                                            },
                                            error: message,
                                        });
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
                    plugin_id: window_plugin_id.clone(),
                    session_id,
                    receipt: receipt.clone(),
                    requested_at,
                    events: events.clone(),
                    fatal_error: fatal_error.clone(),
                    first_frame_reported: first_frame_reported.clone(),
                    cancel_health: cancel_health.clone(),
                    healthy: healthy.clone(),
                })
            }
        },
    )?;
    state.borrow_mut().plugin_windows.insert(
        plugin_id.clone(),
        PluginWindow {
            handle: handle.into(),
            host,
            session_id,
            cancel_health: cancel_health.clone(),
        },
    );
    crate::metrics::value(
        "plugin_window_vm_count",
        state.borrow().plugin_windows.values().count() as u64,
        serde_json::json!({ "plugin_id": plugin_id.clone() }),
    );

    let initial_for_focus = initial.clone();
    if latest.command != initial_for_focus.command || latest.input != initial_for_focus.input {
        let _ = focus_and_send(cx, &state, &latest);
    }

    let events = events.clone();
    let fatal_error_for_timer = fatal_error.clone();
    let first_frame_for_timer = first_frame_reported.clone();
    let cancel_health_for_timer = cancel_health.clone();
    let plugin_id = plugin_id.clone();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_FRAME_TIMEOUT).await;
        if !first_frame_for_timer.load(Ordering::Acquire)
            && !cancel_health_for_timer.load(Ordering::Acquire)
            && fatal_error_for_timer
                .lock()
                .map(|fatal| fatal.is_none())
                .unwrap_or(false)
        {
            let message = "plugin did not render its first frame within 2 seconds".to_string();
            let _ = events.send(PluginRuntimeEvent::Failed {
                plugin_id: plugin_id.clone(),
                session_id,
                request: initial.clone(),
                code: steward_plugin_contract::PluginErrorCode::FirstFrameFailed,
                error: message.clone(),
            });
        }
    })
    .detach();
    Ok(())
}

fn load_plugin_storage(path: &std::path::Path) -> BTreeMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn persist_plugin_storage(
    path: &std::path::Path,
    storage: &BTreeMap<String, String>,
) -> Result<(), String> {
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

pub(crate) fn close(cx: &mut App, state: &Rc<RefCell<AppState>>, plugin_id: &str) {
    let window = state.borrow_mut().plugin_windows.remove(plugin_id);
    if let Some(window) = window {
        window
            .cancel_health
            .store(true, std::sync::atomic::Ordering::Release);
        let handle = window.handle;
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
    crate::metrics::value(
        "plugin_window_vm_count",
        state.borrow().plugin_windows.values().count() as u64,
        serde_json::json!({ "plugin_id": plugin_id, "phase": "closed" }),
    );
}

pub(crate) fn broadcast_theme(state: &Rc<RefCell<AppState>>, accent: &str) {
    let payload = serde_json::json!({ "accent": accent, "dark": true });
    for plugin in state.borrow().plugin_windows.values() {
        let _ = plugin.host.send("steward.theme", payload.clone());
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

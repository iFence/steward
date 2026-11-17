//! One full NSH application window per plugin.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    px, size, App, AppContext, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions,
};
use nsh_engine::HostChannel;
use nsh_platform_gpui::AppOptions;
use steward_plugin_contract::LaunchRequest;
use steward_plugin_host::PluginLaunch;

use crate::nsh_surface::{new_view, plugin_limits};
use crate::services::WindowCommand;
use crate::state::{AppState, PluginRuntimeEvent, PluginWindow};

const PLUGIN_WINDOW_WIDTH: f32 = 900.0;
const PLUGIN_WINDOW_HEIGHT: f32 = 660.0;
const PLUGIN_WINDOW_MIN_WIDTH: f32 = 720.0;
const PLUGIN_WINDOW_MIN_HEIGHT: f32 = 600.0;
const FIRST_FRAME_GRACE: Duration = Duration::from_millis(250);

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
            let cache_dir = cache_dir.clone();
            std::thread::spawn(move || {
                match crate::holiday_schedule::load_year(year, &cache_dir) {
                    Ok(schedule) => {
                        reply.resolve(serde_json::to_value(schedule).unwrap_or_default());
                    }
                    Err(error) => {
                        tracing::warn!(year, %error, "calendar holiday schedule is unavailable");
                        reply.resolve(serde_json::json!({}));
                    }
                }
            });
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
            let error_request = initial.clone();
            move |window, cx| {
                cx.new(|cx| {
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
                })
            }
        },
    )?;
    state.borrow_mut().plugin_windows.insert(
        plugin_id.clone(),
        PluginWindow {
            handle: handle.into(),
            host,
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
    let healthy_for_timer = healthy.clone();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_FRAME_GRACE).await;
        if fatal_error_for_timer
            .lock()
            .map(|fatal| fatal.is_none())
            .unwrap_or(false)
        {
            healthy_for_timer.store(true, Ordering::Relaxed);
            let _ = events.send(PluginRuntimeEvent::Healthy {
                plugin_id,
                receipt,
                requested_at,
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
    let handle = state
        .borrow_mut()
        .plugin_windows
        .remove(plugin_id)
        .map(|window| window.handle);
    if let Some(handle) = handle {
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

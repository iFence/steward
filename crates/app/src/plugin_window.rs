//! One full NSH application window per plugin.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    px, size, App, AppContext, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions,
};
use nsh_engine::HostChannel;
use nsh_platform_gpui::AppOptions;

use crate::nsh_surface::{new_view, plugin_limits};
use crate::services::WindowCommand;
use crate::state::{AppState, PluginWindow};

pub(crate) fn open_or_focus(
    cx: &mut App,
    state: Rc<RefCell<AppState>>,
    plugin_id: String,
    command: String,
    title: String,
    input: String,
) -> anyhow::Result<()> {
    if let Some(existing) = state.borrow().plugin_windows.get(&plugin_id) {
        let _ = existing.host.send(
            "steward.command",
            serde_json::json!({ "command": command, "input": input }),
        );
        let handle = existing.handle;
        let _ = handle.update(cx, |_, window, cx| {
            crate::platform::show(window, 900.0, 680.0);
            cx.activate(true);
        });
        return Ok(());
    }

    let (launch, services, data_dir, entries) = {
        let state_ref = state.borrow();
        let services = state_ref.services.clone();
        let locale = services.locale();
        let accent = services.accent();
        let launch = state_ref
            .search
            .borrow()
            .prepare_launch(&plugin_id, &command, &title, &input, &locale, &accent)?;
        state_ref.search.borrow_mut().accept_launch(&launch)?;
        (
            launch,
            services,
            state_ref.data_dir.join("nsh-data"),
            state_ref.clipboard_entries.clone(),
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
    host.handle("steward.window.close", {
        let plugin_id = plugin_id.clone();
        let services = services.clone();
        move |call| {
            services.send_window_command(WindowCommand::ClosePlugin(plugin_id.clone()));
            call.resolve(serde_json::json!({}));
        }
    });

    let app_id = launch.app_id.clone();
    let package = launch.package;
    let window_plugin_id = plugin_id.clone();
    let data_plugin_id = plugin_id.clone();
    let bounds = Bounds::centered(None, size(px(900.0), px(680.0)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            is_resizable: true,
            show: true,
            focus: true,
            ..Default::default()
        },
        {
            let host = host.clone();
            move |window, cx| {
                cx.new(|cx| {
                    let options = AppOptions {
                        app_id: app_id.clone(),
                        title: window_plugin_id.clone(),
                        data_dir: Some(data_dir.join(&data_plugin_id)),
                        limits: plugin_limits(),
                        host: host.clone(),
                        ..AppOptions::default()
                    };
                    let view = new_view(options, window, cx);
                    view.run_package(&package);
                    view.focus_handle(cx).focus(window, cx);
                    view
                })
            }
        },
    )?;
    state.borrow_mut().plugin_windows.insert(
        plugin_id,
        PluginWindow {
            handle: handle.into(),
            host,
        },
    );
    Ok(())
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
}

pub(crate) fn broadcast_theme(state: &Rc<RefCell<AppState>>, accent: &str) {
    let payload = serde_json::json!({ "accent": accent, "dark": true });
    for plugin in state.borrow().plugin_windows.values() {
        let _ = plugin.host.send("steward.theme", payload.clone());
    }
}

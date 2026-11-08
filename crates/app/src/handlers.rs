//! Domain handlers dispatched by the application event loop.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::AsyncApp;
use steward_plugin_contract::{LaunchRequest, PluginNotice};
use tracing::{error, warn};

use crate::config::{LANGUAGE_SETTING, SUMMON_HOTKEY_SETTING, THEME_COLOR_SETTING};
use crate::services::{PluginLaunchOutcome, RowAction, WindowCommand};
use crate::state::{AppState, PluginRuntimeEvent};

pub(crate) fn drain_requests(state: &Rc<RefCell<AppState>>) {
    let requests = state.borrow().requests.clone();
    while let Ok(request) = requests.try_recv() {
        state.borrow().search.borrow_mut().handle(request);
    }
}

pub(crate) fn drain_commands(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    let commands = state.borrow().commands.clone();
    while let Ok(command) = commands.try_recv() {
        match command {
            WindowCommand::Hide => {
                if let Some(handle) = state.borrow().window {
                    let _ =
                        handle.update(cx, |_, window, cx| crate::window::hide_window(window, cx));
                }
            }
            WindowCommand::OpenSettings => open_settings(cx, state),
            WindowCommand::Resize { height } => {
                state.borrow_mut().launcher_height = height;
                if let Some(handle) = state.borrow().window {
                    let _ = handle.update(cx, |_, window, _| {
                        crate::window::resize_launcher(window, height)
                    });
                }
            }
            WindowCommand::Activate(action) => activate(cx, state, action),
            WindowCommand::ClosePlugin(plugin_id) => {
                cx.update(|cx| crate::plugin_window::close(cx, state, &plugin_id));
            }
            WindowCommand::SetLanguage(locale) => set_language(cx, state, &locale),
            WindowCommand::SetAccent(accent) => set_accent(state, &accent),
            WindowCommand::SetAutostart(enabled) => {
                let effective = crate::autostart::set_autostart(enabled);
                state.borrow().services.set_autostart(effective);
            }
            WindowCommand::SetSummonHotkey(requested) => {
                match crate::hotkeys::replace_summon_hotkey(state, &requested) {
                    Ok(hotkey) => {
                        let value = hotkey.to_string();
                        let storage = state.borrow().storage.clone();
                        if let Err(error) =
                            storage.borrow().set_setting(SUMMON_HOTKEY_SETTING, &value)
                        {
                            error!(error = %format!("{error:#}"), "persist summon hotkey");
                        }
                        state.borrow().services.set_summon_hotkey(value);
                    }
                    Err(error) => error!(error = %format!("{error:#}"), "change summon hotkey"),
                }
            }
        }
    }
}

pub(crate) fn drain_plugin_launches(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    loop {
        let outcome = state.borrow_mut().search.borrow_mut().poll_launch();
        let Some(outcome) = outcome else {
            break;
        };
        match outcome {
            PluginLaunchOutcome::Ready {
                initial,
                latest,
                launch,
            } => {
                let result = cx.update(|cx| {
                    crate::plugin_window::open_launch(
                        cx,
                        state.clone(),
                        initial,
                        latest.clone(),
                        *launch,
                    )
                });
                if let Err(error) = result {
                    report_launch_failure(
                        state,
                        &latest,
                        steward_plugin_contract::PluginErrorCode::LoadFailed,
                        &format!("{error:#}"),
                    );
                }
            }
            PluginLaunchOutcome::Failed {
                request,
                code,
                error,
            } => report_launch_failure(state, &request, code, &error),
        }
    }
}

pub(crate) fn drain_plugin_events(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    let receiver = state.borrow().plugin_events_rx.clone();
    while let Ok(event) = receiver.try_recv() {
        match event {
            PluginRuntimeEvent::Healthy { plugin_id, receipt } => {
                if let Err(error) = state
                    .borrow_mut()
                    .search
                    .borrow_mut()
                    .accept_receipt(&receipt)
                {
                    error!(
                        plugin_id = %plugin_id,
                        error = %format!("{error:#}"),
                        "accept plugin package receipt"
                    );
                } else {
                    let services = state.borrow().services.clone();
                    if services
                        .notice()
                        .is_some_and(|notice| notice.plugin_id.as_deref() == Some(&plugin_id))
                    {
                        services.clear_notice();
                    }
                    tracing::info!(plugin_id = %plugin_id, "plugin first frame healthy");
                }
            }
            PluginRuntimeEvent::Failed {
                plugin_id,
                request,
                code,
                error,
            } => {
                cx.update(|cx| crate::plugin_window::close(cx, state, &plugin_id));
                report_launch_failure(state, &request, code, &error);
            }
        }
    }
}

pub(crate) fn toggle_launcher(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    if let Some(handle) = state.borrow().window {
        let (height, services) = {
            let state = state.borrow();
            (state.launcher_height, state.services.clone())
        };
        let _ = handle.update(cx, |_, window, cx| {
            if crate::platform::is_visible(window) {
                crate::window::hide_window(window, cx);
            } else {
                services.request_launcher_reset();
                crate::platform::show(window, crate::config::LAUNCHER_WIDTH, height);
                cx.activate(true);
            }
        });
        return;
    }
    let (services, data_dir) = {
        let state = state.borrow();
        (state.services.clone(), state.data_dir.clone())
    };
    let handle = cx.update(|cx| {
        crate::window::open_launcher_window(cx, state.clone(), services.clone(), data_dir)
    });
    services.request_launcher_reset();
    state.borrow_mut().window = Some(handle);
    if let Some(handle) = state.borrow().window {
        let _ = handle.update(cx, |_, window, cx| {
            crate::platform::show(
                window,
                crate::config::LAUNCHER_WIDTH,
                crate::config::LAUNCHER_HEIGHT,
            );
            cx.activate(true);
        });
    }
}

pub(crate) fn open_settings(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>) {
    let (services, data_dir) = {
        let state = state.borrow();
        (state.services.clone(), state.data_dir.clone())
    };
    cx.update(|cx| {
        crate::window::open_settings_window(cx, state.clone(), services, data_dir);
    });
}

pub(crate) fn drain_clipboard(state: &Rc<RefCell<AppState>>) {
    let receiver = state.borrow().clipboard_rx.borrow().clone();
    let Some(receiver) = receiver else {
        return;
    };
    let mut latest = None;
    while let Ok(entries) = receiver.try_recv() {
        latest = Some(entries);
    }
    if let Some(entries) = latest {
        if let Ok(mut current) = state.borrow().clipboard_entries.lock() {
            *current = entries;
        }
    }
}

fn set_language(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>, locale: &str) {
    {
        let state = state.borrow();
        if let Err(error) = state.storage.borrow().set_setting(LANGUAGE_SETTING, locale) {
            error!(error = %format!("{error:#}"), "persist language");
            return;
        }
        state.i18n.select_language(locale);
        let strings = state.i18n.ui_strings();
        state.services.set_locale(locale, strings.clone());
        state.search.borrow_mut().set_ui_strings(strings);
        if let Some(tray) = state.tray.borrow().as_ref() {
            if let Err(error) = tray.set_language(&state.i18n) {
                error!(error = %format!("{error:#}"), "update tray language");
            }
        };
    }
    cx.update(|cx| cx.refresh_windows());
}

fn set_accent(state: &Rc<RefCell<AppState>>, accent: &str) {
    let Some(accent) = normalize_accent(accent) else {
        warn!(accent, "ignore invalid accent; expected #rrggbb");
        return;
    };
    let state_ref = state.borrow();
    if let Err(error) = state_ref
        .storage
        .borrow()
        .set_setting(THEME_COLOR_SETTING, &accent)
    {
        error!(error = %format!("{error:#}"), "persist accent");
        return;
    }
    state_ref.services.set_accent(&accent);
    drop(state_ref);
    crate::plugin_window::broadcast_theme(state, &accent);
}

fn activate(cx: &mut AsyncApp, state: &Rc<RefCell<AppState>>, action: RowAction) {
    match action {
        RowAction::LaunchApp(path) => {
            let _ = state.borrow().storage.borrow().upsert_usage(&path);
            if let Err(error) = crate::launch::launch(&path) {
                error!(path = %path.display(), error = %format!("{error:#}"), "launch application");
            }
            if let Some(handle) = state.borrow().window {
                let _ = handle.update(cx, |_, window, cx| crate::window::hide_window(window, cx));
            }
        }
        RowAction::CopyText(text) => {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(text);
            }
            if let Some(handle) = state.borrow().window {
                let _ = handle.update(cx, |_, window, cx| crate::window::hide_window(window, cx));
            }
        }
        RowAction::OpenUrl(url) => {
            if let Err(error) = crate::launch::open_url(&url) {
                error!(url, error = %format!("{error:#}"), "open URL");
            }
        }
        RowAction::DismissNotice => state.borrow().services.clear_notice(),
        RowAction::OpenPlugin {
            plugin_id,
            command,
            title,
            input,
        } => {
            let (locale, accent) = {
                let state = state.borrow();
                (state.services.locale(), state.services.accent())
            };
            let request = LaunchRequest {
                plugin_id,
                command,
                title,
                input,
                locale,
                accent,
                dark: true,
            };
            let focused = cx.update(|cx| crate::plugin_window::focus_and_send(cx, state, &request));
            if focused {
                return;
            }
            if let Err(error) = state
                .borrow_mut()
                .search
                .borrow_mut()
                .start_launch(request.clone())
            {
                report_launch_failure(state, &request, error.code(), &error.to_string());
            }
        }
    }
}

fn report_launch_failure(
    state: &Rc<RefCell<AppState>>,
    request: &LaunchRequest,
    code: steward_plugin_contract::PluginErrorCode,
    error: &str,
) {
    error!(
        plugin_id = %request.plugin_id,
        command = %request.command,
        error_code = code.as_str(),
        error,
        "plugin launch failed"
    );
    let title = state.borrow().i18n.translate("plugin-launch-failed");
    state.borrow().services.publish_notice(PluginNotice {
        title,
        message: format!("{} - {}", request.title, code.as_str()),
        plugin_id: Some(request.plugin_id.clone()),
        code,
    });
}

fn normalize_accent(value: &str) -> Option<String> {
    let hex = value.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::normalize_accent;

    #[test]
    fn accents_are_normalized_and_validated() {
        assert_eq!(normalize_accent("#863BFF").as_deref(), Some("#863bff"));
        assert_eq!(normalize_accent("a6e3a1").as_deref(), Some("#a6e3a1"));
        assert_eq!(normalize_accent("#12345"), None);
        assert_eq!(normalize_accent("#hello!"), None);
    }
}

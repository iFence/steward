//! Shared construction and error handling for NSH AppViews.

use std::sync::Arc;

use nsh_core::js::Limits;
use nsh_core::ui::EngineContext;
use nsh_platform_gpui::{AppOptions, AppView};

use crate::services::AppServices;

pub(crate) fn plugin_limits() -> Limits {
    Limits {
        memory_bytes: 64 << 20,
        stack_bytes: 1 << 20,
        load_budget: std::time::Duration::from_secs(2),
        call_budget: std::time::Duration::from_millis(250),
        max_nodes: 20_000,
        ..Limits::default()
    }
}

pub(crate) fn shell_options(
    services: Arc<AppServices>,
    data_dir: std::path::PathBuf,
) -> AppOptions {
    AppOptions {
        app_id: "com.hiaspirin.steward.shell".into(),
        title: "Steward".into(),
        data_dir: Some(data_dir),
        limits: Limits::standalone(),
        context: EngineContext::default().with(services),
        ..AppOptions::default()
    }
}

pub(crate) fn new_view(
    options: AppOptions,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<AppView>,
) -> AppView {
    let view = AppView::new(options, window, cx);
    view.on_app_error(|error| eprintln!("[steward][nsh] {error}"));
    view
}

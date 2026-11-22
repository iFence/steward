//! Plugin-scoped runtime diagnostics for the developer loop.
//!
//! Capability traces deliberately contain only the host-call name and outcome.
//! Request bodies, arguments, clipboard data, file contents, and signing
//! material never enter this in-memory surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, RwLock,
};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use steward_plugin_contract::PluginErrorCode;

const MAX_RECENT_EVENTS: usize = 12;
const MAX_LOG_LINES: usize = 40;
const LOG_TAIL_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeveloperDiagnosticsSnapshot {
    pub(crate) enabled: bool,
    pub(crate) data_dir: PathBuf,
    pub(crate) plugins: Vec<PluginRuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginRuntimeDiagnostic {
    pub(crate) plugin_id: String,
    pub(crate) active: bool,
    pub(crate) session_id: Option<u64>,
    pub(crate) first_frame_ms: Option<u64>,
    pub(crate) launch_attempts: u64,
    pub(crate) process_exits: u64,
    pub(crate) crashes: u64,
    pub(crate) last_error: Option<String>,
    pub(crate) recent_events: Vec<String>,
    pub(crate) log_excerpt: Vec<String>,
}

#[derive(Default)]
struct PluginRuntimeState {
    active: bool,
    session_id: Option<u64>,
    first_frame_ms: Option<u64>,
    launch_attempts: u64,
    process_exits: u64,
    crashes: u64,
    last_error: Option<String>,
    recent_events: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct DeveloperDiagnosticsPort {
    enabled: Arc<AtomicBool>,
    data_dir: Arc<RwLock<PathBuf>>,
    plugins: Arc<RwLock<BTreeMap<String, PluginRuntimeState>>>,
    revision: Arc<AtomicU64>,
}

impl Default for DeveloperDiagnosticsPort {
    fn default() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            data_dir: Arc::new(RwLock::new(PathBuf::new())),
            plugins: Arc::new(RwLock::new(BTreeMap::new())),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl DeveloperDiagnosticsPort {
    pub(crate) fn configure(&self, enabled: bool, data_dir: &Path) {
        self.enabled.store(enabled, Ordering::Release);
        if let Ok(mut current) = self.data_dir.write() {
            *current = data_dir.to_path_buf();
        }
        self.revision.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(crate) fn snapshot(&self) -> DeveloperDiagnosticsSnapshot {
        let enabled = self.is_enabled();
        let data_dir = self
            .data_dir
            .read()
            .map(|path| path.clone())
            .unwrap_or_default();
        let mut plugins = self
            .plugins
            .read()
            .map(|plugins| {
                plugins
                    .iter()
                    .map(|(plugin_id, plugin)| PluginRuntimeDiagnostic {
                        plugin_id: plugin_id.clone(),
                        active: plugin.active,
                        session_id: plugin.session_id,
                        first_frame_ms: plugin.first_frame_ms,
                        launch_attempts: plugin.launch_attempts,
                        process_exits: plugin.process_exits,
                        crashes: plugin.crashes,
                        last_error: plugin.last_error.clone(),
                        recent_events: plugin.recent_events.clone(),
                        log_excerpt: Vec::new(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for plugin in &mut plugins {
            plugin.log_excerpt = plugin_log_excerpt(&data_dir, &plugin.plugin_id);
        }
        plugins.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
        DeveloperDiagnosticsSnapshot {
            enabled,
            data_dir,
            plugins,
        }
    }

    pub(crate) fn record_launch(
        &self,
        plugin_id: &str,
        session_id: u64,
        requested_at: std::time::Instant,
    ) {
        if !self.is_enabled() {
            return;
        }
        self.update(plugin_id, |plugin| {
            plugin.active = true;
            plugin.session_id = Some(session_id);
            plugin.first_frame_ms = None;
            plugin.launch_attempts = plugin.launch_attempts.saturating_add(1);
            plugin.last_error = None;
            push_event(
                plugin,
                format!(
                    "session {session_id} opened after {} ms",
                    requested_at.elapsed().as_millis()
                ),
            );
        });
    }

    pub(crate) fn record_first_frame(
        &self,
        plugin_id: &str,
        session_id: u64,
        elapsed: std::time::Duration,
    ) {
        if !self.is_enabled() {
            return;
        }
        self.update(plugin_id, |plugin| {
            let elapsed_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
            plugin.active = true;
            plugin.session_id = Some(session_id);
            plugin.first_frame_ms = Some(elapsed_ms);
            plugin.last_error = None;
            push_event(plugin, format!("{elapsed_ms} ms to first frame"));
        });
    }

    pub(crate) fn record_failure(
        &self,
        plugin_id: &str,
        session_id: Option<u64>,
        code: PluginErrorCode,
        error: &str,
    ) {
        if !self.is_enabled() {
            return;
        }
        self.update(plugin_id, |plugin| {
            plugin.active = false;
            plugin.session_id = session_id.or(plugin.session_id);
            plugin.process_exits = plugin.process_exits.saturating_add(1);
            plugin.crashes = plugin.crashes.saturating_add(1);
            plugin.last_error = Some(bounded(error, 512));
            push_event(
                plugin,
                format!("{}: {}", code.as_str(), bounded(error, 180)),
            );
        });
    }

    pub(crate) fn record_stopped(&self, plugin_id: &str, session_id: u64) {
        if !self.is_enabled() {
            return;
        }
        self.update(plugin_id, |plugin| {
            plugin.active = false;
            plugin.process_exits = plugin.process_exits.saturating_add(1);
            push_event(plugin, format!("session {session_id} closed"));
        });
    }

    fn update(&self, plugin_id: &str, update: impl FnOnce(&mut PluginRuntimeState)) {
        if let Ok(mut plugins) = self.plugins.write() {
            update(plugins.entry(plugin_id.to_owned()).or_default());
        }
        self.revision.fetch_add(1, Ordering::Release);
    }
}

fn push_event(plugin: &mut PluginRuntimeState, event: String) {
    plugin.recent_events.push(event);
    if plugin.recent_events.len() > MAX_RECENT_EVENTS {
        plugin
            .recent_events
            .drain(..plugin.recent_events.len() - MAX_RECENT_EVENTS);
    }
}

fn plugin_log_excerpt(data_dir: &Path, plugin_id: &str) -> Vec<String> {
    let logs_dir = data_dir.join("logs");
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return Vec::new();
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("steward.log"))
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    });

    let mut lines = Vec::new();
    for path in paths.into_iter().rev() {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let start = bytes.len().saturating_sub(LOG_TAIL_BYTES);
        for line in String::from_utf8_lossy(&bytes[start..]).lines() {
            let lower = line.to_ascii_lowercase();
            if line.contains(plugin_id)
                && ["error", "fail", "denied", "panic", "exit"]
                    .iter()
                    .any(|needle| lower.contains(needle))
            {
                lines.push(bounded(line.trim(), 320));
            }
        }
        if lines.len() >= MAX_LOG_LINES {
            break;
        }
    }
    if lines.len() > MAX_LOG_LINES {
        lines.drain(..lines.len() - MAX_LOG_LINES);
    }
    lines
}

fn bounded(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

#[cfg(test)]
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_track_first_frame_and_failure_without_payloads() {
        let root = std::env::temp_dir().join(format!(
            "steward-developer-diagnostics-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let port = DeveloperDiagnosticsPort::default();
        port.configure(true, &root);
        port.record_launch("com.example.dev", 7, std::time::Instant::now());
        port.record_first_frame("com.example.dev", 7, std::time::Duration::from_millis(42));
        port.record_failure(
            "com.example.dev",
            Some(7),
            PluginErrorCode::RuntimeError,
            "application panic: boom",
        );

        let snapshot = port.snapshot();
        let plugin = &snapshot.plugins[0];
        assert_eq!(plugin.first_frame_ms, Some(42));
        assert_eq!(plugin.process_exits, 1);
        assert_eq!(plugin.crashes, 1);
        assert!(plugin
            .recent_events
            .iter()
            .any(|event| event.contains("first frame")));
        assert!(!format!("{plugin:?}").contains("{\"secret\":"));
    }

    #[test]
    fn text_truncation_respects_utf8_boundaries() {
        assert_eq!(bounded("你好世界", 4), "你...");
    }
}

//! Plugin host: runtime process lifecycle, routing, permissions and the IPC
//! gateway.
//!
//! The host owns one shared-pool runtime process (plus one dedicated process
//! per `dedicated-process` plugin), routes queries through [`RouteIndex`],
//! invokes commands over NDJSON JSON-RPC, and recycles crashed runtimes with
//! exponential backoff. Every `command.invoke` carries the query generation
//! that sent it; stale responses are surfaced with their generation so the
//! caller can drop them when a newer query already superseded them.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context as _, Result};
use crossbeam_channel::Receiver;
use serde_json::{json, Value};
use steward_ipc_protocol::{
    code, decode_line, encode_line, method, Message, Notification, Request, Response, RpcError,
};
use steward_plugin_registry::{Isolation, PluginMeta};

use crate::route::{RouteHit, RouteIndex};

/// Connection key of the shared pool (a dedicated connection is keyed by its
/// plugin id).
const SHARED_POOL_KEY: &str = "";

const DEFAULT_BASE_BACKOFF: Duration = Duration::from_millis(500);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Host configuration.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Path of the `steward-plugin-runtime` binary.
    pub runtime_bin: PathBuf,
    /// Delay before the first restart attempt after a crash; doubled per
    /// failed attempt.
    pub base_backoff: Duration,
    /// Upper bound on the restart delay.
    pub max_backoff: Duration,
}

impl HostConfig {
    /// Resolve the runtime binary: `STEWARD_PLUGIN_RUNTIME_BIN` override, else
    /// the sibling `steward-plugin-runtime` next to the current executable
    /// (both binaries ship side by side in `target/{debug,release}`).
    pub fn from_env() -> Result<Self> {
        let runtime_bin = if let Ok(path) = std::env::var("STEWARD_PLUGIN_RUNTIME_BIN") {
            PathBuf::from(path)
        } else {
            let exe_name = if cfg!(windows) {
                "steward-plugin-runtime.exe"
            } else {
                "steward-plugin-runtime"
            };
            std::env::current_exe()
                .context("locate current executable")?
                .parent()
                .map(|dir| dir.join(exe_name))
                .ok_or_else(|| anyhow!("current executable has no parent directory"))?
        };
        Ok(Self {
            runtime_bin,
            base_backoff: DEFAULT_BASE_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        })
    }
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            runtime_bin: PathBuf::from("steward-plugin-runtime"),
            base_backoff: DEFAULT_BASE_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
    }
}

/// Events the host hands to the app, drained by the foreground poll task.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// A plugin command produced a view (or failed). `gen` is the query
    /// generation that sent the request; the caller drops events whose
    /// generation no longer matches the current query.
    CommandResult {
        gen: u64,
        plugin_id: String,
        command: String,
        result: Result<Value, RpcError>,
    },
    /// A plugin asked the host to show a toast (`{ message, kind?,
    /// durationMs? }`).
    Toast { params: Value },
    /// A runtime process died; the host schedules a restart with backoff.
    RuntimeCrashed {
        /// `None` for the shared pool, `Some(plugin_id)` for a dedicated
        /// process.
        plugin_id: Option<String>,
    },
    /// A crashed runtime was restarted and its plugins reloaded.
    RuntimeRestarted { plugin_id: Option<String> },
}

/// One decoded frame from a runtime process's stdout.
enum Frame {
    Response(Response),
    Notification(Notification),
    /// The process exited / its stdout closed.
    Eof,
}

/// A live connection to one runtime process.
struct RuntimeConn {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Frame>,
    /// plugin_id -> isolate_id handed out by `plugin.load`.
    isolates: HashMap<String, u64>,
}

/// What a pending request id is waiting for.
enum Pending {
    Load {
        plugin_id: String,
    },
    Invoke {
        gen: u64,
        plugin_id: String,
        command: String,
    },
    Item {
        plugin_id: String,
        item_id: String,
    },
}

/// Restart bookkeeping for one crashed connection.
struct RestartState {
    plugins: Vec<PluginMeta>,
    attempts: u32,
    next_attempt: Instant,
}

/// The plugin host. Not `Send`: it owns `Child` handles and is driven from
/// the app's main thread.
pub struct PluginHost {
    config: HostConfig,
    routes: RouteIndex,
    /// `""` = shared pool, otherwise the plugin id of a dedicated process.
    conns: HashMap<String, RuntimeConn>,
    next_request_id: u64,
    pending: HashMap<u64, Pending>,
    restarts: HashMap<String, RestartState>,
    /// plugin_id -> (isolation, meta) of everything currently loaded; used to
    /// detect whether a plugin-set change needs a process reload and to
    /// reload plugins after a runtime crash.
    loaded: HashMap<String, (Isolation, PluginMeta)>,
}

impl PluginHost {
    pub fn new(config: HostConfig) -> Self {
        Self {
            config,
            routes: RouteIndex::new(),
            conns: HashMap::new(),
            next_request_id: 1,
            pending: HashMap::new(),
            restarts: HashMap::new(),
            loaded: HashMap::new(),
        }
    }

    /// Load the given plugin set: rebuild routing, spawn the shared pool (if
    /// any shared-pool plugin is installed) and one dedicated process per
    /// `dedicated-process` plugin, then send `plugin.load` for each. When the
    /// plugin set is unchanged this is a no-op (the common cold-start path:
    /// registry cache read -> same set -> no process churn).
    pub fn set_plugins(&mut self, metas: &[PluginMeta]) -> Result<()> {
        let target = metas
            .iter()
            .map(|meta| (meta.manifest.id.clone(), meta.manifest.isolation))
            .collect::<HashMap<_, _>>();
        let loaded_iso = self
            .loaded
            .iter()
            .map(|(id, (isolation, _))| (id.clone(), *isolation))
            .collect::<HashMap<_, _>>();
        if target == loaded_iso {
            self.rebuild_routes(metas);
            return Ok(());
        }

        self.shutdown();
        self.loaded = metas
            .iter()
            .map(|meta| {
                (
                    meta.manifest.id.clone(),
                    (meta.manifest.isolation, meta.clone()),
                )
            })
            .collect();

        let has_shared = metas
            .iter()
            .any(|meta| meta.manifest.isolation == Isolation::SharedPool);
        if has_shared {
            self.spawn_conn(SHARED_POOL_KEY, None)?;
        }
        for meta in metas {
            match meta.manifest.isolation {
                Isolation::SharedPool => {
                    self.load_into(SHARED_POOL_KEY, meta);
                }
                Isolation::DedicatedProcess => {
                    if let Err(error) = self.spawn_conn(&meta.manifest.id, Some(&meta.manifest.id))
                    {
                        eprintln!(
                            "[plugin-host] cannot start dedicated runtime for '{}': {error:#}",
                            meta.manifest.id
                        );
                        continue;
                    }
                    self.load_into(&meta.manifest.id, meta);
                }
            }
        }
        self.rebuild_routes(metas);
        Ok(())
    }

    /// Match a query against the loaded plugin routes. No plugin process is
    /// woken here; invoke only the hits the app decides to render.
    pub fn query(&self, query: &str) -> Vec<RouteHit> {
        self.routes.match_query(query)
    }

    /// Number of registered routes (all plugins).
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Send `command.invoke` for a route hit and remember its response under
    /// the query generation `gen`. Returns the request id, or `None` when the
    /// plugin's isolate is not ready yet (still loading, crashed, or absent).
    pub fn invoke(&mut self, gen: u64, hit: &RouteHit) -> Option<u64> {
        let conn_key = self.conn_key_for(&hit.plugin_id);
        let conn = self.conns.get_mut(&conn_key)?;
        let isolate_id = *conn.isolates.get(&hit.plugin_id)?;
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request = Request::new(
            id,
            method::COMMAND_INVOKE,
            json!({
                "isolate_id": isolate_id,
                "command": hit.command,
                "input": hit.input,
                "deadline_ms": hit.deadline_ms,
            }),
        );
        if conn.send(&request).is_err() {
            return None;
        }
        self.pending.insert(
            id,
            Pending::Invoke {
                gen,
                plugin_id: hit.plugin_id.clone(),
                command: hit.command.clone(),
            },
        );
        Some(id)
    }

    /// Send `item.invoke` for a rendered list row (fire-and-forget; the
    /// launcher stays open). Errors surface as toast events.
    pub fn invoke_item(&mut self, plugin_id: &str, item_id: &str) -> Option<u64> {
        let conn_key = self.conn_key_for(plugin_id);
        let conn = self.conns.get_mut(&conn_key)?;
        let isolate_id = *conn.isolates.get(plugin_id)?;
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request = Request::new(
            id,
            method::ITEM_INVOKE,
            json!({
                "isolate_id": isolate_id,
                "item_id": item_id,
                "deadline_ms": crate::route::STATIC_DEADLINE_MS,
            }),
        );
        if conn.send(&request).is_err() {
            return None;
        }
        self.pending.insert(
            id,
            Pending::Item {
                plugin_id: plugin_id.to_string(),
                item_id: item_id.to_string(),
            },
        );
        Some(id)
    }

    /// Process all pending runtime frames and due restarts; returns events
    /// for the caller (the app's foreground poll task).
    pub fn drain_events(&mut self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        let keys = self.conns.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let mut frames = Vec::new();
            if let Some(conn) = self.conns.get_mut(&key) {
                while let Ok(frame) = conn.frames.try_recv() {
                    frames.push(frame);
                }
            }
            for frame in frames {
                match frame {
                    Frame::Response(response) => self.handle_response(&key, response, &mut events),
                    Frame::Notification(notification) => {
                        if notification.method == method::TOAST_SHOW {
                            events.push(HostEvent::Toast {
                                params: notification.params,
                            });
                        }
                    }
                    Frame::Eof => self.handle_eof(&key, &mut events),
                }
            }
        }

        let now = Instant::now();
        let due = self
            .restarts
            .iter()
            .filter(|(_, state)| state.next_attempt <= now)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in due {
            if let Some(event) = self.try_restart(&key) {
                events.push(event);
            }
        }
        events
    }

    /// Stop every runtime process and drop all state.
    pub fn shutdown(&mut self) {
        for (_, mut conn) in self.conns.drain() {
            let _ = conn.child.kill();
            let _ = conn.child.wait();
        }
        self.pending.clear();
        self.restarts.clear();
        self.loaded.clear();
    }

    fn rebuild_routes(&mut self, metas: &[PluginMeta]) {
        self.routes.clear();
        for meta in metas {
            self.routes
                .add_plugin(&meta.manifest.id, &meta.manifest.commands);
        }
    }

    /// Test-only: kill the shared pool's runtime process to exercise crash
    /// detection and restart. The connection is left in place so the reader
    /// thread observes EOF and the normal crash path runs. Returns whether a
    /// shared connection existed.
    #[doc(hidden)]
    pub fn kill_shared_runtime_for_test(&mut self) -> bool {
        let Some(conn) = self.conns.get_mut(SHARED_POOL_KEY) else {
            return false;
        };
        let _ = conn.child.kill();
        true
    }

    fn load_into(&mut self, conn_key: &str, meta: &PluginMeta) {
        let request = Request::new(
            self.next_request_id,
            method::PLUGIN_LOAD,
            json!({
                "id": meta.manifest.id,
                "entry_path": meta.entry,
                "manifest": meta.manifest,
            }),
        );
        let id = request.id;
        self.next_request_id += 1;
        let Some(conn) = self.conns.get_mut(conn_key) else {
            return;
        };
        if conn.send(&request).is_err() {
            eprintln!(
                "[plugin-host] failed to send plugin.load for '{}'",
                meta.manifest.id
            );
            return;
        }
        self.pending.insert(
            id,
            Pending::Load {
                plugin_id: meta.manifest.id.clone(),
            },
        );
    }

    fn handle_response(&mut self, conn_key: &str, response: Response, events: &mut Vec<HostEvent>) {
        let Some(pending) = self.pending.remove(&response.id) else {
            return;
        };
        match pending {
            Pending::Load { plugin_id } => {
                if let Some(isolate_id) = response
                    .result
                    .as_ref()
                    .and_then(|r| r["isolate_id"].as_u64())
                {
                    if let Some(conn) = self.conns.get_mut(conn_key) {
                        conn.isolates.insert(plugin_id, isolate_id);
                    }
                } else if let Some(error) = response.error {
                    eprintln!(
                        "[plugin-host] failed to load plugin {plugin_id}: {} ({})",
                        error.message, error.code
                    );
                }
            }
            Pending::Invoke {
                gen,
                plugin_id,
                command,
            } => {
                let result = match (response.result, response.error) {
                    (Some(result), None) => Ok(result),
                    (_, Some(error)) => Err(error),
                    _ => Err(RpcError::new(code::INTERNAL_ERROR, "empty response")),
                };
                events.push(HostEvent::CommandResult {
                    gen,
                    plugin_id,
                    command,
                    result,
                });
            }
            Pending::Item { plugin_id, item_id } => {
                if let Some(error) = response.error {
                    events.push(HostEvent::Toast {
                        params: json!({
                            "message": format!("{plugin_id} ({item_id}): {}", error.message),
                            "kind": "error",
                        }),
                    });
                }
            }
        }
    }

    fn handle_eof(&mut self, key: &str, events: &mut Vec<HostEvent>) {
        if let Some(mut conn) = self.conns.remove(key) {
            let _ = conn.child.kill();
            let _ = conn.child.wait();
            let plugins = if key == SHARED_POOL_KEY {
                self.loaded
                    .iter()
                    .filter(|(_, (isolation, _))| *isolation == Isolation::SharedPool)
                    .map(|(id, (_, meta))| (id.clone(), meta.clone()))
                    .collect::<Vec<_>>()
            } else {
                self.loaded
                    .get(key)
                    .map(|(_, meta)| (key.to_string(), meta.clone()))
                    .into_iter()
                    .collect()
            };
            // The isolate ids died with the process: forget them so the next
            // invoke reports the plugin as not-ready instead of sending stale
            // ids into a fresh process.
            let plugin_id = (key != SHARED_POOL_KEY).then(|| key.to_string());
            events.push(HostEvent::RuntimeCrashed {
                plugin_id: plugin_id.clone(),
            });
            let attempts = self.restarts.get(key).map_or(0, |state| state.attempts) + 1;
            let delay = backoff(attempts, self.config.base_backoff, self.config.max_backoff);
            self.restarts.insert(
                key.to_string(),
                RestartState {
                    plugins: plugins.into_iter().map(|(_, meta)| meta).collect(),
                    attempts,
                    next_attempt: Instant::now() + delay,
                },
            );
            eprintln!(
                "[plugin-host] runtime '{key}' crashed; restart in {} ms",
                delay.as_millis()
            );
        }
    }

    fn try_restart(&mut self, key: &str) -> Option<HostEvent> {
        let state = self.restarts.remove(key)?;
        let dedicated = key != SHARED_POOL_KEY;
        let spawn = if dedicated {
            self.spawn_conn(key, Some(key))
        } else {
            self.spawn_conn(key, None)
        };
        match spawn {
            Ok(()) => {
                let metas = state.plugins.clone();
                for meta in &metas {
                    self.load_into(key, meta);
                }
                eprintln!("[plugin-host] runtime '{key}' restarted");
                Some(HostEvent::RuntimeRestarted {
                    plugin_id: dedicated.then(|| key.to_string()),
                })
            }
            Err(error) => {
                let attempts = state.attempts + 1;
                let delay = backoff(attempts, self.config.base_backoff, self.config.max_backoff);
                self.restarts.insert(
                    key.to_string(),
                    RestartState {
                        plugins: state.plugins,
                        attempts,
                        next_attempt: Instant::now() + delay,
                    },
                );
                eprintln!(
                    "[plugin-host] restart of '{key}' failed ({error:#}); retry in {} ms",
                    delay.as_millis()
                );
                None
            }
        }
    }

    fn spawn_conn(&mut self, key: &str, plugin_id: Option<&str>) -> Result<()> {
        let mut command = Command::new(&self.config.runtime_bin);
        if plugin_id.is_some() {
            command.arg("--dedicated");
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().with_context(|| {
            format!(
                "spawn plugin runtime '{}'",
                self.config.runtime_bin.display()
            )
        })?;
        let stdin = child.stdin.take().context("take runtime stdin")?;
        let stdout = child.stdout.take().context("take runtime stdout")?;
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                let message = match decode_line(&line) {
                    Ok(Some(message)) => message,
                    Ok(None) => continue,
                    Err(_) => continue,
                };
                let frame = match message {
                    Message::Response(response) => Frame::Response(response),
                    Message::Notification(notification) => Frame::Notification(notification),
                    Message::Request(_) => continue,
                };
                if tx.send(frame).is_err() {
                    break;
                }
            }
            let _ = tx.send(Frame::Eof);
        });
        self.conns.insert(
            key.to_string(),
            RuntimeConn {
                child,
                stdin,
                frames: rx,
                isolates: HashMap::new(),
            },
        );
        Ok(())
    }

    fn conn_key_for(&self, plugin_id: &str) -> String {
        if self
            .loaded
            .get(plugin_id)
            .is_some_and(|(isolation, _)| *isolation == Isolation::DedicatedProcess)
        {
            plugin_id.to_string()
        } else {
            SHARED_POOL_KEY.to_string()
        }
    }
}

/// Exponential backoff: `base * 2^(attempts-1)`, capped at `max`.
fn backoff(attempts: u32, base: Duration, max: Duration) -> Duration {
    let factor = 1u32
        .checked_shl(attempts.saturating_sub(1).min(20))
        .unwrap_or(u32::MAX);
    let delay = base.saturating_mul(factor);
    delay.min(max)
}

impl RuntimeConn {
    fn send(&mut self, request: &Request) -> Result<()> {
        let line = encode_line(&Message::Request(request.clone()))?;
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        for (_, mut conn) in self.conns.drain() {
            let _ = conn.child.kill();
            let _ = conn.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let base = Duration::from_millis(500);
        let max = Duration::from_secs(30);
        assert_eq!(backoff(1, base, max), Duration::from_millis(500));
        assert_eq!(backoff(2, base, max), Duration::from_millis(1000));
        assert_eq!(backoff(3, base, max), Duration::from_millis(2000));
        assert_eq!(backoff(10, base, max), max);
    }
}

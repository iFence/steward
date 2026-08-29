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
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context as _, Result};
use crossbeam_channel::Receiver;
use serde_json::{json, Value};
use steward_ipc_protocol::{
    code, decode_line, encode_line, method, ClipboardEntry, Message, Notification, Request,
    Response, RpcError,
};
use steward_plugin_registry::{Isolation, Permission, PluginMeta};

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
    /// A `search` view's results for a query. `gen` is the generation that
    /// sent the request; the caller drops stale generations like a command
    /// result. The launcher/panel replaces the search view's results area.
    SearchResult {
        gen: u64,
        plugin_id: String,
        command: String,
        query: String,
        result: Result<Value, RpcError>,
    },
    /// A plugin asked the host to show a toast (`{ message, kind?,
    /// durationMs? }`).
    Toast { params: Value },
    /// A list item selection returned a new view (e.g. a `detail` drill-down).
    /// The launcher replaces the command's current view slot with `view`.
    ItemView {
        plugin_id: String,
        command: String,
        item_id: String,
        view: Value,
    },
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
        hit: RouteHit,
    },
    Item {
        plugin_id: String,
        command: String,
        item_id: String,
    },
    Action {
        plugin_id: String,
        action_id: String,
        item_id: Option<String>,
    },
    Submit {
        plugin_id: String,
        values: Value,
    },
    Search {
        gen: u64,
        plugin_id: String,
        command: String,
        query: String,
    },
}

/// A call that must wait for its plugin's isolate to be (re)loaded before it
/// can be dispatched. Used by the lazy-load path so an invoice arriving
/// before the isolate is materialized is replayed once the `plugin.load`
/// response lands, instead of being dropped.
enum QueuedCall {
    Command {
        gen: u64,
        hit: RouteHit,
    },
    Item {
        plugin_id: String,
        command: String,
        item_id: String,
    },
    Action {
        plugin_id: String,
        action_id: String,
        item_id: Option<String>,
    },
    Submit {
        plugin_id: String,
        values: Value,
    },
    Search {
        gen: u64,
        plugin_id: String,
        command: String,
        query: String,
    },
}

/// Restart bookkeeping for one crashed connection.
struct RestartState {
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
    /// Plugins whose `plugin.load` is currently in flight (per connection).
    /// Used to avoid duplicate loads and to drive the reload-on-NotFound path.
    loading_plugins: HashSet<String>,
    /// Calls waiting for their plugin's isolate to load, keyed by plugin id.
    queued: HashMap<String, Vec<QueuedCall>>,
    /// plugin_id -> (isolation, meta) of everything currently loaded; used to
    /// detect whether a plugin-set change needs a process reload and to
    /// reload plugins after a runtime crash.
    loaded: HashMap<String, (Isolation, PluginMeta)>,
    /// Recent clipboard history, handed to plugins that declare
    /// `clipboard.history` on each `command.invoke`.
    clipboard_history: Vec<ClipboardEntry>,
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
            loading_plugins: HashSet::new(),
            queued: HashMap::new(),
            loaded: HashMap::new(),
            clipboard_history: Vec::new(),
        }
    }

    /// Replace the clipboard-history snapshot the host hands to plugins that
    /// declare `clipboard.history`. Called by the app's clipboard watcher.
    pub fn set_clipboard_history(&mut self, entries: Vec<ClipboardEntry>) {
        self.clipboard_history = entries;
    }

    /// The last clipboard-history snapshot (informational / easy testing).
    pub fn clipboard_history(&self) -> &[ClipboardEntry] {
        &self.clipboard_history
    }

    /// Load the given plugin set: rebuild routing, spawn the shared pool (if
    /// any shared-pool plugin is installed) and one dedicated process per
    /// `dedicated-process` plugin. Plugins are *lazy*: no `plugin.load` is
    /// sent here, so cold start evaluates no JS bundle and cost is proportional
    /// to the installed count only through the cheap route build — never the
    /// active plugin count. An isolate is materialized on first invoke and
    /// re-created on demand when it is LRU-evicted or killed. When the plugin
    /// set is unchanged this is a no-op (the common cold-start path: registry
    /// cache read -> same set -> no process churn).
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
            if meta.manifest.isolation == Isolation::DedicatedProcess {
                if let Err(error) = self.spawn_conn(&meta.manifest.id, Some(&meta.manifest.id)) {
                    eprintln!(
                        "[plugin-host] cannot start dedicated runtime for '{}': {error:#}",
                        meta.manifest.id
                    );
                    continue;
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
    /// plugin cannot be dispatched at all (its connection is missing or the
    /// plugin is not in the current set). A plugin whose isolate is not yet
    /// materialized is lazy-loaded here: the invoke is queued and replayed as
    /// soon as `plugin.load` completes, so cold plugins are still rendered
    /// without a full startup scan. When the invoke is queued rather than sent
    /// immediately, `Some(0)` is returned as a sentinel (real request ids
    /// start at 1).
    pub fn invoke(&mut self, gen: u64, hit: &RouteHit) -> Option<u64> {
        let plugin_id = hit.plugin_id.clone();
        let conn_key = self.conn_key_for(&plugin_id);
        if !self.conns.contains_key(&conn_key) {
            return None;
        }
        if self
            .conns
            .get(&conn_key)
            .is_some_and(|conn| conn.isolates.contains_key(&plugin_id))
        {
            let id = self.dispatch_command(&conn_key, hit)?;
            self.pending.insert(
                id,
                Pending::Invoke {
                    gen,
                    hit: hit.clone(),
                },
            );
            return Some(id);
        }
        if !self.loaded.contains_key(&plugin_id) {
            return None;
        }
        self.queued
            .entry(plugin_id.clone())
            .or_default()
            .push(QueuedCall::Command {
                gen,
                hit: hit.clone(),
            });
        self.ensure_loaded(&conn_key, &plugin_id);
        Some(0)
    }

    /// Send `item.invoke` for a rendered list row. When a plugin's `select`
    /// returns a view (e.g. a `detail` drill-down), the host surfaces it as a
    /// [`HostEvent::ItemView`] under `command` so the launcher can replace the
    /// command's view slot; otherwise the launcher stays open (fire-and-forget).
    /// Errors surface as toast events. Like [`invoke`], a not-yet-loaded plugin
    /// is lazy-loaded here, with `Some(0)` returned as the sentinel for a
    /// queued call.
    pub fn invoke_item(&mut self, plugin_id: &str, command: &str, item_id: &str) -> Option<u64> {
        let conn_key = self.conn_key_for(plugin_id);
        if !self.conns.contains_key(&conn_key) {
            return None;
        }
        if self
            .conns
            .get(&conn_key)
            .is_some_and(|conn| conn.isolates.contains_key(plugin_id))
        {
            let id = self.dispatch_item(&conn_key, plugin_id, item_id)?;
            self.pending.insert(
                id,
                Pending::Item {
                    plugin_id: plugin_id.to_string(),
                    command: command.to_string(),
                    item_id: item_id.to_string(),
                },
            );
            return Some(id);
        }
        if !self.loaded.contains_key(plugin_id) {
            return None;
        }
        self.queued
            .entry(plugin_id.to_string())
            .or_default()
            .push(QueuedCall::Item {
                plugin_id: plugin_id.to_string(),
                command: command.to_string(),
                item_id: item_id.to_string(),
            });
        self.ensure_loaded(&conn_key, plugin_id);
        Some(0)
    }

    /// Send `action.invoke` for a view-level `ActionPanel` action. Errors
    /// surface as toast events. Like [`invoke_item`], a not-yet-loaded plugin
    /// is lazy-loaded here, with `Some(0)` returned as the sentinel for a
    /// queued call.
    pub fn invoke_action(
        &mut self,
        plugin_id: &str,
        action_id: &str,
        item_id: Option<&str>,
    ) -> Option<u64> {
        let conn_key = self.conn_key_for(plugin_id);
        if !self.conns.contains_key(&conn_key) {
            return None;
        }
        let item_id = item_id.map(ToString::to_string);
        if self
            .conns
            .get(&conn_key)
            .is_some_and(|conn| conn.isolates.contains_key(plugin_id))
        {
            let id = self.dispatch_action(&conn_key, plugin_id, action_id, item_id.clone())?;
            self.pending.insert(
                id,
                Pending::Action {
                    plugin_id: plugin_id.to_string(),
                    action_id: action_id.to_string(),
                    item_id,
                },
            );
            return Some(id);
        }
        if !self.loaded.contains_key(plugin_id) {
            return None;
        }
        self.queued
            .entry(plugin_id.to_string())
            .or_default()
            .push(QueuedCall::Action {
                plugin_id: plugin_id.to_string(),
                action_id: action_id.to_string(),
                item_id,
            });
        self.ensure_loaded(&conn_key, plugin_id);
        Some(0)
    }

    /// Send `form.submit` for a rendered `form` view. Like [`invoke_action`],
    /// a not-yet-loaded plugin is lazy-loaded here; errors surface as toasts.
    pub fn invoke_submit(&mut self, plugin_id: &str, values: &Value) -> Option<u64> {
        let conn_key = self.conn_key_for(plugin_id);
        if !self.conns.contains_key(&conn_key) {
            return None;
        }
        if self
            .conns
            .get(&conn_key)
            .is_some_and(|conn| conn.isolates.contains_key(plugin_id))
        {
            let id = self.dispatch_submit(&conn_key, plugin_id, values)?;
            self.pending.insert(
                id,
                Pending::Submit {
                    plugin_id: plugin_id.to_string(),
                    values: values.clone(),
                },
            );
            return Some(id);
        }
        if !self.loaded.contains_key(plugin_id) {
            return None;
        }
        self.queued
            .entry(plugin_id.to_string())
            .or_default()
            .push(QueuedCall::Submit {
                plugin_id: plugin_id.to_string(),
                values: values.clone(),
            });
        self.ensure_loaded(&conn_key, plugin_id);
        Some(0)
    }

    /// Send `search.query` for a `search` view. Like [`invoke`], a not-yet-loaded
    /// plugin is lazy-loaded here, with `Some(0)` returned as the sentinel for
    /// a queued call; stale generations are dropped by the caller.
    pub fn invoke_search(
        &mut self,
        gen: u64,
        plugin_id: &str,
        command: &str,
        query: &str,
    ) -> Option<u64> {
        let conn_key = self.conn_key_for(plugin_id);
        if !self.conns.contains_key(&conn_key) {
            return None;
        }
        if self
            .conns
            .get(&conn_key)
            .is_some_and(|conn| conn.isolates.contains_key(plugin_id))
        {
            let id = self.dispatch_search(&conn_key, plugin_id, query)?;
            self.pending.insert(
                id,
                Pending::Search {
                    gen,
                    plugin_id: plugin_id.to_string(),
                    command: command.to_string(),
                    query: query.to_string(),
                },
            );
            return Some(id);
        }
        if !self.loaded.contains_key(plugin_id) {
            return None;
        }
        self.queued
            .entry(plugin_id.to_string())
            .or_default()
            .push(QueuedCall::Search {
                gen,
                plugin_id: plugin_id.to_string(),
                command: command.to_string(),
                query: query.to_string(),
            });
        self.ensure_loaded(&conn_key, plugin_id);
        Some(0)
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
        self.loading_plugins.clear();
        self.queued.clear();
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

    /// Send a `plugin.load` for `meta` and remember it as in-flight. Returns
    /// `true` when the request was actually written (so the caller can rely on
    /// a matching `Pending::Load` to drive the queued calls).
    fn load_into(&mut self, conn_key: &str, meta: &PluginMeta) -> bool {
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
            return false;
        };
        if conn.send(&request).is_err() {
            eprintln!(
                "[plugin-host] failed to send plugin.load for '{}'",
                meta.manifest.id
            );
            return false;
        }
        self.loading_plugins.insert(meta.manifest.id.clone());
        self.pending.insert(
            id,
            Pending::Load {
                plugin_id: meta.manifest.id.clone(),
            },
        );
        true
    }

    /// Materialize a plugin's isolate in `conn_key`, unless it is already
    /// loaded or a load is already in flight. This is the lazy-load entry
    /// point: no JS is evaluated at `set_plugins`; the first `invoke`/`item`
    /// for a plugin triggers `plugin.load`. Returns `true` when the plugin is
    /// (or is about to be) loaded.
    fn ensure_loaded(&mut self, conn_key: &str, plugin_id: &str) -> bool {
        if self.loading_plugins.contains(plugin_id) {
            return true;
        }
        if self
            .conns
            .get(conn_key)
            .is_some_and(|conn| conn.isolates.contains_key(plugin_id))
        {
            return true;
        }
        let Some(meta) = self.loaded.get(plugin_id).map(|(_, meta)| meta.clone()) else {
            return false;
        };
        self.load_into(conn_key, &meta)
    }

    /// Send a `command.invoke` for a hit whose isolate is already loaded,
    /// returning the new request id.
    fn dispatch_command(&mut self, conn_key: &str, hit: &RouteHit) -> Option<u64> {
        let has_history = self.plugin_has_history(&hit.plugin_id);
        let conn = self.conns.get_mut(conn_key)?;
        let isolate_id = *conn.isolates.get(&hit.plugin_id)?;
        let id = self.next_request_id;
        self.next_request_id += 1;
        let mut params = json!({
            "isolate_id": isolate_id,
            "command": hit.command,
            "input": hit.input,
            "deadline_ms": hit.deadline_ms,
        });
        if has_history {
            params["clipboard_history"] =
                serde_json::to_value(&self.clipboard_history).unwrap_or(Value::Null);
        }
        let request = Request::new(id, method::COMMAND_INVOKE, params);
        conn.send(&request).ok()?;
        Some(id)
    }

    /// Send an `item.invoke` for a loaded isolate, returning the new request id.
    fn dispatch_item(&mut self, conn_key: &str, plugin_id: &str, item_id: &str) -> Option<u64> {
        let conn = self.conns.get_mut(conn_key)?;
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
        conn.send(&request).ok()?;
        Some(id)
    }

    /// Send an `action.invoke` for a loaded isolate.
    fn dispatch_action(
        &mut self,
        conn_key: &str,
        plugin_id: &str,
        action_id: &str,
        item_id: Option<String>,
    ) -> Option<u64> {
        let conn = self.conns.get_mut(conn_key)?;
        let isolate_id = *conn.isolates.get(plugin_id)?;
        let id = self.next_request_id;
        self.next_request_id += 1;
        let mut params = json!({
            "isolate_id": isolate_id,
            "action_id": action_id,
            "deadline_ms": crate::route::STATIC_DEADLINE_MS,
        });
        if let Some(item_id) = item_id {
            params["item_id"] = json!(item_id);
        }
        let request = Request::new(id, method::ACTION_INVOKE, params);
        conn.send(&request).ok()?;
        Some(id)
    }

    /// Send a `form.submit` for a loaded isolate.
    fn dispatch_submit(&mut self, conn_key: &str, plugin_id: &str, values: &Value) -> Option<u64> {
        let conn = self.conns.get_mut(conn_key)?;
        let isolate_id = *conn.isolates.get(plugin_id)?;
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request = Request::new(
            id,
            method::FORM_SUBMIT,
            json!({
                "isolate_id": isolate_id,
                "values": values,
                "deadline_ms": crate::route::STATIC_DEADLINE_MS,
            }),
        );
        conn.send(&request).ok()?;
        Some(id)
    }

    /// Send a `search.query` for a loaded isolate.
    fn dispatch_search(&mut self, conn_key: &str, plugin_id: &str, query: &str) -> Option<u64> {
        let conn = self.conns.get_mut(conn_key)?;
        let isolate_id = *conn.isolates.get(plugin_id)?;
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request = Request::new(
            id,
            method::SEARCH_QUERY,
            json!({
                "isolate_id": isolate_id,
                "query": query,
                "deadline_ms": crate::route::STATIC_DEADLINE_MS,
            }),
        );
        conn.send(&request).ok()?;
        Some(id)
    }

    /// Whether the plugin's manifest grants the `clipboard.history` permission.
    fn plugin_has_history(&self, plugin_id: &str) -> bool {
        self.loaded.get(plugin_id).is_some_and(|(_, meta)| {
            meta.manifest
                .permissions
                .contains(&Permission::ClipboardHistory)
        })
    }

    /// Replay a plugin's queued calls now that its isolate finished loading.
    fn flush_queued(&mut self, conn_key: &str, plugin_id: &str) {
        let Some(calls) = self.queued.remove(plugin_id) else {
            return;
        };
        for call in calls {
            match call {
                QueuedCall::Command { gen, hit } => {
                    let id = self.dispatch_command(conn_key, &hit);
                    if let Some(id) = id {
                        self.pending.insert(id, Pending::Invoke { gen, hit });
                    }
                }
                QueuedCall::Item {
                    plugin_id: pid,
                    command,
                    item_id,
                } => {
                    let id = self.dispatch_item(conn_key, &pid, &item_id);
                    if let Some(id) = id {
                        self.pending.insert(
                            id,
                            Pending::Item {
                                plugin_id: pid,
                                command,
                                item_id,
                            },
                        );
                    }
                }
                QueuedCall::Action {
                    plugin_id: pid,
                    action_id,
                    item_id,
                } => {
                    let id = self.dispatch_action(conn_key, &pid, &action_id, item_id.clone());
                    if let Some(id) = id {
                        self.pending.insert(
                            id,
                            Pending::Action {
                                plugin_id: pid,
                                action_id,
                                item_id,
                            },
                        );
                    }
                }
                QueuedCall::Submit {
                    plugin_id: pid,
                    values,
                } => {
                    let id = self.dispatch_submit(conn_key, &pid, &values);
                    if let Some(id) = id {
                        self.pending.insert(
                            id,
                            Pending::Submit {
                                plugin_id: pid,
                                values,
                            },
                        );
                    }
                }
                QueuedCall::Search {
                    gen,
                    plugin_id: pid,
                    command,
                    query,
                } => {
                    let id = self.dispatch_search(conn_key, &pid, &query);
                    if let Some(id) = id {
                        self.pending.insert(
                            id,
                            Pending::Search {
                                gen,
                                plugin_id: pid,
                                command,
                                query,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Handle a `PLUGIN_NOT_FOUND` response: the isolate was LRU-evicted or
    /// killed by timeout/heap since we last dispatched to it. Drop the stale
    /// isolate id, reload the plugin once and replay the call (guarded so a
    /// plugin that is already loading never recurses). If the plugin is no
    /// longer in the set, surface the original error instead.
    fn handle_stale_isolate(
        &mut self,
        conn_key: &str,
        call: QueuedCall,
        events: &mut Vec<HostEvent>,
    ) {
        let plugin_id = match &call {
            QueuedCall::Command { hit, .. } => hit.plugin_id.clone(),
            QueuedCall::Item { plugin_id, .. } => plugin_id.clone(),
            QueuedCall::Action { plugin_id, .. } => plugin_id.clone(),
            QueuedCall::Submit { plugin_id, .. } => plugin_id.clone(),
            QueuedCall::Search { plugin_id, .. } => plugin_id.clone(),
        };
        if let Some(conn) = self.conns.get_mut(conn_key) {
            conn.isolates.remove(&plugin_id);
        }
        if self.loading_plugins.contains(&plugin_id) {
            // An in-flight load will flush this call; keep it queued.
            self.queued.entry(plugin_id).or_default().push(call);
            return;
        }
        if !self.loaded.contains_key(&plugin_id) {
            match call {
                QueuedCall::Command { gen, hit } => events.push(HostEvent::CommandResult {
                    gen,
                    plugin_id: hit.plugin_id,
                    command: hit.command,
                    result: Err(RpcError::new(
                        code::PLUGIN_NOT_FOUND,
                        "plugin isolate is not loaded",
                    )),
                }),
                QueuedCall::Item {
                    plugin_id,
                    item_id,
                    ..
                } => events.push(HostEvent::Toast {
                    params: json!({
                        "message": format!("{plugin_id} ({item_id}): plugin isolate is not loaded"),
                        "kind": "error",
                    }),
                }),
                QueuedCall::Action {
                    plugin_id,
                    action_id,
                    ..
                } => events.push(HostEvent::Toast {
                    params: json!({
                        "message": format!("{plugin_id} ({action_id}): plugin isolate is not loaded"),
                        "kind": "error",
                    }),
                }),
                QueuedCall::Submit { plugin_id, .. } => events.push(HostEvent::Toast {
                    params: json!({
                        "message": format!("{plugin_id}: plugin isolate is not loaded"),
                        "kind": "error",
                    }),
                }),
                QueuedCall::Search {
                    gen,
                    plugin_id,
                    command,
                    query,
                } => events.push(HostEvent::SearchResult {
                    gen,
                    plugin_id,
                    command,
                    query,
                    result: Err(RpcError::new(
                        code::PLUGIN_NOT_FOUND,
                        "plugin isolate is not loaded",
                    )),
                }),
            }
            return;
        }
        self.ensure_loaded(conn_key, &plugin_id);
        self.queued.entry(plugin_id).or_default().push(call);
    }

    /// Drop all in-flight bookkeeping for a crashed connection: its plugins'
    /// loading flags, queued calls and any pending requests addressed to it.
    /// The plugin set in `loaded` is untouched (it drives the next lazy load).
    fn forget_conn_state(&mut self, key: &str) {
        let affected: HashSet<String> = if key == SHARED_POOL_KEY {
            self.loaded
                .iter()
                .filter(|(_, (isolation, _))| *isolation == Isolation::SharedPool)
                .map(|(id, _)| id.clone())
                .collect()
        } else {
            [key.to_string()].into_iter().collect()
        };
        self.loading_plugins.retain(|id| !affected.contains(id));
        self.queued.retain(|id, _| !affected.contains(id));
        self.pending.retain(|_, pending| {
            let plugin_id = match pending {
                Pending::Load { plugin_id }
                | Pending::Item { plugin_id, .. }
                | Pending::Action { plugin_id, .. }
                | Pending::Submit { plugin_id, .. }
                | Pending::Search { plugin_id, .. } => plugin_id,
                Pending::Invoke { hit, .. } => &hit.plugin_id,
            };
            !affected.contains(plugin_id)
        });
    }

    fn handle_response(&mut self, conn_key: &str, response: Response, events: &mut Vec<HostEvent>) {
        let Some(pending) = self.pending.remove(&response.id) else {
            return;
        };
        match pending {
            Pending::Load { plugin_id } => {
                self.loading_plugins.remove(&plugin_id);
                if let Some(isolate_id) = response
                    .result
                    .as_ref()
                    .and_then(|r| r["isolate_id"].as_u64())
                {
                    if let Some(conn) = self.conns.get_mut(conn_key) {
                        conn.isolates.insert(plugin_id.clone(), isolate_id);
                    }
                    self.flush_queued(conn_key, &plugin_id);
                } else if let Some(error) = response.error {
                    eprintln!(
                        "[plugin-host] failed to load plugin {plugin_id}: {} ({})",
                        error.message, error.code
                    );
                    // Loading failed: replay the queued calls as errors instead
                    // of leaving them waiting forever for an isolate that will
                    // not appear.
                    self.fail_queued(&plugin_id, error, events);
                }
            }
            Pending::Invoke { gen, hit } => {
                if let Some(error) = &response.error {
                    if error.code == code::PLUGIN_NOT_FOUND {
                        self.handle_stale_isolate(
                            conn_key,
                            QueuedCall::Command { gen, hit },
                            events,
                        );
                        return;
                    }
                }
                let result = match (response.result, response.error) {
                    (Some(result), None) => Ok(result),
                    (_, Some(error)) => Err(error),
                    _ => Err(RpcError::new(code::INTERNAL_ERROR, "empty response")),
                };
                events.push(HostEvent::CommandResult {
                    gen,
                    plugin_id: hit.plugin_id,
                    command: hit.command,
                    result,
                });
            }
            Pending::Item {
                plugin_id,
                command,
                item_id,
            } => {
                if let Some(error) = &response.error {
                    if error.code == code::PLUGIN_NOT_FOUND {
                        self.handle_stale_isolate(
                            conn_key,
                            QueuedCall::Item {
                                plugin_id: plugin_id.clone(),
                                command,
                                item_id: item_id.clone(),
                            },
                            events,
                        );
                        return;
                    }
                }
                if let Some(error) = response.error {
                    events.push(HostEvent::Toast {
                        params: json!({
                            "message": format!("{plugin_id} ({item_id}): {}", error.message),
                            "kind": "error",
                        }),
                    });
                } else if let Some(view) = response
                    .result
                    .as_ref()
                    .and_then(|result| result.get("view"))
                    .cloned()
                {
                    // The plugin's select returned a drill-down view (e.g. a
                    // `detail`): surface it so the launcher replaces the
                    // command's view slot.
                    events.push(HostEvent::ItemView {
                        plugin_id,
                        command,
                        item_id,
                        view,
                    });
                }
            }
            Pending::Action {
                plugin_id,
                action_id,
                item_id,
            } => {
                if let Some(error) = &response.error {
                    if error.code == code::PLUGIN_NOT_FOUND {
                        self.handle_stale_isolate(
                            conn_key,
                            QueuedCall::Action {
                                plugin_id: plugin_id.clone(),
                                action_id,
                                item_id,
                            },
                            events,
                        );
                        return;
                    }
                }
                if let Some(error) = response.error {
                    events.push(HostEvent::Toast {
                        params: json!({
                            "message": format!("{plugin_id} ({action_id}): {}", error.message),
                            "kind": "error",
                        }),
                    });
                }
            }
            Pending::Submit { plugin_id, values } => {
                if let Some(error) = &response.error {
                    if error.code == code::PLUGIN_NOT_FOUND {
                        self.handle_stale_isolate(
                            conn_key,
                            QueuedCall::Submit {
                                plugin_id: plugin_id.clone(),
                                values,
                            },
                            events,
                        );
                        return;
                    }
                }
                if let Some(error) = response.error {
                    events.push(HostEvent::Toast {
                        params: json!({
                            "message": format!("{plugin_id}: {}", error.message),
                            "kind": "error",
                        }),
                    });
                }
            }
            Pending::Search {
                gen,
                plugin_id,
                command,
                query,
            } => {
                if let Some(error) = &response.error {
                    if error.code == code::PLUGIN_NOT_FOUND {
                        self.handle_stale_isolate(
                            conn_key,
                            QueuedCall::Search {
                                gen,
                                plugin_id: plugin_id.clone(),
                                command: command.clone(),
                                query: query.clone(),
                            },
                            events,
                        );
                        return;
                    }
                }
                let result = match (response.result, response.error) {
                    (Some(result), None) => Ok(result),
                    (_, Some(error)) => Err(error),
                    _ => Err(RpcError::new(code::INTERNAL_ERROR, "empty response")),
                };
                events.push(HostEvent::SearchResult {
                    gen,
                    plugin_id,
                    command,
                    query,
                    result,
                });
            }
        }
    }

    /// Replay queued calls for `plugin_id` with an error, used when the
    /// `plugin.load` request itself failed (so no isolate will ever arrive).
    fn fail_queued(&mut self, plugin_id: &str, error: RpcError, events: &mut Vec<HostEvent>) {
        let Some(calls) = self.queued.remove(plugin_id) else {
            return;
        };
        for call in calls {
            match call {
                QueuedCall::Command { gen, hit } => events.push(HostEvent::CommandResult {
                    gen,
                    plugin_id: hit.plugin_id,
                    command: hit.command,
                    result: Err(error.clone()),
                }),
                QueuedCall::Item {
                    plugin_id, item_id, ..
                } => events.push(HostEvent::Toast {
                    params: json!({
                        "message": format!("{plugin_id} ({item_id}): {}", error.message),
                        "kind": "error",
                    }),
                }),
                QueuedCall::Action {
                    plugin_id,
                    action_id,
                    ..
                } => events.push(HostEvent::Toast {
                    params: json!({
                        "message": format!("{plugin_id} ({action_id}): {}", error.message),
                        "kind": "error",
                    }),
                }),
                QueuedCall::Submit { plugin_id, .. } => events.push(HostEvent::Toast {
                    params: json!({
                        "message": format!("{plugin_id}: {}", error.message),
                        "kind": "error",
                    }),
                }),
                QueuedCall::Search {
                    gen,
                    plugin_id,
                    command,
                    query,
                } => events.push(HostEvent::SearchResult {
                    gen,
                    plugin_id,
                    command,
                    query,
                    result: Err(error.clone()),
                }),
            }
        }
    }

    fn handle_eof(&mut self, key: &str, events: &mut Vec<HostEvent>) {
        if let Some(mut conn) = self.conns.remove(key) {
            let _ = conn.child.kill();
            let _ = conn.child.wait();
            // The isolate ids and any in-flight loads died with the process, so
            // forget them; the next invoke lazily reloads into the fresh process.
            self.forget_conn_state(key);
            let plugin_id = (key != SHARED_POOL_KEY).then(|| key.to_string());
            events.push(HostEvent::RuntimeCrashed {
                plugin_id: plugin_id.clone(),
            });
            let attempts = self.restarts.get(key).map_or(0, |state| state.attempts) + 1;
            let delay = backoff(attempts, self.config.base_backoff, self.config.max_backoff);
            self.restarts.insert(
                key.to_string(),
                RestartState {
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

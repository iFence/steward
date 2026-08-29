//! In-process isolate pool for regular (shared-pool) plugins.
//!
//! Every pool slot owns an independent QuickJS runtime with a hard heap limit
//! and a deadline-driven interrupt handler. When a plugin stalls past its
//! execution deadline or blows past its heap limit, the whole isolate is
//! dropped (killed) and recreated on demand, so one misbehaving plugin can
//! never poison a healthy neighbor. Idle isolates are evicted LRU-style when
//! the pool is full, keeping memory proportional to the *active* plugin count
//! rather than the installed one (the M2 scaling rule).
//!
//! Host functions (`globalThis.steward.*`) are installed per isolate and gated
//! by the plugin's manifest permission whitelist; calling a function the
//! manifest did not grant throws a `permission denied` JavaScript exception,
//! which the service loop maps to JSON-RPC code `-32000`.

use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context as _, Result};
use rquickjs::{Ctx, Exception, Function, Object, Runtime, Value as JsValue};
use serde_json::Value as Json;
use steward_ipc_protocol::{
    code, method, ClipboardEntry, Notification, Request, Response, RpcError,
};
use steward_plugin_registry::{Permission, PluginManifest};

use crate::storage::PluginStorage;

/// Default number of isolates in the shared pool.
pub const DEFAULT_POOL_CAPACITY: usize = 8;
/// Default per-isolate QuickJS heap limit: 64 MB.
pub const DEFAULT_HEAP_LIMIT: usize = 64 * 1024 * 1024;
/// Default QuickJS stack limit: 1 MB (deep recursion is a plugin bug).
pub const DEFAULT_MAX_STACK: usize = 1024 * 1024;

/// Name of the global that holds the plugin module object (the esbuild IIFE
/// `--global-name`).
const PLUGIN_GLOBAL: &str = "__stewardPlugin";

/// Identifies one loaded plugin instance inside the runtime process.
pub type IsolateId = u64;

/// Errors surfaced by pool operations; the service loop maps each variant to
/// a JSON-RPC error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeError {
    /// The isolate id is not loaded (killed after a deadline/heap violation,
    /// unloaded, or never loaded).
    NotFound,
    /// The plugin did not register the requested command.
    CommandNotFound,
    /// The plugin did not answer within its deadline; the isolate was killed.
    Timeout,
    /// The plugin exceeded its heap limit; the isolate was killed.
    Memory,
    /// The plugin called a host function its manifest did not grant.
    PermissionDenied(String),
    /// The plugin's command parked on a cross-process host request (fs /
    /// network). The runtime must send the queued host requests, wait for the
    /// replies, and resume the isolate. Not an error: the service loop handles
    /// it specially.
    Pending,
    /// A JavaScript exception escaped the plugin.
    Plugin(String),
    /// A runtime/plumbing failure.
    Internal(String),
}

/// Loaded state of one plugin instance.
struct PluginHandle {
    commands: Vec<String>,
}

/// One pool slot: an owned QuickJS runtime + context plus the bookkeeping
/// needed for deadlines, LRU eviction and permission gating.
struct Isolate {
    id: IsolateId,
    plugin: PluginHandle,
    context: rquickjs::Context,
    /// Deadline for the currently running invocation; the interrupt handler
    /// reads this cell and returns `true` (interrupt) once it has passed.
    deadline: Rc<Cell<Option<Instant>>>,
    /// Last time this isolate executed anything; the LRU eviction key.
    last_used: Cell<Instant>,
    /// Clipboard-history snapshot injected by the host for this invocation
    /// (read by `clipboard.history()`, gated by `clipboard.history`).
    clipboard_history: Rc<RefCell<Vec<ClipboardEntry>>>,
    /// Cross-process async host-request bookkeeping (fs / network). A command
    /// that awaits one of these parks the isolate until the host replies.
    async_state: Rc<RefCell<AsyncBridgeState>>,
}

/// The shared isolate pool. Not `Send`: QuickJS runtimes are single-threaded
/// and the service loop drives everything from one thread.
pub struct IsolatePool {
    isolates: Vec<Option<Isolate>>,
    next_id: IsolateId,
    capacity: usize,
    heap_limit: usize,
    max_stack: usize,
    /// Dedicated mode hosts exactly one plugin: loading another replaces it.
    dedicated: bool,
    /// Toasts emitted by plugins, drained by the service loop.
    notifications: Rc<RefCell<Vec<Notification>>>,
    /// pending host request id -> isolate id (runtime -> host requests in
    /// flight, waiting for a reply that resumes the parked isolate).
    pending_owner: std::collections::HashMap<u64, IsolateId>,
    /// isolate id -> parked invocation (the runtime sends the host reply only
    /// once the invocation settles, or a timeout if the deadline passes).
    parked: std::collections::HashMap<IsolateId, ParkedInvocation>,
}

impl IsolatePool {
    /// Create a pool with the given limits.
    pub fn new(dedicated: bool, capacity: usize, heap_limit: usize, max_stack: usize) -> Self {
        Self {
            isolates: Vec::with_capacity(capacity.max(1)),
            next_id: 1,
            capacity: capacity.max(1),
            heap_limit,
            max_stack,
            dedicated,
            notifications: Rc::new(RefCell::new(Vec::new())),
            pending_owner: std::collections::HashMap::new(),
            parked: std::collections::HashMap::new(),
        }
    }

    /// Load a plugin bundle into an isolate and return its id.
    pub fn load(&mut self, entry: &Path, manifest: &PluginManifest) -> Result<IsolateId> {
        if self.dedicated {
            // Dedicated process: one plugin at a time; loading replaces it.
            self.isolates.clear();
            self.parked.clear();
            self.pending_owner.clear();
        } else if self.active_count() >= self.capacity {
            self.evict_lru();
        }

        let id = self.next_id;
        self.next_id += 1;

        let runtime = Runtime::new().context("create QuickJS runtime")?;
        runtime.set_memory_limit(self.heap_limit);
        runtime.set_max_stack_size(self.max_stack);
        let deadline = Rc::new(Cell::new(None));
        {
            let deadline = deadline.clone();
            runtime.set_interrupt_handler(Some(Box::new(move || {
                deadline.get().is_some_and(|at| Instant::now() >= at)
            })));
        }
        let context = rquickjs::Context::full(&runtime).context("create QuickJS context")?;

        let permissions = manifest.permissions.clone();
        let notifications = self.notifications.clone();
        let clipboard_history = Rc::new(RefCell::new(Vec::new()));
        let storage = Rc::new(RefCell::new(PluginStorage::load(&manifest.id)?));
        let async_state = Rc::new(RefCell::new(AsyncBridgeState::default()));
        context
            .with(|ctx| -> anyhow::Result<()> {
                install_host_bridge(
                    &ctx,
                    &permissions,
                    &notifications,
                    &clipboard_history,
                    &storage,
                    &async_state,
                )?;
                ctx.globals().set(
                    "__stewardHost",
                    serde_json::to_string(&node_polyfill_host_info())
                        .unwrap_or_else(|_| "{}".into()),
                )?;
                ctx.eval::<(), _>(include_str!("node_polyfill.js"))
                    .map_err(|error| anyhow!("failed to evaluate node polyfill: {error}"))?;
                ctx.eval_file::<JsValue, _>(entry)
                    .map_err(|error| anyhow!("failed to evaluate {}: {error}", entry.display()))?;
                verify_module(&ctx).map_err(|error| {
                    anyhow!(
                        "plugin '{}' bundle does not expose a valid module: {error}",
                        manifest.id
                    )
                })?;
                Ok(())
            })
            .map_err(|error| anyhow!("failed to initialize plugin '{}': {error}", manifest.id))?;

        let commands = manifest.commands.iter().map(|c| c.name.clone()).collect();
        self.isolates.push(Some(Isolate {
            id,
            plugin: PluginHandle { commands },
            context,
            deadline,
            last_used: Cell::new(Instant::now()),
            clipboard_history,
            async_state,
        }));
        Ok(id)
    }

    /// Run one plugin command and return the serialized view it produced.
    pub fn invoke_command(
        &mut self,
        id: IsolateId,
        command: &str,
        input: &Json,
        deadline_ms: u64,
    ) -> Result<Json, InvokeError> {
        self.invoke_command_with_history(id, command, input, deadline_ms, None)
    }

    /// Like [`invoke_command`], but injects a host-provided clipboard-hist
    /// snapshot into the isolate before it runs, so `clipboard.history()` can
    /// read the entries for this invocation. `None` leaves the previous
    /// snapshot (or an empty one) unchanged.
    pub fn invoke_command_with_history(
        &mut self,
        id: IsolateId,
        command: &str,
        input: &Json,
        deadline_ms: u64,
        clipboard_history: Option<Vec<ClipboardEntry>>,
    ) -> Result<Json, InvokeError> {
        {
            let isolate = self.isolate_mut(id).ok_or(InvokeError::NotFound)?;
            if !isolate.plugin.commands.iter().any(|c| c == command) {
                return Err(InvokeError::CommandNotFound);
            }
            if let Some(history) = clipboard_history {
                *isolate.clipboard_history.borrow_mut() = history;
            }
        }
        let input_text = match input {
            Json::String(text) => text.clone(),
            other => serde_json::to_string(other)
                .map_err(|e| InvokeError::Internal(format!("cannot encode input: {e}")))?,
        };
        self.run_in_isolate(id, deadline_ms, |ctx| {
            let module: Object = ctx.globals().get(PLUGIN_GLOBAL)?;
            let command_fn: Function = module.get("command")?;
            let result: JsValue = command_fn.call((command, input_text))?;
            let result = await_view(&ctx, result)?;
            js_value_to_json(&ctx, &result)
        })
    }

    /// Invoke a view-level action from an `ActionPanel`. Calls the plugin's
    /// exported `run(actionId, itemId?)`; a missing export is a successful
    /// no-op (the plugin has no actions to handle).
    pub fn invoke_action(
        &mut self,
        id: IsolateId,
        action_id: &str,
        item_id: Option<String>,
        deadline_ms: u64,
    ) -> Result<(), InvokeError> {
        let action_id = action_id.to_string();
        let item_id = item_id.clone();
        self.run_in_isolate(id, deadline_ms, |ctx| {
            let module: Object = ctx.globals().get(PLUGIN_GLOBAL)?;
            let run: JsValue = module.get("run")?;
            if !run.is_function() {
                return Ok(());
            }
            let run_fn: Function = run.into_function().ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "globalThis.__stewardPlugin.run",
                    "function",
                    "expected a callable run(actionId, itemId)",
                )
            })?;
            if let Some(item_id) = item_id {
                let result: JsValue = run_fn.call((action_id, item_id))?;
                await_view(&ctx, result)?;
            } else {
                let result: JsValue = run_fn.call((action_id,))?;
                await_view(&ctx, result)?;
            }
            Ok(())
        })
    }

    /// Submit a rendered `form` view. Calls the plugin's exported
    /// `submit(values)` (the `values` object is decoded from JSON inside the
    /// isolate); a missing export is a successful no-op.
    pub fn invoke_submit(
        &mut self,
        id: IsolateId,
        values: &Json,
        deadline_ms: u64,
    ) -> Result<(), InvokeError> {
        let values_text = serde_json::to_string(values)
            .map_err(|e| InvokeError::Internal(format!("cannot encode form values: {e}")))?;
        self.run_in_isolate(id, deadline_ms, |ctx| {
            let module: Object = ctx.globals().get(PLUGIN_GLOBAL)?;
            let submit: JsValue = module.get("submit")?;
            if !submit.is_function() {
                return Ok(());
            }
            let submit_fn: Function = submit.into_function().ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "globalThis.__stewardPlugin.submit",
                    "function",
                    "expected a callable submit(values)",
                )
            })?;
            // Decode the values object inside the isolate (JSON.parse) so the
            // plugin receives a real JS object, not a string.
            let json: Object = ctx.globals().get("JSON")?;
            let parse: Function = json.get("parse")?;
            let js_values: JsValue = parse.call((values_text,))?;
            let result: JsValue = submit_fn.call((js_values,))?;
            await_view(&ctx, result)?;
            Ok(())
        })
    }

    /// Invoke a rendered list item's `select` handler (if the plugin exports
    /// one). A missing handler is a no-op success. The handler may return a
    /// new view (e.g. a `detail` drill-down), which is returned as the serialized
    /// JSON view; `None` means the plugin did not return a view.
    pub fn invoke_item(
        &mut self,
        id: IsolateId,
        item_id: &str,
        deadline_ms: u64,
    ) -> Result<Option<Json>, InvokeError> {
        let item_id = item_id.to_string();
        self.run_in_isolate(id, deadline_ms, |ctx| {
            let module: Object = ctx.globals().get(PLUGIN_GLOBAL)?;
            let select: JsValue = module.get("select")?;
            if !select.is_function() {
                return Ok(None);
            }
            let select_fn: Function = select.into_function().ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "globalThis.__stewardPlugin.select",
                    "function",
                    "expected a callable select(itemId)",
                )
            })?;
            let result: JsValue = select_fn.call((item_id,))?;
            let result = await_view(&ctx, result)?;
            let json = js_value_to_json(&ctx, &result)?;
            if json.is_null() {
                Ok(None)
            } else {
                Ok(Some(json))
            }
        })
    }

    /// Stream a `search` view's results. Calls the plugin's exported
    /// `search(query)` (awaited when it returns a Promise) and returns the
    /// serialized view the host renders in the search view's results area.
    /// `Ok(None)` when the plugin does not export a `search` handler (the host
    /// renders an empty results area).
    pub fn invoke_search(
        &mut self,
        id: IsolateId,
        query: &str,
        deadline_ms: u64,
    ) -> Result<Option<Json>, InvokeError> {
        let query = query.to_string();
        self.run_in_isolate(id, deadline_ms, |ctx| {
            let module: Object = ctx.globals().get(PLUGIN_GLOBAL)?;
            let search: JsValue = module.get("search")?;
            if !search.is_function() {
                return Ok(None);
            }
            let search_fn: Function = search.into_function().ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "globalThis.__stewardPlugin.search",
                    "function",
                    "expected a callable search(query)",
                )
            })?;
            let result: JsValue = search_fn.call((query,))?;
            let result = await_view(&ctx, result)?;
            let json = js_value_to_json(&ctx, &result)?;
            if json.is_null() {
                Ok(None)
            } else {
                Ok(Some(json))
            }
        })
    }

    /// Drop a plugin's isolate, freeing its QuickJS runtime immediately.
    pub fn unload(&mut self, id: IsolateId) {
        self.drop_isolate(id);
    }

    /// Number of isolates currently loaded.
    pub fn active_count(&self) -> usize {
        self.isolates.iter().filter(|slot| slot.is_some()).count()
    }

    /// Drain toast notifications emitted by plugins since the last drain.
    pub fn drain_notifications(&self) -> Vec<Notification> {
        std::mem::take(&mut *self.notifications.borrow_mut())
    }

    /// Take the queued runtime -> host requests for `id` and mark them in
    /// flight. Called by the service loop after an invocation parks, so the
    /// requests are sent to the host and later matched back to `id` by their
    /// request id.
    pub fn drain_outbound(&mut self, id: IsolateId) -> Vec<PendingHostRequest> {
        let requests = {
            let Some(isolate) = self.isolate_mut(id) else {
                return Vec::new();
            };
            let mut state = isolate.async_state.borrow_mut();
            let requests = std::mem::take(&mut state.outbound);
            state.in_flight += requests.len();
            requests
        };
        for request in &requests {
            self.pending_owner.insert(request.id, id);
        }
        requests
    }

    /// Record that `id` is parked on the host request `request`; the runtime
    /// will reply to it only when the invocation finally settles.
    pub fn park_invocation(&mut self, id: IsolateId, request: Request) {
        let deadline = Instant::now() + Duration::from_millis(request_deadline_ms(&request));
        self.parked
            .insert(id, ParkedInvocation { request, deadline });
    }

    /// Whether `id` is currently parked on a cross-process host request. A new
    /// host request addressed to a parked isolate cannot re-enter its JS (it is
    /// suspended mid-promise), so the caller rejects it as busy.
    pub fn is_parked(&self, id: IsolateId) -> bool {
        self.parked.contains_key(&id)
    }

    /// Kill any parked invocation whose deadline passed and return a `TIMEOUT`
    /// response to answer its original host request. Called by the service loop
    /// on each tick so a host that never replies cannot hang a plugin forever.
    pub fn expire_parked(&mut self) -> Vec<Response> {
        let now = Instant::now();
        let expired: Vec<IsolateId> = self
            .parked
            .iter()
            .filter(|(_, invocation)| invocation.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        let mut responses = Vec::new();
        for id in expired {
            let request = self.parked.remove(&id).map(|invocation| invocation.request);
            self.drop_isolate(id);
            if let Some(request) = request {
                responses.push(Response::error(
                    request.id,
                    RpcError::new(
                        code::TIMEOUT,
                        "plugin did not respond within its deadline; isolate killed",
                    ),
                ));
            }
        }
        responses
    }

    /// Handle a reply from the host to a runtime -> host request (e.g. the
    /// result of `host.fs.read`). Resolves the plugin's pending promise and
    /// resumes the parked isolate; returns how the service loop should proceed.
    pub fn handle_host_response(&mut self, response: Response) -> ResumeOutcome {
        let pending_id = response.id;
        let Some(isolate_id) = self.pending_owner.remove(&pending_id) else {
            return ResumeOutcome::Dropped;
        };
        let Some(parked) = self.parked.get(&isolate_id).cloned() else {
            return ResumeOutcome::Dropped;
        };
        // Drop the in-flight marker for this host request.
        {
            let Some(isolate) = self.isolate_mut(isolate_id) else {
                return ResumeOutcome::Dropped;
            };
            let mut state = isolate.async_state.borrow_mut();
            state.in_flight = state.in_flight.saturating_sub(1);
        }
        let host_result: Result<Json, String> = match (response.result, response.error) {
            (Some(value), _) => Ok(value),
            (None, Some(error)) => Err(error.message),
            (None, None) => Err("empty host response".into()),
        };
        let resume = self.isolate(isolate_id).map(|isolate| {
            isolate
                .context
                .with(|ctx| resolve_and_drain(&ctx, pending_id, host_result.as_ref()))
        });
        let Some(resume) = resume else {
            return ResumeOutcome::Dropped;
        };
        match resume {
            Ok(view) => {
                self.parked.remove(&isolate_id);
                ResumeOutcome::Reply(build_response(parked.request, view))
            }
            Err(rquickjs::Error::WouldBlock) => {
                // Still parked: the continuation may have queued more host
                // requests (or is waiting on one already in flight). The
                // service loop drains `drain_outbound` again.
                ResumeOutcome::Parked(isolate_id)
            }
            Err(rquickjs::Error::Exception) => {
                let message = self
                    .exception_message(isolate_id)
                    .unwrap_or_else(|| "JavaScript exception".into());
                self.drop_isolate(isolate_id);
                self.parked.remove(&isolate_id);
                let err_code = if message.contains("permission denied") {
                    code::PERMISSION_DENIED
                } else {
                    code::INTERNAL_ERROR
                };
                ResumeOutcome::Reply(Response::error(
                    parked.request.id,
                    RpcError::new(err_code, message),
                ))
            }
            Err(rquickjs::Error::Allocation) => {
                self.drop_isolate(isolate_id);
                self.parked.remove(&isolate_id);
                ResumeOutcome::Reply(Response::error(
                    parked.request.id,
                    RpcError::new(
                        code::INTERNAL_ERROR,
                        "plugin exceeded its heap limit; isolate killed",
                    ),
                ))
            }
            Err(error) => {
                self.drop_isolate(isolate_id);
                self.parked.remove(&isolate_id);
                ResumeOutcome::Reply(Response::error(
                    parked.request.id,
                    RpcError::new(
                        code::INTERNAL_ERROR,
                        format!("plugin resume failed: {error}"),
                    ),
                ))
            }
        }
    }

    /// Whether `id` has a cross-process host request in flight or queued (so a
    /// `WouldBlock` is a legitimate park, not a genuine infinite loop).
    fn isolate_is_async_parked(&self, id: IsolateId) -> bool {
        self.isolate(id).is_some_and(|isolate| {
            let state = isolate.async_state.borrow();
            state.in_flight > 0 || !state.outbound.is_empty()
        })
    }

    /// Run `f` inside the isolate's context under `deadline_ms`, killing the
    /// isolate on deadline or heap-limit violations.
    fn run_in_isolate<T>(
        &mut self,
        id: IsolateId,
        deadline_ms: u64,
        f: impl for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<T>,
    ) -> Result<T, InvokeError> {
        let (result, timed_out) = {
            let isolate = self.isolate_mut(id).ok_or(InvokeError::NotFound)?;
            isolate.last_used.set(Instant::now());
            let deadline = Instant::now() + Duration::from_millis(deadline_ms.max(1));
            isolate.deadline.set(Some(deadline));
            let result = isolate.context.with(f);
            let timed_out = isolate
                .deadline
                .get()
                .is_some_and(|at| Instant::now() >= at);
            isolate.deadline.set(None);
            (result, timed_out)
        };
        if timed_out {
            self.drop_isolate(id);
            return Err(InvokeError::Timeout);
        }
        match result {
            Ok(value) => Ok(value),
            Err(rquickjs::Error::Allocation) => {
                self.drop_isolate(id);
                Err(InvokeError::Memory)
            }
            Err(rquickjs::Error::Exception) => {
                let message = self
                    .exception_message(id)
                    .unwrap_or_else(|| "JavaScript exception".into());
                if message.contains("out of memory") || message.contains("memory limit") {
                    self.drop_isolate(id);
                    return Err(InvokeError::Memory);
                }
                if message.contains("permission denied") {
                    Err(InvokeError::PermissionDenied(message))
                } else {
                    Err(InvokeError::Plugin(message))
                }
            }
            Err(rquickjs::Error::WouldBlock) => {
                // A plugin handler returned a Promise that never settles. If a
                // cross-process host request is in flight (fs / network), park
                // the isolate so the service loop can wait for the host reply
                // and resume it; otherwise it is genuinely stuck (no event will
                // ever arrive) and the isolate is recycled as a timeout.
                if self.isolate_is_async_parked(id) {
                    Err(InvokeError::Pending)
                } else {
                    self.drop_isolate(id);
                    Err(InvokeError::Timeout)
                }
            }
            Err(error) => Err(InvokeError::Internal(error.to_string())),
        }
    }

    /// The pending exception's message, read from the context after an
    /// `Error::Exception`. Returns `None` when the value is not an Error.
    fn exception_message(&self, id: IsolateId) -> Option<String> {
        let isolate = self.isolate(id)?;
        isolate.context.with(|ctx| {
            let value = ctx.catch();
            if let Some(object) = value.as_object() {
                if let Some(exception) = Exception::from_object(object.clone()) {
                    return exception.message();
                }
            }
            value.as_string().and_then(|text| text.to_string().ok())
        })
    }

    fn isolate(&self, id: IsolateId) -> Option<&Isolate> {
        self.slot(id)
            .and_then(|index| self.isolates[index].as_ref())
    }

    fn isolate_mut(&mut self, id: IsolateId) -> Option<&mut Isolate> {
        self.slot(id)
            .and_then(|index| self.isolates[index].as_mut())
    }

    fn slot(&self, id: IsolateId) -> Option<usize> {
        self.isolates
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|isolate| isolate.id == id))
    }

    fn drop_isolate(&mut self, id: IsolateId) {
        self.cleanup_isolate_bookkeeping(id);
        if let Some(index) = self.slot(id) {
            self.isolates[index] = None;
        }
    }

    /// Drop parked / in-flight bookkeeping for an isolate that is being
    /// killed or evicted, so a late host reply is dropped instead of resuming a
    /// dead isolate.
    fn cleanup_isolate_bookkeeping(&mut self, id: IsolateId) {
        self.parked.remove(&id);
        self.pending_owner.retain(|_, owner| *owner != id);
    }

    /// Drop the least-recently-used idle isolate to make room for a new load.
    fn evict_lru(&mut self) {
        let mut lru = None;
        let mut lru_index = None;
        for (index, slot) in self.isolates.iter().enumerate() {
            if let Some(isolate) = slot {
                if lru.is_none_or(|at| isolate.last_used.get() < at) {
                    lru = Some(isolate.last_used.get());
                    lru_index = Some(index);
                }
            }
        }
        if let Some(index) = lru_index {
            let id = self.isolates[index].as_ref().map(|isolate| isolate.id);
            self.isolates[index] = None;
            if let Some(id) = id {
                self.cleanup_isolate_bookkeeping(id);
            }
        }
    }
}

/// A runtime-initiated request to the host (e.g. `host.fs.read`), parked the
/// plugin's promise until the host replies. `id` is the request id the reply
/// will carry so the runtime can resume the parked isolate.
pub type PendingHostRequest = Request;

/// Per-isolate bookkeeping for cross-process async host requests.
#[derive(Default)]
struct AsyncBridgeState {
    /// Host requests queued by the plugin (in `__hostSend`) but not yet sent.
    /// Drained by the service loop after an invocation parks.
    outbound: Vec<PendingHostRequest>,
    /// Number of requests already sent to the host but not yet answered.
    in_flight: usize,
}

/// A parked invocation: the isolate suspended on a cross-process host request
/// and the original host request it must answer once it settles.
#[derive(Debug, Clone)]
struct ParkedInvocation {
    /// The original host request (command/item/search/action/submit) the
    /// runtime must reply to once the invocation settles.
    request: Request,
    /// Absolute deadline; the isolate is killed and the request answered with
    /// `TIMEOUT` if the host has not replied by then.
    deadline: Instant,
}

/// How the service loop should proceed after handling a host reply to a
/// runtime -> host request.
#[derive(Debug)]
pub enum ResumeOutcome {
    /// The parked invocation settled; write `Response` back to the host.
    Reply(Response),
    /// Still parked; drain `drain_outbound(isolate_id)` to send any newly
    /// queued host requests.
    Parked(IsolateId),
    /// The isolate is gone (evicted / killed); nothing to do.
    Dropped,
}

/// Await a JS value returned by a plugin handler: a plain value passes
/// through, a Promise is driven to settlement by draining the QuickJS job
/// queue. Microtask-only async works because every `await` either resolves
/// immediately or awaits a host function that resolves synchronously. When a
/// plugin awaits a *cross-process* host request (fs / network), the job queue
/// empties before the promise settles and `Error::WouldBlock` is returned; the
/// pending promise is stashed on `globalThis.__stewardTopPromise` so the
/// service loop can resume it once the host replies (see `handle_host_response`).
fn await_view<'js>(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    if !value.is_promise() {
        return Ok(value);
    }
    let stash = value.clone();
    let promise = value.into_promise().ok_or_else(|| {
        rquickjs::Error::new_from_js_message(
            "plugin result",
            "Promise",
            "expected a promise object",
        )
    })?;
    match promise.finish::<JsValue>() {
        Ok(value) => Ok(value),
        Err(rquickjs::Error::WouldBlock) => {
            // Keep the top-level promise alive and reachable so a later host
            // reply can resume it (it is re-finished in `resolve_and_drain`).
            ctx.globals().set("__stewardTopPromise", stash)?;
            Err(rquickjs::Error::WouldBlock)
        }
        Err(error) => Err(error),
    }
}

/// Resolve (or reject) a plugin's pending host request and re-drain the parked
/// top-level promise. Returns the settled view as JSON, or `WouldBlock` when
/// the invocation is still parked.
fn resolve_and_drain<'js>(
    ctx: &Ctx<'js>,
    pending_id: u64,
    host_result: Result<&Json, &String>,
) -> rquickjs::Result<Json> {
    let globals = ctx.globals();
    let resolver_map: Object = globals.get("__stewardAsync")?;
    let key = pending_id.to_string();
    let resolver: JsValue = resolver_map.get(key.as_str())?;
    let resolver_obj: Object = resolver.into_object().ok_or_else(|| {
        rquickjs::Error::new_from_js_message(
            "__stewardAsync[id]",
            "object",
            "expected a resolver object",
        )
    })?;
    match host_result {
        Ok(value) => {
            let resolve: Function = resolver_obj.get("resolve")?;
            let js_value = json_to_js(ctx, value)?;
            resolve.call::<_, ()>((js_value,))?;
        }
        Err(message) => {
            let reject: Function = resolver_obj.get("reject")?;
            let error_ctor: Function = globals.get("Error")?;
            let err: JsValue = error_ctor.call((message.as_str(),))?;
            reject.call::<_, ()>((err,))?;
        }
    }
    resolver_map.remove(key.as_str())?;
    let top: JsValue = globals.get("__stewardTopPromise")?;
    let promise = top.into_promise().ok_or_else(|| {
        rquickjs::Error::new_from_js_message("__stewardTopPromise", "Promise", "expected a promise")
    })?;
    match promise.finish::<JsValue>() {
        Ok(value) => js_value_to_json(ctx, &value),
        Err(error) => Err(error),
    }
}

/// Shape the reply to a host request whose parked invocation just settled,
/// based on the original request method (command/item/search/action/submit).
fn build_response(parked: Request, view: Json) -> Response {
    match parked.method.as_str() {
        method::COMMAND_INVOKE => Response::ok(parked.id, serde_json::json!({ "view": view })),
        method::ITEM_INVOKE if !view.is_null() => {
            Response::ok(parked.id, serde_json::json!({ "view": view }))
        }
        method::ITEM_INVOKE => Response::ok(parked.id, serde_json::json!({})),
        method::SEARCH_QUERY if !view.is_null() => {
            Response::ok(parked.id, serde_json::json!({ "view": view }))
        }
        method::SEARCH_QUERY => Response::ok(parked.id, serde_json::json!({})),
        _ => Response::ok(parked.id, serde_json::json!({})),
    }
}

/// The execution deadline (ms) carried by a plugin request, used as the parked
/// invocation's deadline so a host that never replies cannot hang forever.
fn request_deadline_ms(request: &Request) -> u64 {
    request
        .params
        .get("deadline_ms")
        .and_then(Json::as_u64)
        .unwrap_or(500)
}

/// Build the `__stewardHost` object the Node polyfill reads for `process.env`,
/// `process.argv`, `os.*` and the cwd. Everything is stringified lossily so a
/// non-UTF-8 environment variable never aborts plugin load.
fn node_polyfill_host_info() -> Json {
    let mut env = serde_json::Map::new();
    for (key, value) in std::env::vars_os() {
        env.insert(
            key.to_string_lossy().into_owned(),
            Json::String(value.to_string_lossy().into_owned()),
        );
    }
    serde_json::json!({
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "homedir": dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "tmpdir": std::env::temp_dir().to_string_lossy().into_owned(),
        "cwd": std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "env": Json::Object(env),
        "argv": std::env::args().collect::<Vec<_>>(),
    })
}

/// Install `globalThis.steward`: the M3 host bridge (`clipboard.read/write/
/// history`, `storage.*`, `showToast`), gated by the plugin's permission
/// whitelist. Clipboard history and per-plugin storage are `Rc<RefCell<...>>`
/// so the bridge closures see the isolate's live snapshot / backing store.
fn install_host_bridge<'js>(
    ctx: &Ctx<'js>,
    permissions: &[Permission],
    notifications: &Rc<RefCell<Vec<Notification>>>,
    clipboard_history: &Rc<RefCell<Vec<ClipboardEntry>>>,
    storage: &Rc<RefCell<PluginStorage>>,
    async_state: &Rc<RefCell<AsyncBridgeState>>,
) -> rquickjs::Result<()> {
    let can_read = permissions.contains(&Permission::ClipboardRead);
    let can_write = permissions.contains(&Permission::ClipboardWrite);
    let can_history = permissions.contains(&Permission::ClipboardHistory);
    let can_open_url = permissions.contains(&Permission::OpenUrl);
    let can_open_path = permissions.contains(&Permission::OpenPath);
    let can_fs_read = permissions.contains(&Permission::FsRead);
    let can_fs_write = permissions.contains(&Permission::FsWrite);
    let can_network = permissions.contains(&Permission::Network);

    let clipboard = Object::new(ctx.clone())?;
    clipboard.set(
        "read",
        Function::new(ctx.clone(), move |ctx: Ctx| {
            if !can_read {
                return Err(Exception::throw_message(
                    &ctx,
                    "Steward permission denied: clipboard.read",
                ));
            }
            let mut system = arboard::Clipboard::new().map_err(|error| {
                Exception::throw_message(&ctx, &format!("clipboard read failed: {error}"))
            })?;
            let text = system.get_text().map_err(|error| {
                Exception::throw_message(&ctx, &format!("clipboard read failed: {error}"))
            })?;
            Ok(text)
        }),
    )?;
    clipboard.set(
        "write",
        Function::new(ctx.clone(), move |ctx: Ctx, text: String| {
            if !can_write {
                return Err(Exception::throw_message(
                    &ctx,
                    "Steward permission denied: clipboard.write",
                ));
            }
            let mut system = arboard::Clipboard::new().map_err(|error| {
                Exception::throw_message(&ctx, &format!("clipboard write failed: {error}"))
            })?;
            system.set_text(text).map_err(|error| {
                Exception::throw_message(&ctx, &format!("clipboard write failed: {error}"))
            })?;
            Ok(())
        }),
    )?;
    let history = clipboard_history.clone();
    clipboard.set(
        "history",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>| -> rquickjs::Result<JsValue<'js>> {
                if !can_history {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Steward permission denied: clipboard.history",
                    ));
                }
                let snapshot = history.borrow();
                let json = serde_json::to_value(&*snapshot).map_err(|error| {
                    Exception::throw_message(&ctx, &format!("clipboard history failed: {error}"))
                })?;
                json_to_js(&ctx, &json)
            },
        ),
    )?;

    let storage_cell = storage.clone();
    let plugin_storage = Object::new(ctx.clone())?;
    plugin_storage.set(
        "get",
        Function::new(
            ctx.clone(),
            move |key: String| -> rquickjs::Result<Option<String>> {
                Ok(storage_cell.borrow().get(&key))
            },
        ),
    )?;
    let storage_cell = storage.clone();
    plugin_storage.set(
        "set",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, key: String, value: String| -> rquickjs::Result<()> {
                storage_cell
                    .borrow_mut()
                    .set(&key, &value)
                    .map_err(|error| {
                        Exception::throw_message(&ctx, &format!("storage set failed: {error}"))
                    })?;
                Ok(())
            },
        ),
    )?;
    let storage_cell = storage.clone();
    plugin_storage.set(
        "remove",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, key: String| -> rquickjs::Result<()> {
                storage_cell.borrow_mut().remove(&key).map_err(|error| {
                    Exception::throw_message(&ctx, &format!("storage remove failed: {error}"))
                })?;
                Ok(())
            },
        ),
    )?;
    let storage_cell = storage.clone();
    plugin_storage.set(
        "clear",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<()> {
            storage_cell.borrow_mut().clear().map_err(|error| {
                Exception::throw_message(&ctx, &format!("storage clear failed: {error}"))
            })?;
            Ok(())
        }),
    )?;

    let steward = Object::new(ctx.clone())?;
    steward.set("clipboard", clipboard)?;
    steward.set("storage", plugin_storage)?;
    let notifications_toast = notifications.clone();
    steward.set(
        "showToast",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, options: JsValue<'js>| -> rquickjs::Result<()> {
                let options = js_value_to_json(&ctx, &options).unwrap_or(Json::Null);
                let params = match options {
                    Json::Object(_) => options,
                    other => serde_json::json!({ "message": other }),
                };
                notifications_toast
                    .borrow_mut()
                    .push(Notification::new(method::TOAST_SHOW, params));
                Ok(())
            },
        ),
    )?;
    let notifications_open_url = notifications.clone();
    steward.set(
        "openUrl",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, url: String| -> rquickjs::Result<()> {
                if !can_open_url {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Steward permission denied: open.url",
                    ));
                }
                if url.trim().is_empty() {
                    return Err(Exception::throw_message(
                        &ctx,
                        "open.url: url must not be empty",
                    ));
                }
                notifications_open_url.borrow_mut().push(Notification::new(
                    method::OPEN_URL,
                    serde_json::json!({ "url": url }),
                ));
                Ok(())
            },
        ),
    )?;
    let notifications_open_path = notifications.clone();
    steward.set(
        "openPath",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, path: String| -> rquickjs::Result<()> {
                if !can_open_path {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Steward permission denied: open.path",
                    ));
                }
                if path.trim().is_empty() {
                    return Err(Exception::throw_message(
                        &ctx,
                        "open.path: path must not be empty",
                    ));
                }
                notifications_open_path.borrow_mut().push(Notification::new(
                    method::OPEN_PATH,
                    serde_json::json!({ "path": path }),
                ));
                Ok(())
            },
        ),
    )?;
    let async_state_request = async_state.clone();
    steward.set(
        "__hostSend",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  method: String,
                  params: JsValue<'js>,
                  pending_id: u64|
                  -> rquickjs::Result<u64> {
                if method == method::HOST_FS_READ && !can_fs_read {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Steward permission denied: fs.read",
                    ));
                }
                if method == method::HOST_FS_WRITE && !can_fs_write {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Steward permission denied: fs.write",
                    ));
                }
                if method == method::HOST_NET_REQUEST && !can_network {
                    return Err(Exception::throw_message(
                        &ctx,
                        "Steward permission denied: network",
                    ));
                }
                let params = js_value_to_json(&ctx, &params).unwrap_or(Json::Null);
                async_state_request
                    .borrow_mut()
                    .outbound
                    .push(Request::new(pending_id, method, params));
                Ok(pending_id)
            },
        ),
    )?;
    ctx.globals().set("steward", steward)
}

/// Build a JS value by `JSON.parse`-ing a serialized `serde_json::Value`.
/// Used to pass plugin-facing data (clipboard history snapshot) that is cheap
/// to produce as a string and must arrive as a real JS object/array.
fn json_to_js<'js>(ctx: &Ctx<'js>, value: &Json) -> rquickjs::Result<JsValue<'js>> {
    let text = serde_json::to_string(value).map_err(|error| {
        rquickjs::Error::new_from_js_message(
            "Value",
            "serde_json::Value",
            format!("cannot encode value for JS: {error}"),
        )
    })?;
    let json: Object = ctx.globals().get("JSON")?;
    let parse: Function = json.get("parse")?;
    parse.call((text,))
}

/// Verify the plugin bundle exposes the M2 module shape: an object with a
/// callable `command(name, input)`.
fn verify_module(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let module: JsValue = ctx.globals().get(PLUGIN_GLOBAL)?;
    let object = module.as_object().ok_or_else(|| {
        rquickjs::Error::new_from_js_message(
            "globalThis.__stewardPlugin",
            "object",
            "expected an object, got a non-object value",
        )
    })?;
    let command: JsValue = object.get("command")?;
    if !command.is_function() {
        return Err(rquickjs::Error::new_from_js_message(
            "globalThis.__stewardPlugin.command",
            "function",
            "expected a callable command(name, input)",
        ));
    }
    Ok(())
}

/// Serialize a JavaScript value through `JSON.stringify`. `undefined`/`null`
/// map to JSON `null`; values that cannot be stringified (functions,
/// symbols) fail with a conversion error.
fn js_value_to_json<'js>(ctx: &Ctx<'js>, value: &JsValue<'js>) -> rquickjs::Result<Json> {
    if value.is_undefined() || value.is_null() {
        return Ok(Json::Null);
    }
    let json: Object = ctx.globals().get("JSON")?;
    let stringify: Function = json.get("stringify")?;
    let text: String = stringify.call((value.clone(),))?;
    serde_json::from_str(&text).map_err(|error| {
        rquickjs::Error::new_from_js_message(
            "Value",
            "serde_json::Value",
            format!("view is not JSON-serializable: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};
    use steward_ipc_protocol::ClipboardEntry;
    use steward_plugin_registry::PluginManifest;

    /// QuickJS `Runtime::new()` is not safe to run concurrently across test
    /// threads (a flaky `command_returns_serialized_view` showed a missing
    /// argument value when several runtimes were created in parallel). The
    /// production service loop is single-threaded, so this is a test-only
    /// serialization to keep the suite deterministic.
    static RUNTIME_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_test() -> MutexGuard<'static, ()> {
        RUNTIME_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn manifest(permissions: &[&str]) -> PluginManifest {
        let permissions = permissions
            .iter()
            .map(|p| serde_json::Value::String((*p).into()))
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({
            "id": "com.test.pool",
            "name": "Pool test",
            "version": "1.0.0",
            "commands": [
                { "name": "echo", "title": "Echo", "trigger": { "type": "command" } },
                { "name": "loop", "title": "Loop", "trigger": { "type": "command" } },
                { "name": "memory", "title": "Memory", "trigger": { "type": "command" } }
            ],
            "permissions": permissions,
            "isolation": "shared-pool"
        }))
        .unwrap()
    }

    fn write_bundle(source: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "steward-runtime-pool-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bundle.js");
        std::fs::write(&path, source).unwrap();
        path
    }

    #[test]
    fn command_returns_serialized_view() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    return { type: "list", items: [
                        { id: "a", title: input, subtitle: "first" },
                        { id: "b", title: "Beta", keywords: ["bee"] }
                    ] };
                }
                return { command: command };
            })();
            "#,
        );
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let view = pool
            .invoke_command(id, "echo", &Json::String("hello".into()), 1000)
            .unwrap();
        assert_eq!(view["type"], "list");
        assert_eq!(view["items"][0]["title"], "hello");
        assert_eq!(view["items"][1]["id"], "b");
    }

    #[test]
    fn unknown_command_is_rejected() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) { return null; }
                return { command: command };
            })();
            "#,
        );
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "missing", &Json::Null, 1000)
            .unwrap_err();
        assert_eq!(error, InvokeError::CommandNotFound);
    }

    #[test]
    fn infinite_loop_is_interrupted_and_isolate_killed() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function loop() { while (true) {} }
                function echo(name, input) { return { type: "list", items: [] }; }
                return { command: function (name) { if (name === "loop") return loop(); return echo(name); } };
            })();
            "#,
        );
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "loop", &Json::Null, 100)
            .unwrap_err();
        assert_eq!(error, InvokeError::Timeout);
        // The isolate was killed: further invocations report it as gone.
        assert_eq!(
            pool.invoke_command(id, "echo", &Json::Null, 1000),
            Err(InvokeError::NotFound)
        );
    }

    #[test]
    fn heap_limit_kills_isolate() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function memory() {
                    var a = [];
                    for (var i = 0; i < 10000000; i++) a.push("x");
                    return a;
                }
                function echo(name, input) { return { type: "list", items: [] }; }
                return { command: function (name) { if (name === "memory") return memory(); return echo(name); } };
            })();
            "#,
        );
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "memory", &Json::Null, 5000)
            .unwrap_err();
        assert!(
            matches!(error, InvokeError::Memory),
            "unexpected error: {error:?}"
        );
        assert_eq!(
            pool.invoke_command(id, "echo", &Json::Null, 1000),
            Err(InvokeError::NotFound)
        );
    }

    #[test]
    fn host_bridge_enforces_permissions() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.clipboard.write("secret");
                    return { type: "list", items: [] };
                }
                return { command: command };
            })();
            "#,
        );
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::PermissionDenied(message) if message.contains("clipboard.write")),
            "unexpected error: {error:?}"
        );
        // A permission denial is a JS exception, not a kill: the isolate stays
        // alive and reports the same error again.
        assert!(pool.invoke_command(id, "echo", &Json::Null, 1000).is_err());
        assert_eq!(pool.active_count(), 1);
    }

    #[test]
    fn show_toast_emits_notification() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.showToast({ message: "Copied", kind: "success" });
                    return null;
                }
                return { command: command };
            })();
            "#,
        );
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let view = pool.invoke_command(id, "echo", &Json::Null, 1000).unwrap();
        assert_eq!(view, Json::Null);
        let notifications = pool.drain_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].method, method::TOAST_SHOW);
        assert_eq!(notifications[0].params["message"], "Copied");
        assert_eq!(notifications[0].params["kind"], "success");
    }

    #[test]
    fn item_invoke_calls_select_handler() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                var state = { items: [], onSelect: null };
                function command(name, input) {
                    state.items = [{ id: "today", title: "Today" }];
                    state.onSelect = function (item) {
                        steward.showToast({ message: "selected " + item.id });
                    };
                    return { type: "list", items: state.items };
                }
                function select(id) {
                    var item = state.items.find(function (i) { return i.id === id; });
                    if (item && state.onSelect) state.onSelect(item);
                }
                return { command: command, select: select };
            })();
            "#,
        );
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        pool.invoke_command(id, "echo", &Json::Null, 1000).unwrap();
        pool.invoke_item(id, "today", 1000).unwrap();
        let notifications = pool.drain_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].params["message"], "selected today");
    }

    #[test]
    fn pool_evicts_lru_when_full() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(false, 2, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) { return { type: "list", items: [] }; }
                return { command: command };
            })();
            "#,
        );
        let first = pool.load(&entry, &manifest(&[])).unwrap();
        pool.invoke_command(first, "echo", &Json::Null, 1000)
            .unwrap();
        let second = pool.load(&entry, &manifest(&[])).unwrap();
        pool.invoke_command(second, "echo", &Json::Null, 1000)
            .unwrap();
        assert_eq!(pool.active_count(), 2);
        // Loading a third plugin evicts the least-recently-used isolate.
        let third = pool.load(&entry, &manifest(&[])).unwrap();
        assert_eq!(pool.active_count(), 2);
        assert_eq!(
            pool.invoke_command(first, "echo", &Json::Null, 1000),
            Err(InvokeError::NotFound)
        );
        assert!(pool
            .invoke_command(second, "echo", &Json::Null, 1000)
            .is_ok());
        assert!(pool
            .invoke_command(third, "echo", &Json::Null, 1000)
            .is_ok());
    }

    #[test]
    fn dedicated_mode_replaces_plugin() {
        let _guard = lock_test();
        let mut pool = IsolatePool::new(true, 1, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) { return { type: "list", items: [] }; }
                return { command: command };
            })();
            "#,
        );
        let first = pool.load(&entry, &manifest(&[])).unwrap();
        assert!(pool
            .invoke_command(first, "echo", &Json::Null, 1000)
            .is_ok());
        let second = pool.load(&entry, &manifest(&[])).unwrap();
        assert_eq!(pool.active_count(), 1);
        assert_eq!(
            pool.invoke_command(first, "echo", &Json::Null, 1000),
            Err(InvokeError::NotFound)
        );
        assert!(pool
            .invoke_command(second, "echo", &Json::Null, 1000)
            .is_ok());
    }

    #[test]
    fn clipboard_history_is_injected_and_gated() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    var h = steward.clipboard.history();
                    return { type: "list", items: h.map(function (e) {
                        return { id: String(e.id), title: e.text };
                    }) };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool
            .load(&entry, &manifest(&["clipboard.history"]))
            .unwrap();
        let history = vec![
            ClipboardEntry {
                id: "1".into(),
                text: "alpha".into(),
                copied_at: 100,
            },
            ClipboardEntry {
                id: "2".into(),
                text: "beta".into(),
                copied_at: 200,
            },
        ];
        let view = pool
            .invoke_command_with_history(id, "echo", &Json::Null, 1000, Some(history.clone()))
            .unwrap();
        assert_eq!(view["items"][0]["title"], "alpha");
        assert_eq!(view["items"][1]["id"], "2");

        // Without the permission the same call throws permission denied, and
        // the isolate stays alive (a denial is not a kill).
        let mut pool2 = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id2 = pool2.load(&entry, &manifest(&[])).unwrap();
        let error = pool2
            .invoke_command_with_history(id2, "echo", &Json::Null, 1000, Some(history.clone()))
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::PermissionDenied(m) if m.contains("clipboard.history")),
            "unexpected error: {error:?}"
        );
        assert_eq!(pool2.active_count(), 1);
    }

    #[test]
    fn storage_round_trips_through_bridge() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.storage.set("k", "v");
                    var got = steward.storage.get("k") || "null";
                    steward.storage.remove("k");
                    var after = steward.storage.get("k") || "null";
                    steward.storage.set("x", "y");
                    steward.storage.clear();
                    var cleared = steward.storage.get("x") || "null";
                    return { type: "list", items: [
                        { id: "1", title: got },
                        { id: "2", title: after },
                        { id: "3", title: cleared }
                    ] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let view = pool.invoke_command(id, "echo", &Json::Null, 1000).unwrap();
        assert_eq!(view["items"][0]["title"], "v");
        assert_eq!(view["items"][1]["title"], "null");
        assert_eq!(view["items"][2]["title"], "null");
    }

    #[test]
    fn invoke_action_runs_export() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) { return null; }
                function run(actionId, itemId) {
                    steward.showToast({ message: "action " + actionId + ":" + itemId });
                }
                return { command: command, run: run };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        pool.invoke_action(id, "copy", Some("7".into()), 1000)
            .unwrap();
        pool.invoke_action(id, "pin", None, 1000).unwrap();
        let notifications = pool.drain_notifications();
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].params["message"], "action copy:7");
        assert_eq!(notifications[1].params["message"], "action pin:undefined");
    }

    #[test]
    fn invoke_submit_runs_export() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) { return null; }
                function submit(values) {
                    steward.showToast({ message: "name=" + values.name });
                }
                return { command: command, submit: submit };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        pool.invoke_submit(id, &serde_json::json!({ "name": "Ada" }), 1000)
            .unwrap();
        let notifications = pool.drain_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].params["message"], "name=Ada");
    }

    #[test]
    fn open_url_emits_notification_when_granted() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.openUrl("https://github.com");
                    return null;
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["open.url"])).unwrap();
        pool.invoke_command(id, "echo", &Json::Null, 1000).unwrap();
        let notifications = pool.drain_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].method, method::OPEN_URL);
        assert_eq!(notifications[0].params["url"], "https://github.com");
    }

    #[test]
    fn open_url_is_gated_by_permission() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.openUrl("https://github.com");
                    return null;
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::PermissionDenied(message) if message.contains("open.url")),
            "unexpected error: {error:?}"
        );
        // A denial is not a kill; the isolate stays alive.
        assert_eq!(pool.active_count(), 1);
        assert_eq!(pool.drain_notifications(), Vec::new());
    }

    #[test]
    fn open_path_emits_notification_when_granted() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.openPath("C:/notes/file.txt");
                    return null;
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["open.path"])).unwrap();
        pool.invoke_command(id, "echo", &Json::Null, 1000).unwrap();
        let notifications = pool.drain_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].method, method::OPEN_PATH);
        assert_eq!(notifications[0].params["path"], "C:/notes/file.txt");
    }

    #[test]
    fn open_path_is_gated_by_permission() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.openPath("C:/notes/file.txt");
                    return null;
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::PermissionDenied(message) if message.contains("open.path")),
            "unexpected error: {error:?}"
        );
        assert_eq!(pool.drain_notifications(), Vec::new());
    }

    #[test]
    fn open_url_rejects_empty_target() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                function command(name, input) {
                    steward.openUrl("  ");
                    return null;
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["open.url"])).unwrap();
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::Plugin(message) if message.contains("must not be empty")),
            "unexpected error: {error:?}"
        );
        assert_eq!(pool.drain_notifications(), Vec::new());
    }

    #[test]
    fn await_fs_read_parks_and_resumes() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    var data = await steward.fs.readFile("C:/tmp/test.txt", "utf8");
                    return { type: "list", items: [{ id: "1", title: data }] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["fs.read"])).unwrap();

        // The command parks on the cross-process host request instead of
        // timing out (an isolate that stays alive, not one that is killed).
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert_eq!(error, InvokeError::Pending);
        assert_eq!(pool.active_count(), 1, "parked isolate must not be killed");

        // The runtime queued a single `host.fs.read` request to send to the host.
        let requests = pool.drain_outbound(id);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, method::HOST_FS_READ);
        assert_eq!(requests[0].params["path"], "C:/tmp/test.txt");
        assert_eq!(requests[0].params["encoding"], "utf8");

        // The service loop would park the original command request before the
        // host replies; mirror that so the reply knows which request to answer.
        let original = Request::new(42, method::COMMAND_INVOKE, Json::Null);
        pool.park_invocation(id, original);

        // Simulate the host's reply with the file contents.
        let reply = Response::ok(
            requests[0].id,
            serde_json::json!({ "data": "hello", "base64": false }),
        );
        let outcome = pool.handle_host_response(reply);
        let ResumeOutcome::Reply(response) = outcome else {
            panic!("expected the parked command to settle, got {outcome:?}");
        };
        assert_eq!(
            response.id, 42,
            "reply must answer the original command request"
        );
        assert_eq!(
            response.result.as_ref().unwrap()["view"]["items"][0]["title"],
            "hello"
        );
        // The isolate is free again and can be re-used (no lingering parking).
        assert!(!pool.is_parked(id));
    }

    #[test]
    fn fs_read_without_permission_rejects_immediately() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    await steward.fs.readFile("C:/tmp/test.txt", "utf8");
                    return { type: "list", items: [] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::PermissionDenied(message) if message.contains("fs.read")),
            "unexpected error: {error:?}"
        );
        // A denial is not a kill; the isolate stays alive.
        assert_eq!(pool.active_count(), 1);
    }

    #[test]
    fn await_fs_write_parks_and_resumes() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    await steward.fs.writeFile("C:/tmp/out.txt", "written", "utf8");
                    return { type: "list", items: [{ id: "1", title: "done" }] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["fs.write"])).unwrap();

        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert_eq!(error, InvokeError::Pending);
        assert_eq!(pool.active_count(), 1);

        let requests = pool.drain_outbound(id);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, method::HOST_FS_WRITE);
        assert_eq!(requests[0].params["path"], "C:/tmp/out.txt");
        assert_eq!(requests[0].params["data"], "written");
        assert_eq!(requests[0].params["base64"], false);

        let original = Request::new(7, method::COMMAND_INVOKE, Json::Null);
        pool.park_invocation(id, original);
        let outcome =
            pool.handle_host_response(Response::ok(requests[0].id, serde_json::json!({})));
        let ResumeOutcome::Reply(response) = outcome else {
            panic!("expected the parked command to settle, got {outcome:?}");
        };
        assert_eq!(response.id, 7);
        assert_eq!(
            response.result.as_ref().unwrap()["view"]["items"][0]["title"],
            "done"
        );
        assert!(!pool.is_parked(id));
    }

    #[test]
    fn fs_write_without_permission_rejects_immediately() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    await steward.fs.writeFile("C:/tmp/out.txt", "written", "utf8");
                    return { type: "list", items: [] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::PermissionDenied(message) if message.contains("fs.write")),
            "unexpected error: {error:?}"
        );
        assert_eq!(pool.active_count(), 1);
    }

    #[test]
    fn parallel_fs_read_parks_and_resumes() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    var r = await Promise.all([
                        steward.fs.readFile("a", "utf8"),
                        steward.fs.readFile("b", "utf8")
                    ]);
                    return { type: "list", items: [{ id: "1", title: r[0] + r[1] }] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["fs.read"])).unwrap();

        assert_eq!(
            pool.invoke_command(id, "echo", &Json::Null, 1000)
                .unwrap_err(),
            InvokeError::Pending
        );
        // Both reads are queued together (Promise.all -> two in-flight requests).
        let requests = pool.drain_outbound(id);
        assert_eq!(requests.len(), 2);
        let first = requests.iter().find(|r| r.params["path"] == "a").unwrap();
        let second = requests.iter().find(|r| r.params["path"] == "b").unwrap();

        let original = Request::new(3, method::COMMAND_INVOKE, Json::Null);
        pool.park_invocation(id, original);

        // Resolving only one read keeps the invocation parked (Promise.all is
        // still waiting on the other).
        let outcome = pool.handle_host_response(Response::ok(
            first.id,
            serde_json::json!({ "data": "A", "base64": false }),
        ));
        assert!(
            matches!(outcome, ResumeOutcome::Parked(_)),
            "still waiting on the second read"
        );
        assert!(pool.is_parked(id));

        // Resolving the second read settles the command.
        let outcome = pool.handle_host_response(Response::ok(
            second.id,
            serde_json::json!({ "data": "B", "base64": false }),
        ));
        let ResumeOutcome::Reply(response) = outcome else {
            panic!("expected the parked command to settle, got {outcome:?}");
        };
        assert_eq!(response.id, 3);
        assert_eq!(
            response.result.as_ref().unwrap()["view"]["items"][0]["title"],
            "AB"
        );
        assert!(!pool.is_parked(id));
    }

    #[test]
    fn late_host_reply_after_isolate_killed_is_dropped() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    await steward.fs.readFile("x", "utf8");
                    return { type: "list", items: [] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["fs.read"])).unwrap();
        assert_eq!(
            pool.invoke_command(id, "echo", &Json::Null, 1000)
                .unwrap_err(),
            InvokeError::Pending
        );
        let requests = pool.drain_outbound(id);
        let request_id = requests[0].id;
        let original = Request::new(5, method::COMMAND_INVOKE, Json::Null);
        pool.park_invocation(id, original);

        // Kill / evict the parked isolate (e.g. LRU eviction, unload, deadline).
        pool.unload(id);
        assert_eq!(pool.active_count(), 0);
        assert!(!pool.is_parked(id));

        // A late host reply for the dead isolate must be dropped, not resume it.
        let outcome = pool.handle_host_response(Response::ok(
            request_id,
            serde_json::json!({ "data": "late", "base64": false }),
        ));
        assert!(matches!(outcome, ResumeOutcome::Dropped));
        // The same isolate id is gone: further invocations report it as not found.
        assert_eq!(
            pool.invoke_command(id, "echo", &Json::Null, 1000),
            Err(InvokeError::NotFound)
        );
    }

    #[test]
    fn expired_parked_invocation_times_out() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    await steward.fs.readFile("x", "utf8");
                    return { type: "list", items: [] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["fs.read"])).unwrap();
        assert_eq!(
            pool.invoke_command(id, "echo", &Json::Null, 1000)
                .unwrap_err(),
            InvokeError::Pending
        );
        pool.drain_outbound(id);
        // Park with a deadline that has already elapsed (deadline_ms = 0).
        let original = Request::new(
            9,
            method::COMMAND_INVOKE,
            serde_json::json!({ "deadline_ms": 0 }),
        );
        pool.park_invocation(id, original);

        let responses = pool.expire_parked();
        assert_eq!(responses.len(), 1);
        let response = &responses[0];
        assert_eq!(response.id, 9);
        assert_eq!(response.error.as_ref().unwrap().code, code::TIMEOUT);
        // The isolate was killed and its bookkeeping cleared.
        assert_eq!(pool.active_count(), 0);
        assert!(!pool.is_parked(id));
    }

    #[test]
    fn await_net_request_parks_and_resumes() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    var res = await steward.net.request({ url: "https://x.test", method: "GET" });
                    return { type: "list", items: [{ id: "1", title: res.body }] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&["network"])).unwrap();

        assert_eq!(
            pool.invoke_command(id, "echo", &Json::Null, 1000)
                .unwrap_err(),
            InvokeError::Pending
        );
        let requests = pool.drain_outbound(id);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, method::HOST_NET_REQUEST);
        assert_eq!(requests[0].params["url"], "https://x.test");
        assert_eq!(requests[0].params["method"], "GET");

        let original = Request::new(6, method::COMMAND_INVOKE, Json::Null);
        pool.park_invocation(id, original);
        let outcome = pool.handle_host_response(Response::ok(
            requests[0].id,
            serde_json::json!({ "status": 200, "headers": {}, "body": "ok" }),
        ));
        let ResumeOutcome::Reply(response) = outcome else {
            panic!("expected the parked command to settle, got {outcome:?}");
        };
        assert_eq!(response.id, 6);
        assert_eq!(
            response.result.as_ref().unwrap()["view"]["items"][0]["title"],
            "ok"
        );
        assert!(!pool.is_parked(id));
    }

    #[test]
    fn net_request_without_permission_rejects_immediately() {
        let _guard = lock_test();
        let entry = write_bundle(
            r#"
            var __stewardPlugin = (() => {
                async function command(name, input) {
                    await steward.net.request({ url: "https://x.test" });
                    return { type: "list", items: [] };
                }
                return { command: command };
            })();
            "#,
        );
        let mut pool = IsolatePool::new(false, 8, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK);
        let id = pool.load(&entry, &manifest(&[])).unwrap();
        let error = pool
            .invoke_command(id, "echo", &Json::Null, 1000)
            .unwrap_err();
        assert!(
            matches!(&error, InvokeError::PermissionDenied(message) if message.contains("network")),
            "unexpected error: {error:?}"
        );
        assert_eq!(pool.active_count(), 1);
    }
}

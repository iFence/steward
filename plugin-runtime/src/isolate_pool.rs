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
use steward_ipc_protocol::{method, Notification};
use steward_plugin_registry::{Permission, PluginManifest};

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
        }
    }

    /// Load a plugin bundle into an isolate and return its id.
    pub fn load(&mut self, entry: &Path, manifest: &PluginManifest) -> Result<IsolateId> {
        if self.dedicated {
            // Dedicated process: one plugin at a time; loading replaces it.
            self.isolates.clear();
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
        context
            .with(|ctx| -> anyhow::Result<()> {
                install_host_bridge(&ctx, &permissions, &notifications)?;
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
        {
            let isolate = self.isolate_mut(id).ok_or(InvokeError::NotFound)?;
            if !isolate.plugin.commands.iter().any(|c| c == command) {
                return Err(InvokeError::CommandNotFound);
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
            js_value_to_json(&ctx, &result)
        })
    }

    /// Invoke a rendered list item's `select` handler (if the plugin exports
    /// one). A missing handler is a no-op success.
    pub fn invoke_item(
        &mut self,
        id: IsolateId,
        item_id: &str,
        deadline_ms: u64,
    ) -> Result<(), InvokeError> {
        let item_id = item_id.to_string();
        self.run_in_isolate(id, deadline_ms, |ctx| {
            let module: Object = ctx.globals().get(PLUGIN_GLOBAL)?;
            let select: JsValue = module.get("select")?;
            if !select.is_function() {
                return Ok(());
            }
            let select_fn: Function = select.into_function().ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "globalThis.__stewardPlugin.select",
                    "function",
                    "expected a callable select(itemId)",
                )
            })?;
            select_fn.call::<_, ()>((item_id,))?;
            Ok(())
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
        if let Some(index) = self.slot(id) {
            self.isolates[index] = None;
        }
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
            self.isolates[index] = None;
        }
    }
}

/// Install `globalThis.steward`: the M2 host bridge (`clipboard.read/write`,
/// `showToast`), gated by the plugin's permission whitelist.
fn install_host_bridge<'js>(
    ctx: &Ctx<'js>,
    permissions: &[Permission],
    notifications: &Rc<RefCell<Vec<Notification>>>,
) -> rquickjs::Result<()> {
    let can_read = permissions.contains(&Permission::ClipboardRead);
    let can_write = permissions.contains(&Permission::ClipboardWrite);

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

    let steward = Object::new(ctx.clone())?;
    steward.set("clipboard", clipboard)?;
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
    ctx.globals().set("steward", steward)
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
}

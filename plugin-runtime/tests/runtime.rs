//! End-to-end tests: spawn the real `steward-plugin-runtime` binary and speak
//! the NDJSON/JSON-RPC protocol over stdin/stdout, exactly like the host.

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    time::Duration,
};

use crossbeam_channel::Receiver;
use serde_json::{json, Value};
use steward_ipc_protocol::{
    code, decode_line, method, Message, Notification, Request, Response, RpcError,
};

struct RuntimeProc {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next_id: u64,
}

impl RuntimeProc {
    fn spawn(extra_args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_steward-plugin-runtime"))
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn steward-plugin-runtime");
        let stdin = child.stdin.take().expect("runtime stdin");
        let stdout = child.stdout.take().expect("runtime stdout");
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            child,
            stdin,
            rx,
            next_id: 0,
        }
    }

    /// Send one request and wait for its response, collecting any
    /// notifications the runtime emitted first.
    fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> (Option<Value>, Option<RpcError>, Vec<Notification>) {
        self.next_id += 1;
        let id = self.next_id;
        let request = Request::new(id, method, params);
        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        self.stdin.flush().unwrap();
        let mut notifications = Vec::new();
        loop {
            let line = self
                .rx
                .recv_timeout(Duration::from_secs(10))
                .expect("timed out waiting for a runtime response");
            let message = decode_line(&line)
                .expect("runtime emitted a malformed line")
                .expect("empty line");
            match message {
                Message::Response(response) if response.id == id => {
                    return (response.result, response.error, notifications);
                }
                Message::Response(_) => panic!("runtime responded to an unknown request id"),
                Message::Notification(notification) => notifications.push(notification),
                Message::Request(_) => panic!("runtime sent an unexpected request"),
            }
        }
    }

    fn load(&mut self, root: &Path, plugin_id: &str) -> (Value, Option<RpcError>) {
        let (result, error, _) = self.request(
            method::PLUGIN_LOAD,
            json!({
                "id": plugin_id,
                "entry_path": root.join(plugin_id).join("index.js"),
                "manifest": manifest(plugin_id)
            }),
        );
        (result.expect("plugin.load failed"), error)
    }

    /// Load with an explicit manifest (e.g. one granting `fs.read`).
    fn load_with_manifest(
        &mut self,
        root: &Path,
        plugin_id: &str,
        manifest: Value,
    ) -> (Value, Option<RpcError>) {
        let (result, error, _) = self.request(
            method::PLUGIN_LOAD,
            json!({
                "id": plugin_id,
                "entry_path": root.join(plugin_id).join("index.js"),
                "manifest": manifest
            }),
        );
        (result.expect("plugin.load failed"), error)
    }

    /// Like [`request`], but answers runtime -> host `host.fs.read` requests it
    /// receives in-flight by reading the requested file and replying, then waits
    /// for the original request's response (a full cross-process await round-trip).
    fn request_fs_roundtrip(
        &mut self,
        method: &str,
        params: Value,
    ) -> (Option<Value>, Option<RpcError>, Vec<Notification>) {
        self.next_id += 1;
        let id = self.next_id;
        let request = Request::new(id, method, params);
        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        self.stdin.flush().unwrap();
        let mut notifications = Vec::new();
        loop {
            let line = self
                .rx
                .recv_timeout(Duration::from_secs(10))
                .expect("timed out waiting for a runtime response");
            let message = decode_line(&line)
                .expect("runtime emitted a malformed line")
                .expect("empty line");
            match message {
                Message::Response(response) if response.id == id => {
                    return (response.result, response.error, notifications);
                }
                Message::Response(_) => panic!("runtime responded to an unknown request id"),
                Message::Notification(notification) => notifications.push(notification),
                Message::Request(host_req) => {
                    assert_eq!(host_req.method, method::HOST_FS_READ);
                    let path = host_req.params["path"].as_str().expect("host.fs.read path");
                    let text = std::fs::read_to_string(path).unwrap();
                    let reply = Response::ok(host_req.id, json!({ "data": text, "base64": false }));
                    writeln!(self.stdin, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
                    self.stdin.flush().unwrap();
                }
            }
        }
    }
}

impl Drop for RuntimeProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn manifest(plugin_id: &str) -> Value {
    json!({
        "id": plugin_id,
        "name": "Test",
        "version": "1.0.0",
        "commands": [
            { "name": "run", "title": "Run", "trigger": { "type": "command" } },
            { "name": "calendar", "title": "Calendar", "trigger": { "type": "command" } }
        ],
        "permissions": [],
        "isolation": "shared-pool"
    })
}

fn plugin_root(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("steward-runtime-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write_plugin(root: &Path, plugin_id: &str, source: &str) {
    let dir = root.join(plugin_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.js"), source).unwrap();
}

#[test]
fn ping_and_unknown_method() {
    let mut proc = RuntimeProc::spawn(&[]);
    let (result, error, notifications) = proc.request(method::PING, Value::Null);
    assert!(notifications.is_empty());
    assert!(error.is_none());
    assert_eq!(result, Some(json!({ "pong": true })));

    let (_, error, _) = proc.request("bogus.method", Value::Null);
    assert_eq!(error.unwrap().code, code::METHOD_NOT_FOUND);
}

#[test]
fn load_and_invoke_command_returns_view() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("basic");
    write_plugin(
        &root,
        "com.test.basic",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) {
                return { type: "list", items: [
                    { id: "a", title: "Alpha " + input, subtitle: "first" },
                    { id: "b", title: "Beta", keywords: ["bee"] }
                ] };
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, load_error) = proc.load(&root, "com.test.basic");
    assert!(load_error.is_none(), "load failed: {load_error:?}");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();

    let (result, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "hello", "deadline_ms": 1000 }),
    );
    assert!(error.is_none(), "invoke failed: {error:?}");
    let view = result.unwrap()["view"].clone();
    assert_eq!(view["type"], "list");
    assert_eq!(view["items"][0]["title"], "Alpha hello");
    assert_eq!(view["items"][1]["id"], "b");
}

#[test]
fn async_command_resolves_promise_view() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("async-basic");
    write_plugin(
        &root,
        "com.test.async-basic",
        r#"
        var __stewardPlugin = (() => {
            async function command(name, input) {
                await Promise.resolve();
                await Promise.resolve().then((x) => x);
                return { type: "list", items: [{ id: "a", title: "async " + input }] };
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, _) = proc.load(&root, "com.test.async-basic");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (result, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "ok", "deadline_ms": 1000 }),
    );
    assert!(error.is_none(), "async invoke failed: {error:?}");
    let view = result.unwrap()["view"].clone();
    assert_eq!(view["type"], "list");
    assert_eq!(view["items"][0]["title"], "async ok");
}

#[test]
fn async_command_never_settles_is_timeout() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("async-pending");
    write_plugin(
        &root,
        "com.test.async-pending",
        r#"
        var __stewardPlugin = (() => {
            async function command(name, input) {
                // Awaits an event no host function will ever deliver: the job
                // queue empties, leaving the promise pending forever.
                return await new Promise(function (resolve) {});
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, _) = proc.load(&root, "com.test.async-pending");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 100 }),
    );
    assert_eq!(error.unwrap().code, code::TIMEOUT);

    // The isolate was killed: a second invoke reports the plugin as gone.
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "calendar", "input": "", "deadline_ms": 1000 }),
    );
    assert_eq!(error.unwrap().code, code::PLUGIN_NOT_FOUND);
}

#[test]
fn node_polyfill_path_and_buffer_resolve() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("node-polyfill");
    write_plugin(
        &root,
        "com.test.node-polyfill",
        r#"
        var __stewardPlugin = (() => {
            var path = require("path");
            var b = Buffer.from("ab", "hex");
            function command(name, input) {
                return { type: "list", items: [
                    { id: "p", title: path.join("a", "b") + ":" + b.toString("hex") }
                ] };
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, load_error) = proc.load(&root, "com.test.node-polyfill");
    assert!(load_error.is_none(), "load failed: {load_error:?}");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (result, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    assert!(error.is_none(), "invoke failed: {error:?}");
    let view = result.unwrap()["view"].clone();
    assert_eq!(view["type"], "list");
    assert_eq!(view["items"][0]["title"], "a/b:ab");
}

#[test]
fn node_polyfill_fs_throws_not_supported() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("node-polyfill-fs");
    write_plugin(
        &root,
        "com.test.node-polyfill-fs",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) {
                var fs = require("fs");
                fs.readFileSync("x");
                return { type: "list", items: [] };
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, _) = proc.load(&root, "com.test.node-polyfill-fs");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    let error = error.unwrap();
    assert_eq!(error.code, code::INTERNAL_ERROR);
    assert!(
        error.message.contains("not supported in M3"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn search_query_streams_results() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("search");
    write_plugin(
        &root,
        "com.test.search",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) {
                return { type: "search", placeholder: "Search…" };
            }
            function search(query) {
                return { type: "list", items: [
                    { id: "a", title: "result " + query }
                ] };
            }
            return { command: command, search: search };
        })();
        "#,
    );

    let (load_result, _) = proc.load(&root, "com.test.search");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    assert!(error.is_none());

    let (result, error, _) = proc.request(
        method::SEARCH_QUERY,
        json!({ "isolate_id": isolate_id, "query": "hello", "deadline_ms": 1000 }),
    );
    assert!(error.is_none(), "search failed: {error:?}");
    let view = result.unwrap()["view"].clone();
    assert_eq!(view["type"], "list");
    assert_eq!(view["items"][0]["title"], "result hello");
}

#[test]
fn infinite_loop_hits_timeout_and_isolate_is_killed() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("timeout");
    write_plugin(
        &root,
        "com.test.timeout",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) {
                if (name === "calendar") return { type: "list", items: [] };
                while (true) {}
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, _) = proc.load(&root, "com.test.timeout");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();

    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 100 }),
    );
    assert_eq!(error.unwrap().code, code::TIMEOUT);

    // The isolate was killed: a second invoke reports the plugin as gone.
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "calendar", "input": "", "deadline_ms": 1000 }),
    );
    assert_eq!(error.unwrap().code, code::PLUGIN_NOT_FOUND);

    // Reloading works: the runtime recovers from the killed isolate.
    let (load_result, load_error) = proc.load(&root, "com.test.timeout");
    assert!(load_error.is_none());
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (result, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "calendar", "input": "", "deadline_ms": 1000 }),
    );
    assert!(error.is_none());
    assert_eq!(result.unwrap()["view"]["type"], "list");
}

#[test]
fn heap_limit_kills_isolate() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("memory");
    write_plugin(
        &root,
        "com.test.memory",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) {
                if (name === "calendar") return { type: "list", items: [] };
                var a = [];
                for (var i = 0; i < 10000000; i++) a.push("x");
                return a;
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, _) = proc.load(&root, "com.test.memory");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 5000 }),
    );
    assert!(error.is_some(), "expected a heap-limit failure");
    assert_eq!(error.unwrap().code, code::INTERNAL_ERROR);

    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "calendar", "input": "", "deadline_ms": 1000 }),
    );
    assert_eq!(error.unwrap().code, code::PLUGIN_NOT_FOUND);
}

#[test]
fn clipboard_without_permission_is_denied() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("permission");
    write_plugin(
        &root,
        "com.test.permission",
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

    let (load_result, _) = proc.load(&root, "com.test.permission");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    let error = error.unwrap();
    assert_eq!(error.code, code::PERMISSION_DENIED);
    assert!(error.message.contains("clipboard.write"));
}

#[test]
fn show_toast_emits_notification_before_response() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("toast");
    write_plugin(
        &root,
        "com.test.toast",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) {
                steward.showToast({ message: "Copied", kind: "success", durationMs: 1200 });
                return null;
            }
            return { command: command };
        })();
        "#,
    );

    let (load_result, _) = proc.load(&root, "com.test.toast");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (result, error, notifications) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    assert!(error.is_none());
    assert_eq!(result.unwrap()["view"], Value::Null);
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].method, method::TOAST_SHOW);
    assert_eq!(notifications[0].params["message"], "Copied");
    assert_eq!(notifications[0].params["kind"], "success");
    assert_eq!(notifications[0].params["durationMs"], 1200);
}

#[test]
fn item_invoke_runs_select_handler() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("select");
    write_plugin(
        &root,
        "com.test.select",
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

    let (load_result, _) = proc.load(&root, "com.test.select");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    let (result, error, notifications) = proc.request(
        method::ITEM_INVOKE,
        json!({ "isolate_id": isolate_id, "item_id": "today", "deadline_ms": 1000 }),
    );
    assert!(error.is_none(), "item invoke failed: {error:?}");
    assert_eq!(result, Some(json!({})));
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].params["message"], "selected today");
}

#[test]
fn unload_frees_isolate() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("unload");
    write_plugin(
        &root,
        "com.test.unload",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) { return { type: "list", items: [] }; }
            return { command: command };
        })();
        "#,
    );
    let (load_result, _) = proc.load(&root, "com.test.unload");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();
    let (result, error, _) =
        proc.request(method::PLUGIN_UNLOAD, json!({ "isolate_id": isolate_id }));
    assert!(error.is_none());
    assert_eq!(result, Some(json!({})));
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    assert_eq!(error.unwrap().code, code::PLUGIN_NOT_FOUND);
}

#[test]
fn dedicated_mode_replaces_plugin_on_reload() {
    let mut proc = RuntimeProc::spawn(&["--dedicated"]);
    let root = plugin_root("dedicated");
    write_plugin(
        &root,
        "com.test.dedicated",
        r#"
        var __stewardPlugin = (() => {
            function command(name, input) { return { type: "list", items: [] }; }
            return { command: command };
        })();
        "#,
    );
    let (load_result, _) = proc.load(&root, "com.test.dedicated");
    let first = load_result["isolate_id"].as_u64().unwrap();
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": first, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    assert!(error.is_none());

    // Loading again in dedicated mode replaces the previous isolate.
    let (load_result, _) = proc.load(&root, "com.test.dedicated");
    let second = load_result["isolate_id"].as_u64().unwrap();
    assert_ne!(first, second);
    let (_, error, _) = proc.request(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": first, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    assert_eq!(error.unwrap().code, code::PLUGIN_NOT_FOUND);
}

#[test]
fn invalid_load_params_are_rejected() {
    let mut proc = RuntimeProc::spawn(&[]);
    let (_, error, _) = proc.request(method::PLUGIN_LOAD, json!({ "id": 42 }));
    assert_eq!(error.unwrap().code, code::INVALID_PARAMS);
}

#[test]
fn fs_read_roundtrip_via_cross_process_await() {
    let mut proc = RuntimeProc::spawn(&[]);
    let root = plugin_root("fs-read");
    let file = root.join("data.txt");
    std::fs::write(&file, "hello-e2e").unwrap();
    let file_js = file.to_string_lossy().replace('\\', "/");
    write_plugin(
        &root,
        "com.test.fs-read",
        &format!(
            r#"
            var __stewardPlugin = (() => {{
                async function command(name, input) {{
                    var text = await steward.fs.readFile("{file_js}", "utf8");
                    return {{ type: "list", items: [{{ id: "1", title: text }}] }};
                }}
                return {{ command: command }};
            }})();
            "#
        ),
    );
    let manifest = json!({
        "id": "com.test.fs-read",
        "name": "Test",
        "version": "1.0.0",
        "commands": [
            { "name": "run", "title": "Run", "trigger": { "type": "command" } }
        ],
        "permissions": ["fs.read"],
        "isolation": "shared-pool"
    });
    let (load_result, load_error) = proc.load_with_manifest(&root, "com.test.fs-read", manifest);
    assert!(load_error.is_none(), "load failed: {load_error:?}");
    let isolate_id = load_result["isolate_id"].as_u64().unwrap();

    let (result, error, _) = proc.request_fs_roundtrip(
        method::COMMAND_INVOKE,
        json!({ "isolate_id": isolate_id, "command": "run", "input": "", "deadline_ms": 1000 }),
    );
    assert!(error.is_none(), "fs roundtrip failed: {error:?}");
    let view = result.unwrap()["view"].clone();
    assert_eq!(view["items"][0]["title"], "hello-e2e");
}

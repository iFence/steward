//! Integration tests: drive a real `PluginHost` against the built
//! `steward-plugin-runtime` binary, exactly like the app will.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::json;
use steward_plugin_host::{HostConfig, HostEvent, PluginHost, RouteHit};
use steward_plugin_registry::PluginMeta;

const CALENDAR_JS: &str = r#"
var __stewardPlugin = (() => {
    function command(name, input) {
        if (input.indexOf("toast") >= 0) {
            steward.showToast({ message: "hello from calendar", kind: "success" });
        }
        return { type: "list", items: [
            { id: "today", title: "Today", subtitle: "2026-08-27" },
            { id: "tomorrow", title: "Tomorrow", subtitle: "2026-08-28" }
        ] };
    }
    function select(id) {
        steward.showToast({ message: "selected " + id });
    }
    return { command: command, select: select };
})();
"#;

fn runtime_bin() -> PathBuf {
    if let Ok(path) = std::env::var("STEWARD_PLUGIN_RUNTIME_BIN") {
        return PathBuf::from(path);
    }
    let exe = if cfg!(windows) {
        "steward-plugin-runtime.exe"
    } else {
        "steward-plugin-runtime"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("debug")
        .join(exe)
}

fn temp_root(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("steward-host-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write_plugin(
    root: &Path,
    id: &str,
    isolation: &str,
    permissions: &[&str],
    js: &str,
) -> PluginMeta {
    let dir = root.join(id);
    std::fs::create_dir_all(dir.join("dist")).unwrap();
    let manifest = json!({
        "id": id,
        "name": id,
        "version": "1.0.0",
        "commands": [
            { "name": "calendar", "title": "Calendar", "trigger": { "type": "command" } },
            { "name": "search", "title": "Search", "trigger": { "type": "dynamic" } }
        ],
        "permissions": permissions,
        "isolation": isolation
    });
    std::fs::write(
        dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("dist").join("index.js"), js).unwrap();
    let manifest = steward_plugin_registry::manifest::load_manifest(&dir).unwrap();
    PluginMeta {
        manifest,
        dir: dir.clone(),
        entry: dir.join("dist").join("index.js"),
        icon: None,
        scanned_at: 0,
    }
}

fn host_with(backoff_ms: u64) -> PluginHost {
    PluginHost::new(HostConfig {
        runtime_bin: runtime_bin(),
        base_backoff: Duration::from_millis(backoff_ms),
        max_backoff: Duration::from_secs(2),
    })
}

/// Drain host events until `pred` matches (returning everything collected),
/// panicking after `timeout`.
fn drain_until(
    host: &mut PluginHost,
    timeout: Duration,
    pred: impl Fn(&HostEvent) -> bool,
) -> Vec<HostEvent> {
    let deadline = Instant::now() + timeout;
    let mut collected = Vec::new();
    loop {
        for event in host.drain_events() {
            let matches = pred(&event);
            collected.push(event);
            if matches {
                return collected;
            }
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for host event; got {collected:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Retry `invoke` until the plugin's isolate is registered, then wait for its
/// `CommandResult` and return the collected events.
fn invoke_and_wait(host: &mut PluginHost, gen: u64, hit: RouteHit) -> Vec<HostEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // Drain load responses (and anything else) so the plugin's isolate
        // becomes registered; `invoke` then has an isolate to target.
        host.drain_events();
        if host.invoke(gen, &hit).is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "plugin isolate never became ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    drain_until(host, Duration::from_secs(10), |event| {
        matches!(
            event,
            HostEvent::CommandResult { gen: event_gen, plugin_id, command, .. }
                if *event_gen == gen && plugin_id == &hit.plugin_id && command == &hit.command
        )
    })
}

#[test]
fn route_and_invoke_shared_pool_plugin() {
    let root = temp_root("shared");
    let meta = write_plugin(
        &root,
        "com.example.calendar",
        "shared-pool",
        &["clipboard.write"],
        CALENDAR_JS,
    );
    let mut host = host_with(100);
    host.set_plugins(&[meta]).unwrap();

    let hits = host.query("calendar toast");
    let calendar = hits
        .iter()
        .find(|hit| hit.command == "calendar")
        .expect("expected a calendar hit");
    assert_eq!(calendar.plugin_id, "com.example.calendar");
    assert_eq!(calendar.input, "calendar toast");
    assert_eq!(calendar.deadline_ms, 500);

    let events = invoke_and_wait(&mut host, 1, calendar.clone());
    let result = events
        .iter()
        .find_map(|event| match event {
            HostEvent::CommandResult {
                result: Ok(view), ..
            } => Some(view.clone()),
            _ => None,
        })
        .expect("expected a command result");
    let view = &result["view"];
    assert_eq!(view["type"], "list");
    assert_eq!(view["items"][0]["title"], "Today");
    assert_eq!(view["items"][1]["id"], "tomorrow");

    // The toast notification the plugin emitted rides the same drain.
    let toast = events
        .iter()
        .find_map(|event| match event {
            HostEvent::Toast { params } => Some(params.clone()),
            _ => None,
        })
        .expect("expected the toast notification");
    assert_eq!(toast["message"], "hello from calendar");
    assert_eq!(toast["kind"], "success");

    // Non-matching queries wake no static plugin route (only the dynamic one).
    assert!(!host
        .query("zzz does not match")
        .iter()
        .any(|hit| hit.command == "calendar"));
    // Dynamic routes participate in any query with the short deadline.
    let dynamic = host.query("anything");
    assert!(dynamic
        .iter()
        .any(|hit| hit.command == "search" && hit.deadline_ms == 100));
}

#[test]
fn responses_carry_their_query_generation() {
    let root = temp_root("gens");
    let meta = write_plugin(
        &root,
        "com.example.calendar",
        "shared-pool",
        &[],
        CALENDAR_JS,
    );
    let mut host = host_with(100);
    host.set_plugins(&[meta]).unwrap();

    let hit = host.query("calendar")[0].clone();
    let first = invoke_and_wait(&mut host, 1, hit.clone());
    assert!(first
        .iter()
        .any(|event| { matches!(event, HostEvent::CommandResult { gen: 1, .. }) }));
    let second = invoke_and_wait(&mut host, 2, hit);
    assert!(second
        .iter()
        .any(|event| { matches!(event, HostEvent::CommandResult { gen: 2, .. }) }));
}

#[test]
fn crash_is_detected_and_runtime_restarts() {
    let root = temp_root("crash");
    let meta = write_plugin(
        &root,
        "com.example.calendar",
        "shared-pool",
        &[],
        CALENDAR_JS,
    );
    let mut host = host_with(100);
    host.set_plugins(&[meta]).unwrap();
    let hit = host.query("calendar")[0].clone();
    let events = invoke_and_wait(&mut host, 1, hit.clone());
    assert!(events
        .iter()
        .any(|event| { matches!(event, HostEvent::CommandResult { result: Ok(_), .. }) }));

    // Kill the runtime out from under the host.
    assert!(host.kill_shared_runtime_for_test());
    drain_until(&mut host, Duration::from_secs(5), |event| {
        matches!(event, HostEvent::RuntimeCrashed { plugin_id: None })
    });

    // Backoff is 100 ms; give it a moment and drain for the restart.
    std::thread::sleep(Duration::from_millis(180));
    drain_until(&mut host, Duration::from_secs(5), |event| {
        matches!(event, HostEvent::RuntimeRestarted { plugin_id: None })
    });

    // The plugin is reloaded into the fresh process and answers again.
    let events = invoke_and_wait(&mut host, 2, hit);
    assert!(events.iter().any(|event| {
        matches!(event, HostEvent::CommandResult { gen: 2, result: Ok(view), .. } if view["view"]["type"] == "list")
    }));
}

#[test]
fn dedicated_process_plugin_runs_isolated() {
    let root = temp_root("dedicated");
    let meta = write_plugin(
        &root,
        "com.example.dedicated",
        "dedicated-process",
        &[],
        CALENDAR_JS,
    );
    let mut host = host_with(100);
    host.set_plugins(&[meta]).unwrap();

    let hit = host.query("calendar")[0].clone();
    let events = invoke_and_wait(&mut host, 1, hit);
    assert!(events.iter().any(|event| {
        matches!(event, HostEvent::CommandResult { result: Ok(view), .. } if view["view"]["type"] == "list")
    }));
}

#[test]
fn set_plugins_is_idempotent_for_unchanged_set() {
    let root = temp_root("idempotent");
    let meta = write_plugin(
        &root,
        "com.example.calendar",
        "shared-pool",
        &[],
        CALENDAR_JS,
    );
    let mut host = host_with(100);
    host.set_plugins(std::slice::from_ref(&meta)).unwrap();
    host.set_plugins(&[meta]).unwrap();
    assert!(host.route_count() >= 2);
    assert!(host
        .query("calendar")
        .iter()
        .any(|hit| hit.command == "calendar"));
}

#[test]
fn missing_runtime_binary_is_reported() {
    let root = temp_root("missing-bin");
    let meta = write_plugin(
        &root,
        "com.example.calendar",
        "shared-pool",
        &[],
        CALENDAR_JS,
    );
    let mut host = PluginHost::new(HostConfig {
        runtime_bin: PathBuf::from("definitely-not-a-real-binary"),
        base_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_secs(2),
    });
    assert!(host.set_plugins(&[meta]).is_err());
    // The host degrades gracefully: no routes, no events.
    assert!(host.query("calendar").is_empty());
    assert!(host.drain_events().is_empty());
}

#[test]
fn item_invoke_reaches_select_handler() {
    let root = temp_root("select");
    let meta = write_plugin(
        &root,
        "com.example.calendar",
        "shared-pool",
        &[],
        CALENDAR_JS,
    );
    let mut host = host_with(100);
    host.set_plugins(&[meta]).unwrap();
    let hit = host.query("calendar")[0].clone();
    invoke_and_wait(&mut host, 1, hit.clone());

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        host.drain_events();
        if host
            .invoke_item("com.example.calendar", "calendar", "today")
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "isolate never became ready");
        std::thread::sleep(Duration::from_millis(5));
    }
    let events = drain_until(
        &mut host,
        Duration::from_secs(5),
        |event| matches!(event, HostEvent::Toast { params } if params["message"] == "selected today"),
    );
    assert!(!events.is_empty());
}

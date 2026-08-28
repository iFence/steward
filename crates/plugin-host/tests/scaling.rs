//! M2 scaling regression tests.
//!
//! Steward's M2 acceptance is "cold start and search latency must not grow
//! linearly with the *installed* plugin count — only with the *active* count".
//! These tests drive a real `PluginHost` + `steward-plugin-runtime` against a
//! generated plugin set and assert that:
//!
//! - `set_plugins` only builds routes and spawns the pool (no per-plugin
//!   `plugin.load`, no JS eval) so cold start is roughly independent of N; and
//! - any plugin index — not just the last few that stayed inside the pool's
//!   LRU capacity — resolves a view after a lazy load, proving the host never
//!   sends a stale isolate id for an evicted or killed isolate.
//!
//! The correctness assertions are the primary regression guard; the time
//! ceilings are deliberately loose so CI stays deterministic.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::json;
use steward_plugin_host::{HostConfig, HostEvent, PluginHost};
use steward_plugin_registry::PluginMeta;

const PLUGIN_JS: &str = r#"
var __stewardPlugin = (() => {
    function command(name, input) {
        return { type: "list", items: [
            { id: "one", title: name, subtitle: input, keywords: [] }
        ] };
    }
    return { command: command };
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
    let root = std::env::temp_dir().join(format!(
        "steward-scaling-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Generate `count` shared-pool plugin directories each with a command trigger
/// `cmd{i}` and return their `PluginMeta` (without touching the host).
fn write_plugins(root: &Path, count: usize) -> Vec<PluginMeta> {
    (0..count)
        .map(|i| {
            let id = format!("com.bench.plugin{i:04}");
            let dir = root.join(&id);
            std::fs::create_dir_all(dir.join("dist")).unwrap();
            let manifest = json!({
                "id": id,
                "name": format!("Bench {i}"),
                "version": "1.0.0",
                "commands": [
                    { "name": format!("cmd{i}"), "title": format!("Bench {i}"), "trigger": { "type": "command" } }
                ],
                "permissions": [],
                "isolation": "shared-pool"
            });
            std::fs::write(
                dir.join("plugin.json"),
                serde_json::to_string_pretty(&manifest).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.join("dist").join("index.js"), PLUGIN_JS).unwrap();
            let manifest = steward_plugin_registry::manifest::load_manifest(&dir).unwrap();
            PluginMeta {
                manifest,
                dir: dir.clone(),
                entry: dir.join("dist").join("index.js"),
                icon: None,
                scanned_at: 0,
            }
        })
        .collect()
}

fn host_with(backoff_ms: u64) -> PluginHost {
    PluginHost::new(HostConfig {
        runtime_bin: runtime_bin(),
        base_backoff: Duration::from_millis(backoff_ms),
        max_backoff: Duration::from_secs(2),
    })
}

/// Drain host events until one matches `pred`, returning everything collected.
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

/// Resolve the view for `cmd{i}` from a N-plugin host, proving the plugin is
/// reachable even though it was never eagerly loaded.
fn command_views(host: &mut PluginHost, indices: &[usize]) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for i in indices {
        let name = format!("cmd{i}");
        let hit = host
            .query(&name)
            .into_iter()
            .find(|hit| hit.command == name)
            .unwrap_or_else(|| panic!("no route hit for {name}"));
        assert!(
            host.invoke(1, &hit).is_some(),
            "host refused to dispatch command for {name}"
        );
        let events = drain_until(host, Duration::from_secs(20), |event| {
            matches!(
                event,
                HostEvent::CommandResult { plugin_id, command, result: Ok(_), .. }
                    if plugin_id == &hit.plugin_id && command == &hit.command
            )
        });
        let view = events
            .iter()
            .find_map(|event| match event {
                HostEvent::CommandResult {
                    result: Ok(result), ..
                } => Some(result["view"].clone()),
                _ => None,
            })
            .expect("expected a command result");
        out.push((name.clone(), view));
    }
    out
}

#[test]
fn arbitrary_index_plugin_is_available_at_scale() {
    // N is comfortably above the shared pool's capacity (8): if the host were
    // to keep stale isolate ids for evicted plugins, `cmd50` / `cmd75` /
    // `cmd99` would come back PLUGIN_NOT_FOUND. Lazy loading must make any
    // index reachable.
    let n = 100;
    let root = temp_root("scale-available");
    let metas = write_plugins(&root, n);
    let mut host = host_with(100);
    host.set_plugins(&metas).unwrap();

    // Routing is built up-front from the cache/manifests: all N commands visible.
    assert_eq!(host.route_count(), n);

    for (name, view) in command_views(&mut host, &[0, 50, 75, 99]) {
        assert_eq!(
            view["type"], "list",
            "plugin {name} returned a non-list view"
        );
        assert_eq!(
            view["items"][0]["title"], name,
            "plugin {name} returned the wrong view"
        );
        assert_eq!(
            view["items"][0]["subtitle"], name,
            "plugin {name} lost its input"
        );
    }
}

#[test]
fn cold_start_and_single_query_cost_do_not_grow_linearly() {
    // set_plugins must not eagerly load every plugin (no per-plugin process
    // spawn, no JS eval), so a 100-plugin set is not dramatically slower to
    // "cold start" than a 1-plugin set. We assert a generous ceiling rather
    // than a strict ratio to keep CI deterministic; the correctness assertions
    // above are the primary regression guard.
    let root = temp_root("scale-cost");

    let metas_one = write_plugins(&root, 1);
    let t_one_start = Instant::now();
    let mut host_one = host_with(100);
    host_one.set_plugins(&metas_one).unwrap();
    let elapsed_one = t_one_start.elapsed();

    let metas_many = write_plugins(&root, 100);
    let t_many_start = Instant::now();
    let mut host_many = host_with(100);
    host_many.set_plugins(&metas_many).unwrap();
    let elapsed_many = t_many_start.elapsed();

    // Loose ceilings: even a heavily loaded CI box keeps this well under.
    assert!(
        elapsed_one < Duration::from_secs(5),
        "set_plugins(1) took {elapsed_one:?}"
    );
    assert!(
        elapsed_many < Duration::from_secs(10),
        "set_plugins(100) took {elapsed_many:?}"
    );
    // A per-plugin process spawn at set_plugins would blow this up by ~100x.
    assert!(
        elapsed_many < elapsed_one * 20 + Duration::from_secs(2),
        "cold start grew from {elapsed_one:?} to {elapsed_many:?} with 100x plugins"
    );

    // A single cold command resolves at scale, and the first-view latency stays
    // bounded (dominated by one JS eval, not by the installed count).
    let start = Instant::now();
    let views = command_views(&mut host_many, &[99]);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "first cold query took too long"
    );
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].1["type"], "list");
}

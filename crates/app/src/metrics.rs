//! Opt-in JSONL metrics for release acceptance and performance diagnostics.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();

pub(crate) fn init() {
    let sink = std::env::var_os("STEWARD_METRICS_PATH").and_then(|path| {
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => Some(Mutex::new(file)),
            Err(error) => {
                tracing::warn!(error = %error, "open Steward metrics file");
                None
            }
        }
    });
    let _ = SINK.set(sink);
}

pub(crate) fn duration(event: &str, elapsed: Duration, fields: serde_json::Value) {
    let mut record = fields.as_object().cloned().unwrap_or_default();
    record.insert("event".into(), serde_json::Value::String(event.into()));
    record.insert(
        "duration_ms".into(),
        serde_json::json!(elapsed.as_secs_f64() * 1_000.0),
    );
    write_record(&record);
}

pub(crate) fn value(event: &str, value: u64, fields: serde_json::Value) {
    let mut record = fields.as_object().cloned().unwrap_or_default();
    record.insert("event".into(), serde_json::Value::String(event.into()));
    record.insert("value".into(), serde_json::json!(value));
    write_record(&record);
}

fn write_record(record: &serde_json::Map<String, serde_json::Value>) {
    let Some(Some(sink)) = SINK.get() else {
        return;
    };
    let mut record = record.clone();
    record.insert(
        "timestamp_ms".into(),
        serde_json::json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()),
    );
    if let Ok(mut file) = sink.lock() {
        if let Ok(line) = serde_json::to_string(&record) {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

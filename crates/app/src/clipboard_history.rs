//! Host-side clipboard history capture.
//!
//! A background thread polls the system clipboard (`arboard`) and records each
//! observed textual change into a dedicated SQLite table. Consecutive
//! duplicates are collapsed (copying the same text twice is one entry). Each
//! observed change pushes the newest [`CLIPBOARD_HISTORY_LIMIT`] entries over a
//! channel, which the foreground poll task forwards to `PluginHost` so plugins
//! with the `clipboard.history` permission can read the snapshot during
//! `command.invoke`.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use crossbeam_channel::Sender;
use steward_ipc_protocol::ClipboardEntry;
use steward_storage::Storage;

/// How often the clipboard is polled.
const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// Number of history entries kept/forwarded to plugins.
const CLIPBOARD_HISTORY_LIMIT: i64 = 50;
/// Upper bound on a single clipboard text length we record (avoid huge blobs).
const MAX_TEXT_LEN: usize = 100_000;

/// A running clipboard watcher. Dropping it signals the background thread to
/// let the OS reap it (the thread is detached by dropping its join handle; the
/// process termination stops it). A unit struct: the app only needs it alive
/// for the process lifetime.
pub(crate) struct ClipboardWatcher;

impl ClipboardWatcher {
    /// Start the watcher against `data_dir` (host `%APPDATA%/Steward`). Newest
    /// entries are pushed to `tx`.
    pub(crate) fn spawn(data_dir: PathBuf, tx: Sender<Vec<ClipboardEntry>>) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        // Detached: dropping the `JoinHandle` lets the thread keep running for
        // the app's lifetime; on process exit it is killed by the OS.
        let _handle = thread::spawn(move || run_watcher(data_dir, tx, stop_flag));
        Self
    }
}

/// The watcher thread body. Owns the clipboard and a private SQLite connection
/// (rusqlite `Connection` is `Send`, so it can be moved here and used only by
/// this thread; WAL lets it coexist with the app's storage connection).
fn run_watcher(data_dir: PathBuf, tx: Sender<Vec<ClipboardEntry>>, stop: Arc<AtomicBool>) {
    let storage = match Storage::open_at(&data_dir) {
        Ok(storage) => storage,
        Err(error) => {
            eprintln!("[steward] clipboard watcher: cannot open storage: {error:#}");
            return;
        }
    };
    // Seed the host with whatever history already exists (cold start).
    let _ = tx.send(recent_entries(&storage));

    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            eprintln!("[steward] clipboard watcher: cannot open clipboard: {error}");
            return;
        }
    };

    // Tracks the last text we recorded so consecutive duplicates are ignored.
    let mut last_text: Option<String> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(text) = clipboard.get_text() {
            let text = text.trim_end().to_string();
            if !text.is_empty()
                && text.len() <= MAX_TEXT_LEN
                && last_text.as_deref() != Some(text.as_str())
            {
                last_text = Some(text.clone());
                let now = unix_seconds();
                if let Err(error) = storage.insert_clipboard(&text, now) {
                    eprintln!("[steward] clipboard watcher: insert failed: {error:#}");
                } else if let Err(error) = storage.trim_clipboard(CLIPBOARD_HISTORY_LIMIT) {
                    eprintln!("[steward] clipboard watcher: trim failed: {error:#}");
                }
                let _ = tx.send(recent_entries(&storage));
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Read the newest [`CLIPBOARD_HISTORY_LIMIT`] entries as the IPC wire type.
fn recent_entries(storage: &Storage) -> Vec<ClipboardEntry> {
    storage
        .recent_clipboard(CLIPBOARD_HISTORY_LIMIT)
        .map(|rows| {
            rows.into_iter()
                .map(|row| ClipboardEntry {
                    id: row.id.to_string(),
                    text: row.text,
                    copied_at: row.copied_at,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_entries_roundtrip_through_storage() {
        let dir = std::env::temp_dir().join(format!(
            "steward-clip-watch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let storage = Storage::open_at(&dir).unwrap();
        storage.insert_clipboard("hello", 100).unwrap();
        storage.insert_clipboard("world", 200).unwrap();
        let entries = recent_entries(&storage);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "world");
        assert_eq!(entries[1].text, "hello");
        assert_eq!(entries[0].copied_at, 200);
        std::fs::remove_dir_all(&dir).ok();
    }
}

//! Real-time filesystem change notifications (Windows `ReadDirectoryChangesW`).
//!
//! The USN Journal is the incremental-maintenance fast path, but reading it
//! requires an elevated handle to the raw volume. Steward normally runs with the
//! user's token, so a walk-built index has no journal at all. This watcher is
//! the non-privileged complement: it opens each indexed root as a directory and
//! asks Windows to report every create/delete/rename/write under it, which needs
//! only read access to the root.
//!
//! One blocking thread per root keeps the implementation simple and lets a
//! single watch cover a whole volume (`bWatchSubtree = TRUE`). The kernel
//! delivers a buffer of `FILE_NOTIFY_INFORMATION` records; [`parse_notify`]
//! turns them into root-relative [`FsChange`]s. A buffer overflow means changes
//! were dropped, so the batch is flagged [`WatchBatch::overflow`] and the caller
//! reconciles (a USN catch-up, or a rebuild when there is no journal).
//!
//! The USN path and this one are deliberately both idempotent when applied to
//! the index, so the same change arriving through both is harmless.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::CancelIoEx;

/// Windows `ERROR_OPERATION_ABORTED`: the read was cancelled by [`CancelIoEx`].
const ERROR_OPERATION_ABORTED: u32 = 995;

/// Change buffer per root. Large enough that an ordinary burst (a `cargo build`,
/// an unzip) does not overflow; when it does, the batch is flagged and the
/// caller reconciles from the USN journal (or a rebuild).
const NOTIFY_BUFFER_BYTES: usize = 256 * 1024;

/// The kind of change a [`FsChange`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsAction {
    Created,
    Removed,
    /// Size or last-write time changed, or a directory gained content.
    Modified,
    /// The "from" half of a rename; the matching [`FsAction::RenamedNew`]
    /// follows in the same batch.
    RenamedOld,
    /// The "to" half of a rename.
    RenamedNew,
}

/// One filesystem change, with an absolute (rooted) path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsChange {
    pub action: FsAction,
    pub path: PathBuf,
}

/// A batch of changes read from one root, or an overflow notice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchBatch {
    pub changes: Vec<FsChange>,
    /// The OS buffer overflowed and changes were lost. The caller must
    /// reconcile (USN catch-up or rebuild); the changes in this batch are
    /// still valid, just incomplete.
    pub overflow: bool,
}

/// A running set of recursive directory watches, one thread per root.
pub struct DirectoryWatcher {
    stop: Arc<AtomicBool>,
    /// Threads still in their read loop. Drop spins on this while cancelling,
    /// which closes the race where a thread issues its first `ReadDirectoryChangesW`
    /// just after the first `CancelIoEx` and would otherwise block the join.
    running: Arc<AtomicUsize>,
    handles: Vec<usize>,
    threads: Vec<JoinHandle<()>>,
    events: Receiver<WatchBatch>,
}

/// Decrements [`DirectoryWatcher::running`] when its thread exits.
struct RunningGuard(Arc<AtomicUsize>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl DirectoryWatcher {
    /// Start watching every root that can be opened. Roots that cannot are
    /// skipped: a single unwatchable root (a disconnected network share) must
    /// not disable the others.
    pub fn watch(roots: &[PathBuf]) -> Self {
        let (events, rx) = unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        let mut threads = Vec::new();
        for root in roots {
            let Some(handle) = open_directory(root) else {
                continue;
            };
            let raw = handle.0 as usize;
            handles.push(raw);
            let path = root.clone();
            let events = events.clone();
            let stop = Arc::clone(&stop);
            let thread_running = Arc::clone(&running);
            running.fetch_add(1, Ordering::AcqRel);
            match std::thread::Builder::new()
                .name(format!("steward-watch-{}", root.display()))
                .spawn(move || {
                    let _guard = RunningGuard(thread_running);
                    watch_root(raw, path, stop, events);
                }) {
                Ok(thread) => threads.push(thread),
                Err(_) => {
                    running.fetch_sub(1, Ordering::AcqRel);
                    // SAFETY: the handle is owned here and was never handed to
                    // a thread.
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    handles.pop();
                }
            }
        }
        drop(events);
        Self {
            stop,
            running,
            handles,
            threads,
            events: rx,
        }
    }

    /// Whether any root is actually being watched.
    pub fn is_watching(&self) -> bool {
        !self.handles.is_empty()
    }

    /// Take the next batch without blocking, if one is ready.
    pub fn try_recv(&self) -> Option<WatchBatch> {
        match self.events.try_recv() {
            Ok(batch) => Some(batch),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Cancel repeatedly until every thread has left its read loop: a thread
        // may not have reached `ReadDirectoryChangesW` when the first cancel
        // lands, and an uncancelled blocking read would hang the join below.
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.running.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            for raw in &self.handles {
                // SAFETY: every handle was opened by `CreateFileW` and is still
                // open; cancelling pending I/O on it is safe from any thread.
                unsafe {
                    let _ = CancelIoEx(HANDLE(*raw as *mut core::ffi::c_void), None);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        for raw in self.handles.drain(..) {
            // SAFETY: the owning thread has exited, so this is the only handle.
            unsafe {
                let _ = CloseHandle(HANDLE(raw as *mut core::ffi::c_void));
            }
        }
    }
}

/// Open a directory for change notification. `FILE_FLAG_BACKUP_SEMANTICS` is
/// what allows a *directory* handle at all.
fn open_directory(path: &Path) -> Option<HANDLE> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .ok()?;
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    Some(handle)
}

/// Block on `ReadDirectoryChangesW` until the watcher is dropped, forwarding
/// batches as they arrive.
///
/// The handle arrives as a `usize` because `HANDLE` wraps a raw pointer and is
/// therefore not `Send`; it is rebuilt here on the thread that owns the read.
fn watch_root(raw: usize, root: PathBuf, stop: Arc<AtomicBool>, events: Sender<WatchBatch>) {
    let handle = HANDLE(raw as *mut core::ffi::c_void);
    // Names only, deliberately: size/last-write notifications fire for every
    // byte written anywhere on the volume, and search is name-based. Watching
    // just creates, deletes and renames keeps a busy disk from drowning the
    // index in metadata refreshes while still making the drop-down "instant".
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME;
    let mut buffer = vec![0u8; NOTIFY_BUFFER_BYTES];
    while !stop.load(Ordering::Acquire) {
        let mut returned = 0u32;
        // SAFETY: `buffer` is a valid, exclusively borrowed buffer, and the
        // handle is kept open by the `DirectoryWatcher` for the whole loop.
        let result = unsafe {
            ReadDirectoryChangesW(
                handle,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                true,
                filter,
                Some(&mut returned),
                None,
                None,
            )
        };
        match result {
            Ok(()) => {
                if returned == 0 {
                    // An empty successful read means the buffer overflowed.
                    if events.send(WatchBatch::overflow()).is_err() {
                        break;
                    }
                    continue;
                }
                let mut changes = Vec::new();
                parse_notify(&buffer[..returned as usize], &root, &mut changes);
                if !changes.is_empty()
                    && events
                        .send(WatchBatch {
                            changes,
                            overflow: false,
                        })
                        .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let code = error.code().0 as u32 & 0xFFFF;
                if code == ERROR_OPERATION_ABORTED {
                    // Cancelled without `stop` being set (a handle torn down by
                    // the OS, e.g. a removed volume): nothing left to watch.
                    break;
                }
                if events.send(WatchBatch::overflow()).is_err() {
                    break;
                }
                // Do not spin on a persistent error; the caller's reconcile
                // covers anything missed while this root is unreadable.
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

impl WatchBatch {
    /// A batch carrying only the overflow flag.
    pub fn overflow() -> Self {
        Self {
            changes: Vec::new(),
            overflow: true,
        }
    }
}

/// Decode a `FILE_NOTIFY_INFORMATION` chain into absolute paths.
///
/// The buffer is a linked list: each record starts with `NextEntryOffset`,
/// `Action` and `FileNameLength` (all `u32`), followed by `FileNameLength` bytes
/// of UTF-16 without a terminator. `NextEntryOffset == 0` marks the end.
fn parse_notify(buffer: &[u8], root: &Path, out: &mut Vec<FsChange>) {
    let mut offset = 0usize;
    loop {
        if offset + 12 > buffer.len() {
            break;
        }
        let next = u32::from_le_bytes(
            buffer[offset..offset + 4]
                .try_into()
                .expect("4 bytes checked"),
        ) as usize;
        let action = u32::from_le_bytes(
            buffer[offset + 4..offset + 8]
                .try_into()
                .expect("4 bytes checked"),
        );
        let name_bytes = u32::from_le_bytes(
            buffer[offset + 8..offset + 12]
                .try_into()
                .expect("4 bytes checked"),
        ) as usize;
        let name_at = offset + 12;
        if name_at + name_bytes > buffer.len() || !name_bytes.is_multiple_of(2) {
            break;
        }
        let utf16: Vec<u16> = buffer[name_at..name_at + name_bytes]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let name = String::from_utf16_lossy(&utf16);
        if let Some(action) = fs_action(action) {
            if !name.is_empty() {
                out.push(FsChange {
                    action,
                    path: root.join(&name),
                });
            }
        }
        if next == 0 {
            break;
        }
        offset += next;
    }
}

/// Map a `FILE_ACTION_*` value; `None` for an action the index does not model.
fn fs_action(action: u32) -> Option<FsAction> {
    match action {
        1 => Some(FsAction::Created),
        2 => Some(FsAction::Removed),
        3 => Some(FsAction::Modified),
        4 => Some(FsAction::RenamedOld),
        5 => Some(FsAction::RenamedNew),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn notify_record(next: u32, action: u32, name: &str) -> Vec<u8> {
        let utf16: Vec<u16> = name.encode_utf16().collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&next.to_le_bytes());
        bytes.extend_from_slice(&action.to_le_bytes());
        bytes.extend_from_slice(&((utf16.len() * 2) as u32).to_le_bytes());
        for unit in utf16 {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn notify_chain_is_decoded_with_absolute_paths() {
        let mut first = notify_record(0, 1, "new.txt");
        let second = notify_record(0, 2, "old.txt");
        // Point the first record at the second.
        let first_len = first.len() as u32;
        first[0..4].copy_from_slice(&first_len.to_le_bytes());
        first.extend_from_slice(&second);

        let mut out = Vec::new();
        parse_notify(&first, Path::new("C:\\root"), &mut out);
        assert_eq!(
            out,
            vec![
                FsChange {
                    action: FsAction::Created,
                    path: PathBuf::from("C:\\root\\new.txt"),
                },
                FsChange {
                    action: FsAction::Removed,
                    path: PathBuf::from("C:\\root\\old.txt"),
                },
            ]
        );
    }

    #[test]
    fn truncated_and_unknown_records_are_ignored() {
        let mut out = Vec::new();
        // Unknown action 99.
        parse_notify(&notify_record(0, 99, "x"), Path::new("C:\\"), &mut out);
        assert!(out.is_empty());
        // Truncated name.
        let mut record = notify_record(0, 1, "abcdef");
        record.truncate(record.len() - 2);
        parse_notify(&record, Path::new("C:\\"), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_watcher_reports_create_rename_and_delete() {
        let root = std::env::temp_dir().join(format!(
            "steward-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let watcher = DirectoryWatcher::watch(std::slice::from_ref(&root));
        assert!(watcher.is_watching());
        // The watch threads arm asynchronously; give them a moment so the
        // first create is not missed by a test that runs immediately.
        std::thread::sleep(Duration::from_millis(250));

        let created = root.join("hello.txt");
        std::fs::write(&created, b"hi").unwrap();
        let renamed = root.join("renamed.txt");
        std::fs::rename(&created, &renamed).unwrap();
        std::fs::remove_file(&renamed).unwrap();

        // Collect for a bounded time: the notifications are asynchronous.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen_add = false;
        let mut seen_remove = false;
        while Instant::now() < deadline && !(seen_add && seen_remove) {
            if let Some(batch) = watcher.try_recv() {
                for change in batch.changes {
                    if change.path == created && change.action == FsAction::Created {
                        seen_add = true;
                    }
                    if change.path == renamed
                        && matches!(change.action, FsAction::Removed | FsAction::RenamedOld)
                    {
                        seen_remove = true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(watcher);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(seen_add, "the create was not reported");
        assert!(seen_remove, "the delete/rename was not reported");
    }
}

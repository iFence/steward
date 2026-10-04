//! Full-disk file index plumbing for the launcher.
//!
//! The index itself lives in `steward-core-engine::file_index`; this module is
//! the app-side lifecycle around it:
//!
//! - **load**: adopt the persisted snapshot synchronously (one SQLite read plus
//!   a decode), so a cold start shows files immediately.
//! - **build**: pick the roots, try each volume's NTFS `$MFT` fast path, fall back
//!   to a directory walk, and run it all on a worker thread.
//! - **catch up**: after a build, replay the USN Journal from the stored cursor.
//! - **search**: run a query off the UI thread, cancelling the previous one, and
//!   deliver hits back through a channel.
//!
//! Everything is synchronous and channel-based on purpose: the launcher's event
//! loop already polls channels for app scans, icons and plugins, so one more data
//! source does not justify a second async runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use steward_core_engine::file_index::{
    self, apply_usn_records, parse_roots, search_filtered, Cancel, Emit, EntryInfo, FileDb,
    FileDbBuilder, FileHit, IndexBackend, IndexProgress, JournalState, KindFilter, ScanOptions,
    SearchOptions,
};
#[cfg(target_os = "windows")]
use steward_core_engine::file_index::{apply_fs_changes, DirectoryWatcher, FsAction, FsChange};

/// Records per search request from the launcher.
pub(crate) const FILE_RESULT_LIMIT: usize = 12;

/// How often a live update is written back to the snapshot.
///
/// A build is persisted at once, but a real-time change only marks the snapshot
/// dirty: writing the whole blob (and its SQLite row) on every create/delete on
/// a busy disk would cost far more than the update. Nothing is lost if the app
/// exits before it flushes — the next start replays the journal, or rebuilds.
const LIVE_PERSIST_INTERVAL: Duration = Duration::from_secs(30);

/// Default excluded directory names on top of the walker's own list.
const EXTRA_EXCLUSIONS: [&str; 4] = ["node_modules", ".git", "winsxs", "$recycle.bin"];

/// A finished build: the index, the roots it covered, and the number of names
/// that had to be truncated.
pub(crate) type BuildOutput = (FileDb, Vec<(PathBuf, IndexBackend)>, usize);

/// A finished build plus the live helper session, when the helper built it.
pub(crate) struct BuildResult {
    pub(crate) output: BuildOutput,
    pub(crate) session: Option<crate::file_index_helper::HelperSession>,
}

/// What one USN catch-up pass applied to the index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ReplayCounts {
    pub(crate) replayed: usize,
    pub(crate) created: usize,
    pub(crate) removed: usize,
    pub(crate) renamed: usize,
}

/// A finished index build or load.
pub(crate) enum IndexEvent {
    /// Periodic progress while enumerating.
    Progress(IndexProgress),
    /// The build finished. The worker has already streamed the snapshot into
    /// SQLite itself (see [`SnapshotWriter`]), so no index-sized payload
    /// travels through this channel.
    Ready {
        records: usize,
        directories: usize,
        journal: bool,
        backends: Vec<(PathBuf, IndexBackend)>,
        elapsed: Duration,
    },
    /// A catch-up pass applied replayed journal changes.
    Updated {
        records: usize,
        journal: bool,
        replayed: usize,
        created: usize,
        removed: usize,
        renamed: usize,
    },
    /// Nothing could be indexed.
    Failed(String),
    /// A catch-up pass finished with nothing to do (the journal is caught up).
    Idle,
    /// The real-time watcher's change buffer overflowed: changes were lost, so
    /// the index must be reconciled (caught up or rebuilt).
    Reconcile,
    /// The USN journal can no longer be replayed (recreated, wrapped, or a
    /// cursor ahead of the journal): only a full rebuild can restore the index,
    /// so catch-up must not be retried on this volume.
    Rebuild,
}

/// A search reply, paired with the generation that asked for it. The generation
/// is what lets the launcher tell a reply for the query now in the box from one
/// for a query the user has already typed past: ile_index::search is debounced,
/// so a reply in hand does not imply it answers the current input.
pub(crate) struct SearchReply {
    pub(crate) generation: u64,
    pub(crate) hits: Vec<FileHit>,
    pub(crate) elapsed: Duration,
}

/// Commands sent to the index worker.
enum Command {
    Build(ScanOptions),
    ReplayJournal,
    Search {
        generation: u64,
        filter: file_index::Filter,
        kind: KindFilter,
        cancel: Arc<Cancel>,
    },
}

/// The launcher's handle on the file index.
pub(crate) struct FileIndex {
    commands: crossbeam_channel::Sender<Command>,
    events: crossbeam_channel::Receiver<IndexEvent>,
    /// Search replies for the launcher's poll task.
    pub(crate) replies: crossbeam_channel::Receiver<SearchReply>,
    /// Search generation, bumped per request so stale replies are dropped.
    pub(crate) generation: u64,
    /// Cancel handle of the running search.
    cancel: Option<Arc<Cancel>>,
    /// Roots this index covers.
    roots: Vec<PathBuf>,
    /// Whether a build or catch-up pass is running.
    building: bool,
    /// Whether the current index has at least one live record. Mirrors the
    /// worker's index without sharing it: the worker owns the `FileDb` outright,
    /// so a live update never has to clone it.
    ready: bool,
    /// Records in the current index.
    pub(crate) records: usize,
    /// Whether the current index carries a usable USN journal cursor.
    journal: bool,
    /// Set when the watcher lost changes and the index needs a reconcile.
    needs_reconcile: bool,
    /// Set when a volume's journal can never be replayed again and the only fix
    /// is a full rebuild.
    needs_rebuild: bool,
    /// The last failure message, so a repeating one (an unreadable volume polled
    /// every couple of seconds) is logged once instead of on every tick.
    last_failure: Option<String>,
}

impl FileIndex {
    /// Start the worker, adopting `snapshot` when it decodes.
    ///
    /// The snapshot is decoded on the calling thread; the worker owns the index
    /// (and its own SQLite connection) from then on, so a rebuild or catch-up
    /// never has to clone the index to persist it.
    pub(crate) fn start(snapshot: Option<Vec<u8>>, configured_roots: &[String]) -> Self {
        let (command_tx, command_rx) = crossbeam_channel::unbounded::<Command>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<IndexEvent>();
        let (reply_tx, reply_rx) = crossbeam_channel::unbounded::<SearchReply>();

        let roots = if configured_roots.is_empty() {
            default_roots()
        } else {
            parse_roots(configured_roots)
        };

        // Adopt the persisted snapshot on the calling thread so `is_ready` and
        // `supports_journal` are correct before the first event lands. The
        // decoded index then moves into the worker, which owns it exclusively
        // for the process lifetime 鈥?that ownership is what lets a live update
        // mutate in place instead of cloning the whole index.
        let (loaded, ready, journal) = match snapshot {
            Some(blob) => match file_index::persist::decode(&blob, unix_seconds()) {
                Ok(index) => {
                    let journal = index.journals().values().any(|state| state.journal_id != 0);
                    let ready = !index.is_empty();
                    eprintln!(
                        "file index: loaded {} records from the snapshot",
                        index.len()
                    );
                    (Some(index), ready, journal)
                }
                Err(error) => {
                    eprintln!("file index: snapshot rejected ({error}); a rebuild will follow");
                    (None, false, false)
                }
            },
            None => (None, false, false),
        };
        let records = loaded.as_ref().map_or(0, FileDb::len);

        // Real-time changes: watch every indexed root. Reading the USN journal
        // needs an elevated handle that a normal launch does not have, so this
        // is what makes a create or delete visible without a restart. The
        // worker owns the watcher (and its threads) for the process lifetime.
        #[cfg(target_os = "windows")]
        let watcher = DirectoryWatcher::watch(&roots);
        #[cfg(not(target_os = "windows"))]
        let watcher = ();

        std::thread::Builder::new()
            .name("steward-file-index".into())
            .spawn(move || worker_loop(command_rx, event_tx, reply_tx, loaded, watcher))
            .expect("spawn the file index worker");

        Self {
            commands: command_tx,
            events: event_rx,
            replies: reply_rx,
            generation: 0,
            cancel: None,
            roots,
            building: false,
            ready,
            records,
            journal,
            needs_reconcile: false,
            needs_rebuild: false,
            last_failure: None,
        }
    }

    /// Whether an index with at least one record is available.
    pub(crate) fn is_ready(&self) -> bool {
        self.ready
    }

    /// Whether a build or catch-up pass is running.
    pub(crate) fn is_building(&self) -> bool {
        self.building
    }

    /// Whether the loaded index carries a usable USN journal cursor.
    ///
    /// A `$MFT`-built index has one per volume; a directory-walk index (the
    /// non-elevated fallback) has none, so it cannot be caught up and must be
    /// rebuilt to fold in changes made while Steward was not running.
    pub(crate) fn supports_journal(&self) -> bool {
        self.journal
    }

    /// Ask for a full build. Ignored while one is already running.
    pub(crate) fn request_build(&mut self) {
        if self.building {
            return;
        }
        let mut options = ScanOptions::for_roots(self.roots.iter().cloned());
        for extra in EXTRA_EXCLUSIONS {
            options = options.exclude_dir(extra);
        }
        // The currently loaded index is the best available estimate of how many
        // records the rebuild will produce, and pre-sizing the arena avoids
        // repeatedly reallocating a multi-hundred-megabyte buffer.
        options.records_hint = (self.records > 0).then_some(self.records);
        self.building = true;
        let _ = self.commands.send(Command::Build(options));
    }

    /// Ask for a USN catch-up pass over the current index.
    pub(crate) fn request_catch_up(&mut self) {
        if self.building || !self.is_ready() {
            return;
        }
        self.building = true;
        let _ = self.commands.send(Command::ReplayJournal);
    }

    /// Drain worker events. Returns `true` when the index changed, so the caller
    /// re-runs the visible query.
    pub(crate) fn poll_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                IndexEvent::Progress(progress) => {
                    if progress.entries >= 25_000 && progress.entries % 25_000 == 0 {
                        eprintln!(
                            "file index: {} records in {} directories ({})",
                            progress.entries,
                            progress.directories,
                            progress.current.display()
                        );
                    }
                }
                IndexEvent::Ready {
                    records,
                    directories,
                    journal,
                    backends,
                    elapsed,
                } => {
                    self.building = false;
                    self.records = records;
                    self.ready = records > 0;
                    self.journal = journal;
                    self.last_failure = None;
                    changed = true;
                    let covered: Vec<String> = backends
                        .iter()
                        .map(|(path, backend)| format!("{} [{backend}]", path.display()))
                        .collect();
                    eprintln!(
                        "file index: {records} records / {directories} directories from {} in {:.1}s",
                        covered.join(", "),
                        elapsed.as_secs_f32()
                    );
                }
                IndexEvent::Updated {
                    records,
                    journal,
                    replayed,
                    created,
                    removed,
                    renamed,
                } => {
                    self.building = false;
                    self.records = records;
                    self.ready = records > 0;
                    self.journal = journal;
                    self.last_failure = None;
                    changed = true;
                    eprintln!(
                        "file index: applied {replayed} changes (+{created} -{removed} ~{renamed})"
                    );
                }
                IndexEvent::Failed(message) => {
                    self.building = false;
                    // A volume that stays unreadable would otherwise log on
                    // every catch-up tick; only a change is worth reporting.
                    if self.last_failure.as_deref() != Some(message.as_str()) {
                        eprintln!("file index: {message}");
                        self.last_failure = Some(message);
                    }
                }
                IndexEvent::Idle => {
                    self.building = false;
                    self.last_failure = None;
                }
                IndexEvent::Reconcile => {
                    self.needs_reconcile = true;
                }
                IndexEvent::Rebuild => {
                    self.building = false;
                    self.needs_rebuild = true;
                }
            }
        }
        changed
    }

    /// Take the "the index lost changes and must be reconciled" flag, if set.
    pub(crate) fn take_reconcile_request(&mut self) -> bool {
        std::mem::take(&mut self.needs_reconcile)
    }

    /// Take the "this volume's journal is dead, rebuild" flag, if set.
    pub(crate) fn take_rebuild_request(&mut self) -> bool {
        std::mem::take(&mut self.needs_rebuild)
    }

    /// Run `query` against the index, cancelling any search still in flight.
    pub(crate) fn search(&mut self, query: &str, kind: KindFilter) -> bool {
        if !self.is_ready() {
            return false;
        }
        if let Some(previous) = self.cancel.take() {
            previous.cancel();
        }
        let cancel = Arc::new(Cancel::new());
        self.cancel = Some(Arc::clone(&cancel));
        self.generation += 1;
        self.commands
            .send(Command::Search {
                generation: self.generation,
                filter: file_index::parse_filter(query),
                kind,
                cancel,
            })
            .is_ok()
    }
}

/// Streams snapshots from the index worker into SQLite.
///
/// The worker owns the `FileDb` exclusively, so it is the only place that can
/// encode a snapshot without copying the index. It opens its own WAL connection
/// (lazily, on the first persist) and streams the encoding straight into the
/// `settings_blob` row, so neither thread ever holds an index-sized `Vec`.
struct SnapshotWriter {
    storage: Option<steward_storage::Storage>,
    /// Set when the database could not be opened; persistence is then skipped
    /// for the rest of the session instead of retrying (and logging) on every
    /// batch.
    unavailable: bool,
}

impl SnapshotWriter {
    fn new() -> Self {
        Self {
            storage: None,
            unavailable: false,
        }
    }

    /// Write `db` as the persisted snapshot, if persistence is available.
    fn store(&mut self, db: &FileDb, truncated_names: usize) {
        if self.unavailable {
            return;
        }
        if self.storage.is_none() {
            match steward_storage::Storage::open() {
                Ok(storage) => self.storage = Some(storage),
                Err(error) => {
                    eprintln!("file index: cannot open the snapshot database: {error:#}");
                    self.unavailable = true;
                    return;
                }
            }
        }
        let storage = self.storage.as_ref().expect("opened just above");
        let taken_at = unix_seconds();
        let length = file_index::persist::encoded_len(db);
        let result =
            storage.set_setting_blob_streaming(file_index::persist::SETTING_KEY, length, |sink| {
                file_index::persist::encode_to(db, truncated_names, taken_at, sink)
            });
        if let Err(error) = result {
            eprintln!("file index: failed to persist the snapshot: {error:#}");
        }
    }
}

/// The worker thread: builds, replays, answers searches, and applies the
/// real-time filesystem changes the watcher reports.
fn worker_loop(
    commands: crossbeam_channel::Receiver<Command>,
    events: crossbeam_channel::Sender<IndexEvent>,
    replies: crossbeam_channel::Sender<SearchReply>,
    mut index: Option<FileDb>,
    #[cfg(target_os = "windows")] watcher: DirectoryWatcher,
) {
    // How long the worker is willing to sit idle before checking the watcher.
    // A change notification is only as useful as this delay is short.
    const WATCH_POLL: Duration = Duration::from_millis(150);
    // Changes seen while the first build is still running, applied as soon as
    // an index exists. Capped so a long build on a busy disk cannot grow it
    // without bound; past the cap a rebuild is requested instead.
    #[cfg(target_os = "windows")]
    const MAX_PENDING_CHANGES: usize = 100_000;
    #[cfg(target_os = "windows")]
    let mut pending: Vec<FsChange> = Vec::new();
    // When the live index was last encoded for persistence. Live updates are
    // write-behind: most batches mutate the index in place and skip the O(n)
    // snapshot encode entirely.
    let mut last_encode = Instant::now();
    // Persistence: streams a snapshot straight into SQLite from this thread.
    let mut writer = SnapshotWriter::new();
    // Live helper deltas, when the helper built the index. While this is set the
    // worker ignores its own watcher: the helper's USN stream is the single
    // source of truth, and two sources would duplicate every change.
    #[cfg(target_os = "windows")]
    let mut helper_rx: Option<
        crossbeam_channel::Receiver<steward_index_helper::protocol::Frame>,
    > = None;
    loop {
        let command = match commands.recv_timeout(WATCH_POLL) {
            Ok(command) => command,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                #[cfg(target_os = "windows")]
                pump_sources(
                    &watcher,
                    &mut index,
                    &mut helper_rx,
                    &events,
                    &mut pending,
                    MAX_PENDING_CHANGES,
                    &mut last_encode,
                    &mut writer,
                );
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        match command {
            Command::Build(options) => {
                let started = Instant::now();
                match build_index(&options, &events) {
                    Ok(result) => {
                        let (mut built, backends, truncated) = result.output;
                        if built.build_name_index() {
                            if let Some(accelerator) = built.name_index() {
                                eprintln!(
                                    "file index: name accelerator over {} records ({} KB)",
                                    accelerator.records(),
                                    accelerator.approx_bytes() / 1024
                                );
                            }
                        }
                        #[cfg(target_os = "windows")]
                        {
                            helper_rx = result.session.map(|session| session.start());
                            if helper_rx.is_some() {
                                eprintln!(
                                    "file index: helper session is live; the local watcher stays idle"
                                );
                            }
                        }
                        #[cfg(not(target_os = "windows"))]
                        {
                            let _ = result.session;
                        }
                        let records = built.len();
                        let directories = built.dir_count();
                        let journal = has_journal(&built);
                        writer.store(&built, truncated);
                        index = Some(built);
                        last_encode = Instant::now();
                        let _ = events.send(IndexEvent::Ready {
                            records,
                            directories,
                            journal,
                            backends,
                            elapsed: started.elapsed(),
                        });
                    }
                    Err(message) => {
                        let _ = events.send(IndexEvent::Failed(message));
                    }
                }
            }
            Command::ReplayJournal => {
                let Some(current) = index.as_mut() else {
                    let _ = events.send(IndexEvent::Failed("no index to update".into()));
                    continue;
                };
                let report = replay_journal(current);
                // The `id → record` map only exists for the batch; release it
                // so it does not stay resident between catch-up passes.
                current.clear_key_map();
                if let Some(counts) = report.counts {
                    if last_encode.elapsed() >= LIVE_PERSIST_INTERVAL {
                        last_encode = Instant::now();
                        writer.store(current, 0);
                    }
                    let _ = events.send(IndexEvent::Updated {
                        records: current.len(),
                        journal: has_journal(current),
                        replayed: counts.replayed,
                        created: counts.created,
                        removed: counts.removed,
                        renamed: counts.renamed,
                    });
                }
                if report.needs_rebuild {
                    eprintln!("file index: a volume's journal cannot be replayed; rebuilding");
                    let _ = events.send(IndexEvent::Rebuild);
                } else if report.counts.is_none() {
                    if report.errors.is_empty() {
                        // Nothing new on any volume (or no journal at all). Not
                        // an error; just clear the busy flag.
                        let _ = events.send(IndexEvent::Idle);
                    } else {
                        let _ = events.send(IndexEvent::Failed(report.errors.join("; ")));
                    }
                }
            }
            Command::Search {
                generation,
                filter,
                kind,
                cancel,
            } => {
                let started = Instant::now();
                let Some(index) = index.as_ref() else {
                    let _ = replies.send(SearchReply {
                        generation,
                        hits: Vec::new(),
                        elapsed: started.elapsed(),
                    });
                    continue;
                };
                let options = SearchOptions {
                    limit: FILE_RESULT_LIMIT,
                    kind,
                    sort_by_recency: false,
                };
                let outcome = search_filtered(index, &filter, &options, Some(&cancel));
                let _ = options.limit;
                let _ = replies.send(SearchReply {
                    generation,
                    hits: outcome.hits,
                    elapsed: started.elapsed(),
                });
            }
        }
        // A long build/search just delayed the watcher; apply what queued up
        // before going back to waiting.
        #[cfg(target_os = "windows")]
        pump_sources(
            &watcher,
            &mut index,
            &mut helper_rx,
            &events,
            &mut pending,
            MAX_PENDING_CHANGES,
            &mut last_encode,
            &mut writer,
        );
    }
}

/// Whether an index carries at least one usable USN journal cursor.
fn has_journal(index: &FileDb) -> bool {
    index.journals().values().any(|state| state.journal_id != 0)
}

/// Route changes from whichever source is authoritative: the live helper
/// session when there is one, otherwise the in-process watcher.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn pump_sources(
    watcher: &DirectoryWatcher,
    index: &mut Option<FileDb>,
    helper_rx: &mut Option<crossbeam_channel::Receiver<steward_index_helper::protocol::Frame>>,
    events: &crossbeam_channel::Sender<IndexEvent>,
    pending: &mut Vec<FsChange>,
    max_pending: usize,
    last_encode: &mut Instant,
    writer: &mut SnapshotWriter,
) {
    if let Some(receiver) = helper_rx.as_ref() {
        if drain_helper(index, receiver, events, last_encode, writer) {
            *helper_rx = None;
            eprintln!("file index: helper session ended; resuming the local watcher");
        }
    } else {
        drain_watcher(
            watcher,
            index,
            events,
            pending,
            max_pending,
            last_encode,
            writer,
        );
    }
}

/// Apply queued helper deltas in place.
///
/// Returns `true` when the session ended (the helper closed the pipe, or its
/// journal needs a rebuild). A `Reconcile` event is raised in both cases, which
/// makes the launcher rebuild and fall back to the local walk.
#[cfg(target_os = "windows")]
fn drain_helper(
    index: &mut Option<FileDb>,
    receiver: &crossbeam_channel::Receiver<steward_index_helper::protocol::Frame>,
    events: &crossbeam_channel::Sender<IndexEvent>,
    last_encode: &mut Instant,
    writer: &mut SnapshotWriter,
) -> bool {
    use steward_index_helper::protocol::Frame;

    let mut changes: Vec<FsChange> = Vec::new();
    let mut ended = false;
    loop {
        match receiver.try_recv() {
            Ok(Frame::Delta(delta)) => changes.push(helper_change(delta)),
            Ok(Frame::Resync) => {
                ended = true;
                let _ = events.send(IndexEvent::Reconcile);
                break;
            }
            Ok(_) => {}
            Err(crossbeam_channel::TryRecvError::Empty) => break,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                ended = true;
                let _ = events.send(IndexEvent::Reconcile);
                break;
            }
        }
    }
    if changes.is_empty() {
        return ended;
    }
    let Some(index) = index.as_mut() else {
        return ended;
    };
    let outcome = apply_fs_changes(index, &changes);
    if outcome.is_empty() {
        return ended;
    }
    if last_encode.elapsed() >= LIVE_PERSIST_INTERVAL {
        *last_encode = Instant::now();
        writer.store(index, 0);
    }
    let _ = events.send(IndexEvent::Updated {
        records: index.len(),
        journal: has_journal(index),
        replayed: changes.len(),
        created: outcome.created,
        removed: outcome.removed,
        renamed: outcome.renamed,
    });
    ended
}

/// Map a helper delta onto the same `FsChange` the local watcher produces.
#[cfg(target_os = "windows")]
fn helper_change(delta: steward_index_helper::protocol::DeltaFrame) -> FsChange {
    use steward_index_helper::DeltaAction;

    let action = match delta.action {
        DeltaAction::Created => FsAction::Created,
        DeltaAction::Removed => FsAction::Removed,
        DeltaAction::Modified => FsAction::Modified,
        DeltaAction::RenamedOld => FsAction::RenamedOld,
        DeltaAction::RenamedNew => FsAction::RenamedNew,
    };
    FsChange {
        action,
        path: std::path::PathBuf::from(delta.path),
    }
}

/// Apply every queued filesystem change to the index and publish the result.
///
/// Batches are coalesced into one edit (and one re-sort). While no index exists
/// yet (the first build is running) the changes are buffered in `pending` and
/// applied as soon as one does. An overflow notice raises
/// [`IndexEvent::Reconcile`], which makes the launcher ask for a full rebuild —
/// the watcher cannot say what it missed.
#[cfg(target_os = "windows")]
fn drain_watcher(
    watcher: &DirectoryWatcher,
    index: &mut Option<FileDb>,
    events: &crossbeam_channel::Sender<IndexEvent>,
    pending: &mut Vec<FsChange>,
    max_pending: usize,
    last_encode: &mut Instant,
    writer: &mut SnapshotWriter,
) {
    let mut overflow = false;
    while let Some(batch) = watcher.try_recv() {
        pending.extend(batch.changes);
        overflow |= batch.overflow;
    }
    if overflow {
        let _ = events.send(IndexEvent::Reconcile);
    }
    if pending.is_empty() {
        return;
    }
    let Some(index) = index.as_mut() else {
        // No index yet: keep buffering, but do not let a slow build accumulate
        // without bound. Dropping the batch and rebuilding afterwards is the
        // documented reconcile anyway.
        if pending.len() > max_pending {
            pending.clear();
            let _ = events.send(IndexEvent::Reconcile);
        }
        return;
    };
    let changes = std::mem::take(pending);
    let outcome = apply_fs_changes(index, &changes);
    if outcome.is_empty() {
        return;
    }
    // Write-behind: persist at most once per interval. A busy disk (a build, an
    // unzip) produces many batches; each one still updates the searchable index
    // immediately, only the SQLite snapshot is deferred.
    if last_encode.elapsed() >= LIVE_PERSIST_INTERVAL {
        *last_encode = Instant::now();
        writer.store(index, 0);
    }
    let _ = events.send(IndexEvent::Updated {
        records: index.len(),
        journal: has_journal(index),
        replayed: changes.len(),
        created: outcome.created,
        removed: outcome.removed,
        renamed: outcome.renamed,
    });
}

/// Roots to index when nothing is configured: every fixed (local) drive.
///
/// `GetDriveTypeW` keeps removable media, network shares and optical drives out:
/// indexing them reads media that may not be present at the next boot, and on a
/// multi-disk machine every fixed volume is included (not just the system one).
fn default_roots() -> Vec<PathBuf> {
    let roots = file_index::fixed_drive_roots();
    if !roots.is_empty() {
        return roots;
    }
    // Non-Windows (or an API hiccup): fall back to whatever roots exist.
    (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .filter(|root| root.is_dir())
        .collect()
}

/// Build an index, preferring each volume's `$MFT` fast path.
fn build_index(
    options: &ScanOptions,
    events: &crossbeam_channel::Sender<IndexEvent>,
) -> Result<BuildResult, String> {
    // The optional privileged helper reads `$MFT` at a fraction of a walk's
    // cost. It is strictly an accelerator: when it is not listening, fall back
    // to the in-process enumerator below without making the user wait.
    if let Some(result) = crate::file_index_helper::try_build(options) {
        match result {
            Ok((output, session)) => {
                eprintln!(
                    "file index: helper streamed {} records from {}",
                    output.0.len(),
                    output
                        .1
                        .iter()
                        .map(|(path, backend)| format!("{} [{backend}]", path.display()))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return Ok(BuildResult { output, session });
            }
            Err(error) => {
                eprintln!("file index: helper failed ({error}); falling back to the local walk");
            }
        }
    }
    // Pre-size the builder from the record count of the index being replaced
    // (when one is loaded): a multi-million-record build otherwise doubles a
    // multi-hundred-megabyte arena several times on the way up.
    let mut builder = match options.records_hint {
        Some(expected) => FileDbBuilder::with_capacity(expected),
        None => FileDbBuilder::new(),
    };
    let mut backends = Vec::new();
    let mut walk_roots: Vec<PathBuf> = Vec::new();

    for root in &options.roots {
        #[cfg(target_os = "windows")]
        if file_index::is_volume_root(root) {
            if let Some(letter) = file_index::drive_letter(root) {
                match build_volume_from_mft(letter, root, &mut builder, options) {
                    Ok(records) => {
                        backends.push((root.clone(), IndexBackend::Mft));
                        eprintln!(
                            "file index: {} read from $MFT ({records} records)",
                            root.display()
                        );
                        continue;
                    }
                    Err(error) => {
                        // Elevation, a non-NTFS volume, or no journal. The walk
                        // covers the same tree and produces the same record
                        // shape, so nothing downstream changes.
                        eprintln!(
                            "file index: {} falling back to the directory walk ({error})",
                            root.display()
                        );
                    }
                }
            }
        }
        walk_roots.push(root.clone());
    }

    if !walk_roots.is_empty() {
        let mut walk_options = options.clone();
        walk_options.roots = walk_roots.clone();
        let mut adapter = WalkAdapter::new(&mut builder, &walk_roots, events);
        // The root indices are captured up front so the closure does not borrow
        // the adapter that scan also mutably borrows.
        let root_indexes = adapter.root_indexes.clone();
        let roots = walk_roots.clone();
        let report = file_index::scan(
            &walk_options,
            |root| {
                roots
                    .iter()
                    .position(|candidate| candidate == root)
                    .and_then(|position| root_indexes.get(position).copied())
                    .unwrap_or(0)
            },
            &mut adapter,
        );
        for root in &walk_roots {
            backends.push((root.clone(), IndexBackend::Walk));
        }
        eprintln!(
            "file index: directory walk over {} root(s): {} entries, {} denied, {} excluded",
            walk_roots.len(),
            report.entries,
            report.denied,
            report.excluded
        );
    }

    if builder.is_empty() {
        return Err("nothing could be indexed (no readable root)".into());
    }
    let truncated = builder.truncated_names();
    let index = builder.finalize();
    if index.is_empty() {
        return Err("the index came out empty".into());
    }
    Ok(BuildResult {
        output: (index, backends, truncated),
        session: None,
    })
}

/// Enumerate one volume's `$MFT` into `builder`.
#[cfg(target_os = "windows")]
fn build_volume_from_mft(
    letter: u8,
    root: &Path,
    builder: &mut FileDbBuilder,
    options: &ScanOptions,
) -> Result<usize, String> {
    let mut volume = file_index::RawVolume::open(letter).map_err(|error| error.to_string())?;
    let journal = volume.query_journal().ok();
    let root_name = root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string();
    // Record 5 is the volume root; it becomes this volume's root record, so the
    // sweep must not add it a second time.
    let root_id = volume
        .read_mft_record(5)
        .ok()
        .and_then(|record| {
            let sequence = u16::from_le_bytes([*record.get(16)?, *record.get(17)?]) as u64;
            Some((sequence << 48) | 5)
        })
        .unwrap_or(0);
    builder.add_root(&root_name, root_id);
    if let Some(state) = journal {
        builder.set_journal(letter, state);
    }
    let excluded = options.excluded_dirs.clone();
    let mut records = 0usize;
    volume
        .enumerate_mft(|info| {
            if info.id == root_id && root_id != 0 {
                return;
            }
            if info.is_dir && excluded.contains(&info.name.to_lowercase()) {
                return;
            }
            builder.add_entry(info);
            records += 1;
        })
        .map_err(|error| error.to_string())?;
    Ok(records)
}

/// What one catch-up pass did, across every volume.
#[derive(Debug, Default)]
struct ReplayReport {
    /// `Some` when at least one volume applied journal records.
    counts: Option<ReplayCounts>,
    /// A volume's journal can never be replayed again; only a rebuild fixes it.
    needs_rebuild: bool,
    /// Per-volume failures. Only reported when no volume applied anything.
    errors: Vec<String>,
}

/// Why one volume's journal could not be replayed.
#[cfg(target_os = "windows")]
enum VolumeReplayError {
    /// The journal was recreated, wrapped, or the cursor is ahead of it, so the
    /// volume has to be rebuilt rather than caught up.
    NeedsRebuild(String),
    /// A transient failure (volume locked, unreadable, no root record).
    Other(String),
}

#[cfg(target_os = "windows")]
impl VolumeReplayError {
    fn into_message(self) -> String {
        match self {
            VolumeReplayError::NeedsRebuild(message) | VolumeReplayError::Other(message) => message,
        }
    }
}

/// Replay the USN journal of **every** indexed volume into `db`, in place.
///
/// Each volume keeps its own cursor, so a multi-disk index catches up each one
/// independently: a locked or disconnected volume is reported and skipped while
/// the others still advance. The index is mutated directly instead of being
/// cloned first: the worker owns it exclusively, and a clone would double the
/// resident footprint on every two-second catch-up tick even when the journal
/// turned out to be empty.
#[cfg(target_os = "windows")]
fn replay_journal(db: &mut FileDb) -> ReplayReport {
    let mut cursors: Vec<(u8, JournalState)> = db
        .journals()
        .iter()
        .filter(|(_, state)| state.journal_id != 0)
        .map(|(letter, state)| (*letter, *state))
        .collect();
    cursors.sort_by_key(|(letter, _)| *letter);
    if cursors.is_empty() {
        return ReplayReport::default();
    }

    let mut counts = ReplayCounts::default();
    let mut any = false;
    let mut report = ReplayReport::default();
    for (letter, stored) in cursors {
        match replay_one_volume(db, letter, stored) {
            Ok(Some((volume_replayed, outcome))) => {
                any = true;
                counts.replayed += volume_replayed;
                counts.created += outcome.created;
                counts.removed += outcome.removed;
                counts.renamed += outcome.renamed;
            }
            Ok(None) => {}
            Err(VolumeReplayError::NeedsRebuild(message)) => {
                report.needs_rebuild = true;
                report.errors.push(message);
            }
            Err(error) => report.errors.push(error.into_message()),
        }
    }
    report.counts = any.then_some(counts);
    report
}

/// Replay one volume's journal into `db`, advancing that volume's cursor.
///
/// Returns `None` when the volume had no new records; the cursor is still
/// advanced so an idle volume is not re-read from the same point every tick.
#[cfg(target_os = "windows")]
fn replay_one_volume(
    db: &mut FileDb,
    letter: u8,
    stored: JournalState,
) -> Result<Option<(usize, file_index::UsnOutcome)>, VolumeReplayError> {
    let volume = file_index::RawVolume::open(letter).map_err(|error| {
        VolumeReplayError::Other(format!(
            "cannot open {}: for USN catch-up ({error})",
            letter as char
        ))
    })?;
    let live = volume
        .query_journal()
        .map_err(|error| VolumeReplayError::Other(error.to_string()))?;
    if !file_index::cursor_is_usable(Some(stored), live) {
        return Err(VolumeReplayError::NeedsRebuild(format!(
            "the USN journal of {}: changed; a rebuild is required",
            letter as char
        )));
    }
    let mut records = Vec::new();
    let read = volume
        .read_usn(stored.next_usn, live.journal_id, |record| {
            records.push(record)
        })
        .map_err(|error| VolumeReplayError::Other(error.to_string()))?;
    let Some(root_index) = file_index::volume_root(db, letter) else {
        return Err(VolumeReplayError::Other(format!(
            "no root record for {}: in the index",
            letter as char
        )));
    };
    let outcome = if records.is_empty() {
        file_index::UsnOutcome::default()
    } else {
        apply_usn_records(db, root_index, &records)
    };
    // Use the read's own next USN, not the one [`query_journal`] reported
    // before the read: a record written while reading would otherwise be
    // skipped on the next pass.
    db.set_journal(
        letter,
        JournalState {
            journal_id: live.journal_id,
            next_usn: read.next_usn,
        },
    );
    if records.is_empty() {
        Ok(None)
    } else {
        Ok(Some((records.len(), outcome)))
    }
}

/// Non-Windows builds have no USN journal to replay.
#[cfg(not(target_os = "windows"))]
fn replay_journal(_db: &mut FileDb) -> ReplayReport {
    ReplayReport::default()
}

/// Bridges the directory walker to the index builder.
///
/// The walker reports a parent *path*; the builder wants a parent *record index*.
/// The adapter keeps the stack of directories currently being walked, each with
/// the record index the builder handed back for it, which is why `Emit` carries
/// the parent's index alongside its path.
struct WalkAdapter<'a> {
    builder: &'a mut FileDbBuilder,
    /// Record index of every root, in the same order as the options' roots.
    root_indexes: Vec<u32>,
    /// Directory stack: `(path, record index)`.
    stack: Vec<(PathBuf, u32)>,
    events: &'a crossbeam_channel::Sender<IndexEvent>,
    last_report: Instant,
    records: usize,
}

impl<'a> WalkAdapter<'a> {
    fn new(
        builder: &'a mut FileDbBuilder,
        roots: &'a [PathBuf],
        events: &'a crossbeam_channel::Sender<IndexEvent>,
    ) -> Self {
        let root_indexes = roots
            .iter()
            .map(|root| {
                let name = root
                    .to_string_lossy()
                    .trim_end_matches(['\\', '/'])
                    .to_string();
                builder.add_root(&name, 0)
            })
            .collect();
        Self {
            builder,
            root_indexes,
            stack: Vec::new(),
            events,
            last_report: Instant::now(),
            records: 0,
        }
    }
}

impl Emit for WalkAdapter<'_> {
    fn enter_dir(&mut self, path: &Path, info: &EntryInfo, parent: u32) -> u32 {
        let index = self.builder.add_child(parent, info);
        self.stack.push((path.to_path_buf(), index));
        self.records += 1;
        index
    }

    fn leave_dir(&mut self, path: &Path) {
        if self
            .stack
            .last()
            .is_some_and(|(current, _)| current == path)
        {
            self.stack.pop();
        }
    }

    fn emit(&mut self, _parent: &Path, parent_index: u32, info: &EntryInfo) {
        self.builder.add_child(parent_index, info);
        self.records += 1;
    }

    fn progress(&mut self, progress: &IndexProgress) {
        if self.last_report.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_report = Instant::now();
        let _ = self.events.send(IndexEvent::Progress(progress.clone()));
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// The binary snapshot stored in the shared database, if any.
pub(crate) fn load_snapshot(storage: &steward_storage::Storage) -> Option<Vec<u8>> {
    if let Some(blob) = storage.get_setting_blob(file_index::persist::SETTING_KEY) {
        return Some(blob);
    }
    // A snapshot written by the pre-binary build lives under the text
    // `settings` key. This build cannot decode it, so drop it rather than
    // leaving a possibly huge orphan row in the database.
    let _ = storage.remove_setting(file_index::persist::SETTING_KEY);
    None
}

#[cfg(all(test, target_os = "windows"))]
mod helper_delta_tests {
    use super::*;
    use steward_index_helper::protocol::{DeltaFrame, Frame};
    use steward_index_helper::DeltaAction;

    #[test]
    fn every_helper_delta_action_maps_to_a_watcher_action() {
        // The app applies helper deltas through `apply_fs_changes`, so the
        // mapping must be exact; a silent mismatch would drop live changes.
        let cases = [
            (DeltaAction::Created, FsAction::Created),
            (DeltaAction::Removed, FsAction::Removed),
            (DeltaAction::Modified, FsAction::Modified),
            (DeltaAction::RenamedOld, FsAction::RenamedOld),
            (DeltaAction::RenamedNew, FsAction::RenamedNew),
        ];
        for (action, expected) in cases {
            let change = helper_change(DeltaFrame {
                action,
                path: "C:\\Users\\a.txt".into(),
            });
            assert_eq!(change.action, expected, "action {action:?}");
            assert_eq!(change.path.to_string_lossy(), "C:\\Users\\a.txt");
        }
    }

    #[test]
    fn helper_deltas_are_applied_in_place_and_published() {
        let root = std::env::temp_dir().join(format!(
            "steward-helper-drain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let added = root.join("added.txt");
        std::fs::write(&added, b"x").unwrap();

        let mut builder = FileDbBuilder::new();
        builder.add_root(&root.to_string_lossy(), 0);
        let mut index = Some(builder.finalize());

        let (sender, receiver) = crossbeam_channel::unbounded();
        sender
            .send(Frame::Delta(DeltaFrame {
                action: DeltaAction::Created,
                path: added.to_string_lossy().into_owned(),
            }))
            .unwrap();
        let (events, published) = crossbeam_channel::unbounded();
        let mut last_encode = Instant::now();
        // Persistence is disabled in this test: it exercises the in-place
        // apply, not the SQLite write.
        let mut writer = SnapshotWriter {
            storage: None,
            unavailable: true,
        };

        assert!(!drain_helper(
            &mut index,
            &receiver,
            &events,
            &mut last_encode,
            &mut writer,
        ));
        let hits = file_index::search(
            index.as_ref().unwrap(),
            "added",
            &SearchOptions::with_limit(5),
        );
        assert_eq!(hits.hits.len(), 1, "the delta must be searchable");
        match published.try_recv() {
            Ok(IndexEvent::Updated { created, .. }) => assert_eq!(created, 1),
            _ => panic!("expected an Updated event"),
        }

        // A dropped helper closes the channel, which ends the session and asks
        // for a rebuild so the local watcher takes over.
        drop(sender);
        let (events, _published) = crossbeam_channel::unbounded();
        assert!(drain_helper(
            &mut index,
            &receiver,
            &events,
            &mut last_encode,
            &mut writer,
        ));

        std::fs::remove_dir_all(&root).unwrap();
    }
}

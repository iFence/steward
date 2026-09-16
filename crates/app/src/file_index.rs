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

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use steward_core_engine::file_index::{
    self, apply_usn_records, parse_roots, search_filtered, Cancel, Emit, EntryInfo, FileDb,
    FileDbBuilder, FileHit, IndexBackend, IndexProgress, JournalState, KindFilter, ScanOptions,
    SearchOptions,
};

/// Records per search request from the launcher.
pub(crate) const FILE_RESULT_LIMIT: usize = 12;

/// Default excluded directory names on top of the walker's own list.
const EXTRA_EXCLUSIONS: [&str; 4] = ["node_modules", ".git", "winsxs", "$recycle.bin"];

/// A finished build: the index, the roots it covered, and the number of names
/// that had to be truncated.
type BuildOutput = (FileDb, Vec<(PathBuf, IndexBackend)>, usize);

/// A finished catch-up pass: the updated index plus what the journal changed.
type CatchUpOutput = (FileDb, usize, usize, usize, usize);

/// A finished index build or load.
pub(crate) enum IndexEvent {
    /// Periodic progress while enumerating.
    Progress(IndexProgress),
    /// The build finished; `snapshot` is ready to be persisted.
    Ready {
        db: Box<FileDb>,
        snapshot: String,
        records: usize,
        directories: usize,
        backends: Vec<(PathBuf, IndexBackend)>,
        elapsed: Duration,
    },
    /// A catch-up pass applied replayed journal changes.
    Updated {
        db: Box<FileDb>,
        snapshot: String,
        replayed: usize,
        created: usize,
        removed: usize,
        renamed: usize,
    },
    /// Nothing could be indexed.
    Failed(String),
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
    /// Current index, swapped wholesale by load/build/replay.
    db: Arc<Mutex<Option<Arc<FileDb>>>>,
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
    /// Records in the current index.
    pub(crate) records: usize,
    /// A snapshot waiting to be written by the launcher's thread (the worker
    /// holds no SQLite connection, so persistence stays on the UI side).
    pending_snapshot: Option<String>,
}

impl FileIndex {
    /// Start the worker, adopting `snapshot` when it decodes.
    ///
    /// `storage` is opened on the calling thread only to read the snapshot;
    /// persistence is driven from [`FileIndex::persist_snapshot`], which the
    /// launcher calls from its own thread.
    pub(crate) fn start(snapshot: Option<String>, configured_roots: &[String]) -> Self {
        let (command_tx, command_rx) = crossbeam_channel::unbounded::<Command>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<IndexEvent>();
        let (reply_tx, reply_rx) = crossbeam_channel::unbounded::<SearchReply>();
        let db = Arc::new(Mutex::new(None));
        let worker_db = Arc::clone(&db);
        std::thread::Builder::new()
            .name("steward-file-index".into())
            .spawn(move || worker_loop(command_rx, event_tx, reply_tx, worker_db))
            .expect("spawn the file index worker");

        let roots = if configured_roots.is_empty() {
            default_roots()
        } else {
            parse_roots(configured_roots)
        };

        let mut index = Self {
            db,
            commands: command_tx,
            events: event_rx,
            replies: reply_rx,
            generation: 0,
            cancel: None,
            roots,
            building: false,
            records: 0,
            pending_snapshot: None,
        };
        if let Some(blob) = snapshot {
            match file_index::persist::decode(&blob, unix_seconds()) {
                Ok(loaded) => {
                    index.records = loaded.len();
                    *index.db.lock().expect("file index lock") = Some(Arc::new(loaded));
                    eprintln!(
                        "file index: loaded {} records from the snapshot",
                        index.records
                    );
                }
                Err(error) => {
                    eprintln!("file index: snapshot rejected ({error}); a rebuild will follow");
                }
            }
        }
        index
    }

    /// Whether an index with at least one record is available.
    pub(crate) fn is_ready(&self) -> bool {
        self.db
            .lock()
            .expect("file index lock")
            .as_ref()
            .is_some_and(|db| !db.is_empty())
    }

    /// Whether a build or catch-up pass is running.
    pub(crate) fn is_building(&self) -> bool {
        self.building
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
                    db,
                    snapshot,
                    records,
                    directories,
                    backends,
                    elapsed,
                } => {
                    self.building = false;
                    self.records = records;
                    *self.db.lock().expect("file index lock") = Some(Arc::new(*db));
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
                    // Keep the snapshot for the launcher to write.
                    self.pending_snapshot = Some(snapshot);
                }
                IndexEvent::Updated {
                    db,
                    snapshot,
                    replayed,
                    created,
                    removed,
                    renamed,
                } => {
                    self.building = false;
                    self.records = db.len();
                    *self.db.lock().expect("file index lock") = Some(Arc::new(*db));
                    changed = true;
                    eprintln!(
                        "file index: replayed {replayed} USN records (+{created} -{removed} ~{renamed})"
                    );
                    self.pending_snapshot = Some(snapshot);
                }
                IndexEvent::Failed(message) => {
                    self.building = false;
                    eprintln!("file index: {message}");
                }
            }
        }
        changed
    }

    /// Take the snapshot waiting to be written, if any.
    ///
    /// The worker encodes it (it holds the index) and the launcher writes it (it
    /// holds the SQLite connection), so this is the hand-off between them.
    pub(crate) fn take_pending_snapshot(&mut self) -> Option<String> {
        self.pending_snapshot.take()
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

/// The worker thread: builds, replays, and answers searches.
fn worker_loop(
    commands: crossbeam_channel::Receiver<Command>,
    events: crossbeam_channel::Sender<IndexEvent>,
    replies: crossbeam_channel::Sender<SearchReply>,
    db: Arc<Mutex<Option<Arc<FileDb>>>>,
) {
    while let Ok(command) = commands.recv() {
        match command {
            Command::Build(options) => {
                let started = Instant::now();
                match build_index(&options, &events) {
                    Ok((index, backends, truncated)) => {
                        let snapshot =
                            file_index::persist::encode(&index, truncated, unix_seconds());
                        let records = index.len();
                        let directories = index.dir_count();
                        let shared = Arc::new(index);
                        *db.lock().expect("file index lock") = Some(Arc::clone(&shared));
                        let _ = events.send(IndexEvent::Ready {
                            db: Box::new(shared.as_ref().duplicate()),
                            snapshot,
                            records,
                            directories,
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
                let current = db.lock().expect("file index lock").clone();
                let Some(current) = current else {
                    let _ = events.send(IndexEvent::Failed("no index to update".into()));
                    continue;
                };
                match replay_journal(&current) {
                    Ok(Some((updated, replayed, created, removed, renamed))) => {
                        let snapshot = file_index::persist::encode(&updated, 0, unix_seconds());
                        let shared = Arc::new(updated);
                        *db.lock().expect("file index lock") = Some(Arc::clone(&shared));
                        let _ = events.send(IndexEvent::Updated {
                            db: Box::new(shared.as_ref().duplicate()),
                            snapshot,
                            replayed,
                            created,
                            removed,
                            renamed,
                        });
                    }
                    Ok(None) => {
                        // No journal, or the index cannot be updated in place.
                        let _ = events.send(IndexEvent::Failed(
                            "the USN journal is unavailable; keeping the current index".into(),
                        ));
                    }
                    Err(error) => {
                        let _ = events.send(IndexEvent::Failed(error));
                    }
                }
            }
            Command::Search {
                generation,
                filter,
                kind,
                cancel,
            } => {
                let index = db.lock().expect("file index lock").clone();
                let started = Instant::now();
                let Some(index) = index else {
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
                let outcome = search_filtered(&index, &filter, &options, Some(&cancel));
                let _ = options.limit;
                let _ = replies.send(SearchReply {
                    generation,
                    hits: outcome.hits,
                    elapsed: started.elapsed(),
                });
            }
        }
    }
}

/// Roots to index when nothing is configured: every fixed drive.
///
/// "Full disk" is exactly this — the configured volumes. Removable and network
/// drives are excluded because indexing them reads media that may not be present
/// at the next boot.
fn default_roots() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .filter(|root| root.is_dir())
        .collect()
}

/// Build an index, preferring each volume's `$MFT` fast path.
fn build_index(
    options: &ScanOptions,
    events: &crossbeam_channel::Sender<IndexEvent>,
) -> Result<BuildOutput, String> {
    let mut builder = FileDbBuilder::new();
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
    Ok((index, backends, truncated))
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

/// Replay the USN journal into `db`, if the volume's journal is usable.
///
/// Returns `None` when there is nothing to replay on this platform or the index
/// has no journal cursor (a walk-built index), and an error string when the
/// volume cannot be read.
#[cfg(target_os = "windows")]
fn replay_journal(db: &FileDb) -> Result<Option<CatchUpOutput>, String> {
    let Some((letter, stored)) = db.journals().iter().next().map(|(l, s)| (*l, *s)) else {
        return Ok(None);
    };
    if stored.journal_id == 0 {
        return Ok(None);
    }
    let volume = match file_index::RawVolume::open(letter) {
        Ok(volume) => volume,
        Err(error) => {
            return Err(format!(
                "cannot open {}: for USN catch-up ({error})",
                letter as char
            ))
        }
    };
    let live = volume.query_journal().map_err(|error| error.to_string())?;
    if !file_index::cursor_is_usable(Some(stored), live) {
        return Err(format!(
            "the USN journal of {}: changed; a rebuild is required",
            letter as char
        ));
    }
    let mut records = Vec::new();
    volume
        .read_usn(stored.next_usn, live.journal_id, |record| {
            records.push(record)
        })
        .map_err(|error| error.to_string())?;
    let replayed = records.len();
    if replayed == 0 {
        return Ok(None);
    }
    let Some(root_index) = file_index::volume_root(db, letter) else {
        return Err(format!(
            "no root record for {}: in the index",
            letter as char
        ));
    };
    let mut updated = db.duplicate();
    let outcome = apply_usn_records(&mut updated, root_index, &records);
    updated.set_journal(
        letter,
        JournalState {
            journal_id: live.journal_id,
            next_usn: live.next_usn,
        },
    );
    Ok(Some((
        updated,
        replayed,
        outcome.created,
        outcome.removed,
        outcome.renamed,
    )))
}

/// Non-Windows builds have no USN journal to replay.
#[cfg(not(target_os = "windows"))]
fn replay_journal(_db: &FileDb) -> Result<Option<CatchUpOutput>, String> {
    Ok(None)
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

/// The snapshot stored in the shared settings table, if any.
pub(crate) fn load_snapshot(storage: &steward_storage::Storage) -> Option<String> {
    storage.get_setting(file_index::persist::SETTING_KEY)
}

/// Persist `snapshot` through the shared SQLite settings table.
pub(crate) fn store_snapshot(
    storage: &Rc<RefCell<steward_storage::Storage>>,
    snapshot: &str,
) -> anyhow::Result<()> {
    storage
        .borrow()
        .set_setting(file_index::persist::SETTING_KEY, snapshot)
}

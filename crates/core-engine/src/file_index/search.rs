//! Query execution: match the compiled filter against the whole index, in
//! parallel, over index blocks.
//!
//! This is the parallel execution path: the query is compiled once, workers
//! are derived from the *block* count (not the file count), each worker scans a
//! contiguous block range and appends to its own result vector, and the main
//! thread merges. A single-term query takes the fast path and only reaches for
//! the parent chain when its scope actually needs it.

use std::sync::atomic::{AtomicBool, Ordering};

use super::db::FileDb;
use super::query::{self, Filter, Haystack, KindFilter};

/// One search hit.
///
/// `PartialEq` lets the launcher recognise a re-published, identical result set
/// and skip the re-render — which would otherwise reset the selected row on
/// every poll tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    /// Record index in the [`FileDb`].
    pub index: u32,
    /// Name of the record.
    pub name: String,
    /// Full path, built from the parent chain.
    pub path: std::path::PathBuf,
    pub size: Option<u64>,
    /// Last-write time in seconds since the Windows epoch.
    pub mtime: Option<u64>,
    pub is_dir: bool,
    /// Ranking score: higher is better.
    pub score: i32,
}

/// What a search should return.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Maximum number of hits to return (after ranking).
    pub limit: usize,
    /// Restrict to files, folders, or either.
    pub kind: KindFilter,
    /// Return results closest to this block first? Kept for API symmetry with
    /// the launcher's "list everything" mode.
    pub sort_by_recency: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 64,
            kind: KindFilter::Any,
            sort_by_recency: false,
        }
    }
}

impl SearchOptions {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }
}

/// Execution statistics, so `docs/benchmarks.md` can record real numbers.
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchStats {
    /// Blocks handed to workers.
    pub blocks: usize,
    /// Records actually compared (tombstones and filtered kinds excluded).
    pub scanned: usize,
    /// Workers spawned.
    pub workers: usize,
    /// Records that satisfied the filter before ranking/limiting.
    pub matched: usize,
    /// Whether the scan finished or was cancelled by a newer query.
    pub cancelled: bool,
}

/// Result of a search: the ranked hits plus execution statistics.
#[derive(Debug, Clone, Default)]
pub struct SearchOutcome {
    pub hits: Vec<FileHit>,
    pub stats: SearchStats,
}

/// Concurrency ceiling: `ceil(block_count / 16)` workers, clamped by the
/// system limit (the core count).
const BLOCKS_PER_WORKER: usize = 16;

/// Above this many trigram candidates the parallel scan wins; the candidate
/// path verifies and scores on one thread.
const MAX_INDEX_CANDIDATES: usize = 32_768;

/// Cancel handle handed to search workers: one flag for the whole query, so a
/// newer keystroke can stop a running scan inside the block loop.
#[derive(Debug, Default)]
pub struct Cancel {
    flag: AtomicBool,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask every worker to stop at its next record.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Search `index` for `query`.
pub fn search(index: &FileDb, query: &str, options: &SearchOptions) -> SearchOutcome {
    search_filtered(index, &query::parse(query), options, None)
}

/// Search `index` with an already-compiled filter and an optional cancel flag.
pub fn search_filtered(
    index: &FileDb,
    filter: &Filter,
    options: &SearchOptions,
    cancel: Option<&Cancel>,
) -> SearchOutcome {
    let kind = if options.kind == KindFilter::Any {
        filter.kind()
    } else {
        options.kind
    };
    let empty = matches!(filter, Filter::Empty);
    if filter.is_never() {
        return SearchOutcome::default();
    }
    let blocks = index.block_count();
    if blocks == 0 {
        return SearchOutcome::default();
    }

    // A query with no text at all ("list everything") is answered from the
    // index order; no scanning is needed.
    if empty {
        return list_all(index, kind, options);
    }

    // A plain folded substring term can be answered from the 3-gram name index
    // the app builds for large indexes: the postings give a superset of the
    // matches, which are then verified against each record's current name.
    if let Some(needle) = filter.indexed_needle() {
        if let Some(accelerator) = index.name_index() {
            if let Some(candidates) = accelerator.candidates(needle.as_bytes()) {
                // A very broad needle (a large share of the index is a
                // candidate) is cheaper to answer with the parallel scan: the
                // candidate path verifies and scores single-threaded. Selective
                // needles get the index.
                if candidates.len() <= MAX_INDEX_CANDIDATES {
                    return search_candidates(
                        index,
                        filter,
                        kind,
                        &candidates,
                        options,
                        cancel,
                        blocks,
                    );
                }
            }
        }
    }

    // Only the best `keep` records per worker are retained. A broad needle
    // ("re" matching every report) must not materialise a name, a full path and
    // a `FileHit` for millions of matches before truncating to `limit`.
    let keep = options.limit.max(1);
    let workers = worker_count(blocks);
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut scanned = 0usize;
    let mut matched = 0usize;
    let mut cancelled = false;
    let worker_total;
    if workers <= 1 {
        worker_total = 1;
        scan_range(
            index,
            filter,
            kind,
            0,
            blocks,
            keep,
            &mut candidates,
            &mut scanned,
            &mut matched,
            cancel,
            &mut cancelled,
        );
    } else {
        let chunk = blocks.div_ceil(workers);
        let ranges: Vec<(usize, usize)> = (0..workers)
            .map(|worker| {
                let from = worker * chunk;
                let to = ((worker + 1) * chunk).min(blocks);
                (from, to)
            })
            .filter(|(from, to)| from < to)
            .collect();
        worker_total = ranges.len();
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_total);
            for (from, to) in ranges {
                handles.push(scope.spawn(move || {
                    let mut local = Vec::new();
                    let mut local_scanned = 0usize;
                    let mut local_matched = 0usize;
                    let mut local_cancelled = false;
                    scan_range(
                        index,
                        filter,
                        kind,
                        from,
                        to,
                        keep,
                        &mut local,
                        &mut local_scanned,
                        &mut local_matched,
                        cancel,
                        &mut local_cancelled,
                    );
                    (local, local_scanned, local_matched, local_cancelled)
                }));
            }
            for handle in handles {
                if let Ok((local, local_scanned, local_matched, local_cancelled)) = handle.join() {
                    candidates.extend(local);
                    scanned += local_scanned;
                    matched += local_matched;
                    cancelled |= local_cancelled;
                }
            }
        });
    }
    finish_candidates(
        index,
        candidates,
        keep,
        options,
        SearchStats {
            blocks,
            scanned,
            workers: worker_total,
            matched,
            cancelled,
        },
    )
}

/// Answer a query from the 3-gram index's candidate list.
///
/// Postings may over-include (a renamed or deleted record stays in an old
/// bitmap), so every candidate is verified with the full filter against its
/// current name before it is scored.
#[allow(clippy::too_many_arguments)]
fn search_candidates(
    index: &FileDb,
    filter: &Filter,
    kind: KindFilter,
    candidates: &[u32],
    options: &SearchOptions,
    cancel: Option<&Cancel>,
    blocks: usize,
) -> SearchOutcome {
    let keep = options.limit.max(1);
    let needles: Vec<&str> = filter
        .text_predicates()
        .into_iter()
        .map(|predicate| predicate.text.as_str())
        .collect();
    let mut best: Vec<Candidate> = Vec::new();
    let mut scanned = 0usize;
    let mut matched = 0usize;
    let mut cancelled = false;
    for record in candidates.iter().copied() {
        if let Some(cancel) = cancel {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
        }
        scanned += 1;
        if !index.is_live(record) || !kind_accepts(index, record, kind) {
            continue;
        }
        let haystack = Haystack::new(index.name_bytes(record));
        let (size, mtime) = index.meta(record);
        if !query::evaluate(filter, &haystack, size, mtime, index.is_dir(record)) {
            continue;
        }
        matched += 1;
        best.push(Candidate {
            score: score(index, record, &needles, ""),
            record,
        });
    }
    trim_candidates(&mut best, keep);
    finish_candidates(
        index,
        best,
        keep,
        options,
        SearchStats {
            blocks,
            scanned,
            workers: 1,
            matched,
            cancelled,
        },
    )
}

/// Answer an empty query from the index order (used for "show me everything").
fn list_all(index: &FileDb, kind: KindFilter, options: &SearchOptions) -> SearchOutcome {
    let mut hits = Vec::new();
    let mut matched = 0usize;
    if options.sort_by_recency {
        // Picking the newest needs every mtime, so this branch materialises all
        // accepted rows; the launcher never issues an empty file query.
        for record in index.iter_ordered() {
            if !kind_accepts(index, record, kind) {
                continue;
            }
            matched += 1;
            hits.push(materialize(index, record, 0));
        }
        hits.sort_by(|left, right| {
            right
                .mtime
                .unwrap_or(0)
                .cmp(&left.mtime.unwrap_or(0))
                .then_with(|| left.path.cmp(&right.path))
        });
    } else {
        // No ordering to apply: pay for only the rows that survive the limit.
        for record in index.iter_ordered() {
            if !kind_accepts(index, record, kind) {
                continue;
            }
            matched += 1;
            if hits.len() < options.limit {
                hits.push(materialize(index, record, 0));
            }
        }
    }
    hits.truncate(options.limit);
    SearchOutcome {
        hits,
        stats: SearchStats {
            blocks: index.block_count(),
            scanned: index.len(),
            workers: 1,
            matched,
            cancelled: false,
        },
    }
}

/// A record that matched, before its name, path and metadata are materialised.
///
/// The scan keeps these compact candidates instead of full [`FileHit`]s, so a
/// broad needle costs one `i32` + `u32` per match rather than a name `String`,
/// an allocated full path and a parent-chain walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Candidate {
    score: i32,
    record: u32,
}

/// Scan blocks `[from, to)`, keeping at most `keep` best candidates.
#[allow(clippy::too_many_arguments)]
fn scan_range(
    index: &FileDb,
    filter: &Filter,
    kind: KindFilter,
    from: usize,
    to: usize,
    keep: usize,
    out: &mut Vec<Candidate>,
    scanned: &mut usize,
    matched: &mut usize,
    cancel: Option<&Cancel>,
    cancelled: &mut bool,
) {
    let needs_path = filter.needs_path();
    let needles: Vec<&str> = filter
        .text_predicates()
        .into_iter()
        .map(|predicate| predicate.text.as_str())
        .collect();
    let mut path_buffer = String::new();
    for block in from..to {
        let start = index.block_start(block as u32);
        let end = index.block_start(block as u32 + 1);
        for position in start..end {
            if let Some(cancel) = cancel {
                if cancel.is_cancelled() {
                    *cancelled = true;
                    trim_candidates(out, keep);
                    return;
                }
            }
            let Some(record) = index.ordered_at(position) else {
                continue;
            };
            if !index.is_live(record) {
                continue;
            }
            *scanned += 1;
            if !kind_accepts(index, record, kind) {
                continue;
            }
            let haystack = if needs_path {
                index.path_into(record, &mut path_buffer);
                Haystack::with_path(index.name_bytes(record), &path_buffer)
            } else {
                Haystack::new(index.name_bytes(record))
            };
            let (size, mtime) = index.meta(record);
            let is_dir = index.is_dir(record);
            if !query::evaluate(filter, &haystack, size, mtime, is_dir) {
                continue;
            }
            *matched += 1;
            // `path:` needs the path both to match and to score; a name-only
            // filter already proved the name hit, so an empty path is fine here
            // (the path branch of `score` can never fire).
            let score = if needs_path {
                index.path_into(record, &mut path_buffer);
                score(index, record, &needles, &path_buffer)
            } else {
                score(index, record, &needles, "")
            };
            out.push(Candidate { score, record });
        }
    }
    trim_candidates(out, keep);
}

/// Keep only the best `keep` candidates (score desc, then record for
/// determinism). `select_nth_unstable_by` is O(n) and runs once per worker.
fn trim_candidates(candidates: &mut Vec<Candidate>, keep: usize) {
    let keep = keep.max(1);
    if candidates.len() <= keep {
        return;
    }
    candidates.select_nth_unstable_by(keep, |left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.record.cmp(&right.record))
    });
    candidates.truncate(keep);
}

/// Whether a record passes the kind filter.
fn kind_accepts(index: &FileDb, record: u32, kind: KindFilter) -> bool {
    match kind {
        KindFilter::Any => true,
        KindFilter::FilesOnly => index.is_file(record),
        KindFilter::FoldersOnly => index.is_dir(record),
    }
}

/// Build a hit for a record that survived ranking. Only the bounded winner set
/// reaches this function, so the name `String` and `PathBuf` are allocated a
/// handful of times per query instead of once per match.
fn materialize(index: &FileDb, record: u32, score: i32) -> FileHit {
    let (size, mtime) = index.meta(record);
    let mut path = String::new();
    index.path_into(record, &mut path);
    FileHit {
        index: record,
        name: String::from_utf8_lossy(index.name_bytes(record)).into_owned(),
        path: std::path::PathBuf::from(path),
        size,
        mtime,
        is_dir: index.is_dir(record),
        score,
    }
}

/// Ranking. The folded substring match against the name is the cheap and
/// strongest signal:
///
/// 1. a term that hits the *name* counts for far more than one that only hits
///    the *path* (a `path:` query still reaches everything, but a name hit is
///    what the user meant);
/// 2. an exact or prefix name match beats a mid-name hit;
/// 3. a shorter name is a better match than a long name containing the term;
/// 4. shallower records are easier to recognise;
/// 5. everything else equal, newer wins.
fn score(index: &FileDb, record: u32, predicates: &[&str], path: &str) -> i32 {
    let name = index.name_bytes(record);
    let mut score = 0i32;
    for needle in predicates {
        let needle = needle.as_bytes();
        if needle.is_empty() {
            continue;
        }
        if query::substring_probe(name, needle) {
            score += 200;
        } else if query::substring_probe(path.as_bytes(), needle) {
            score += 60;
        }
    }
    if let Some(first) = predicates.first() {
        let name_text = String::from_utf8_lossy(name).to_lowercase();
        let needle = first.to_lowercase();
        if name_text == needle {
            score += 400;
        } else if name_text.starts_with(&needle) {
            score += 160;
        }
    }
    score -= (name.len() as i32).min(120);
    score -= (index.depth_of(record) as i32).min(20) * 2;
    if let Some(mtime) = index.meta(record).1 {
        // Recency as a tie-break, scaled so it can never outweigh the
        // match-quality terms above.
        score += ((mtime / 86_400) % 100_000) as i32 / 100;
    }
    score
}

/// Rank the retained candidates, materialise the winners and order them.
fn finish_candidates(
    index: &FileDb,
    mut candidates: Vec<Candidate>,
    keep: usize,
    options: &SearchOptions,
    stats: SearchStats,
) -> SearchOutcome {
    trim_candidates(&mut candidates, keep);
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.record.cmp(&right.record))
    });
    candidates.truncate(options.limit);
    let mut hits: Vec<FileHit> = candidates
        .into_iter()
        .map(|candidate| materialize(index, candidate.record, candidate.score))
        .collect();
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    SearchOutcome { hits, stats }
}

/// `ceil(block_count / 16)`, clamped by the core count.
///
/// The worker count derives from the number of *index blocks*
/// (`ceil(block_count / 16)`), not from the number of files.
fn worker_count(blocks: usize) -> usize {
    let by_blocks = blocks.div_ceil(BLOCKS_PER_WORKER);
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4);
    by_blocks.min(cores).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_index::db::{EntryInfo, FileDbBuilder};

    /// A tree with `count` files spread over `dirs` directories.
    fn tree(count: usize, dirs: usize) -> FileDb {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        let mut folders = Vec::new();
        for dir in 0..dirs {
            folders.push(builder.add_child(root, &EntryInfo::dir(format!("dir{dir}"))));
        }
        for file in 0..count {
            let folder = folders[file % dirs.max(1)];
            builder.add_child(
                folder,
                &EntryInfo::file(format!("file-{file:05}.txt"))
                    .with_size((file as u64) * 1024)
                    .with_mtime(100 + file as u64),
            );
        }
        builder.finalize()
    }

    #[test]
    fn finds_a_file_by_name() {
        let index = tree(200, 4);
        let outcome = search(&index, "file-00007", &SearchOptions::with_limit(10));
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].name, "file-00007.txt");
        assert!(outcome.hits[0]
            .path
            .to_string_lossy()
            .starts_with("C:\\dir"));
    }

    #[test]
    fn results_are_capped_by_the_limit() {
        let index = tree(500, 5);
        let outcome = search(&index, "file", &SearchOptions::with_limit(20));
        assert_eq!(outcome.hits.len(), 20);
        assert!(outcome.stats.matched >= 500);
    }

    #[test]
    fn a_query_with_no_match_returns_nothing() {
        let index = tree(50, 2);
        let outcome = search(&index, "definitely-not-here", &SearchOptions::default());
        assert!(outcome.hits.is_empty());
        assert!(!outcome.stats.cancelled);
    }

    #[test]
    fn folder_queries_exclude_files() {
        let index = tree(20, 3);
        let outcome = search(&index, "dir1", &SearchOptions::with_limit(10));
        assert!(outcome.hits.iter().all(|hit| hit.is_dir));
        assert!(!outcome.hits.is_empty());
    }

    #[test]
    fn path_scoped_queries_reach_parent_directories() {
        let index = tree(9, 1);
        // "dir0" is a parent, never part of a file name.
        let outcome = search(
            &index,
            "path:dir0 file-00001",
            &SearchOptions::with_limit(5),
        );
        assert_eq!(outcome.hits.len(), 1);
    }

    #[test]
    fn empty_query_lists_everything_in_index_order() {
        let index = tree(10, 2);
        let outcome = search(&index, "", &SearchOptions::with_limit(100));
        assert_eq!(outcome.hits.len(), index.len());
    }

    #[test]
    fn empty_query_with_recency_sort_puts_newest_first() {
        let index = tree(10, 2);
        let options = SearchOptions {
            limit: 5,
            kind: KindFilter::FilesOnly,
            sort_by_recency: true,
        };
        let outcome = search(&index, "", &options);
        let mtimes: Vec<u64> = outcome.hits.iter().filter_map(|hit| hit.mtime).collect();
        let mut sorted = mtimes.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(mtimes, sorted);
    }

    #[test]
    fn contradictory_queries_short_circuit() {
        let index = tree(10, 2);
        let outcome = search(&index, "file !file", &SearchOptions::default());
        assert!(outcome.hits.is_empty());
        assert_eq!(
            outcome.stats.scanned, 0,
            "an unsatisfiable query is skipped"
        );
    }

    #[test]
    fn cancellation_stops_the_scan() {
        // A big index and a broad query: cancel before searching so every
        // worker sees the flag on its first record.
        let index = tree(20_000, 32);
        assert!(index.block_count() > 1);
        let cancel = Cancel::new();
        cancel.cancel();
        let outcome = search_filtered(
            &index,
            &query::parse("file"),
            &SearchOptions::with_limit(10),
            Some(&cancel),
        );
        assert!(outcome.stats.cancelled);
        assert!(outcome.stats.scanned < index.len());
    }

    #[test]
    fn name_hits_outrank_path_only_hits() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        let reports = builder.add_child(root, &EntryInfo::dir("reports"));
        let other = builder.add_child(root, &EntryInfo::dir("other"));
        builder.add_child(reports, &EntryInfo::file("yearly-summary.txt"));
        builder.add_child(other, &EntryInfo::file("reports-summary.txt"));
        let index = builder.finalize();

        // Without `path:`, only names match: the file inside `reports\` does
        // not, the file whose own name contains the term does.
        let name_only = search(&index, "reports", &SearchOptions::with_limit(10));
        let names: Vec<&str> = name_only.hits.iter().map(|hit| hit.name.as_str()).collect();
        assert!(names.contains(&"reports-summary.txt"), "got {names:?}");
        assert!(!names.contains(&"yearly-summary.txt"), "got {names:?}");

        // With `path:`, both match, and the one whose *name* contains the term
        // must rank above the one that only matches through its parent.
        let path_scoped = search(
            &index,
            "path:reports summary",
            &SearchOptions::with_limit(10),
        );
        let names: Vec<&str> = path_scoped
            .hits
            .iter()
            .map(|hit| hit.name.as_str())
            .collect();
        assert_eq!(names, vec!["reports-summary.txt", "yearly-summary.txt"]);
    }

    #[test]
    fn exact_and_prefix_name_matches_rank_highest() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        builder.add_child(root, &EntryInfo::file("notes-archive.txt"));
        builder.add_child(root, &EntryInfo::file("notes.txt"));
        builder.add_child(root, &EntryInfo::file("my-notes.txt"));
        let index = builder.finalize();
        let outcome = search(&index, "notes", &SearchOptions::with_limit(10));
        assert_eq!(outcome.hits[0].name, "notes.txt");
    }

    /// The 3-gram path must return exactly the same hits as the linear scan;
    /// postings are a superset that verification then narrows.
    #[test]
    fn the_name_index_answers_exactly_what_the_scan_does() {
        let plain = tree(500, 8);
        let mut accelerated = tree(500, 8);
        assert!(accelerated.build_name_index_with_min(1));

        for (query, limit) in [
            ("file", 20usize),
            ("file-0001", 20),
            ("00012", 20),
            ("dir3", 20),
            ("FILE-00042", 20),
            ("missing", 20),
            ("fi", 20),
        ] {
            let options = SearchOptions::with_limit(limit);
            let fast = search(&accelerated, query, &options);
            let exact = search(&plain, query, &options);
            assert_eq!(
                fast.hits, exact.hits,
                "the accelerator must not change the result for {query:?}"
            );
        }
    }

    #[test]
    fn worker_count_follows_the_block_count() {
        // `ceil(blocks / 16)`, clamped by the core count.
        assert_eq!(worker_count(0), 1);
        assert_eq!(worker_count(1), 1);
        assert_eq!(worker_count(16), 1);
        assert_eq!(worker_count(17), 2);
        assert!(worker_count(100_000) <= 64);
    }

    #[test]
    fn stats_account_for_every_scanned_record() {
        let index = tree(1_000, 8);
        let outcome = search(&index, "file-0000", &SearchOptions::with_limit(10));
        // Every live record is visited exactly once for a single-block-range
        // scan, regardless of how many workers split it.
        assert_eq!(outcome.stats.scanned, index.len());
        assert_eq!(outcome.stats.blocks, index.block_count());
    }
}

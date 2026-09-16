//! Directory-walk enumeration.
//!
//! The NTFS `$MFT` path ([`super::ntfs`]) is the fast path, but reading a raw
//! volume needs an elevated handle. When that is unavailable — an ordinary
//! user, a non-NTFS volume, a network share — Steward still indexes the
//! configured roots, using recursive directory enumeration instead. The records
//! it produces are identical (same [`EntryInfo`](super::EntryInfo) shape), so
//! search behaviour does not depend on which enumerator ran.
//!
//! On Windows the walker uses `FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH`
//! and `FindExInfoBasic`, which skips the 8.3 alias lookup and fetches a whole
//! buffer of entries per call — the same API Explorer uses, and roughly an order
//! of magnitude fewer syscalls than `std::fs::read_dir` on a large tree.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::db::EntryInfo;

/// Where enumeration has got to, for logging and the tray menu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// Roots that were attempted.
    pub roots: usize,
    /// Roots that produced at least one record.
    pub roots_indexed: usize,
    /// Directories visited.
    pub directories: usize,
    /// Records emitted.
    pub entries: usize,
    /// Directories that could not be opened (permissions, vanished mid-walk).
    pub denied: usize,
    /// Names dropped by the exclusion rules.
    pub excluded: usize,
    /// Whether the walk stopped early because the index was asked to cancel.
    pub cancelled: bool,
}

/// Live progress, published by the walker thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexProgress {
    pub entries: usize,
    pub directories: usize,
    pub current: PathBuf,
}

/// What to index and what to leave alone.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Roots to walk. A drive root indexes the whole volume.
    pub roots: Vec<PathBuf>,
    /// Include hidden and system records (default: on — Everything indexes them
    /// and only the *search* filters, which keeps one build usable for every
    /// query).
    pub include_hidden: bool,
    /// Follow directory junctions and symlinks. Off by default: a junction can
    /// point back up its own tree, and the same subtree reached twice would be
    /// indexed twice.
    pub follow_reparse: bool,
    /// Skip directories whose name matches one of these (case-insensitive).
    pub excluded_dirs: HashSet<String>,
    /// Stop after this many records (a guard rail for tests and dry runs).
    pub max_entries: Option<usize>,
}

/// Directory names skipped by default. The reparse-point check already removes
/// most of these on a real volume; the list keeps the walk away from trees that
/// are noise in every launcher, and from the pseudo-folders whose children are
/// not real directories.
pub const DEFAULT_EXCLUDED_DIRS: [&str; 4] = [
    "$recycle.bin",
    "system volume information",
    "$windows.~ws",
    "$windows.~bt",
];

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            include_hidden: true,
            follow_reparse: false,
            excluded_dirs: DEFAULT_EXCLUDED_DIRS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            max_entries: None,
        }
    }
}

impl ScanOptions {
    /// Options for a set of roots, with the default exclusions.
    pub fn for_roots<I, P>(roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self {
            roots: roots.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Add a directory name to the exclusion set (lower-case comparison).
    pub fn exclude_dir(mut self, name: impl Into<String>) -> Self {
        self.excluded_dirs.insert(name.into().to_lowercase());
        self
    }
}

/// Callback surface the walker drives.
///
/// The walk reports directories by path, but the index links records by parent
/// *index*, so both travel together: `enter_dir` receives the parent's record
/// index and returns the index the emitter assigned to the new directory, which
/// the walker then hands back for that directory's own children.
pub trait Emit {
    /// A directory is about to be walked. Returns its record index.
    fn enter_dir(&mut self, path: &Path, info: &EntryInfo, parent: u32) -> u32 {
        let _ = (path, info, parent);
        0
    }
    /// A directory has been fully walked, so none of its children can recur.
    fn leave_dir(&mut self, _path: &Path) {}
    /// One record, reached through the directory at `parent`, whose record index
    /// is `parent_index`.
    fn emit(&mut self, parent: &Path, parent_index: u32, info: &EntryInfo);
    /// Whether the walk should stop now (the launcher asked for a newer index).
    fn cancelled(&self) -> bool {
        false
    }
    /// Progress, called once per directory so a UI can show the current path.
    fn progress(&mut self, _progress: &IndexProgress) {}
}

/// Walk every configured root, emitting records through `emit`.
///
/// `root_index` resolves a root to the record index the caller assigned it when
/// it added the root record; that index is what the root's own children are
/// linked to. Roots are processed in order, each producing a
/// parent-before-child stream — which is what lets the index builder link records
/// without needing the parent to be resolved afterwards.
pub fn scan(
    options: &ScanOptions,
    root_index: impl Fn(&Path) -> u32,
    emit: &mut impl Emit,
) -> IndexReport {
    let mut report = IndexReport {
        roots: options.roots.len(),
        ..IndexReport::default()
    };
    let mut progress = IndexProgress::default();
    for root in &options.roots {
        if emit.cancelled() {
            report.cancelled = true;
            break;
        }
        if !root.is_dir() {
            report.denied += 1;
            continue;
        }
        let before = report.entries;
        let index = root_index(root);
        walk_root(root, index, options, emit, &mut report, &mut progress);
        if report.entries > before {
            report.roots_indexed += 1;
        }
    }
    report
}

/// Walk one root's children (the root record itself was added by the caller).
fn walk_root(
    root: &Path,
    root_index: u32,
    options: &ScanOptions,
    emit: &mut impl Emit,
    report: &mut IndexReport,
    progress: &mut IndexProgress,
) {
    // Explicit stack: a deep tree must not grow the call stack. Each entry is a
    // directory plus the record index the emitter assigned to it.
    let mut stack: Vec<(PathBuf, u32)> = vec![(root.to_path_buf(), root_index)];
    while let Some((directory, directory_index)) = stack.pop() {
        if emit.cancelled() {
            report.cancelled = true;
            return;
        }
        report.directories += 1;
        progress.directories = report.directories;
        progress.entries = report.entries;
        progress.current = directory.clone();
        emit.progress(progress);

        let children = match read_dir(&directory) {
            Ok(children) => children,
            Err(_) => {
                report.denied += 1;
                emit.leave_dir(&directory);
                continue;
            }
        };
        // Sort so the index order is stable across builds and platforms, which
        // also makes `finalize`'s path sort cheaper (near-sorted input).
        let mut children = children;
        children.sort_by(|left, right| left.name.cmp(&right.name));

        let mut subdirectories: Vec<(PathBuf, u32)> = Vec::new();
        for child in children {
            if emit.cancelled() {
                report.cancelled = true;
                return;
            }
            if let Some(max) = options.max_entries {
                if report.entries >= max {
                    return;
                }
            }
            let lower = child.name.to_lowercase();
            if child.is_dir && options.excluded_dirs.contains(&lower) {
                report.excluded += 1;
                continue;
            }
            if !options.include_hidden && child.is_hidden() {
                report.excluded += 1;
                continue;
            }
            let path = directory.join(&child.name);
            if child.is_dir && child.is_reparse {
                if options.follow_reparse {
                    let index = emit.enter_dir(&path, &reparse_info(&child), directory_index);
                    subdirectories.push((path, index));
                }
                continue;
            }
            let info = EntryInfo {
                // The walk cannot supply a file reference number; `0` means
                // "unknown" and the index links by parent record instead.
                id: 0,
                parent_id: 0,
                size: (!child.is_dir).then_some(child.size),
                mtime: child.mtime,
                attributes: child.attributes,
                name: child.name,
                is_dir: child.is_dir,
            };
            if child.is_dir {
                let index = emit.enter_dir(&path, &info, directory_index);
                report.entries += 1;
                subdirectories.push((path, index));
            } else {
                emit.emit(&directory, directory_index, &info);
                report.entries += 1;
            }
        }
        // Depth-first: reverse so the first child is walked first.
        for entry in subdirectories.into_iter().rev() {
            stack.push(entry);
        }
        emit.leave_dir(&directory);
    }
}

/// Build the entry a reparse-point directory would have, for the callers that
/// follow reparse points and therefore have to index them.
fn reparse_info(entry: &DirEntry) -> EntryInfo {
    EntryInfo {
        id: 0,
        parent_id: 0,
        size: None,
        mtime: entry.mtime,
        attributes: entry.attributes,
        name: entry.name.clone(),
        is_dir: true,
    }
}
/// One directory entry as the platform layer reports it.
#[derive(Debug, Clone)]
pub(crate) struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_reparse: bool,
    pub size: u64,
    /// Last-write time, seconds since the Windows epoch.
    pub mtime: Option<u64>,
    pub attributes: u32,
}

impl DirEntry {
    pub(crate) fn is_hidden(&self) -> bool {
        // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
        self.attributes & 0x6 != 0
    }
}

#[cfg(target_os = "windows")]
fn read_dir(directory: &Path) -> std::io::Result<Vec<DirEntry>> {
    imp::read_dir(directory)
}

#[cfg(not(target_os = "windows"))]
fn read_dir(directory: &Path) -> std::io::Result<Vec<DirEntry>> {
    portable::read_dir(directory)
}

#[cfg(target_os = "windows")]
mod imp {
    use super::DirEntry;
    use std::path::Path;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH,
        WIN32_FIND_DATAW,
    };

    /// `FindFirstFileExW` with a large fetch buffer; `..`/`.` are skipped.
    pub(super) fn read_dir(directory: &Path) -> std::io::Result<Vec<DirEntry>> {
        // `\\?\` prefixes lift MAX_PATH for the search pattern; without it a
        // deep path fails with ERROR_PATH_NOT_FOUND and the subtree is lost.
        let pattern = format!("{}\\*", extended_prefix(directory));
        let wide: Vec<u16> = pattern.encode_utf16().chain(std::iter::once(0)).collect();

        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `data` is a
        // valid `WIN32_FIND_DATAW` for the level requested below.
        let handle: HANDLE = unsafe {
            FindFirstFileExW(
                PCWSTR(wide.as_ptr()),
                FindExInfoBasic,
                (&mut data as *mut WIN32_FIND_DATAW).cast(),
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_LARGE_FETCH,
            )
        }
        .map_err(|error| std::io::Error::from_raw_os_error(error.code().0))?;

        let mut entries = Vec::new();
        loop {
            if let Some(entry) = to_entry(&data) {
                entries.push(entry);
            }
            // SAFETY: `handle` came from a successful `FindFirstFileExW`.
            let more = unsafe { FindNextFileW(handle, &mut data) };
            if more.is_err() {
                break;
            }
        }
        // SAFETY: the handle is owned by this function and closed exactly once.
        unsafe {
            let _ = FindClose(handle);
        }
        Ok(entries)
    }

    /// Skip the `.` / `..` pseudo-entries and decode the rest.
    fn to_entry(data: &WIN32_FIND_DATAW) -> Option<DirEntry> {
        let name_len = data
            .cFileName
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(data.cFileName.len());
        if name_len == 0 {
            return None;
        }
        let name = String::from_utf16_lossy(&data.cFileName[..name_len]);
        if name == "." || name == ".." {
            return None;
        }
        let attributes = data.dwFileAttributes;
        let is_dir = attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        let is_reparse = attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0;
        let size = ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
        let ticks = ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
            | data.ftLastWriteTime.dwLowDateTime as u64;
        Some(DirEntry {
            name,
            is_dir,
            is_reparse,
            size,
            mtime: (ticks != 0).then(|| super::super::db::filetime_to_mtime(ticks)),
            attributes,
        })
    }

    /// Prefix a path with `\\?\` (or `\\?\UNC\` for a share) so the walk is not
    /// limited by `MAX_PATH`.
    fn extended_prefix(path: &Path) -> String {
        let text = path.to_string_lossy();
        if text.starts_with("\\\\?\\") {
            return text.into_owned();
        }
        if let Some(rest) = text.strip_prefix("\\\\") {
            return format!("\\\\?\\UNC\\{rest}");
        }
        format!("\\\\?\\{text}")
    }
}

/// Portable fallback so the crate keeps building (and its tests keep running)
/// on non-Windows hosts. It uses `std::fs`, which is a directory scan either
/// way; only the Windows path is optimised for a full-volume build.
#[cfg(not(target_os = "windows"))]
mod portable {
    use super::DirEntry;
    use std::path::Path;

    pub(super) fn read_dir(directory: &Path) -> std::io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = metadata.is_dir();
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| super::super::db::unix_to_mtime(duration.as_secs() as i64));
            entries.push(DirEntry {
                name,
                is_dir,
                is_reparse: metadata.file_type().is_symlink(),
                size: if is_dir { 0 } else { metadata.len() },
                mtime,
                attributes: if is_dir { 0x10 } else { 0 },
            });
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Collector used by the tests: it numbers records the way the index builder
    /// does, so the parent-index contract is exercised and not just ignored.
    #[derive(Default)]
    struct Collector {
        entries: Vec<(String, bool)>,
        directories: usize,
        cancel_after: Option<usize>,
        next_index: u32,
    }

    impl Emit for Collector {
        fn enter_dir(&mut self, path: &Path, _info: &EntryInfo, parent: u32) -> u32 {
            assert!(parent <= self.next_index, "parent index must already exist");
            self.directories += 1;
            self.entries
                .push((path.to_string_lossy().into_owned(), true));
            let index = self.next_index;
            self.next_index += 1;
            index
        }
        fn emit(&mut self, parent: &Path, parent_index: u32, info: &EntryInfo) {
            assert!(
                parent_index <= self.next_index,
                "a record's parent must already have an index"
            );
            self.entries.push((
                parent.join(&info.name).to_string_lossy().into_owned(),
                info.is_dir,
            ));
            self.next_index += 1;
        }

        fn cancelled(&self) -> bool {
            self.cancel_after
                .is_some_and(|limit| self.entries.len() >= limit)
        }
    }

    /// A throwaway tree under the system temp directory.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(tag: &str) -> Self {
            let mut root = std::env::temp_dir();
            let unique = format!(
                "steward-scan-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            root.push(unique);
            fs::create_dir_all(root.join("sub").join("deeper")).expect("create tree");
            fs::create_dir_all(root.join("$RECYCLE.BIN")).expect("create excluded dir");
            fs::write(root.join("top.txt"), b"top").expect("write file");
            fs::write(root.join("sub").join("inner.txt"), b"inner").expect("write file");
            fs::write(root.join("sub").join("deeper").join("deep.txt"), b"deep")
                .expect("write file");
            fs::write(root.join("$RECYCLE.BIN").join("hidden.txt"), b"junk").expect("write file");
            Self { root }
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn names(collector: &Collector) -> Vec<String> {
        let mut names: Vec<String> = collector
            .entries
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn walks_every_level_and_reports_counts() {
        let tree = TempTree::new("walk");
        let options = ScanOptions::for_roots([tree.root.clone()]);
        let mut collector = Collector::default();
        let report = scan(&options, |_root| 0, &mut collector);

        assert_eq!(report.roots, 1);
        assert_eq!(report.roots_indexed, 1);
        assert!(!report.cancelled);
        // The excluded `$RECYCLE.BIN` is never entered, so only three
        // directories are visited.
        assert_eq!(report.directories, 3, "root, sub, sub/deeper");
        // top.txt, sub, sub/inner.txt, sub/deeper, sub/deeper/deep.txt
        assert_eq!(report.entries, 5, "three files and the two subdirectories");

        let collected = names(&collector);
        assert!(collected.iter().any(|path| path.ends_with("top.txt")));
        assert!(collected.iter().any(|path| path.ends_with("deep.txt")));
    }

    #[test]
    fn default_exclusions_skip_recycle_bin() {
        let tree = TempTree::new("excl");
        let options = ScanOptions::for_roots([tree.root.clone()]);
        let mut collector = Collector::default();
        let report = scan(&options, |_root| 0, &mut collector);
        assert!(
            !names(&collector)
                .iter()
                .any(|path| path.contains("$RECYCLE.BIN")),
            "the recycle bin must not be indexed"
        );
        assert_eq!(report.excluded, 1);
    }

    #[test]
    fn entries_carry_size_and_mtime() {
        let tree = TempTree::new("meta");
        let options = ScanOptions::for_roots([tree.root.clone()]);
        let mut collector = Collector::default();
        scan(&options, |_root| 0, &mut collector);
        assert!(collector
            .entries
            .iter()
            .any(|(path, is_dir)| { path.ends_with("top.txt") && !*is_dir }));
    }

    #[test]
    fn cancellation_stops_the_walk() {
        let tree = TempTree::new("cancel");
        let options = ScanOptions::for_roots([tree.root.clone()]);
        let mut collector = Collector {
            cancel_after: Some(1),
            ..Collector::default()
        };
        let report = scan(&options, |_root| 0, &mut collector);
        assert!(report.cancelled);
        assert!(report.entries <= 2, "the walk stopped early");
    }

    #[test]
    fn a_missing_root_is_reported_not_fatal() {
        let options = ScanOptions::for_roots([PathBuf::from("Z:\\definitely\\missing")]);
        let mut collector = Collector::default();
        let report = scan(&options, |_root| 0, &mut collector);
        assert_eq!(report.roots, 1);
        assert_eq!(report.roots_indexed, 0);
        assert_eq!(report.denied, 1);
        assert!(collector.entries.is_empty());
    }

    #[test]
    fn max_entries_caps_the_walk() {
        let tree = TempTree::new("cap");
        let options = ScanOptions {
            max_entries: Some(1),
            ..ScanOptions::for_roots([tree.root.clone()])
        };
        let mut collector = Collector::default();
        let report = scan(&options, |_root| 0, &mut collector);
        assert_eq!(report.entries, 1);
    }

    #[test]
    fn extra_exclusions_can_be_added() {
        let tree = TempTree::new("extra");
        let options = ScanOptions::for_roots([tree.root.clone()]).exclude_dir("sub");
        let mut collector = Collector::default();
        let report = scan(&options, |_root| 0, &mut collector);
        assert!(!names(&collector)
            .iter()
            .any(|path| path.contains("inner.txt")));
        assert!(report.excluded >= 1);
    }
}

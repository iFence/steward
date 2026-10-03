//! Full-disk file indexing and retrieval.
//!
//! A compact, resident, name-only index that answers a query by scanning itself
//! in parallel, rather than by walking the filesystem per query.
//!
//! Module map:
//!
//! | Concern | Here |
//! |---|---|
//! | Enumeration records handed to the builder | [`EntryInfo`] |
//! | Enumerate → accept → resolve parents → sort | [`FileDbBuilder`], [`scan`] |
//! | Record layout: parent pointer, 1-byte name length, UTF-8 name, metadata after it | [`db`] |
//! | `0xff` length byte → real `u32` length at `record - 4` | [`db::FileDb::entry`] |
//! | Directory/file name-pointer arrays + block index | [`db::FileDb::finalize`] |
//! | Parse terms/modifiers → compile file+folder predicates | [`query::parse`] |
//! | `ceil(block_count / 16)` workers over index blocks | [`search::search_parallel`] |
//! | Single-operation fast path, otherwise the `next`/`notnext` graph | [`search::match_record`] |
//! | USN Journal incremental maintenance | [`update`], [`ntfs`] |
//! | Snapshot persistence header | `FileDb::to_bytes` / `FileDb::from_bytes` |
//!
//! Design notes:
//!
//! - **NTFS MFT is the fast path, a directory walk is the fallback.** Reading
//!   `\\.\C:` and its `$MFT` needs an elevated handle; when that fails the index
//!   is built with `FindFirstFileExW` instead (same record shape, slower build).
//! - **Names are UTF-8, not UTF-16.** CJK names therefore cost 3 bytes per
//!   character instead of 2.
//! - **No hand-rolled regex engine.** The `regex` crate is used for `regex:`
//!   terms.
//! - **No `content:` (full-text) search.** The index is a name index, not an
//!   inverted index over file contents.

use std::path::{Path, PathBuf};

mod db;
mod name_index;
pub mod persist;
mod query;
mod scan;
mod search;

#[cfg(target_os = "windows")]
mod update;

#[cfg(target_os = "windows")]
pub mod ntfs;

#[cfg(target_os = "windows")]
pub mod watch;

pub use db::{
    describe_bytes, filetime_to_mtime, mtime_to_unix, unix_to_mtime, EntryInfo, FileDb,
    FileDbBuilder, FileEntry, IndexError, JournalState, BLOCKS, FORMAT_VERSION, INLINE_NAME_MAX,
    MAGIC, MAX_NAME_BYTES, ROOT_PARENT,
};
pub use name_index::{NameIndex, MIN_RECORDS};
pub use query::{
    evaluate, match_tier, parse as parse_filter, CaseMode, Compare, Filter, Haystack, KindFilter,
    MatchMode, MatchTier, Predicate, Scope, SizeFilter, TextPredicate,
};
pub use scan::{scan, Emit, IndexProgress, IndexReport, ScanOptions, DEFAULT_EXCLUDED_DIRS};
pub use search::{
    search, search_filtered, Cancel, FileHit, SearchOptions, SearchOutcome, SearchStats,
};

#[cfg(target_os = "windows")]
pub use ntfs::{DataRun, NtfsError, RawVolume, UsnReadOutcome, UsnRecord, VolumeGeometry};
#[cfg(target_os = "windows")]
pub use update::{
    apply_fs_changes, apply_usn_records, cursor_is_usable, volume_root, FsOutcome, UsnOutcome,
};
#[cfg(target_os = "windows")]
pub use watch::{DirectoryWatcher, FsAction, FsChange, WatchBatch};

/// Which enumerator produced a root's records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexBackend {
    /// Direct `$MFT` parsing on an NTFS volume (the fast path).
    Mft,
    /// Recursive directory enumeration.
    Walk,
}

impl IndexBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            IndexBackend::Mft => "mft",
            IndexBackend::Walk => "walk",
        }
    }
}

impl std::fmt::Display for IndexBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A root's starting point: a drive letter (`C`) or an absolute folder path.
///
/// Parsing is deliberately forgiving so the launcher can accept whatever the
/// user typed in a settings field:
///
/// - `C`, `c:`, `C:\`, `C:/` → the volume root of drive `C`
/// - `D:\Media`, `D:/Media`, `D:\Media\` → that folder, on the volume's backend
/// - a bare absolute folder path → that folder
pub fn parse_root(spec: &str) -> Option<PathBuf> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    // A drive root: `C`, `c`, `C:`, `C:\`, `c:/`.
    if bytes[0].is_ascii_alphabetic() && (trimmed.len() == 1 || bytes[1] == b':') {
        let rest = trimmed.get(2..).unwrap_or("").replace(['/', '\\'], "");
        if rest.is_empty() {
            return Some(PathBuf::from(format!(
                "{}:\\",
                trimmed[..1].to_ascii_uppercase()
            )));
        }
    }
    // `C:\dir`, `C:/dir`: normalize the separator after the colon so the rest of
    // the codebase can rely on Windows-style paths.
    if trimmed.len() > 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let mut rest = trimmed[2..].replace('/', "\\");
        if !rest.starts_with('\\') {
            rest.insert(0, '\\');
        }
        let rest = rest.trim_end_matches('\\');
        return Some(PathBuf::from(format!(
            "{}:{}",
            trimmed[..1].to_ascii_uppercase(),
            rest
        )));
    }
    let path = Path::new(trimmed);
    path.is_absolute().then(|| path.to_path_buf())
}

/// Parse a settings string into a de-duplicated root list, preserving order.
pub fn parse_roots(specs: &[String]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for spec in specs {
        if let Some(root) = parse_root(spec) {
            if !roots.iter().any(|existing| existing == &root) {
                roots.push(root);
            }
        }
    }
    roots
}

/// The drive letter of an absolute path (`C:\...` → `Some('C')`).
pub fn drive_letter(path: &Path) -> Option<u8> {
    let bytes = path.to_string_lossy();
    let bytes = bytes.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        Some(bytes[0].to_ascii_uppercase())
    } else {
        None
    }
}

/// Whether `path` names a volume root (`C:\`).
pub fn is_volume_root(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let text = text.trim_end_matches(['\\', '/']);
    text.len() == 2 && text.as_bytes()[1] == b':'
}

/// Every fixed (local, non-removable) drive root, e.g. `C:\`, `D:\`.
///
/// This is the default set when nothing is configured. `GetDriveTypeW` filters
/// out removable media, network shares, CD-ROMs and RAM disks: those may not be
/// present (or readable) at the next start, and indexing them would make the
/// index's contents depend on what happened to be plugged in. All fixed volumes
/// are returned, so a multi-disk machine is indexed in full.
///
/// Returns an empty vector on non-Windows targets.
#[cfg(target_os = "windows")]
pub fn fixed_drive_roots() -> Vec<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    /// `DRIVE_FIXED` from `winbase.h`.
    const DRIVE_FIXED: u32 = 3;

    // SAFETY: no arguments, no output buffers; the bitmask is fully described
    // by the API contract.
    let mask = unsafe { GetLogicalDrives() };
    let mut roots = Vec::new();
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = (b'A' + index as u8) as char;
        let wide: Vec<u16> = format!("{letter}:\\")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        let kind = unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) };
        if kind == DRIVE_FIXED {
            roots.push(PathBuf::from(format!("{letter}:\\")));
        }
    }
    roots
}

/// Every fixed drive root; empty on non-Windows targets.
#[cfg(not(target_os = "windows"))]
pub fn fixed_drive_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_roots_are_normalized() {
        for spec in ["C", "c", "C:", "C:\\", "c:/"] {
            let root = parse_root(spec).unwrap_or_else(|| panic!("{spec} should parse"));
            assert_eq!(root, PathBuf::from("C:\\"), "spec {spec}");
        }
    }

    #[test]
    fn directories_keep_one_separator_style() {
        assert_eq!(
            parse_root("D:/Media/Photos").unwrap(),
            PathBuf::from("D:\\Media\\Photos")
        );
        assert_eq!(
            parse_root("D:\\Media\\Photos\\").unwrap(),
            PathBuf::from("D:\\Media\\Photos")
        );
    }

    #[test]
    fn relative_and_empty_specs_are_rejected() {
        assert!(parse_root("").is_none());
        assert!(parse_root("   ").is_none());
        assert!(parse_root("relative\\dir").is_none());
        assert!(parse_root("12").is_none());
    }

    #[test]
    fn roots_are_deduplicated_in_order() {
        let roots = parse_roots(&["C:".into(), "c:\\".into(), "D".into()]);
        assert_eq!(roots, vec![PathBuf::from("C:\\"), PathBuf::from("D:\\")]);
    }

    #[test]
    fn drive_letters_and_volume_roots_are_detected() {
        assert_eq!(drive_letter(Path::new("C:\\Windows")), Some(b'C'));
        assert_eq!(drive_letter(Path::new("\\\\server\\share")), None);
        assert!(is_volume_root(Path::new("C:\\")));
        assert!(!is_volume_root(Path::new("C:\\Windows")));
    }
}

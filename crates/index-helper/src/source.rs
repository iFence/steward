//! What the helper streams: one volume's records at a time.
//!
//! The elevated fast path opens `\\.\X:` and walks the `$MFT`; when that is not
//! possible (not elevated, not NTFS, a configured folder root) the same records
//! are produced by the directory walk. Both paths emit the same
//! [`RecordFrame`](crate::protocol::RecordFrame) shape, so the client does not
//! care which one ran.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use steward_core_engine::file_index::{self, scan, Emit, EntryInfo, IndexProgress, ScanOptions};

use crate::protocol::{write_frame, Backend, EndFrame, Frame, RecordFrame, VolumeFrame};

/// Default directory names skipped on top of the walker's own list, matching the
/// app's local build.
pub const DEFAULT_EXCLUDED_DIRS: [&str; 4] = ["node_modules", ".git", "winsxs", "$recycle.bin"];

/// One client request, sent as a single JSON line before the binary stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamRequest {
    pub version: u32,
    pub roots: Vec<String>,
    #[serde(default)]
    pub excluded_dirs: Vec<String>,
    /// Keep the connection open after the snapshot and stream USN deltas.
    /// Snapshot-only callers (tests, tooling) can turn this off.
    #[serde(default = "default_live")]
    pub live: bool,
}

fn default_live() -> bool {
    true
}

impl StreamRequest {
    /// A request for `roots` with the app's default exclusions.
    pub fn new(roots: impl IntoIterator<Item = String>) -> Self {
        Self {
            version: crate::protocol::PROTOCOL_VERSION,
            roots: roots.into_iter().collect(),
            excluded_dirs: DEFAULT_EXCLUDED_DIRS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            live: true,
        }
    }

    /// Compile the request into scan options.
    pub fn options(&self) -> ScanOptions {
        let roots: Vec<PathBuf> = self
            .roots
            .iter()
            .filter_map(|spec| file_index::parse_root(spec))
            .collect();
        let mut options = ScanOptions::for_roots(roots);
        // The walker's own defaults ($RECYCLE.BIN, System Volume Information,
        // Windows upgrade leftovers) stay in place; the request adds its list on
        // top, matching the app's local build.
        for name in &self.excluded_dirs {
            options.excluded_dirs.insert(name.to_lowercase());
        }
        options
    }
}

/// How many records a stream carried.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub records: u64,
    pub directories: u64,
}

/// One MFT volume kept open so the server can pump live USN deltas.
///
/// The directory map is portable; the volume handle and journal cursor only
/// exist on Windows, where the fast path does.
pub struct VolumeSession {
    #[cfg(target_os = "windows")]
    pub volume: steward_core_engine::file_index::RawVolume,
    #[cfg(target_os = "windows")]
    pub journal: steward_core_engine::file_index::JournalState,
    pub dirs: crate::delta::DirIndex,
}

/// Stream every configured root, then an [`EndFrame`].
pub fn stream(options: &ScanOptions, writer: &mut impl Write) -> io::Result<Summary> {
    Ok(stream_with_sessions(options, writer)?.0)
}

/// [`stream`], plus the volumes that can be maintained live from their journal.
///
/// An empty session list means the stream is snapshot-only: every root was a
/// directory walk, or no volume had a usable USN journal.
pub fn stream_with_sessions(
    options: &ScanOptions,
    writer: &mut impl Write,
) -> io::Result<(Summary, Vec<VolumeSession>)> {
    let mut summary = Summary::default();
    let mut sessions = Vec::new();
    for root in &options.roots {
        #[cfg(target_os = "windows")]
        if file_index::is_volume_root(root) {
            if let Some(letter) = file_index::drive_letter(root) {
                if stream_mft(
                    letter,
                    root,
                    options,
                    &mut *writer,
                    &mut summary,
                    &mut sessions,
                )? {
                    continue;
                }
            }
        }
        stream_walk(root, options, &mut *writer, &mut summary)?;
    }
    write_frame(
        writer,
        &Frame::End(EndFrame {
            records: summary.records,
            directories: summary.directories,
        }),
    )?;
    Ok((summary, sessions))
}

/// Walk one root and emit its volume header, root record and children.
fn stream_walk(
    root: &Path,
    options: &ScanOptions,
    writer: &mut impl Write,
    summary: &mut Summary,
) -> io::Result<()> {
    write_frame(
        writer,
        &Frame::Volume(VolumeFrame {
            drive: file_index::drive_letter(root).unwrap_or(0),
            backend: Backend::Walk,
            root_name: root.to_string_lossy().into_owned(),
        }),
    )?;

    let mut adapter = WalkStream {
        writer,
        ids: Vec::new(),
        summary: Summary::default(),
        error: None,
    };
    let root_info = root_entry(root);
    let root_index = adapter.push_root(&root_info)?;

    let mut single = options.clone();
    single.roots = vec![root.to_path_buf()];
    let report = scan(&single, |_root| root_index, &mut adapter);
    if let Some(error) = adapter.error {
        return Err(error);
    }
    if report.cancelled {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "walk cancelled"));
    }
    summary.records += adapter.summary.records;
    summary.directories += adapter.summary.directories;
    Ok(())
}

/// The record a configured folder root gets (a volume root is handled by the
/// MFT path; the walk path names it by its own path, like the local build).
fn root_entry(root: &Path) -> EntryInfo {
    EntryInfo {
        id: 1,
        parent_id: 0,
        size: None,
        mtime: std::fs::metadata(root)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| {
                duration.as_secs() + 11_644_473_600 // Windows FILETIME epoch
            }),
        attributes: 0x10,
        name: root.to_string_lossy().into_owned(),
        is_dir: true,
    }
}

/// Bridges the walker to the frame stream: the walker links by *record index*,
/// the wire links by a synthetic id equal to `index + 1` (so `0` stays "none").
struct WalkStream<'a, W: Write> {
    writer: &'a mut W,
    ids: Vec<u64>,
    summary: Summary,
    error: Option<io::Error>,
}

impl<W: Write> WalkStream<'_, W> {
    fn record(&mut self, info: &EntryInfo, is_root: bool) -> (u64, RecordFrame) {
        let id = self.ids.len() as u64 + 1;
        self.ids.push(id);
        (
            id,
            RecordFrame {
                id,
                parent_id: 0,
                size: info.size,
                mtime: info.mtime,
                attributes: info.attributes,
                name: info.name.clone(),
                is_dir: info.is_dir,
                is_reparse: info.attributes & 0x400 != 0,
                is_root,
            },
        )
    }

    fn send(&mut self, frame: RecordFrame) {
        if self.error.is_some() {
            return;
        }
        self.summary.records += 1;
        if frame.is_dir {
            self.summary.directories += 1;
        }
        if let Err(error) = write_frame(self.writer, &Frame::Record(frame)) {
            self.error = Some(error);
        }
    }

    fn push_root(&mut self, info: &EntryInfo) -> io::Result<u32> {
        let (_, frame) = self.record(info, true);
        self.send(frame);
        match self.error.take() {
            Some(error) => Err(error),
            None => Ok(0),
        }
    }

    fn parent_id(&self, parent_index: u32) -> u64 {
        self.ids.get(parent_index as usize).copied().unwrap_or(0)
    }
}

impl<W: Write> Emit for WalkStream<'_, W> {
    fn enter_dir(&mut self, _path: &Path, info: &EntryInfo, parent: u32) -> u32 {
        let parent_id = self.parent_id(parent);
        let (_, mut frame) = self.record(info, false);
        frame.parent_id = parent_id;
        self.send(frame);
        (self.ids.len() - 1) as u32
    }

    fn emit(&mut self, _parent: &Path, parent_index: u32, info: &EntryInfo) {
        let parent_id = self.parent_id(parent_index);
        let (_, mut frame) = self.record(info, false);
        frame.parent_id = parent_id;
        self.send(frame);
    }

    fn cancelled(&self) -> bool {
        self.error.is_some()
    }

    fn progress(&mut self, _progress: &IndexProgress) {}
}

/// The NTFS fast path. Returns `Ok(false)` when the volume cannot be read raw
/// (not elevated, not NTFS, no such volume) so the caller falls back to a walk.
#[cfg(target_os = "windows")]
fn stream_mft(
    letter: u8,
    root: &Path,
    options: &ScanOptions,
    writer: &mut impl Write,
    summary: &mut Summary,
    sessions: &mut Vec<VolumeSession>,
) -> io::Result<bool> {
    use file_index::RawVolume;

    let Ok(mut volume) = RawVolume::open(letter) else {
        return Ok(false);
    };
    // Capture the cursor *before* enumerating: any change that lands while the
    // snapshot is being read is then replayed as a delta (idempotent upsert),
    // instead of falling into a gap.
    let start_journal = volume
        .query_journal()
        .ok()
        .filter(|journal| journal.journal_id != 0);
    // Record 5 is the volume root; its file reference is `sequence << 48 | 5`.
    let root_id = volume
        .read_mft_record(5)
        .ok()
        .and_then(|record| {
            let sequence = u16::from_le_bytes([*record.get(16)?, *record.get(17)?]) as u64;
            Some((sequence << 48) | 5)
        })
        .unwrap_or(0);
    let root_name = root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string();
    let streamed_root_id = if root_id == 0 { 1 } else { root_id };
    let mut dirs = crate::delta::DirIndex::new();
    dirs.insert_root(streamed_root_id, root_name.clone());

    write_frame(
        writer,
        &Frame::Volume(VolumeFrame {
            drive: letter,
            backend: Backend::Mft,
            root_name: root.to_string_lossy().into_owned(),
        }),
    )?;
    write_frame(
        writer,
        &Frame::Record(RecordFrame {
            id: streamed_root_id,
            parent_id: 0,
            size: None,
            mtime: None,
            attributes: 0x10,
            name: root_name,
            is_dir: true,
            is_reparse: false,
            is_root: true,
        }),
    )?;
    summary.records += 1;
    summary.directories += 1;

    let excluded = options.excluded_dirs.clone();
    let mut write_error: Option<io::Error> = None;
    let mut sent = 0u64;
    let mut directories = 0u64;
    volume
        .enumerate_mft(|info| {
            if write_error.is_some() || (info.id == root_id && root_id != 0) {
                return;
            }
            if info.is_dir && excluded.contains(&info.name.to_lowercase()) {
                return;
            }
            if info.name.len() > crate::protocol::MAX_NAME_BYTES {
                return;
            }
            if info.is_dir {
                dirs.insert(info.id, info.parent_id, info.name.clone());
            }
            if let Err(error) = write_frame(
                writer,
                &Frame::Record(RecordFrame {
                    id: info.id,
                    parent_id: info.parent_id,
                    size: info.size,
                    mtime: info.mtime,
                    attributes: info.attributes,
                    name: info.name.clone(),
                    is_dir: info.is_dir,
                    is_reparse: info.attributes & 0x400 != 0,
                    is_root: false,
                }),
            ) {
                write_error = Some(error);
                return;
            }
            sent += 1;
            if info.is_dir {
                directories += 1;
            }
        })
        .map_err(|error| io::Error::other(error.to_string()))?;
    if let Some(error) = write_error {
        return Err(error);
    }
    if let Some(journal) = start_journal {
        sessions.push(VolumeSession {
            volume,
            journal,
            dirs,
        });
    }
    summary.records += sent;
    summary.directories += directories;
    Ok(true)
}

/// Non-Windows builds have no `$MFT` to read; everything is a walk.
#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn stream_mft(
    _letter: u8,
    _root: &Path,
    _options: &ScanOptions,
    _writer: &mut impl Write,
    _summary: &mut Summary,
    _sessions: &mut Vec<VolumeSession>,
) -> io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_parses_roots_and_exclusions() {
        let request = StreamRequest {
            version: 1,
            roots: vec!["C".into(), "D:/Media".into(), "relative".into()],
            excluded_dirs: vec!["Node_Modules".into()],
            live: true,
        };
        let options = request.options();
        assert_eq!(options.roots.len(), 2, "relative roots are dropped");
        assert!(options.excluded_dirs.contains("node_modules"));
    }

    #[test]
    fn a_walk_stream_links_parents_by_record_index() {
        let root = std::env::temp_dir().join(format!(
            "steward-helper-walk-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("file.txt"), b"x").unwrap();

        let mut wire = Vec::new();
        let options = ScanOptions::for_roots([root.clone()]);
        let summary = stream(&options, &mut wire).unwrap();
        assert!(summary.directories >= 2, "root and sub");
        assert_eq!(summary.records, summary.directories + 1, "one file");

        // Decode and check that every non-root record points at a preceding id.
        let mut reader = wire.as_slice();
        let mut roots = 0;
        let mut seen = std::collections::HashMap::new();
        while let Some(frame) = crate::protocol::read_frame(&mut reader).unwrap() {
            match frame {
                Frame::Record(record) => {
                    if record.is_root {
                        roots += 1;
                    } else {
                        assert!(
                            seen.contains_key(&record.parent_id),
                            "parent {} must precede {}",
                            record.parent_id,
                            record.name
                        );
                    }
                    seen.insert(record.id, record);
                }
                Frame::End(_) => break,
                Frame::Volume(_) | Frame::Delta(_) | Frame::Resync => {}
            }
        }
        assert_eq!(roots, 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

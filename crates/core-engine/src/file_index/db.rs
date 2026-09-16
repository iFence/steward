//! The compact, resident file index.
//!
//! Record layout, mirroring what was recovered from the Everything binary
//! (`recovered_core.c`, "Valid after rebuild converts temporary parent FRNs to
//! pointers"):
//!
//! ```text
//! Header at record[0..24] (little endian, 4-byte aligned):
//!   +00 u32 parent      index of the parent directory, or ROOT_PARENT
//!   +04 u8  name_len    0..=254, or 0xff meaning "the real u32 length follows"
//!   +05 u8  status      in-use / directory / reparse point / size valid
//!   +06 u16 meta_len    0, or sizeof(metadata) = 20
//!   +08 u64 size        bytes, meaningful only when SIZE_VALID is set
//!   +10 u64 mtime       seconds since the Windows epoch (1601), 0 = unknown
//! then the UTF-8 name (no terminator), then, when `name_len == 0xff`, a u32
//! holding the real name length (the original reads that slot as `record - 4`
//! from the name pointer), then `meta_len` bytes of metadata, then padding to a
//! 4-byte boundary.
//! ```
//!
//! Storing a parent *index* rather than a parent path is what keeps the index
//! small: a directory with 100k children stores its path once, and renaming the
//! directory only rewrites the links that point at it (report §4).

use std::collections::HashMap;
use std::path::PathBuf;

/// Parent index marking a record whose parent is not itself indexed (a root).
pub const ROOT_PARENT: u32 = u32::MAX;
/// Records per search block. The report derives worker count from *block*
/// count (`ceil(block_count / 16)`), not from file count.
pub const BLOCKS: usize = 4096;
/// Name bytes stored inline (0..=254). Longer names use the `0xff` escape with
/// a full `u32` length, like the original.
pub const INLINE_NAME_MAX: usize = 254;
/// Names longer than this are truncated at build time, which keeps the escaped
/// layout from needing a second escape level.
pub const MAX_NAME_BYTES: usize = 4096;
/// Header size; the metadata tail starts at a 4-byte boundary after the name.
const HEADER_LEN: u32 = 24;
/// Size of the metadata tail (`size` + `mtime`).
const META_LEN: u16 = 20;
/// Sentinel meaning "this record's file id is unknown".
const NO_ID: u64 = 0;
/// Parent key that never resolved to a record: the parent is outside the
/// indexed scope, which makes the record an orphan (report §4, orphan cleanup).
const UNRESOLVED: u32 = u32::MAX - 1;

/// File record status bits (`status` byte).
pub(crate) mod bits {
    pub const IN_USE: u8 = 0b0000_0001;
    pub const DIRECTORY: u8 = 0b0000_0010;
    pub const REPARSE: u8 = 0b0000_0100;
    pub const SIZE_VALID: u8 = 0b0000_1000;
}

/// Seconds between the Windows `FILETIME` epoch (1601-01-01) and the UNIX one.
const WINDOWS_EPOCH_OFFSET_SECS: u64 = 11_644_473_600;

/// `FILE_ATTRIBUTE_DIRECTORY`.
const ATTR_DIRECTORY: u32 = 0x10;
/// `FILE_ATTRIBUTE_REPARSE_POINT`.
const ATTR_REPARSE: u32 = 0x400;

/// A record as handed over by an enumerator — the recovered `IndexInput`
/// structure (0x40 bytes on x64 in the original), with idiomatic field types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    /// NTFS file reference number (`sequence << 48 | record number`). `0` when
    /// the enumerator cannot supply one (directory walk).
    pub id: u64,
    /// Parent directory's file reference number. `0` for a root.
    pub parent_id: u64,
    /// Size in bytes; `None` when the enumerator reported none.
    pub size: Option<u64>,
    /// Last-write time in seconds since the Windows epoch; `None` when unknown.
    pub mtime: Option<u64>,
    /// Windows attribute bits (`FILE_ATTRIBUTE_*`).
    pub attributes: u32,
    pub name: String,
    pub is_dir: bool,
}

impl EntryInfo {
    /// A file entry with no metadata (tests and simple enumerators).
    pub fn file(name: impl Into<String>) -> Self {
        Self {
            id: 0,
            parent_id: 0,
            size: None,
            mtime: None,
            attributes: 0,
            name: name.into(),
            is_dir: false,
        }
    }

    /// A directory entry with no metadata.
    pub fn dir(name: impl Into<String>) -> Self {
        Self {
            id: 0,
            parent_id: 0,
            size: None,
            mtime: None,
            attributes: ATTR_DIRECTORY,
            name: name.into(),
            is_dir: true,
        }
    }

    pub fn with_id(mut self, id: u64, parent_id: u64) -> Self {
        self.id = id;
        self.parent_id = parent_id;
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_mtime(mut self, mtime: u64) -> Self {
        self.mtime = Some(mtime);
        self
    }
}

/// A record resolved out of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Record index (stable until the next build or incremental edit).
    pub index: u32,
    /// NTFS file reference number, or `0` when unknown.
    pub id: u64,
    pub parent: u32,
    pub name: String,
    /// Size in bytes, when known.
    pub size: Option<u64>,
    /// Last-write time, seconds since the Windows epoch, when known.
    pub mtime: Option<u64>,
    pub is_dir: bool,
    pub is_reparse: bool,
}

/// Resumable USN Journal position for one volume (report §5.1, §7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JournalState {
    /// Journal identity. A different id means the journal was recreated, so the
    /// volume has to be rebuilt instead of caught up.
    pub journal_id: u64,
    /// Next USN to read from.
    pub next_usn: i64,
}

/// Why a persisted index could not be decoded.
#[derive(Debug)]
pub enum IndexError {
    Short,
    Magic,
    Version(u32),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Short => f.write_str("file index blob is truncated"),
            Self::Magic => f.write_str("not a Steward file index (bad magic)"),
            Self::Version(version) => write!(f, "unsupported file index version {version}"),
        }
    }
}

impl std::error::Error for IndexError {}

/// Persisted-index magic: `"FSDB"` little endian. Steward's own signature —
/// the original's `ESDb` marks a different, unimplemented format (report §7).
pub const MAGIC: u32 = 0x4244_5346;
/// Current persisted format version.
pub const FORMAT_VERSION: u32 = 1;

/// The resident file index: a byte arena of variable-length records plus the
/// parallel arrays the search workers read.
///
/// Fields are crate-visible so the persistence and USN-update modules can
/// rebuild and maintain the index without a second copy of the layout rules.
pub struct FileDb {
    pub(crate) data: Vec<u8>,
    pub(crate) offsets: Vec<u32>,
    pub(crate) lengths: Vec<u16>,
    pub(crate) parent: Vec<u32>,
    pub(crate) ids: Vec<u64>,
    /// Records sorted by (parent path, name). A parent always precedes its
    /// descendants, so serving a folder listing is a contiguous slice and path
    /// construction walks over records that are already neighbours.
    pub(crate) name_index: Vec<u32>,
    /// Position in `name_index` where each block starts.
    pub(crate) blocks: Vec<u32>,
    pub(crate) removed: Vec<u8>,
    pub(crate) depths: Vec<u32>,
    pub(crate) live: usize,
    pub(crate) dirs: usize,
    pub(crate) max_depth: u32,
    /// `file id → record index`, kept for incremental (USN) updates.
    pub(crate) key_map: HashMap<u64, u32>,
    pub(crate) journals: HashMap<u8, JournalState>,
}

impl FileDb {
    /// An empty index.
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            offsets: Vec::new(),
            lengths: Vec::new(),
            parent: Vec::new(),
            ids: Vec::new(),
            name_index: Vec::new(),
            blocks: Vec::new(),
            removed: Vec::new(),
            depths: Vec::new(),
            live: 0,
            dirs: 0,
            max_depth: 1,
            key_map: HashMap::new(),
            journals: HashMap::new(),
        }
    }

    /// Number of live records.
    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Number of live directories.
    pub fn dir_count(&self) -> usize {
        self.dirs
    }

    /// Bytes held by the record arena (the dominant memory cost).
    pub fn arena_bytes(&self) -> usize {
        self.data.capacity()
    }

    /// Number of search blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Deepest directory nesting in the index (path buffers are sized by it).
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Position in `name_index` where `block` starts; one past the end for the
    /// terminator.
    pub fn block_start(&self, block: u32) -> u32 {
        match self.blocks.get(block as usize) {
            Some(start) => *start,
            None => self.name_index.len() as u32,
        }
    }

    /// The persisted USN cursor for `drive` (upper-case letter), if known.
    pub fn journal(&self, drive: u8) -> Option<JournalState> {
        self.journals.get(&drive.to_ascii_uppercase()).copied()
    }

    /// Record a volume's USN cursor.
    pub fn set_journal(&mut self, drive: u8, state: JournalState) {
        self.journals.insert(drive.to_ascii_uppercase(), state);
    }

    /// All recorded journal cursors.
    pub fn journals(&self) -> &HashMap<u8, JournalState> {
        &self.journals
    }

    /// Resolve a record index into its fields.
    pub fn entry(&self, index: u32) -> Option<FileEntry> {
        if !self.is_live(index) {
            return None;
        }
        let offset = self.offsets[index as usize] as usize;
        let status = self.data[offset + 5];
        let (size, mtime) = self.meta(index);
        Some(FileEntry {
            index,
            id: self.ids[index as usize],
            parent: self.parent[index as usize],
            name: String::from_utf8_lossy(self.name_bytes(index)).into_owned(),
            size,
            mtime,
            is_dir: status & bits::DIRECTORY != 0,
            is_reparse: status & bits::REPARSE != 0,
        })
    }

    /// Name bytes of a record, borrowed from the arena — the hot path the
    /// search workers use: no allocation, no copy.
    ///
    /// A name longer than [`INLINE_NAME_MAX`] stores only its first 254 bytes
    /// inline and puts the real `u32` length in the escape slot right after
    /// them, which is the original's layout seen from the other side (the
    /// report reads that slot as `record - 4` from a name pointer).
    pub fn name_bytes(&self, index: u32) -> &[u8] {
        let offset = self.offsets[index as usize] as usize;
        let escaped = self.data[offset + 4] == 0xff;
        // Escaped names store a 4-byte real-length slot right after the 24-byte
        // header, then the name; shorter names start immediately after the
        // header. Read the length from `lengths` either way.
        let start = offset + HEADER_LEN as usize + if escaped { 4 } else { 0 };
        let length = self.lengths[index as usize] as usize;
        &self.data[start..start + length]
    }

    /// Status byte of a record.
    pub fn status(&self, index: u32) -> u8 {
        self.data[self.offsets[index as usize] as usize + 5]
    }

    /// Parent index of a record, or [`ROOT_PARENT`].
    pub fn parent_of(&self, index: u32) -> u32 {
        self.parent[index as usize]
    }

    /// Size and mtime fields of a record without decoding its name.
    pub fn meta(&self, index: u32) -> (Option<u64>, Option<u64>) {
        let offset = self.offsets[index as usize] as usize;
        let status = self.data[offset + 5];
        let size = u64::from_le_bytes(self.data[offset + 8..offset + 16].try_into().unwrap());
        let mtime = u64::from_le_bytes(self.data[offset + 16..offset + 24].try_into().unwrap());
        (
            (status & bits::SIZE_VALID != 0).then_some(size),
            (mtime != 0).then_some(mtime),
        )
    }

    /// Whether a record is a live directory.
    pub fn is_dir(&self, index: u32) -> bool {
        self.is_live(index) && self.status(index) & bits::DIRECTORY != 0
    }

    /// Whether a record is a live file (not a directory).
    pub fn is_file(&self, index: u32) -> bool {
        self.is_live(index) && self.status(index) & bits::DIRECTORY == 0
    }

    /// Whether `index` refers to a live record.
    pub fn is_live(&self, index: u32) -> bool {
        let index = index as usize;
        index < self.offsets.len() && self.removed[index] == 0
    }

    /// Directory nesting level of a record (0 for a root).
    pub fn depth_of(&self, index: u32) -> u32 {
        self.depths.get(index as usize).copied().unwrap_or(0)
    }

    /// Iterate live records in index order.
    pub fn iter_ordered(&self) -> impl Iterator<Item = u32> + '_ {
        self.name_index
            .iter()
            .copied()
            .filter(|index| self.is_live(*index))
    }

    /// The record at `position` in index order, live or not.
    pub fn ordered_at(&self, position: u32) -> Option<u32> {
        self.name_index.get(position as usize).copied()
    }

    /// Number of live records inside `block`.
    pub fn block_len(&self, block: u32) -> usize {
        let start = self.block_start(block) as usize;
        let end = self.block_start(block + 1) as usize;
        let end = end.min(self.name_index.len());
        self.name_index[start..end]
            .iter()
            .filter(|index| self.is_live(**index))
            .count()
    }

    /// Full path of a record, including the root's own name.
    pub fn path_of(&self, index: u32) -> PathBuf {
        let mut buffer = String::new();
        self.path_into(index, &mut buffer);
        PathBuf::from(buffer)
    }

    /// Full path of a record appended into `out`, returning the byte range of
    /// the appended segment so callers can borrow it without allocating.
    ///
    /// This is the parent-pointer walk the original performs when it needs a
    /// full path (report §4): no record stores its own full path.
    pub fn path_into(&self, index: u32, out: &mut String) -> std::ops::Range<usize> {
        out.clear();
        if !self.is_live(index) {
            return 0..0;
        }
        // `depth_of` is safe to call while `depths` is still empty (the first
        // `path_into` during `finalize` happens before depths are computed).
        let mut chain = Vec::with_capacity(self.depth_of(index) as usize + 1);
        let mut current = index;
        loop {
            chain.push(current);
            let parent = self.parent[current as usize];
            if parent == ROOT_PARENT || parent as usize >= self.offsets.len() {
                break;
            }
            current = parent;
        }
        let start = out.len();
        for (position, record) in chain.iter().rev().enumerate() {
            let name = String::from_utf8_lossy(self.name_bytes(*record));
            if position == 0 {
                // The root contributes its own name; a volume root already ends
                // with a separator (`C:\`), a configured folder root does not.
                out.push_str(&name);
            } else {
                let needs_separator = !out.ends_with('\\') && !out.ends_with('/');
                if needs_separator {
                    out.push('\\');
                }
                out.push_str(&name);
            }
        }
        start..out.len()
    }

    /// Children of `index` in index order. Returns an empty vector for a record
    /// that is not a live directory.
    pub fn children(&self, index: u32) -> Vec<u32> {
        if !self.is_dir(index) {
            return Vec::new();
        }
        let start = self
            .name_index
            .partition_point(|record| self.parent[*record as usize] < index);
        let end = self
            .name_index
            .partition_point(|record| self.parent[*record as usize] <= index);
        self.name_index[start..end]
            .iter()
            .copied()
            .filter(|child| self.is_live(*child))
            .collect()
    }

    /// Find a live child of `parent` by name (case-insensitive), used by USN
    /// "create" events whose parent directory already exists.
    pub fn child_by_name(&self, parent: u32, name: &str, is_dir: bool) -> Option<u32> {
        if !self.is_dir(parent) {
            return None;
        }
        let start = self
            .name_index
            .partition_point(|record| self.parent[*record as usize] < parent);
        let end = self
            .name_index
            .partition_point(|record| self.parent[*record as usize] <= parent);
        self.name_index[start..end].iter().copied().find(|record| {
            self.is_live(*record)
                && self.is_dir(*record) == is_dir
                && String::from_utf8_lossy(self.name_bytes(*record)).eq_ignore_ascii_case(name)
        })
    }

    /// Record index for a file reference number, via the `id → record` map.
    ///
    /// The map is derived, never persisted: it is built incrementally as
    /// records are inserted and re-derived by [`Self::rebuild_key_map`] when an
    /// index is restored from a snapshot ([`Self::from_parts`]).
    pub fn index_of_id(&self, id: u64) -> Option<u32> {
        self.key_map.get(&id).copied()
    }

    /// Re-derive the `id → record` map from the record ids. Called by
    /// [`Self::from_parts`], because a snapshot carries ids but not the map.
    pub fn rebuild_key_map(&mut self) {
        self.key_map.clear();
        self.key_map.reserve(self.ids.len() / 4);
        for (index, id) in self.ids.iter().enumerate() {
            if *id != NO_ID && self.removed[index] == 0 {
                self.key_map.insert(*id, index as u32);
            }
        }
    }

    /// Append a record under an existing directory, as USN-style incremental
    /// maintenance needs. Returns `None` when `parent` is not a live directory
    /// or a live entry with the same name already exists.
    pub fn insert_child(&mut self, parent: u32, info: &EntryInfo) -> Option<u32> {
        if !self.is_dir(parent) {
            return None;
        }
        if self
            .child_by_name(parent, &info.name, info.is_dir)
            .is_some()
        {
            return None;
        }
        let index = self.append_record(info, ParentRef::Index(parent))?;
        if info.id != NO_ID {
            self.key_map.insert(info.id, index);
        }
        Some(index)
    }

    /// Tombstone a record and everything beneath it (a delete, or the "old
    /// half" of a move).
    pub fn remove_subtree(&mut self, index: u32) {
        if !self.is_live(index) {
            return;
        }
        let mut stack = vec![index];
        while let Some(current) = stack.pop() {
            if !self.is_live(current) {
                continue;
            }
            self.removed[current as usize] = 1;
            self.live -= 1;
            if self.status(current) & bits::DIRECTORY != 0 {
                self.dirs -= 1;
            }
            if let Some(id) = self.ids.get(current as usize).copied() {
                if id != NO_ID {
                    self.key_map.remove(&id);
                }
            }
            stack.extend(self.children(current));
        }
    }

    /// Update a record's size/mtime in place.
    ///
    /// The metadata tail is always written by [`Self::append_record`] (20
    /// bytes), so a record created by an earlier version of the index without
    /// it cannot be patched this way; those are re-read on the next rebuild.
    pub fn set_metadata(&mut self, index: u32, size: Option<u64>, mtime: Option<u64>) -> bool {
        if !self.is_live(index) {
            return false;
        }
        let offset = self.offsets[index as usize] as usize;
        let name_start = offset + HEADER_LEN as usize;
        let stored = self.lengths[index as usize] as usize;
        let escaped = self.data[offset + 4] == 0xff;
        let meta_at = name_start + stored + if escaped { 4 } else { 0 };
        if meta_at + META_LEN as usize > self.data.len() {
            return false;
        }
        self.data[offset + 8..offset + 16].copy_from_slice(&size.unwrap_or(0).to_le_bytes());
        self.data[offset + 16..offset + 24].copy_from_slice(&mtime.unwrap_or(0).to_le_bytes());
        if size.is_some() {
            self.data[offset + 5] |= bits::SIZE_VALID;
        } else {
            self.data[offset + 5] &= !bits::SIZE_VALID;
        }
        true
    }

    /// Recompute the index order, block boundaries and counters after a batch
    /// of incremental edits. One re-sort per batch, not per record.
    pub fn finish_incremental(&mut self) {
        self.prune_orphans();
        self.max_depth = self.compute_depths();
        self.resort();
        self.blocks = (0..self.name_index.len())
            .step_by(BLOCKS)
            .map(|position| position as u32)
            .collect();
        if self.blocks.is_empty() {
            self.blocks.push(0);
        }
    }

    /// Tombstone records whose parent is gone or is no longer a directory,
    /// repeatedly, until nothing more can be pruned.
    ///
    /// A USN delete removes the top of a subtree; its descendants arrive as
    /// their own delete records, so leaving them behind would leak orphan
    /// records into the index until the next full rebuild.
    fn prune_orphans(&mut self) {
        loop {
            let mut changed = false;
            for index in 0..self.offsets.len() as u32 {
                if !self.is_live(index) {
                    continue;
                }
                let parent = self.parent[index as usize];
                if parent == ROOT_PARENT {
                    continue;
                }
                let parent_ok = (parent as usize) < self.offsets.len() && self.is_dir(parent);
                if !parent_ok {
                    self.removed[index as usize] = 1;
                    self.live -= 1;
                    if self.status(index) & bits::DIRECTORY != 0 {
                        self.dirs -= 1;
                    }
                    if let Some(id) = self.ids.get(index as usize).copied() {
                        if id != NO_ID {
                            self.key_map.remove(&id);
                        }
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Append a record to the arena of a finalized index. Only used by
    /// incremental maintenance; a full build goes through [`FileDbBuilder`].
    fn append_record(&mut self, info: &EntryInfo, parent: ParentRef) -> Option<u32> {
        let mut name = info.name.as_str();
        if name.is_empty() || name == "." || name == ".." {
            return None;
        }
        if name.len() > MAX_NAME_BYTES {
            let mut cut = MAX_NAME_BYTES;
            while cut > 0 && !name.is_char_boundary(cut) {
                cut -= 1;
            }
            name = &name[..cut];
        }
        let name_len = name.len();
        let escaped = name_len > INLINE_NAME_MAX;
        let stored_len = name_len;
        let mut status_byte = bits::IN_USE;
        if info.is_dir || info.attributes & ATTR_DIRECTORY != 0 {
            status_byte |= bits::DIRECTORY;
        }
        if info.attributes & ATTR_REPARSE != 0 {
            status_byte |= bits::REPARSE;
        }
        if info.size.is_some() {
            status_byte |= bits::SIZE_VALID;
        }
        // An escaped name reserves a 4-byte real-length slot between the header
        // and the name; shorter names start right after the header.
        let name_start = HEADER_LEN as usize + if escaped { 4 } else { 0 };
        let used = name_start + stored_len;
        let total = (used as u32 + META_LEN as u32 + 3) & !3;
        let offset = self.data.len() as u32;
        self.data.resize(self.data.len() + total as usize, 0);
        let parent_slot = match parent {
            ParentRef::Index(index) => index,
            ParentRef::Id(_) => ROOT_PARENT,
        };
        let record = &mut self.data[offset as usize..(offset + total) as usize];
        record[0..4].copy_from_slice(&parent_slot.to_le_bytes());
        record[4] = if escaped { 0xff } else { name_len as u8 };
        record[5] = status_byte;
        record[6..8].copy_from_slice(&META_LEN.to_le_bytes());
        record[8..16].copy_from_slice(&info.size.unwrap_or(0).to_le_bytes());
        record[16..24].copy_from_slice(&info.mtime.unwrap_or(0).to_le_bytes());
        if escaped {
            record[HEADER_LEN as usize..HEADER_LEN as usize + 4]
                .copy_from_slice(&(name_len as u32).to_le_bytes());
        }
        record[name_start..name_start + stored_len].copy_from_slice(name.as_bytes());

        let index = self.offsets.len() as u32;
        self.offsets.push(offset);
        self.lengths.push(name_len as u16);
        self.parent.push(parent_slot);
        self.ids.push(info.id);
        self.removed.push(0);
        self.depths
            .push(self.depth_of(parent_slot).saturating_add(1));
        self.live += 1;
        if status_byte & bits::DIRECTORY != 0 {
            self.dirs += 1;
        }
        self.name_index.push(index);
        self.max_depth = self.max_depth.max(self.depths[index as usize]);
        Some(index)
    }

    /// Rebuild an index from persisted parts. The caller (the persistence
    /// module) is responsible for having validated every invariant.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        data: Vec<u8>,
        offsets: Vec<u32>,
        lengths: Vec<u16>,
        parent: Vec<u32>,
        ids: Vec<u64>,
        name_index: Vec<u32>,
        blocks: Vec<u32>,
        removed: Vec<u8>,
        depths: Vec<u32>,
        live: usize,
        dirs: usize,
        max_depth: u32,
        journals: HashMap<u8, JournalState>,
    ) -> Self {
        let mut db = Self {
            data,
            offsets,
            lengths,
            parent,
            ids,
            name_index,
            blocks,
            removed,
            depths,
            live,
            dirs,
            max_depth,
            // Filled in by `rebuild_key_map` below: `key_map` is derived state
            // and is deliberately not persisted (see `persist::encode`), so
            // every restore has to re-derive it. Leaving it empty here made
            // restored indexes silently un-maintainable — `index_of_id` returns
            // `None` for every pre-existing file, so `apply_usn_records` counted
            // each delete/rename/metadata change as `skipped` and dropped it.
            key_map: HashMap::new(),
            journals,
        };
        db.rebuild_key_map();
        db
    }
}

impl Default for FileDb {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FileDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileDb")
            .field("records", &self.live)
            .field("dirs", &self.dirs)
            .field("blocks", &self.blocks.len())
            .field("max_depth", &self.max_depth)
            .field("arena_bytes", &self.data.len())
            .finish()
    }
}

/// Where a record's parent comes from while the index is being built.
#[derive(Debug, Clone, Copy)]
enum ParentRef {
    /// Already known: a record index, or [`ROOT_PARENT`].
    Index(u32),
    /// Not known yet: an NTFS file reference number resolved in `finalize`.
    Id(u64),
}

/// Incrementally built index.
///
/// The builder keeps temporary parent identities (file reference numbers)
/// alongside the records and resolves them to record indices in
/// [`FileDbBuilder::finalize`] — the report's "rebuild converts temporary
/// parent FRNs to pointers, removes orphans and sorts the name pointer arrays".
pub struct FileDbBuilder {
    data: Vec<u8>,
    offsets: Vec<u32>,
    lengths: Vec<u16>,
    parent: Vec<u32>,
    parent_refs: Vec<ParentRef>,
    ids: Vec<u64>,
    key_map: HashMap<u64, u32>,
    journals: HashMap<u8, JournalState>,
    removed: Vec<u8>,
    truncated_names: usize,
}

impl FileDbBuilder {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    /// Pre-allocate room for `expected` records. Index memory is dominated by
    /// the arena, so sizing it up front keeps a full-volume build from
    /// repeatedly doubling a multi-hundred-megabyte buffer.
    pub fn with_capacity(expected: usize) -> Self {
        Self {
            data: Vec::with_capacity(expected.saturating_mul(64)),
            offsets: Vec::with_capacity(expected),
            lengths: Vec::with_capacity(expected),
            parent: Vec::with_capacity(expected),
            parent_refs: Vec::with_capacity(expected),
            ids: Vec::with_capacity(expected),
            key_map: HashMap::with_capacity(expected / 4),
            journals: HashMap::new(),
            removed: Vec::with_capacity(expected),
            truncated_names: 0,
        }
    }

    /// Number of records added so far.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// How many names were truncated to [`MAX_NAME_BYTES`].
    pub fn truncated_names(&self) -> usize {
        self.truncated_names
    }

    /// Add a record whose parent is identified by a file reference number — the
    /// NTFS path, where records arrive in MFT order and a parent may not have
    /// been seen yet.
    pub fn add_entry(&mut self, info: &EntryInfo) -> u32 {
        let parent = if info.parent_id == 0 || info.parent_id == info.id {
            ParentRef::Index(ROOT_PARENT)
        } else {
            ParentRef::Id(info.parent_id)
        };
        let index = self.push(info, parent);
        if info.id != NO_ID {
            self.key_map.insert(info.id, index);
        }
        index
    }

    /// Add a root record (a volume root or a configured folder).
    pub fn add_root(&mut self, name: &str, id: u64) -> u32 {
        let info = EntryInfo {
            id,
            parent_id: 0,
            size: None,
            mtime: None,
            attributes: ATTR_DIRECTORY,
            name: name.to_string(),
            is_dir: true,
        };
        let index = self.push(&info, ParentRef::Index(ROOT_PARENT));
        if id != NO_ID {
            self.key_map.insert(id, index);
        }
        index
    }

    /// Add a record whose parent record index is already known. The directory
    /// walk uses this: it emits parents before their children, so no
    /// FRN resolution is needed.
    pub fn add_child(&mut self, parent: u32, info: &EntryInfo) -> u32 {
        let index = self.push(info, ParentRef::Index(parent));
        // Record the file id whenever one is supplied: incremental USN
        // maintenance resolves parents and renames through this map.
        if info.id != NO_ID {
            self.key_map.insert(info.id, index);
        }
        index
    }

    /// Tombstone a record. The slot survives so indices stay stable until the
    /// next `finalize`.
    pub fn remove(&mut self, index: u32) {
        if let Some(flag) = self.removed.get_mut(index as usize) {
            *flag = 1;
        }
    }

    /// Tombstone the subtree rooted at `index` (a delete or an overwriting
    /// rename). Walk-based builders keep parent indices, so children are found
    /// by scanning parent slots once.
    pub fn remove_subtree(&mut self, index: u32) {
        let mut stack = vec![index];
        while let Some(current) = stack.pop() {
            self.remove(current);
            for (child, parent) in self.parent.iter().enumerate() {
                if *parent == current {
                    stack.push(child as u32);
                }
            }
        }
    }

    pub fn journal(&self, drive: u8) -> Option<JournalState> {
        self.journals.get(&drive.to_ascii_uppercase()).copied()
    }

    pub fn set_journal(&mut self, drive: u8, state: JournalState) {
        self.journals.insert(drive.to_ascii_uppercase(), state);
    }

    /// Resolve parent identities, drop orphans, build the block index and sort
    /// the name array. Consumes the builder.
    pub fn finalize(mut self) -> FileDb {
        let count = self.offsets.len();
        let mut db = FileDb::new();
        db.journals = std::mem::take(&mut self.journals);
        if count == 0 {
            return db;
        }

        // 1. Resolve every parent reference that is still a file id. A parent
        //    that never produced a record becomes `UNRESOLVED`, which the prune
        //    pass below treats as an orphan rather than as a root.
        for index in 0..count {
            self.parent[index] = match self.parent_refs[index] {
                ParentRef::Index(parent) => parent,
                ParentRef::Id(id) => self.key_map.get(&id).copied().unwrap_or(UNRESOLVED),
            };
        }
        for index in 0..count {
            if self.parent[index] == index as u32 {
                self.parent[index] = ROOT_PARENT;
            }
        }
        let status_at = |index: usize| self.data[self.offsets[index] as usize + 5];

        // 2. Prune to a fixpoint: a record whose parent is missing, tombstoned,
        //    or not a directory is an orphan (report §4's orphan cleanup).
        //
        //    Empty directories are deliberately *kept*: the index mirrors the
        //    disk, and a folder the user can see in Explorer must be findable
        //    even when it is empty — including when a move targets it while its
        //    file is still on its way.
        let mut removed = std::mem::take(&mut self.removed);
        loop {
            let mut changed = false;
            for index in 0..count {
                if removed[index] != 0 {
                    continue;
                }
                let parent = self.parent[index];
                if parent == ROOT_PARENT {
                    continue;
                }
                let parent_index = parent as usize;
                let parent_ok = parent_index < count
                    && removed[parent_index] == 0
                    && status_at(parent_index) & bits::DIRECTORY != 0;
                if !parent_ok {
                    removed[index] = 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // 3. Compact: keep the arena bytes, remap the indices.
        let mut remap: Vec<u32> = vec![ROOT_PARENT; count];
        for index in 0..count {
            if removed[index] != 0 {
                continue;
            }
            remap[index] = db.offsets.len() as u32;
            db.offsets.push(self.offsets[index]);
            db.lengths.push(self.lengths[index]);
            db.ids.push(self.ids[index]);
        }
        db.parent = vec![ROOT_PARENT; db.offsets.len()];
        for index in 0..count {
            let target = remap[index];
            if target == ROOT_PARENT {
                continue;
            }
            let parent = self.parent[index];
            db.parent[target as usize] = if parent == ROOT_PARENT {
                ROOT_PARENT
            } else {
                remap[parent as usize]
            };
        }
        for (id, index) in std::mem::take(&mut self.key_map) {
            let mapped = remap.get(index as usize).copied().unwrap_or(ROOT_PARENT);
            if mapped != ROOT_PARENT && id != NO_ID {
                db.key_map.insert(id, mapped);
            }
        }
        db.removed = vec![0; db.offsets.len()];
        db.live = db.offsets.len();
        // The arena moves into the index here: the pruning passes above read
        // record bytes through `self.data`, everything below reads them through
        // `db`.
        db.data = std::mem::take(&mut self.data);
        db.dirs = (0..db.offsets.len())
            .filter(|index| db.status(*index as u32) & bits::DIRECTORY != 0)
            .count();

        // 4. Sort the name array by (parent path, name). This also makes the
        //    array a topological order (every parent before its children),
        //    which `compute_depths` relies on.
        db.name_index = (0..db.offsets.len() as u32).collect();
        let mut order = std::mem::take(&mut db.name_index);
        sort_by_path(&mut order, &db);
        db.name_index = order;
        db.max_depth = db.compute_depths();

        // 5. Block entry points over the sorted array.
        db.blocks = (0..db.name_index.len())
            .step_by(BLOCKS)
            .map(|position| position as u32)
            .collect();
        if db.blocks.is_empty() {
            db.blocks.push(0);
        }
        db
    }
}

impl Default for FileDbBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Sort record indices by (parent path, name).
///
/// The keys are materialised once per record into an explicit vector and sorted
/// on that, which is O(n) parent-path builds instead of one per comparison and
/// keeps the order completely deterministic: `sort_unstable_by_key` on
/// `(path, name, index)` has no ties to break arbitrarily.
fn sort_by_path(indices: &mut [u32], db: &FileDb) {
    let mut path_cache: HashMap<u32, String> = HashMap::new();
    let mut keyed: Vec<(String, u32)> = Vec::with_capacity(indices.len());
    for index in indices.iter().copied() {
        let parent = db.parent[index as usize];
        let parent_path = if let Some(cached) = path_cache.get(&parent) {
            cached.clone()
        } else {
            let computed = if parent == ROOT_PARENT {
                String::new()
            } else {
                let mut buffer = String::new();
                db.path_into(parent, &mut buffer);
                buffer
            };
            path_cache.insert(parent, computed.clone());
            computed
        };
        let name = String::from_utf8_lossy(db.name_bytes(index)).into_owned();
        keyed.push((join_path(&parent_path, &name), index));
    }
    // Compare full paths byte-wise (case-insensitively first, then exactly), so
    // the order is a plain total order with no locale surprises. Records under
    // the same directory are adjacent — the parent path is a common prefix — and
    // a directory precedes the entries whose names start with its own name.
    keyed.sort_by(|left, right| {
        left.0
            .to_ascii_lowercase()
            .cmp(&right.0.to_ascii_lowercase())
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });
    for (slot, (_, index)) in indices.iter_mut().zip(keyed) {
        *slot = index;
    }
}

/// `parent\name`, without doubling a separator the parent already ends with.
fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        return name.to_owned();
    }
    if parent.ends_with('\\') || parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}\\{name}")
    }
}

impl FileDbBuilder {
    /// Push one record into the arena.
    fn push(&mut self, info: &EntryInfo, parent: ParentRef) -> u32 {
        let mut name = info.name.as_str();
        if name == "." || name == ".." {
            name = "";
        }
        if name.len() > MAX_NAME_BYTES {
            let mut cut = MAX_NAME_BYTES;
            while cut > 0 && !name.is_char_boundary(cut) {
                cut -= 1;
            }
            name = &name[..cut];
            self.truncated_names += 1;
        }
        let name_len = name.len();
        let escaped = name_len > INLINE_NAME_MAX;
        let mut status_byte = bits::IN_USE;
        if info.is_dir || info.attributes & ATTR_DIRECTORY != 0 {
            status_byte |= bits::DIRECTORY;
        }
        if info.attributes & ATTR_REPARSE != 0 {
            status_byte |= bits::REPARSE;
        }
        // The metadata tail is always present, even when the enumerator had no
        // size/time to report: a record's layout never depends on whether its
        // metadata happens to be known, which keeps USN-driven updates (which
        // write size/mtime in place) simple and its offsets stable.
        let meta_len = META_LEN;
        if info.size.is_some() {
            status_byte |= bits::SIZE_VALID;
        }

        let offset = self.data.len() as u32;
        let name_start = HEADER_LEN as usize + if escaped { 4 } else { 0 };
        let used = name_start + name_len;
        let total = (used as u32 + meta_len as u32 + 3) & !3;
        self.data.resize(self.data.len() + total as usize, 0);
        let record = &mut self.data[offset as usize..(offset + total) as usize];
        let parent_slot = match parent {
            ParentRef::Index(index) => index,
            // Placeholder: `finalize` overwrites it after resolution. Keeping a
            // known-bad value here makes a missed resolution obvious.
            ParentRef::Id(_) => ROOT_PARENT,
        };
        record[0..4].copy_from_slice(&parent_slot.to_le_bytes());
        record[4] = if escaped { 0xff } else { name_len as u8 };
        record[5] = status_byte;
        record[6..8].copy_from_slice(&meta_len.to_le_bytes());
        record[8..16].copy_from_slice(&info.size.unwrap_or(0).to_le_bytes());
        record[16..24].copy_from_slice(&info.mtime.unwrap_or(0).to_le_bytes());
        if escaped {
            // Real length in the slot between the header and the name.
            record[HEADER_LEN as usize..HEADER_LEN as usize + 4]
                .copy_from_slice(&(name_len as u32).to_le_bytes());
        }
        record[name_start..name_start + name_len].copy_from_slice(name.as_bytes());

        let index = self.offsets.len() as u32;
        self.offsets.push(offset);
        // The length array always holds the real name length.
        self.lengths.push(name_len as u16);
        self.parent.push(parent_slot);
        self.parent_refs.push(parent);
        self.ids.push(info.id);
        self.removed.push(0);
        index
    }
}

impl FileDb {
    /// Directory nesting level per record (0 for a root).
    ///
    /// One pass over `name_index`, which is a topological order because every
    /// parent precedes its children, so a single forward sweep suffices.
    fn compute_depths(&mut self) -> u32 {
        let mut depths = vec![0u32; self.offsets.len()];
        let mut max_depth = 0;
        for record in self.name_index.iter().copied() {
            let parent = self.parent[record as usize];
            depths[record as usize] = if parent == ROOT_PARENT || parent as usize >= depths.len() {
                0
            } else {
                depths[parent as usize] + 1
            };
            max_depth = max_depth.max(depths[record as usize]);
        }
        self.depths = depths;
        max_depth.max(1)
    }

    /// Sort the name array in place. Used after incremental edits that changed
    /// paths (a directory rename moves a whole subtree in the order).
    pub(crate) fn resort(&mut self) {
        let mut order = std::mem::take(&mut self.name_index);
        sort_by_path(&mut order, self);
        self.name_index = order;
    }

    /// Number of record slots, live plus tombstoned. Equals the index count
    /// between `finalize` calls and is what incremental updates iterate.
    pub fn slot_count(&self) -> usize {
        self.offsets.len()
    }

    /// An independent copy of the index.
    ///
    /// The persistence path uses this so a snapshot can be encoded and written
    /// while the search workers keep reading the live index through their
    /// `Arc<FileDb>` clones.
    pub fn duplicate(&self) -> Self {
        Self {
            data: self.data.clone(),
            offsets: self.offsets.clone(),
            lengths: self.lengths.clone(),
            parent: self.parent.clone(),
            ids: self.ids.clone(),
            name_index: self.name_index.clone(),
            blocks: self.blocks.clone(),
            removed: self.removed.clone(),
            depths: self.depths.clone(),
            live: self.live,
            dirs: self.dirs,
            max_depth: self.max_depth,
            key_map: self.key_map.clone(),
            journals: self.journals.clone(),
        }
    }
}

/// Convert a Windows `FILETIME` (100 ns ticks since 1601) to the stored mtime.
pub fn filetime_to_mtime(ticks: u64) -> u64 {
    ticks / 10_000_000
}

/// Convert a stored mtime into a UNIX timestamp in seconds.
pub fn mtime_to_unix(mtime: u64) -> i64 {
    mtime as i64 - WINDOWS_EPOCH_OFFSET_SECS as i64
}

/// Convert a UNIX timestamp in seconds into the stored mtime representation.
pub fn unix_to_mtime(unix: i64) -> u64 {
    (unix + WINDOWS_EPOCH_OFFSET_SECS as i64).max(0) as u64
}

/// Human-readable byte size for logs and diagnostics.
pub fn describe_bytes(bytes: usize) -> String {
    const MIB: f64 = (1u64 << 20) as f64;
    const KIB: f64 = (1u64 << 10) as f64;
    let bytes_f = bytes as f64;
    if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.1} KiB", bytes_f / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `C:\` → `dir` → `name`, returning `(db, index)`.
    fn one_file() -> (FileDb, u32) {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 100);
        let dir = builder.add_child(root, &EntryInfo::dir("Users").with_id(101, 100));
        let file = builder.add_child(
            dir,
            &EntryInfo::file("notes.txt")
                .with_id(102, 101)
                .with_size(2048)
                .with_mtime(999),
        );
        (builder.finalize(), file)
    }

    #[test]
    fn path_is_built_from_parent_pointers() {
        let (db, file) = one_file();
        assert_eq!(db.path_of(file).to_string_lossy(), "C:\\Users\\notes.txt");
        let root = db.parent_of(db.parent_of(file));
        assert_eq!(db.path_of(root).to_string_lossy(), "C:\\");
    }

    #[test]
    fn a_directory_root_does_not_get_a_double_separator() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("D:\\Media", 1);
        let child = builder.add_child(root, &EntryInfo::file("clip.mp4"));
        let db = builder.finalize();
        assert_eq!(db.path_of(child).to_string_lossy(), "D:\\Media\\clip.mp4");
    }

    #[test]
    fn metadata_round_trips_through_the_arena() {
        let (db, file) = one_file();
        let entry = db.entry(file).expect("record is live");
        assert_eq!(entry.name, "notes.txt");
        assert_eq!(entry.size, Some(2048));
        assert_eq!(entry.mtime, Some(999));
        assert!(!entry.is_dir);
        assert!(!entry.is_reparse);
    }

    /// Guards the inline/escaped boundary: 254 bytes is the largest inline name.
    fn name_len_after_build(len: usize) -> usize {
        let name = "b".repeat(len);
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        let file = builder.add_child(root, &EntryInfo::file(name));
        builder.finalize().name_bytes(file).len()
    }

    #[test]
    fn long_names_use_the_escaped_length_slot() {
        // 300 ASCII bytes: past the 254-byte inline limit, inside the cap.
        let name = "a".repeat(300);
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        let file = builder.add_child(root, &EntryInfo::file(name.clone()));
        let db = builder.finalize();
        assert_eq!(db.name_bytes(file).len(), name.len());
        assert_eq!(
            db.entry(file).expect("live").name.len(),
            300,
            "the escaped u32 length must be read back"
        );
        assert_eq!(db.path_of(file).to_string_lossy().len(), "C:\\".len() + 300);
    }

    #[test]
    fn a_254_byte_name_still_fits_inline() {
        assert_eq!(name_len_after_build(254), 254);
        assert_eq!(name_len_after_build(255), 255);
    }

    #[test]
    fn names_longer_than_the_cap_are_truncated_on_a_char_boundary() {
        let name = "x".repeat(MAX_NAME_BYTES + 10);
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        builder.add_child(root, &EntryInfo::file(name));
        assert_eq!(builder.truncated_names(), 1);

        // A multi-byte character straddling the cap must not be split.
        let name = format!("{}中", "y".repeat(MAX_NAME_BYTES - 1));
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        builder.add_child(root, &EntryInfo::file(name));
        let db = builder.finalize();
        for index in db.iter_ordered() {
            assert!(std::str::from_utf8(db.name_bytes(index)).is_ok());
        }
    }

    #[test]
    fn records_sort_by_parent_path_then_name() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        let b_dir = builder.add_child(root, &EntryInfo::dir("b"));
        let a_dir = builder.add_child(root, &EntryInfo::dir("a"));
        builder.add_child(b_dir, &EntryInfo::file("inside-b.txt"));
        builder.add_child(a_dir, &EntryInfo::file("inside-a.txt"));
        builder.add_child(root, &EntryInfo::file("top.txt"));
        let db = builder.finalize();

        let names: Vec<String> = db
            .iter_ordered()
            .map(|index| db.path_of(index).to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "C:\\",
                "C:\\a",
                "C:\\a\\inside-a.txt",
                "C:\\b",
                "C:\\b\\inside-b.txt",
                "C:\\top.txt",
            ]
        );
    }

    #[test]
    fn children_are_a_contiguous_slice() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        let dir = builder.add_child(root, &EntryInfo::dir("dir"));
        let first = builder.add_child(dir, &EntryInfo::file("one.txt"));
        let second = builder.add_child(dir, &EntryInfo::file("two.txt"));
        let db = builder.finalize();
        let mut children = db.children(dir);
        children.sort_unstable();
        let mut expected = vec![first, second];
        expected.sort_unstable();
        assert_eq!(children, expected);
        assert!(db.children(first).is_empty(), "a file has no children");
        assert_eq!(db.child_by_name(dir, "TWO.TXT", false), Some(second));
        assert_eq!(db.child_by_name(dir, "missing", false), None);
    }

    #[test]
    fn a_child_listed_before_its_parent_is_still_linked() {
        // The MFT enumerator hands records over in file-reference order, so a
        // child routinely arrives before its parent.
        let mut builder = FileDbBuilder::new();
        let file = builder.add_entry(&EntryInfo::file("deep.txt").with_id(7, 6));
        let dir = builder.add_entry(&EntryInfo::dir("sub").with_id(6, 5));
        let root = builder.add_entry(&EntryInfo::dir("C:\\").with_id(5, 0));
        let db = builder.finalize();
        assert_eq!(db.len(), 3);
        assert_eq!(db.parent_of(file), dir);
        assert_eq!(db.parent_of(dir), root);
        assert_eq!(db.parent_of(root), ROOT_PARENT);
        assert_eq!(db.path_of(file).to_string_lossy(), "C:\\sub\\deep.txt");
    }

    #[test]
    fn orphans_and_their_subtrees_are_dropped() {
        // The parent (id 19) never arrives; both it and its child are gone.
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        builder.add_entry(&EntryInfo::file("orphan.txt").with_id(20, 19));
        builder.add_child(root, &EntryInfo::file("kept.txt"));
        let db = builder.finalize();
        assert_eq!(db.len(), 2, "root + kept.txt survive");
        let names: Vec<String> = db
            .iter_ordered()
            .map(|index| db.entry(index).unwrap().name)
            .collect();
        assert!(names.contains(&"kept.txt".to_string()));
        assert!(!names.contains(&"orphan.txt".to_string()));
    }

    #[test]
    fn empty_directories_are_indexed() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        builder.add_child(root, &EntryInfo::dir("excluded"));
        let db = builder.finalize();
        // A folder the user can see in Explorer stays findable with nothing in
        // it, and a move targeting it has a parent to link against.
        assert_eq!(db.len(), 2);
        assert_eq!(db.dir_count(), 2);
        let names: Vec<String> = db
            .iter_ordered()
            .map(|index| db.entry(index).unwrap().name)
            .collect();
        assert_eq!(names, vec!["C:\\".to_string(), "excluded".to_string()]);
    }

    #[test]
    fn block_index_covers_the_index_order() {
        // 5000 records: two blocks at BLOCKS = 4096.
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        for i in 0..5000 {
            builder.add_child(root, &EntryInfo::file(format!("f{i:05}.txt")));
        }
        let db = builder.finalize();
        assert_eq!(db.block_count(), 2);
        assert_eq!(db.len(), 5001);
        let total: usize = (0..db.block_count() as u32)
            .map(|block| db.block_len(block))
            .sum();
        assert_eq!(total, db.len());
        assert_eq!(db.block_start(0), 0);
        assert_eq!(db.block_start(1), BLOCKS as u32);
        assert_eq!(db.block_start(2), 5001);
    }

    #[test]
    fn depths_track_the_parent_chain() {
        let (db, file) = one_file();
        let dir = db.parent_of(file);
        let root = db.parent_of(dir);
        // The configured root is level 0; `Users` is one deeper than `C:\`, and
        // `notes.txt` one deeper again.
        assert_eq!(db.depth_of(root), 0);
        assert_eq!(db.depth_of(dir), 1);
        assert_eq!(db.depth_of(file), 2);
        assert_eq!(db.max_depth(), 2);
    }

    #[test]
    fn key_map_resolves_file_ids() {
        let (db, file) = one_file();
        assert_eq!(db.index_of_id(102), Some(file));
        assert_eq!(db.index_of_id(101), Some(db.parent_of(file)));
        assert_eq!(db.index_of_id(9999), None);
    }

    #[test]
    fn removing_a_record_marks_a_tombstone() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 1);
        let doomed = builder.add_child(root, &EntryInfo::file("doomed.txt"));
        builder.remove(doomed);
        let db = builder.finalize();
        assert_eq!(db.len(), 1);
        let names: Vec<String> = db
            .iter_ordered()
            .map(|index| db.entry(index).unwrap().name)
            .collect();
        assert_eq!(names, vec!["C:\\".to_string()]);
    }

    #[test]
    fn filetime_conversions_round_trip() {
        // 2024-01-01T00:00:00Z
        let unix = 1_704_067_200i64;
        assert_eq!(mtime_to_unix(unix_to_mtime(unix)), unix);
        assert_eq!(filetime_to_mtime(133_485_408_000_000_000), 13_348_540_800);
    }

    #[test]
    fn describe_bytes_picks_a_sensible_unit() {
        assert_eq!(describe_bytes(512), "512 B");
        assert_eq!(describe_bytes(2048), "2.0 KiB");
        assert_eq!(describe_bytes(3 << 20), "3.0 MiB");
    }
}

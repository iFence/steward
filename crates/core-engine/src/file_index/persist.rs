//! Persistence: a compact snapshot of the index plus the USN cursors, so a
//! restart can load instead of re-enumerating the volume.
//!
//! The snapshot is a formal binary stream with a signature, a format-version
//! field, counts and the volume/USN information. Steward stores it as a single
//! BLOB in the `settings_blob` table of the same SQLite database that already
//! caches apps and plugin metadata; the byte arena travels at its native width
//! instead of base64, so the row is roughly a third smaller and decoding is a
//! bounds-checked `try_into` rather than a JSON parse.
//!
//! Invariants worth stating explicitly, because they decide whether a load is
//! safe:
//!
//! - The parent-pointer layout is *positional*: every array must have the same
//!   length as the arena's record count, or the blob is rejected.
//! - The `name_index` must be exactly the permutation `0..count`, or a search
//!   would silently skip records.
//! - The blob carries a timestamp; a snapshot older than the staleness window
//!   is discarded so a long-dormant index is not trusted as current.

use std::collections::HashMap;
use std::io::{self, Write};

use super::db::{
    FileDb, FileDbBuilder, IndexError, JournalState, BLOCKS, FORMAT_VERSION, MAGIC, MAX_NAME_BYTES,
    ROOT_PARENT,
};

/// `settings_blob` key holding the snapshot.
pub const SETTING_KEY: &str = "file_index";

/// A snapshot older than this is not trusted; the index is rebuilt instead.
pub const MAX_SNAPSHOT_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// Version of the binary snapshot layout. Bumping it rejects older rows.
pub const SNAPSHOT_VERSION: u32 = 2;

/// Bytes in the fixed part of the header: magic, version, taken-at, record
/// count, truncated-name count and the arena length.
const HEADER_BYTES: usize = 4 + 4 + 8 + 8 + 8 + 8;

/// Bytes [`encode_to`] writes for `db` (the fixed header, the arena, the
/// parallel arrays and the journal cursors).
///
/// Callers that stream the snapshot into a fixed-size destination (the
/// `zeroblob` + incremental BLOB write in `steward-storage`) need the exact
/// length up front; keeping the arithmetic here keeps it next to the encoder
/// that must match it.
pub fn encoded_len(db: &FileDb) -> usize {
    let count = db.offsets.len();
    HEADER_BYTES
        + db.data.len()
        + count * (4 + 2 + 4 + 8 + 4 + 1 + 4)
        + 4
        + db.journals().len() * (1 + 8 + 8)
}

/// Encode an index into its compact binary snapshot, written to `sink`.
///
/// The layout is positional and little-endian (see the module docs). It carries
/// the same data the in-memory index holds, at its native width, so decoding is
/// a bounds-checked `try_into` per element instead of a JSON parse plus a base64
/// decode.
pub fn encode_to<W: Write + ?Sized>(
    db: &FileDb,
    truncated_names: usize,
    taken_at: u64,
    sink: &mut W,
) -> io::Result<()> {
    let count = db.offsets.len();
    let mut journals: Vec<(u8, JournalState)> = db
        .journals()
        .iter()
        .map(|(drive, state)| (*drive, *state))
        .collect();
    // `HashMap` iteration order is not deterministic; sort so equal indexes
    // produce equal snapshots.
    journals.sort_by_key(|(drive, _)| *drive);

    sink.write_all(&MAGIC.to_le_bytes())?;
    sink.write_all(&SNAPSHOT_VERSION.to_le_bytes())?;
    sink.write_all(&taken_at.to_le_bytes())?;
    sink.write_all(&(count as u64).to_le_bytes())?;
    sink.write_all(&(truncated_names as u64).to_le_bytes())?;
    sink.write_all(&(db.data.len() as u64).to_le_bytes())?;
    sink.write_all(&db.data)?;
    write_u32s(sink, &db.offsets)?;
    write_u16s(sink, &db.lengths)?;
    write_u32s(sink, &db.parent)?;
    write_u64s(sink, &db.ids)?;
    write_u32s(sink, &db.name_index)?;
    sink.write_all(&db.removed)?;
    write_u32s(sink, &db.depths)?;
    sink.write_all(&(journals.len() as u32).to_le_bytes())?;
    for (drive, state) in journals {
        sink.write_all(&[drive])?;
        sink.write_all(&state.journal_id.to_le_bytes())?;
        sink.write_all(&state.next_usn.to_le_bytes())?;
    }
    Ok(())
}

/// Encode a snapshot into a fresh buffer (tests and in-memory callers).
pub fn encode(db: &FileDb, truncated_names: usize, taken_at: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded_len(db));
    encode_to(db, truncated_names, taken_at, &mut out).expect("writing into a Vec cannot fail");
    out
}

/// Elements converted per scratch buffer, so a multi-million-record array is
/// never materialized a second time just to write it.
const WRITE_CHUNK: usize = 8192;

fn write_u16s<W: Write + ?Sized>(sink: &mut W, values: &[u16]) -> io::Result<()> {
    let mut scratch = Vec::with_capacity(WRITE_CHUNK.min(values.len().max(1)) * 2);
    for chunk in values.chunks(WRITE_CHUNK) {
        scratch.clear();
        for value in chunk {
            scratch.extend_from_slice(&value.to_le_bytes());
        }
        sink.write_all(&scratch)?;
    }
    Ok(())
}

fn write_u32s<W: Write + ?Sized>(sink: &mut W, values: &[u32]) -> io::Result<()> {
    let mut scratch = Vec::with_capacity(WRITE_CHUNK.min(values.len().max(1)) * 4);
    for chunk in values.chunks(WRITE_CHUNK) {
        scratch.clear();
        for value in chunk {
            scratch.extend_from_slice(&value.to_le_bytes());
        }
        sink.write_all(&scratch)?;
    }
    Ok(())
}

fn write_u64s<W: Write + ?Sized>(sink: &mut W, values: &[u64]) -> io::Result<()> {
    let mut scratch = Vec::with_capacity(WRITE_CHUNK.min(values.len().max(1)) * 8);
    for chunk in values.chunks(WRITE_CHUNK) {
        scratch.clear();
        for value in chunk {
            scratch.extend_from_slice(&value.to_le_bytes());
        }
        sink.write_all(&scratch)?;
    }
    Ok(())
}

/// Decode a binary snapshot, rejecting anything that fails the invariants.
pub fn decode(blob: &[u8], now: u64) -> Result<FileDb, IndexError> {
    let mut cursor = Cursor::new(blob);
    if cursor.u32()? != MAGIC {
        return Err(IndexError::Magic);
    }
    let version = cursor.u32()?;
    if version != SNAPSHOT_VERSION {
        return Err(IndexError::Version(version));
    }
    let taken_at = cursor.u64()?;
    let count = usize::try_from(cursor.u64()?).map_err(|_| IndexError::Short)?;
    let _truncated = cursor.u64()?;
    let data_len = usize::try_from(cursor.u64()?).map_err(|_| IndexError::Short)?;
    if taken_at != 0 && now.saturating_sub(taken_at) > MAX_SNAPSHOT_AGE_SECS {
        return Err(IndexError::Version(taken_at as u32));
    }

    let data = cursor.take(data_len)?.to_vec();
    let offsets = cursor.u32s(count)?;
    let lengths = cursor.u16s(count)?;
    let parent = cursor.u32s(count)?;
    let ids = cursor.u64s(count)?;
    let name_index = cursor.u32s(count)?;
    let removed = cursor.take(count)?.to_vec();
    let depths = cursor.u32s(count)?;
    let journal_count = usize::try_from(cursor.u32()?).map_err(|_| IndexError::Short)?;

    // The name array must be exactly the permutation `0..count`.
    let mut seen = vec![false; count];
    for index in &name_index {
        let index = usize::try_from(*index).map_err(|_| IndexError::Magic)?;
        if index >= count || std::mem::replace(&mut seen[index], true) {
            return Err(IndexError::Magic);
        }
    }
    // Every record must point at a valid slot or at ROOT_PARENT.
    for parent in &parent {
        if *parent != ROOT_PARENT && *parent as usize >= count {
            return Err(IndexError::Magic);
        }
    }
    // Every name must lie inside the arena, allowing for the 4-byte escaped
    // length slot that follows the 24-byte record header for long names.
    for record in 0..count {
        let offset = offsets[record] as usize;
        if offset + 24 > data.len() {
            return Err(IndexError::Magic);
        }
        let escaped = data[offset + 4] == 0xff;
        let start = offset + 24 + if escaped { 4 } else { 0 };
        let length = lengths[record] as usize;
        if length > MAX_NAME_BYTES || start + length > data.len() {
            return Err(IndexError::Magic);
        }
    }

    let mut journals = HashMap::new();
    for _ in 0..journal_count {
        let drive = cursor.u8()?;
        let journal_id = cursor.u64()?;
        let next_usn = cursor.i64()?;
        journals.insert(
            drive.to_ascii_uppercase(),
            JournalState {
                journal_id,
                next_usn,
            },
        );
    }
    if cursor.remaining() != 0 {
        return Err(IndexError::Magic);
    }

    let max_depth = depths.iter().copied().max().unwrap_or(1).max(1);
    let live = removed.iter().filter(|flag| **flag == 0).count();
    let dirs = (0..count)
        .filter(|index| removed[*index] == 0 && data[offsets[*index] as usize + 5] & 0b10 != 0)
        .count();
    let blocks: Vec<u32> = (0..count).step_by(BLOCKS).map(|at| at as u32).collect();
    Ok(FileDb::from_parts(
        data,
        offsets,
        lengths,
        parent,
        ids,
        name_index,
        if blocks.is_empty() { vec![0] } else { blocks },
        removed,
        depths,
        live,
        dirs,
        max_depth,
        journals,
    ))
}

/// A bounds-checked reader over the snapshot bytes.
///
/// Every array read is size-checked before it is taken, so a corrupt count
/// cannot make the decoder allocate before it knows the bytes are really there.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], IndexError> {
        let end = self.at.checked_add(len).ok_or(IndexError::Short)?;
        if end > self.bytes.len() {
            return Err(IndexError::Short);
        }
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, IndexError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, IndexError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn u64(&mut self) -> Result<u64, IndexError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn i64(&mut self) -> Result<i64, IndexError> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn u32s(&mut self, count: usize) -> Result<Vec<u32>, IndexError> {
        let bytes = self.take(count.checked_mul(4).ok_or(IndexError::Short)?)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4 bytes")))
            .collect())
    }

    fn u16s(&mut self, count: usize) -> Result<Vec<u16>, IndexError> {
        let bytes = self.take(count.checked_mul(2).ok_or(IndexError::Short)?)?;
        Ok(bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes(chunk.try_into().expect("2 bytes")))
            .collect())
    }

    fn u64s(&mut self, count: usize) -> Result<Vec<u64>, IndexError> {
        let bytes = self.take(count.checked_mul(8).ok_or(IndexError::Short)?)?;
        Ok(bytes
            .chunks_exact(8)
            .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("8 bytes")))
            .collect())
    }
}

/// Header written ahead of the envelope, so a truncated or foreign row is
/// rejected before JSON parsing: `"FSDB"`, the format version, and the record
/// count as little-endian `u32`s.
pub fn encode_header(version: u32, count: u32) -> [u8; 12] {
    let mut header = [0u8; 12];
    header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    header[4..8].copy_from_slice(&version.to_le_bytes());
    header[8..12].copy_from_slice(&count.to_le_bytes());
    header
}

/// Validate a header written by [`encode_header`].
pub fn check_header(header: &[u8], expected_count: u32) -> Result<(), IndexError> {
    if header.len() < 12 {
        return Err(IndexError::Short);
    }
    let magic = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes"));
    if magic != MAGIC {
        return Err(IndexError::Magic);
    }
    let version = u32::from_le_bytes(header[4..8].try_into().expect("4 bytes"));
    if version != FORMAT_VERSION {
        return Err(IndexError::Version(version));
    }
    let count = u32::from_le_bytes(header[8..12].try_into().expect("4 bytes"));
    if count != expected_count {
        return Err(IndexError::Short);
    }
    Ok(())
}

/// Build an index from a walk/scanner and return both the index and the
/// builder's diagnostic counter.
pub fn build<F>(expected: usize, fill: F) -> (FileDb, usize)
where
    F: FnOnce(&mut FileDbBuilder),
{
    let mut builder = FileDbBuilder::with_capacity(expected);
    fill(&mut builder);
    let truncated = builder.truncated_names();
    (builder.finalize(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_index::db::EntryInfo;

    fn sample() -> (FileDb, usize) {
        build(8, |builder| {
            let root = builder.add_root("C:\\", 1);
            let dir = builder.add_child(root, &EntryInfo::dir("Users"));
            builder.add_child(
                dir,
                &EntryInfo::file("notes.txt").with_size(11).with_mtime(22),
            );
            builder.add_child(root, &EntryInfo::file("readme.md"));
        })
    }

    #[test]
    fn a_snapshot_round_trips() {
        let (db, truncated) = sample();
        let blob = encode(&db, truncated, 1_000);
        let restored = decode(&blob, 1_000).expect("snapshot decodes");

        assert_eq!(restored.len(), db.len());
        assert_eq!(restored.dir_count(), db.dir_count());
        assert_eq!(restored.block_count(), db.block_count());
        assert_eq!(restored.max_depth(), db.max_depth());
        // `arena_bytes` reports capacity, which the decoder cannot reproduce
        // exactly; compare the live record bytes through every restored record.
        for record in restored.iter_ordered() {
            assert!(restored.entry(record).is_some());
        }
        let paths: Vec<String> = restored
            .iter_ordered()
            .map(|record| restored.path_of(record).to_string_lossy().into_owned())
            .collect();
        // Index order is grouped by parent record index (roots last), with
        // entries under one parent keeping their creation order.
        assert_eq!(
            paths,
            vec!["C:\\Users", "C:\\readme.md", "C:\\Users\\notes.txt", "C:\\"]
        );
        let notes = restored
            .iter_ordered()
            .find(|record| restored.entry(*record).unwrap().name == "notes.txt")
            .expect("notes.txt present");
        let entry = restored.entry(notes).expect("live");
        assert_eq!(entry.size, Some(11));
        assert_eq!(entry.mtime, Some(22));
    }

    #[test]
    fn journal_cursors_survive_a_round_trip() {
        let (mut db, truncated) = sample();
        db.set_journal(
            b'c',
            JournalState {
                journal_id: 0xDEAD_BEEF,
                next_usn: 42,
            },
        );
        let blob = encode(&db, truncated, 1_000);
        let restored = decode(&blob, 1_000).expect("snapshot decodes");
        assert_eq!(
            restored.journal(b'C'),
            Some(JournalState {
                journal_id: 0xDEAD_BEEF,
                next_usn: 42,
            })
        );
    }

    #[test]
    fn a_stale_snapshot_is_rejected() {
        let (db, truncated) = sample();
        let blob = encode(&db, truncated, 1_000);
        assert!(decode(&blob, 1_000 + MAX_SNAPSHOT_AGE_SECS).is_ok());
        assert!(matches!(
            decode(&blob, 1_000 + MAX_SNAPSHOT_AGE_SECS + 1),
            Err(IndexError::Version(_))
        ));
    }

    #[test]
    fn corrupt_blobs_are_rejected_rather_than_trusted() {
        assert!(matches!(
            decode(b"not a snapshot", 0),
            Err(IndexError::Magic)
        ));
        let (db, truncated) = sample();
        let mut blob = encode(&db, truncated, 1_000);
        // Truncating the envelope must not produce a half-built index.
        blob.truncate(blob.len() / 2);
        assert!(decode(&blob, 1_000).is_err());
    }

    #[test]
    fn a_tampered_name_index_is_rejected() {
        let (db, truncated) = sample();
        let mut blob = encode(&db, truncated, 1_000);
        // Duplicate an entry instead of permuting: not a permutation any more.
        let count = db.offsets.len();
        let name_index_at = HEADER_BYTES + db.data.len() + count * (4 + 2 + 4 + 8);
        let first = blob[name_index_at..name_index_at + 4].to_vec();
        blob[name_index_at + 4..name_index_at + 8].copy_from_slice(&first);
        assert!(matches!(decode(&blob, 1_000), Err(IndexError::Magic)));
    }

    #[test]
    fn a_tampered_parent_index_is_rejected() {
        let (db, truncated) = sample();
        let mut blob = encode(&db, truncated, 1_000);
        let count = db.offsets.len();
        let parent_at = HEADER_BYTES + db.data.len() + count * (4 + 2);
        blob[parent_at + 4..parent_at + 8].copy_from_slice(&999u32.to_le_bytes());
        assert!(matches!(decode(&blob, 1_000), Err(IndexError::Magic)));
    }

    #[test]
    fn headers_round_trip_and_guard_the_envelope() {
        let header = encode_header(FORMAT_VERSION, 7);
        assert!(check_header(&header, 7).is_ok());
        assert!(matches!(check_header(&header, 8), Err(IndexError::Short)));
        assert!(matches!(check_header(b"xx", 7), Err(IndexError::Short)));
        let mut wrong_version = header;
        wrong_version[4..8].copy_from_slice(&9u32.to_le_bytes());
        assert!(matches!(
            check_header(&wrong_version, 7),
            Err(IndexError::Version(9))
        ));
    }

    /// The streamed encoder has to produce exactly the buffered bytes (and
    /// `encoded_len` has to match), whatever chunk sizes the destination
    /// happens to accept. `zeroblob` + incremental BLOB I/O relies on both.
    #[test]
    fn the_streamed_encoding_matches_the_buffered_one() {
        let (db, truncated) = sample();
        let expected = encode(&db, truncated, 1_000);
        assert_eq!(encoded_len(&db), expected.len());

        /// Accepts at most 7 bytes per `write`, forcing `write_all` to loop and
        /// exercising every partial-write path in the encoder.
        struct Trickle(Vec<u8>);
        impl std::io::Write for Trickle {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let take = buf.len().min(7);
                self.0.extend_from_slice(&buf[..take]);
                Ok(take)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut streamed = Trickle(Vec::new());
        encode_to(&db, truncated, 1_000, &mut streamed).expect("a Vec never fails");
        assert_eq!(streamed.0, expected);
    }

    /// A snapshot carries record ids but not the derived `id → record` map, so
    /// a USN batch has to open a lookup session before it can resolve a change
    /// that names a pre-existing file.
    ///
    /// This is the cold-start path: the app decodes the snapshot, then replays
    /// the journal over it (`crates/app/src/file_index.rs`).
    #[cfg(target_os = "windows")]
    #[test]
    fn a_restored_index_can_still_apply_usn_records() {
        use crate::file_index::ntfs::UsnRecord;
        use crate::file_index::update::{apply_usn_records, volume_root};
        use windows::Win32::System::Ioctl::{
            USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE, USN_REASON_RENAME_NEW_NAME,
            USN_REASON_RENAME_OLD_NAME,
        };

        let (db, truncated) = build(8, |builder| {
            let root = builder.add_root("C:\\", 1);
            let dir = builder.add_child(root, &EntryInfo::dir("Users").with_id(2, 1));
            builder.add_child(dir, &EntryInfo::file("a.txt").with_id(3, 2));
        });
        let blob = encode(&db, truncated, 1_000);
        let mut db = decode(&blob, 1_000).expect("snapshot decodes");

        // The map is derived state and stays released after decoding; the USN
        // batch below is what opens it.
        assert_eq!(db.index_of_id(3), None, "no lookup session yet");
        assert_eq!(db.path_of(2).to_string_lossy(), "C:\\Users\\a.txt");
        db.rebuild_key_map();
        assert_eq!(db.index_of_id(3), Some(2), "id → record map was rebuilt");

        let root = volume_root(&db, b'C').expect("volume root survives the snapshot");
        // `FILE_ATTRIBUTE_DIRECTORY`: a rename record for a directory has to say
        // so, otherwise the replacement entry is indexed as a file and cannot
        // take children.
        const ATTR_DIRECTORY: u32 = 0x10;
        let record = |file_id, parent_id, name: &str, reason| UsnRecord {
            file_id,
            parent_id,
            usn: 1,
            reason,
            attributes: 0,
            mtime: Some(1000),
            name: name.to_string(),
        };

        // A delete of a file that was already in the snapshot: the branch that
        // used to be dropped because the map was empty.
        let outcome = apply_usn_records(
            &mut db,
            root,
            &[record(3, 2, "a.txt", USN_REASON_FILE_DELETE)],
        );
        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.skipped, 0, "an indexed file must not be skipped");
        assert!(db.index_of_id(3).is_none());
        assert!(!db
            .iter_ordered()
            .any(|record| db.entry(record).unwrap().name == "a.txt"));

        // A rename of a pre-existing file resolves its old record by id too.
        let renamed = apply_usn_records(
            &mut db,
            root,
            &[
                record(2, 1, "Users", USN_REASON_RENAME_OLD_NAME),
                UsnRecord {
                    attributes: ATTR_DIRECTORY,
                    ..record(2, 1, "Profiles", USN_REASON_RENAME_NEW_NAME)
                },
            ],
        );
        assert_eq!(renamed.renamed, 1);
        assert_eq!(renamed.skipped, 0);
        assert_eq!(
            db.path_of(db.index_of_id(2).unwrap()).to_string_lossy(),
            "C:\\Profiles"
        );

        // A create needs no map lookup and keeps working either way. Its parent
        // is the renamed directory, which only resolves because the map was
        // rebuilt from the snapshot.
        let created = apply_usn_records(
            &mut db,
            root,
            &[record(4, 2, "b.txt", USN_REASON_FILE_CREATE)],
        );
        assert_eq!(created.created, 1);
        assert_eq!(created.skipped, 0);
        assert_eq!(
            db.path_of(db.index_of_id(4).unwrap()).to_string_lossy(),
            "C:\\Profiles\\b.txt"
        );
    }
}

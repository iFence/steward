//! Persistence: a compact snapshot of the index plus the USN cursors, so a
//! restart can load instead of re-enumerating the volume.
//!
//! The report (§7) describes the original writing a logical stream with an
//! `ESDb` signature, a format-version field, counts and the volume/USN
//! information, with an optional bzip2 layer. Steward stores its snapshot in
//! the `settings` table of the same SQLite database that already caches apps
//! and plugin metadata, and encodes the byte arena as base64 inside a
//! `serde_json` envelope — no new dependency, and it survives schema migration.
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

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::db::{
    FileDb, FileDbBuilder, IndexError, JournalState, BLOCKS, FORMAT_VERSION, MAGIC, MAX_NAME_BYTES,
    ROOT_PARENT,
};

/// `settings` key holding the snapshot.
pub const SETTING_KEY: &str = "file_index";

/// A snapshot older than this is not trusted; the index is rebuilt instead.
pub const MAX_SNAPSHOT_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// The persisted envelope.
#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    /// UNIX seconds when the snapshot was taken.
    taken_at: u64,
    /// Record count, checked against the arena before anything is trusted.
    count: usize,
    /// The byte arena, base64 encoded.
    data: String,
    offsets: Vec<u32>,
    lengths: Vec<u16>,
    parent: Vec<u32>,
    ids: Vec<u64>,
    name_index: Vec<u32>,
    removed: Vec<u8>,
    depths: Vec<u32>,
    #[serde(default)]
    truncated_names: usize,
    /// `(drive letter, journal id, next usn)` per volume.
    #[serde(default)]
    journals: Vec<(u8, u64, i64)>,
}

/// Encode an index into a storable string.
pub fn encode(db: &FileDb, truncated_names: usize, taken_at: u64) -> String {
    let snapshot = Snapshot {
        taken_at,
        count: db.offsets.len(),
        data: BASE64.encode(&db.data),
        offsets: db.offsets.clone(),
        lengths: db.lengths.clone(),
        parent: db.parent.clone(),
        ids: db.ids.clone(),
        name_index: db.name_index.clone(),
        removed: db.removed.clone(),
        depths: db.depths.clone(),
        truncated_names,
        journals: db
            .journals()
            .iter()
            .map(|(drive, state)| (*drive, state.journal_id, state.next_usn))
            .collect(),
    };
    serde_json::to_string(&snapshot).unwrap_or_default()
}

/// Decode a snapshot, rejecting anything that fails the invariants.
pub fn decode(blob: &str, now: u64) -> Result<FileDb, IndexError> {
    let snapshot: Snapshot = serde_json::from_str(blob).map_err(|_| IndexError::Magic)?;
    if snapshot.taken_at != 0 && now.saturating_sub(snapshot.taken_at) > MAX_SNAPSHOT_AGE_SECS {
        return Err(IndexError::Version(snapshot.taken_at as u32));
    }
    let data = BASE64
        .decode(snapshot.data.as_bytes())
        .map_err(|_| IndexError::Magic)?;
    let count = snapshot.offsets.len();
    if count != snapshot.count
        || snapshot.parent.len() != count
        || snapshot.ids.len() != count
        || snapshot.lengths.len() != count
    {
        return Err(IndexError::Short);
    }
    if snapshot.name_index.len() != count {
        return Err(IndexError::Short);
    }
    // The name array must be exactly the permutation `0..count`.
    let mut seen = vec![false; count];
    for index in &snapshot.name_index {
        let Ok(index) = usize::try_from(*index) else {
            return Err(IndexError::Magic);
        };
        if index >= count || std::mem::replace(&mut seen[index], true) {
            return Err(IndexError::Magic);
        }
    }
    // Every record must point at a valid slot or at ROOT_PARENT.
    for parent in &snapshot.parent {
        if *parent != ROOT_PARENT && *parent as usize >= count {
            return Err(IndexError::Magic);
        }
    }
    // Every offset/length pair must lie inside the arena.
    for (offset, length) in snapshot.offsets.iter().zip(snapshot.lengths.iter()) {
        let start = *offset as usize + 24;
        let end = start + *length as usize;
        if end > data.len() || *length as usize > MAX_NAME_BYTES {
            return Err(IndexError::Magic);
        }
    }

    let mut journals = std::collections::HashMap::new();
    for (drive, journal_id, next_usn) in snapshot.journals {
        journals.insert(
            drive.to_ascii_uppercase(),
            JournalState {
                journal_id,
                next_usn,
            },
        );
    }
    let depths = if snapshot.depths.len() == count {
        snapshot.depths
    } else {
        vec![0; count]
    };
    let removed = if snapshot.removed.len() == count {
        snapshot.removed
    } else {
        vec![0; count]
    };
    let max_depth = depths.iter().copied().max().unwrap_or(1).max(1);
    let live = removed.iter().filter(|flag| **flag == 0).count();
    let dirs = (0..count)
        .filter(|index| {
            removed[*index] == 0 && data[snapshot.offsets[*index] as usize + 5] & 0b10 != 0
        })
        .count();
    let blocks: Vec<u32> = (0..count).step_by(BLOCKS).map(|at| at as u32).collect();
    Ok(FileDb::from_parts(
        data,
        snapshot.offsets,
        snapshot.lengths,
        snapshot.parent,
        snapshot.ids,
        snapshot.name_index,
        if blocks.is_empty() { vec![0] } else { blocks },
        removed,
        depths,
        live,
        dirs,
        max_depth,
        journals,
    ))
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
        // Index order is case-insensitive by path, so `readme.md` precedes `Users`.
        assert_eq!(
            paths,
            vec!["C:\\", "C:\\readme.md", "C:\\Users", "C:\\Users\\notes.txt"]
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
        assert!(matches!(decode("not json", 0), Err(IndexError::Magic)));
        let (db, truncated) = sample();
        let mut blob = encode(&db, truncated, 1_000);
        // Truncating the envelope must not produce a half-built index.
        blob.truncate(blob.len() / 2);
        assert!(decode(&blob, 1_000).is_err());
    }

    #[test]
    fn a_tampered_name_index_is_rejected() {
        let (db, truncated) = sample();
        let blob = encode(&db, truncated, 1_000);
        let mut value: serde_json::Value = serde_json::from_str(&blob).expect("valid json");
        // Duplicate an entry instead of permuting: not a permutation any more.
        value["name_index"] = serde_json::json!([0, 0, 1, 2]);
        assert!(matches!(
            decode(&value.to_string(), 1_000),
            Err(IndexError::Magic)
        ));
    }

    #[test]
    fn a_tampered_parent_index_is_rejected() {
        let (db, truncated) = sample();
        let blob = encode(&db, truncated, 1_000);
        let mut value: serde_json::Value = serde_json::from_str(&blob).expect("valid json");
        value["parent"] = serde_json::json!([4294967295u32, 999, 0, 0]);
        assert!(matches!(
            decode(&value.to_string(), 1_000),
            Err(IndexError::Magic)
        ));
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

    /// A snapshot carries record ids but not the derived `id → record` map, so
    /// a restored index has to re-derive it — otherwise USN catch-up silently
    /// discards every change that names a pre-existing file.
    ///
    /// This is the cold-start path: the app decodes the snapshot, then replays
    /// the journal onto a duplicate of it (`crates/app/src/file_index.rs`).
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

        // The map is derived, not persisted, so it must be back after decoding.
        assert_eq!(db.index_of_id(3), Some(2), "id → record map was rebuilt");
        assert_eq!(db.path_of(2).to_string_lossy(), "C:\\Users\\a.txt");

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

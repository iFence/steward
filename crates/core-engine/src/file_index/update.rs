//! Incremental index maintenance from the USN Journal (report §5.1).
//!
//! The original does not treat a USN record as a row to insert. Directory and
//! file records take different branches, a rename needs the old name paired
//! with the new one, and some changes require re-reading metadata:
//!
//! | reason bits | handling |
//! |---|---|
//! | `0x100` / `0x200` | create / delete |
//! | `0x1000` / `0x2000` | rename old / new name: remember, then move |
//! | `0x8000` and data bits | refresh size/time/attributes |
//!
//! Steward keeps that shape. What it does *not* keep is the original's
//! fully-sorted incremental update: after applying a batch, the name array is
//! re-sorted once, which is O(n log n) on a batch boundary rather than per
//! record and keeps one code path for "the index is in index order".

use std::collections::HashSet;

use super::db::{EntryInfo, FileDb, JournalState, ROOT_PARENT};
use super::ntfs::UsnRecord;

/// What happened to a batch of USN records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsnOutcome {
    /// Records handed to [`apply_usn_records`].
    pub seen: usize,
    pub created: usize,
    pub removed: usize,
    pub renamed: usize,
    pub metadata_refreshed: usize,
    /// Changes that had to be ignored (parent not indexed, unknown file).
    pub skipped: usize,
    /// Set when the journal identity changed, so the volume must be rebuilt.
    pub requires_rebuild: bool,
}

impl UsnOutcome {
    /// Whether anything changed the index, i.e. whether a re-sort is needed.
    pub fn is_empty(&self) -> bool {
        self.created == 0 && self.removed == 0 && self.renamed == 0 && self.metadata_refreshed == 0
    }
}

/// A pending "rename old name" whose matching new name has not arrived yet.
///
/// Only the file id is needed: the record is still indexed under its old name,
/// so the paired new-name record replaces it by id (report §5.1: "save the old
/// name and old parent, then process the new name").
type PendingRenames = HashSet<u64>;

/// Apply a batch of USN changes to `db`.
///
/// `volume_root` is the record index of the volume's root directory inside
/// `db`, which is where records whose parent id is the volume root land. A
/// record whose parent is not indexed at all is skipped, and the next full
/// rebuild reconciles it.
pub fn apply_usn_records(db: &mut FileDb, volume_root: u32, records: &[UsnRecord]) -> UsnOutcome {
    let mut outcome = UsnOutcome {
        seen: records.len(),
        ..UsnOutcome::default()
    };
    // Renames arrive as two records (old name, then new name); pair them.
    // File ids whose "rename old name" half has arrived and whose paired
    // "rename new name" has not yet (report §5.1). Only the id matters: the
    // record is still indexed under its old name, so the new-name record
    // replaces it by id.
    let mut pending: PendingRenames = PendingRenames::new();
    let mut changed = false;

    for record in records {
        if record.is_delete() {
            let Some(index) = db.index_of_id(record.file_id) else {
                outcome.skipped += 1;
                continue;
            };
            db.remove_subtree(index);
            outcome.removed += 1;
            changed = true;
            continue;
        }
        if record.is_rename_old() {
            pending.insert(record.file_id);
            continue;
        }
        if record.is_rename_new() {
            let _paired = pending.remove(&record.file_id);
            // A rename is "remove the old entry, insert the new one": the name
            // changed, and the parent may have too, which is exactly a move.
            // The new parent is resolved *before* the removal, because removing
            // the record also drops its `id → record` entry, and a move into a
            // different directory still needs the new parent to be reachable.
            let parent = resolve_parent(db, volume_root, record.parent_id);
            if let Some(index) = db.index_of_id(record.file_id) {
                db.remove_subtree(index);
                outcome.removed += 1;
            }
            match parent.and_then(|parent| db.insert_child(parent, &record.to_entry())) {
                Some(_) => {
                    outcome.renamed += 1;
                    changed = true;
                }
                None => outcome.skipped += 1,
            }
            continue;
        }
        if record.is_create() {
            match insert(db, volume_root, &record.to_entry()) {
                Some(_) => {
                    outcome.created += 1;
                    changed = true;
                }
                // A create for an entry that is already indexed (a replayed
                // journal window) is not an error.
                None => outcome.skipped += 1,
            }
            continue;
        }
        if record.is_data_or_basic_change() {
            // Refresh from the filesystem rather than from the USN record: the
            // journal carries no size, and the report notes that some changes
            // require re-reading metadata.
            if let Some(index) = db.index_of_id(record.file_id) {
                if refresh_metadata(db, index) {
                    outcome.metadata_refreshed += 1;
                    changed = true;
                }
            } else {
                outcome.skipped += 1;
            }
            continue;
        }
        outcome.skipped += 1;
    }

    if changed {
        db.finish_incremental();
    }
    // Unmatched "rename old" records are harmless: the file stays indexed under
    // its old name until the next rebuild or the paired new-name record.
    outcome
}

/// Insert one record from a USN event.
fn insert(db: &mut FileDb, volume_root: u32, info: &EntryInfo) -> Option<u32> {
    // A new file whose directory is not indexed cannot be linked yet; the next
    // rebuild picks it up.
    let parent = resolve_parent(db, volume_root, info.parent_id)?;
    db.insert_child(parent, info)
}

/// Map a parent file reference number to a record index.
///
/// Special cases, both from the report: the volume root's own record has no
/// parent, and some system records report a parent id of 0 or 5 (`$Extend`
/// style roots) — those fall back to the volume root so their children still
/// land inside the index.
fn resolve_parent(db: &FileDb, volume_root: u32, parent_id: u64) -> Option<u32> {
    if parent_id == 0 {
        return Some(volume_root);
    }
    match db.index_of_id(parent_id) {
        Some(index) if db.is_dir(index) => Some(index),
        _ => None,
    }
}

/// Re-read a record's size and mtime from the filesystem.
fn refresh_metadata(db: &mut FileDb, index: u32) -> bool {
    let path = db.path_of(index);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return false;
    };
    let size = if metadata.is_dir() {
        None
    } else {
        Some(metadata.len())
    };
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| super::db::unix_to_mtime(duration.as_secs() as i64));
    db.set_metadata(index, size, mtime)
}

/// Whether a stored cursor can still be used, or the volume must be rebuilt
/// (report §5.2: journal identity changed, monitor out of date, journal
/// deleted).
pub fn cursor_is_usable(stored: Option<JournalState>, live: JournalState) -> bool {
    match stored {
        Some(stored) => stored.journal_id == live.journal_id && stored.next_usn <= live.next_usn,
        None => false,
    }
}

/// Root index for a volume letter inside `db`, if the index has one.
pub fn volume_root(db: &FileDb, letter: u8) -> Option<u32> {
    let prefix = format!("{}:\\", (letter as char).to_ascii_uppercase());
    (0..db.slot_count() as u32).find(|index| {
        db.parent_of(*index) == ROOT_PARENT
            && db.is_live(*index)
            && db
                .entry(*index)
                .is_some_and(|entry| entry.name.eq_ignore_ascii_case(&prefix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_index::db::FileDbBuilder;
    use crate::file_index::ntfs::UsnRecord;
    use windows::Win32::System::Ioctl::{
        USN_REASON_DATA_EXTEND, USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE,
        USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME,
    };

    /// `C:\` (id 5) → `Users` (id 6) → `a.txt` (id 7).
    fn populated() -> (FileDb, u32) {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 5);
        builder.add_entry(&EntryInfo::dir("Users").with_id(6, 5));
        builder.add_entry(&EntryInfo::file("a.txt").with_id(7, 6));
        (builder.finalize(), root)
    }

    fn record(file_id: u64, parent_id: u64, name: &str, reason: u32) -> UsnRecord {
        UsnRecord {
            file_id,
            parent_id,
            usn: 1,
            reason,
            attributes: 0,
            mtime: Some(1000),
            name: name.to_string(),
        }
    }

    #[test]
    fn a_create_inserts_a_record() {
        let (mut db, root) = populated();
        let before = db.len();
        let outcome = apply_usn_records(
            &mut db,
            root,
            &[record(8, 6, "b.txt", USN_REASON_FILE_CREATE)],
        );
        assert_eq!(outcome.created, 1);
        assert_eq!(db.len(), before + 1);
        let index = db.index_of_id(8).expect("new record is indexed");
        assert_eq!(db.path_of(index).to_string_lossy(), "C:\\Users\\b.txt");
    }

    #[test]
    fn a_create_under_an_unknown_parent_is_skipped() {
        let (mut db, root) = populated();
        let outcome = apply_usn_records(
            &mut db,
            root,
            &[record(9, 999, "orphan.txt", USN_REASON_FILE_CREATE)],
        );
        assert_eq!(outcome.created, 0);
        assert_eq!(outcome.skipped, 1);
        assert!(db.index_of_id(9).is_none());
    }

    #[test]
    fn a_create_for_the_volume_root_parent_lands_at_the_root() {
        let (mut db, root) = populated();
        let outcome = apply_usn_records(
            &mut db,
            root,
            &[record(10, 0, "boot.ini", USN_REASON_FILE_CREATE)],
        );
        assert_eq!(outcome.created, 1);
        let index = db.index_of_id(10).expect("indexed");
        assert_eq!(db.path_of(index).to_string_lossy(), "C:\\boot.ini");
    }

    #[test]
    fn a_delete_removes_the_record() {
        let (mut db, root) = populated();
        let outcome = apply_usn_records(
            &mut db,
            root,
            &[record(7, 6, "a.txt", USN_REASON_FILE_DELETE)],
        );
        assert_eq!(outcome.removed, 1);
        assert!(db.index_of_id(7).is_none());
        assert!(!db
            .iter_ordered()
            .any(|index| db.entry(index).unwrap().name == "a.txt"));
    }

    #[test]
    fn deleting_a_directory_removes_its_subtree() {
        let (mut db, root) = populated();
        let outcome = apply_usn_records(
            &mut db,
            root,
            &[record(6, 5, "Users", USN_REASON_FILE_DELETE)],
        );
        assert_eq!(outcome.removed, 1);
        assert_eq!(db.len(), 1, "only the volume root survives");
        assert!(db.index_of_id(7).is_none(), "the child went with it");
    }

    #[test]
    fn a_rename_replaces_the_old_entry() {
        let (mut db, root) = populated();
        let outcome = apply_usn_records(
            &mut db,
            root,
            &[
                record(7, 6, "a.txt", USN_REASON_RENAME_OLD_NAME),
                record(7, 6, "renamed.txt", USN_REASON_RENAME_NEW_NAME),
            ],
        );
        assert_eq!(outcome.renamed, 1);
        let index = db.index_of_id(7).expect("still indexed");
        assert_eq!(db.entry(index).unwrap().name, "renamed.txt");
        assert_eq!(
            db.path_of(index).to_string_lossy(),
            "C:\\Users\\renamed.txt"
        );
        assert_eq!(db.len(), 3, "no duplicate was left behind");
    }

    #[test]
    fn a_move_into_another_directory_updates_the_path() {
        let mut builder = FileDbBuilder::new();
        let root = builder.add_root("C:\\", 5);
        builder.add_entry(&EntryInfo::dir("from").with_id(6, 5));
        builder.add_entry(&EntryInfo::dir("to").with_id(9, 5));
        builder.add_entry(&EntryInfo::file("a.txt").with_id(7, 6));
        let mut db = builder.finalize();
        assert_eq!(
            db.path_of(db.index_of_id(7).expect("indexed"))
                .to_string_lossy(),
            "C:\\from\\a.txt"
        );

        let outcome = apply_usn_records(
            &mut db,
            root,
            &[
                record(7, 6, "a.txt", USN_REASON_RENAME_OLD_NAME),
                record(7, 9, "a.txt", USN_REASON_RENAME_NEW_NAME),
            ],
        );
        assert_eq!(outcome.renamed, 1, "the move is applied");
        assert_eq!(outcome.skipped, 0);
        let index = db.index_of_id(7).expect("still indexed");
        assert_eq!(db.path_of(index).to_string_lossy(), "C:\\to\\a.txt");
        // The old entry is gone, so the file is not indexed twice.
        assert_eq!(
            db.iter_ordered()
                .filter(|record| db.entry(*record).is_some_and(|entry| entry.name == "a.txt"))
                .count(),
            1
        );
    }

    #[test]
    fn data_changes_refresh_metadata_from_disk() {
        // The refresh reads the filesystem, so the index is rooted at the temp
        // directory that holds a real probe file.
        let mut path = std::env::temp_dir();
        path.push(format!("steward-usn-{}.txt", std::process::id()));
        std::fs::write(&path, b"0123456789").expect("write probe file");
        let file_name = path
            .file_name()
            .expect("probe file has a name")
            .to_string_lossy()
            .into_owned();
        let root_name = std::env::temp_dir().to_string_lossy().into_owned();

        let mut builder = FileDbBuilder::new();
        let root = builder.add_root(&root_name, 5);
        builder.add_child(
            root,
            &EntryInfo::file(&file_name).with_id(7, 5).with_size(0),
        );
        let mut db = builder.finalize();

        let outcome = apply_usn_records(
            &mut db,
            root,
            &[record(7, 5, &file_name, USN_REASON_DATA_EXTEND)],
        );
        assert_eq!(outcome.metadata_refreshed, 1);
        let index = db.index_of_id(7).expect("indexed");
        assert_eq!(db.entry(index).expect("live").size, Some(10));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_reasons_are_counted_as_skipped() {
        let (mut db, root) = populated();
        let outcome = apply_usn_records(&mut db, root, &[record(7, 6, "a.txt", 0x0040_0000)]);
        assert_eq!(outcome.skipped, 1);
        assert!(outcome.is_empty());
    }

    #[test]
    fn cursor_usability_follows_the_journal_identity() {
        let live = JournalState {
            journal_id: 10,
            next_usn: 100,
        };
        assert!(cursor_is_usable(Some(live), live));
        assert!(cursor_is_usable(
            Some(JournalState {
                journal_id: 10,
                next_usn: 50
            }),
            live
        ));
        assert!(
            !cursor_is_usable(
                Some(JournalState {
                    journal_id: 11,
                    next_usn: 100
                }),
                live
            ),
            "a recreated journal forces a rebuild"
        );
        assert!(
            !cursor_is_usable(
                Some(JournalState {
                    journal_id: 10,
                    next_usn: 200
                }),
                live
            ),
            "a cursor ahead of the journal is stale"
        );
        assert!(!cursor_is_usable(None, live));
    }

    #[test]
    fn volume_root_lookup_finds_the_root_record() {
        let (db, root) = populated();
        assert_eq!(volume_root(&db, b'c'), Some(root));
        assert_eq!(volume_root(&db, b'z'), None);
    }
}

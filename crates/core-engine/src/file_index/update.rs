//! Incremental index maintenance from the USN Journal.
//!
//! A USN record is not a row to insert. Directory and file records take
//! different branches, a rename needs the old name paired with the new one, and
//! some changes require re-reading metadata:
//!
//! | reason bits | handling |
//! |---|---|
//! | `0x100` / `0x200` | create / delete |
//! | `0x1000` / `0x2000` | rename old / new name: remember, then move |
//! | `0x8000` and data bits | refresh size/time/attributes |
//!
//! After applying a batch the name array is regrouped by parent with an O(n)
//! integer counting sort 鈥?no path strings and no comparisons. A record rename
//! replaces the record slot and re-points its direct children, so a moved
//! directory keeps its subtree; deletes tombstone the whole subtree in one walk.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use super::db::{unix_to_mtime, EntryInfo, FileDb, JournalState, ROOT_PARENT};
use super::ntfs::UsnRecord;
use super::watch::{FsAction, FsChange};

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
/// so the paired new-name record replaces it by id.
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
    // "rename new name" has not yet. Only the id matters: the
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
            let entry = record.to_entry();
            let replaced = db.index_of_id(record.file_id).map(|index| {
                // Keep the subtree: a renamed directory's descendants are
                // re-pointed at the new slot instead of being dropped.
                parent.and_then(|parent| db.replace_record(index, parent, &entry))
            });
            match replaced {
                Some(Some(_)) => {
                    outcome.renamed += 1;
                    changed = true;
                }
                // Not indexed (a replayed journal window): the new name is a
                // plain create.
                None => match parent.and_then(|parent| db.insert_child(parent, &entry)) {
                    Some(_) => {
                        outcome.renamed += 1;
                        changed = true;
                    }
                    None => outcome.skipped += 1,
                },
                Some(None) => outcome.skipped += 1,
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
            // journal carries no size, and some changes require re-reading
            // metadata.
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
/// Special cases: the volume root's own record has no
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
/// (the journal identity changed or its data was deleted).
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

/// What applying a batch of filesystem changes did to the index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FsOutcome {
    pub created: usize,
    pub removed: usize,
    pub renamed: usize,
    pub refreshed: usize,
    /// Changes whose parent (or entry) is not in the index; the next reconcile
    /// picks them up.
    pub skipped: usize,
}

impl FsOutcome {
    /// Whether anything changed the index, i.e. whether a re-sort is needed.
    pub fn is_empty(&self) -> bool {
        self.created == 0 && self.removed == 0 && self.renamed == 0 && self.refreshed == 0
    }
}

/// Apply real-time filesystem changes (from [`super::watch::DirectoryWatcher`])
/// to `db`.
///
/// This is the non-privileged analogue of [`apply_usn_records`]: the USN path
/// keys on NTFS file-reference numbers, while `ReadDirectoryChangesW` only
/// reports names, so a record is located by walking the index from the root
/// whose path prefixes the change. Creates, deletes, renames and metadata
/// changes all resolve that way, and a duplicate event (the same change also
/// seen through the USN journal) is a no-op.
pub fn apply_fs_changes(db: &mut FileDb, changes: &[FsChange]) -> FsOutcome {
    let mut outcome = FsOutcome::default();
    let mut changed = false;
    // "rename old" halves whose new half has not arrived yet in this batch.
    // They are removed only after the whole batch: a directory rename keeps its
    // subtree by moving the old record, and a cross-volume move (whose new half
    // lands on another volume's watcher) still ends up removed.
    let mut pending_renames: VecDeque<(PathBuf, u32)> = VecDeque::new();
    for change in changes {
        match change.action {
            FsAction::Created | FsAction::Modified => {
                if upsert_path(db, &change.path, &mut outcome) {
                    changed = true;
                }
            }
            FsAction::Removed => {
                if remove_path(db, &change.path, &mut outcome) {
                    changed = true;
                }
            }
            FsAction::RenamedOld => match resolve_entry(db, &change.path) {
                Some(index) => pending_renames.push_back((change.path.clone(), index)),
                None => outcome.skipped += 1,
            },
            FsAction::RenamedNew => {
                // A rename arrives as old-name then new-name. Pair the new half
                // with the oldest waiting old half; that covers both the
                // interleaved and the back-to-back event orders.
                let paired = pending_renames.pop_front().map(|(_, old_index)| old_index);
                let moved = paired.is_some_and(|old_index| {
                    rename_record(db, old_index, &change.path, &mut outcome)
                });
                if moved {
                    outcome.renamed += 1;
                    changed = true;
                    continue;
                }
                if let Some(old_index) = paired {
                    // The new name could not be linked (its parent is not
                    // indexed yet, or the file vanished): the old entry is gone
                    // either way, so drop it now.
                    db.remove_subtree(old_index);
                    outcome.removed += 1;
                    changed = true;
                } else if upsert_path(db, &change.path, &mut outcome) {
                    changed = true;
                    outcome.renamed += 1;
                }
            }
        }
    }
    // Any old half without a matching new half moved away from this volume.
    for (_path, index) in pending_renames {
        db.remove_subtree(index);
        outcome.removed += 1;
        changed = true;
    }
    if changed {
        db.finish_incremental();
    }
    outcome
}

/// The root record whose path prefixes `path`, plus the component names below
/// it (empty when `path` is the root itself).
fn root_and_rest(db: &FileDb, path: &Path) -> Option<(u32, Vec<String>)> {
    let text = path.to_string_lossy();
    // Compare without a trailing separator so the root itself (`C:\`) and a
    // child (`C:\Windows`) take the same path.
    let trimmed = text.trim_end_matches(['\\', '/']);
    let mut best: Option<(u32, usize)> = None;
    for index in 0..db.slot_count() as u32 {
        if !db.is_live(index) || db.parent_of(index) != ROOT_PARENT {
            continue;
        }
        let root = db.path_of(index).to_string_lossy().into_owned();
        let root = root.trim_end_matches(['\\', '/']);
        let prefix_len = if trimmed.eq_ignore_ascii_case(root) {
            trimmed.len()
        } else if trimmed.len() > root.len()
            && trimmed[..root.len()].eq_ignore_ascii_case(root)
            && matches!(trimmed.as_bytes().get(root.len()), Some(b'\\') | Some(b'/'))
        {
            root.len() + 1
        } else {
            continue;
        };
        // Longest matching root wins, so `D:\Media` beats a hypothetical `D:`
        // if both are indexed.
        if best.is_none_or(|(_, len)| prefix_len > len) {
            best = Some((index, prefix_len));
        }
    }
    let (root, prefix_len) = best?;
    let rest = text[prefix_len.min(text.len())..]
        .split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    Some((root, rest))
}

/// Resolve a directory path to its record index, if every component is
/// indexed.
fn resolve_dir(db: &FileDb, path: &Path) -> Option<u32> {
    let (root, rest) = root_and_rest(db, path)?;
    let mut current = root;
    for part in &rest {
        current = db.child_by_name(current, part, true)?;
    }
    Some(current)
}

/// Resolve any indexed path (file or directory) to its record index.
fn resolve_entry(db: &FileDb, path: &Path) -> Option<u32> {
    let (root, rest) = root_and_rest(db, path)?;
    let mut current = root;
    for (position, part) in rest.iter().enumerate() {
        current = if position + 1 == rest.len() {
            db.child_by_name(current, part, true)
                .or_else(|| db.child_by_name(current, part, false))?
        } else {
            db.child_by_name(current, part, true)?
        };
    }
    Some(current)
}

/// Build the index record for a path from its current on-disk metadata.
fn entry_info(path: &Path) -> Option<EntryInfo> {
    let metadata = std::fs::metadata(path).ok()?;
    let is_dir = metadata.is_dir();
    let size = (!is_dir).then_some(metadata.len());
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| unix_to_mtime(duration.as_secs() as i64));
    Some(EntryInfo {
        // A watcher cannot supply an NTFS file id; `replace_record` preserves
        // the old record's id when this one is unknown.
        id: 0,
        parent_id: 0,
        size,
        mtime,
        attributes: if is_dir { 0x10 } else { 0 },
        name: path.file_name()?.to_string_lossy().into_owned(),
        is_dir,
    })
}

/// Move `old_index` to `path`, keeping its subtree (a same-batch rename).
fn rename_record(db: &mut FileDb, old_index: u32, path: &Path, outcome: &mut FsOutcome) -> bool {
    let (Some(parent_path), Some(info)) = (path.parent(), entry_info(path)) else {
        outcome.skipped += 1;
        return false;
    };
    let Some(parent) = resolve_dir(db, parent_path) else {
        outcome.skipped += 1;
        return false;
    };
    match db.replace_record(old_index, parent, &info) {
        Some(_) => true,
        None => {
            outcome.skipped += 1;
            false
        }
    }
}

/// Insert a created path, or refresh the metadata of one already indexed.
fn upsert_path(db: &mut FileDb, path: &Path, outcome: &mut FsOutcome) -> bool {
    let (Some(parent_path), Some(name)) = (path.parent(), path.file_name()) else {
        outcome.skipped += 1;
        return false;
    };
    let name = name.to_string_lossy();
    let Some(parent) = resolve_dir(db, parent_path) else {
        // The parent is not indexed (an excluded directory, or a create that
        // arrived before its parent's own event): reconcile later.
        outcome.skipped += 1;
        return false;
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        outcome.skipped += 1;
        return false;
    };
    let is_dir = metadata.is_dir();
    let size = (!is_dir).then_some(metadata.len());
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| unix_to_mtime(duration.as_secs() as i64));
    if let Some(index) = db.child_by_name(parent, &name, is_dir) {
        if db.set_metadata(index, size, mtime) {
            outcome.refreshed += 1;
            return true;
        }
        return false;
    }
    let info = EntryInfo {
        // A watcher cannot supply an NTFS file id; `0` means "unknown" and the
        // record links by parent index (exactly like a walk-built record).
        id: 0,
        parent_id: 0,
        size,
        mtime,
        attributes: if is_dir { 0x10 } else { 0 },
        name: name.into_owned(),
        is_dir,
    };
    match db.insert_child(parent, &info) {
        Some(_) => {
            outcome.created += 1;
            true
        }
        None => {
            outcome.skipped += 1;
            false
        }
    }
}

/// Tombstone the record at `path`, if the index has one.
fn remove_path(db: &mut FileDb, path: &Path, outcome: &mut FsOutcome) -> bool {
    let Some(index) = resolve_entry(db, path) else {
        outcome.skipped += 1;
        return false;
    };
    db.remove_subtree(index);
    outcome.removed += 1;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_index::db::FileDbBuilder;
    use crate::file_index::ntfs::UsnRecord;
    use std::path::PathBuf;
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

    fn scratch_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "steward-fs-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn fs_changes_add_refresh_rename_and_remove_by_path() {
        let root = scratch_dir("changes");
        let mut builder = FileDbBuilder::new();
        let root_index = builder.add_root(&root.to_string_lossy(), 0);
        let mut db = builder.finalize();

        // Create: the file exists on disk and is inserted under the root.
        let created = root.join("added.txt");
        std::fs::write(&created, b"one").unwrap();
        let outcome = apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::Created,
                path: created.clone(),
            }],
        );
        assert_eq!(outcome.created, 1);
        let index = db.child_by_name(root_index, "added.txt", false).unwrap();
        assert_eq!(db.meta(index).0, Some(3));

        // Modify: same path, new size, metadata refreshed in place.
        std::fs::write(&created, b"longer").unwrap();
        let outcome = apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::Modified,
                path: created.clone(),
            }],
        );
        assert_eq!(outcome.refreshed, 1);
        assert_eq!(db.meta(index).0, Some(6));

        // Rename: the old record goes, the new path is inserted.
        let renamed = root.join("renamed.txt");
        std::fs::rename(&created, &renamed).unwrap();
        let outcome = apply_fs_changes(
            &mut db,
            &[
                FsChange {
                    action: FsAction::RenamedOld,
                    path: created.clone(),
                },
                FsChange {
                    action: FsAction::RenamedNew,
                    path: renamed.clone(),
                },
            ],
        );
        assert_eq!(outcome.renamed, 1);
        assert!(db.child_by_name(root_index, "added.txt", false).is_none());
        assert!(db.child_by_name(root_index, "renamed.txt", false).is_some());

        // Delete: the record and its subtree are tombstoned.
        std::fs::remove_file(&renamed).unwrap();
        let outcome = apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::Removed,
                path: renamed.clone(),
            }],
        );
        assert_eq!(outcome.removed, 1);
        assert!(db.child_by_name(root_index, "renamed.txt", false).is_none());
        assert_eq!(db.len(), 1, "only the root record is left");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fs_changes_before_the_parent_is_indexed_are_skipped() {
        let root = scratch_dir("skipped");
        let mut builder = FileDbBuilder::new();
        builder.add_root(&root.to_string_lossy(), 0);
        let mut db = builder.finalize();
        let nested = root.join("not-indexed").join("file.txt");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, b"x").unwrap();
        let outcome = apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::Created,
                path: nested,
            }],
        );
        assert_eq!(outcome.created, 0);
        assert_eq!(outcome.skipped, 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A whole batch of nested edits, then a directory rename and a subtree
    /// delete. Locks in the two properties the old fixpoint orphan sweep used
    /// to paper over: `remove_subtree` really visits the descendants, and
    /// child lookups stay correct while a batch is still being applied.
    #[test]
    fn nested_batches_keep_child_lookups_correct_and_leave_no_orphans() {
        let root = scratch_dir("nested-batch");
        let mut builder = FileDbBuilder::new();
        let root_index = builder.add_root(&root.to_string_lossy(), 0);
        let mut db = builder.finalize();

        let dir_a = root.join("dir_a");
        let deep = dir_a.join("deep");
        let file = deep.join("file.txt");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(&file, b"x").unwrap();
        let dir_b = root.join("dir_b");
        std::fs::create_dir_all(&dir_b).unwrap();

        let outcome = apply_fs_changes(
            &mut db,
            &[
                FsChange {
                    action: FsAction::Created,
                    path: dir_a.clone(),
                },
                FsChange {
                    action: FsAction::Created,
                    path: deep.clone(),
                },
                FsChange {
                    action: FsAction::Created,
                    path: file.clone(),
                },
                FsChange {
                    action: FsAction::Created,
                    path: dir_b.clone(),
                },
            ],
        );
        assert_eq!(outcome.created, 4);

        let dir_a_index = db.child_by_name(root_index, "dir_a", true).unwrap();
        let deep_index = db.child_by_name(dir_a_index, "deep", true).unwrap();
        let file_index = db.child_by_name(deep_index, "file.txt", false).unwrap();
        assert_eq!(db.children(dir_a_index), vec![deep_index]);
        assert_eq!(db.children(deep_index), vec![file_index]);
        assert_eq!(
            db.path_of(file_index).to_string_lossy(),
            file.to_string_lossy()
        );

        // Move the whole subtree by renaming its top directory: the record's
        // descendants keep their parent pointers, so only the top entry moves.
        let moved = root.join("dir_c");
        std::fs::rename(&dir_a, &moved).unwrap();
        apply_fs_changes(
            &mut db,
            &[
                FsChange {
                    action: FsAction::RenamedOld,
                    path: dir_a.clone(),
                },
                FsChange {
                    action: FsAction::RenamedNew,
                    path: moved.clone(),
                },
            ],
        );
        let moved_index = db.child_by_name(root_index, "dir_c", true).unwrap();
        let moved_file = moved.join("deep").join("file.txt");
        assert_eq!(
            db.path_of(db.child_by_name(moved_index, "deep", true).unwrap())
                .to_string_lossy(),
            moved.join("deep").to_string_lossy()
        );
        let file_index = db
            .child_by_name(
                db.child_by_name(moved_index, "deep", true).unwrap(),
                "file.txt",
                false,
            )
            .unwrap();
        assert_eq!(
            db.path_of(file_index).to_string_lossy(),
            moved_file.to_string_lossy()
        );

        // Delete the renamed tree: the directory record and every descendant go.
        std::fs::remove_dir_all(&moved).unwrap();
        let outcome = apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::Removed,
                path: moved.clone(),
            }],
        );
        assert_eq!(outcome.removed, 1);
        assert_eq!(db.len(), 2, "root and dir_b survive");
        assert!(db.child_by_name(root_index, "dir_c", true).is_none());
        assert!(db.child_by_name(root_index, "dir_b", true).is_some());

        // No orphan may stay live: every record's parent is a live directory.
        for index in db.iter_ordered() {
            let parent = db.parent_of(index);
            if parent == crate::file_index::db::ROOT_PARENT {
                continue;
            }
            assert!(
                db.is_dir(parent),
                "record {} points at dead parent {parent}",
                db.entry(index).unwrap().name
            );
        }

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_rename_split_across_batches_still_moves_the_record() {
        let root = scratch_dir("rename-split");
        let mut builder = FileDbBuilder::new();
        let root_index = builder.add_root(&root.to_string_lossy(), 0);
        let mut db = builder.finalize();
        let old = root.join("before.txt");
        std::fs::write(&old, b"x").unwrap();
        apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::Created,
                path: old.clone(),
            }],
        );
        assert!(db.child_by_name(root_index, "before.txt", false).is_some());

        // The cross-volume move case: the old-name half and the new-name half
        // are applied in separate batches.
        std::fs::remove_file(&old).unwrap();
        apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::RenamedOld,
                path: old.clone(),
            }],
        );
        let new = root.join("after.txt");
        std::fs::write(&new, b"x").unwrap();
        apply_fs_changes(
            &mut db,
            &[FsChange {
                action: FsAction::RenamedNew,
                path: new.clone(),
            }],
        );
        assert!(db.child_by_name(root_index, "before.txt", false).is_none());
        assert!(db.child_by_name(root_index, "after.txt", false).is_some());
        std::fs::remove_dir_all(&root).unwrap();
    }
}

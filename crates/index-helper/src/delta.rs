//! Turn USN change records into path-based deltas.
//!
//! A USN record carries the changed entry's *name* and its *parent's file id*,
//! not a path. The helper therefore keeps only the directory subset of the
//! index (`id -> (parent_id, name)`), which is a few percent of the entries, and
//! resolves the parent chain for each change. The result is exactly what
//! `core_engine::file_index::apply_fs_changes` consumes, so the app can apply a
//! delta with the same code path as its own filesystem watcher.
//!
//! This module is deliberately free of Windows types: the USN reason bits are
//! mirrored here so the conversion can be unit-tested on any host.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `USN_REASON_FILE_CREATE`.
pub const REASON_FILE_CREATE: u32 = 0x0000_0100;
/// `USN_REASON_FILE_DELETE`.
pub const REASON_FILE_DELETE: u32 = 0x0000_0200;
/// `USN_REASON_RENAME_OLD_NAME`.
pub const REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
/// `USN_REASON_RENAME_NEW_NAME`.
pub const REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;
/// `USN_REASON_DATA_OVERWRITE`.
pub const REASON_DATA_OVERWRITE: u32 = 0x0000_0001;
/// `USN_REASON_DATA_EXTEND`.
pub const REASON_DATA_EXTEND: u32 = 0x0000_0002;
/// `USN_REASON_DATA_TRUNCATION`.
pub const REASON_DATA_TRUNCATION: u32 = 0x0000_0004;
/// `USN_REASON_BASIC_INFO_CHANGE`.
pub const REASON_BASIC_INFO_CHANGE: u32 = 0x0000_8000;

/// `FILE_ATTRIBUTE_DIRECTORY`.
const ATTR_DIRECTORY: u32 = 0x10;
/// Any data/basic-info change is a metadata refresh for the index.
const REASON_METADATA: u32 =
    REASON_DATA_OVERWRITE | REASON_DATA_EXTEND | REASON_DATA_TRUNCATION | REASON_BASIC_INFO_CHANGE;
/// Guard against a corrupt/cyclic parent chain.
const MAX_DEPTH: usize = 256;

/// What the app should do with the entry at [`Delta::path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaAction {
    Created,
    Removed,
    Modified,
    /// The "from" half of a rename.
    RenamedOld,
    /// The "to" half of a rename.
    RenamedNew,
}

/// One change, resolved to an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub action: DeltaAction,
    pub path: PathBuf,
}

/// The portable subset of a USN record this module needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRecord {
    pub file_id: u64,
    pub parent_id: u64,
    pub name: String,
    pub attributes: u32,
    pub reason: u32,
}

/// The directory subset of the index: `file id -> (parent id, name)`.
///
/// Only directories are tracked, because a change is resolved through its
/// parent's path; files never need to be paths for other records.
#[derive(Debug, Default)]
pub struct DirIndex {
    entries: HashMap<u64, DirEntry>,
    /// The volume root's file id, used when a record reports `parent_id == 0`.
    root: Option<u64>,
}

#[derive(Debug, Clone)]
struct DirEntry {
    parent_id: u64,
    name: String,
}

impl DirIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the volume root (its `parent_id` is `0` by convention).
    pub fn insert_root(&mut self, id: u64, name: impl Into<String>) {
        self.entries.insert(
            id,
            DirEntry {
                parent_id: 0,
                name: name.into(),
            },
        );
        self.root = Some(id);
    }

    /// Record a directory. Re-inserting an existing id updates it.
    pub fn insert(&mut self, id: u64, parent_id: u64, name: impl Into<String>) -> bool {
        self.entries
            .insert(
                id,
                DirEntry {
                    parent_id,
                    name: name.into(),
                },
            )
            .is_none()
    }

    /// Forget a directory (its descendants are dropped implicitly: they can no
    /// longer resolve a parent and are skipped until the next snapshot).
    pub fn remove(&mut self, id: u64) -> bool {
        let removed = self.entries.remove(&id).is_some();
        if self.root == Some(id) {
            self.root = None;
        }
        removed
    }

    /// Move/rename a directory in place.
    pub fn rename(&mut self, id: u64, parent_id: u64, name: impl Into<String>) -> bool {
        match self.entries.get_mut(&id) {
            Some(entry) => {
                entry.parent_id = parent_id;
                entry.name = name.into();
                true
            }
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Absolute path of a directory, or `None` when its chain is incomplete.
    pub fn path_of(&self, id: u64) -> Option<PathBuf> {
        let mut names: Vec<&str> = Vec::new();
        let mut current = id;
        for _ in 0..MAX_DEPTH {
            let entry = self.entries.get(&current)?;
            names.push(entry.name.as_str());
            if entry.parent_id == 0 {
                names.reverse();
                return Some(PathBuf::from(join_chain(&names)));
            }
            current = entry.parent_id;
        }
        None
    }

    /// Path of a record's parent directory. `0` means the volume root, matching
    /// the USN records system entries emit.
    fn parent_path(&self, parent_id: u64) -> Option<PathBuf> {
        let effective = if parent_id == 0 {
            self.root?
        } else {
            parent_id
        };
        self.path_of(effective)
    }
}

fn join_chain(names: &[&str]) -> String {
    let mut path = String::new();
    for (position, name) in names.iter().enumerate() {
        if position > 0 && !path.ends_with('\\') && !path.ends_with('/') {
            path.push('\\');
        }
        path.push_str(name);
    }
    path
}

/// `parent\name`, without doubling a separator and without `Path::join`'s
/// drive-relative behavior (`"C:".join("x")` would produce `"C:x"`).
fn join_path(parent: &Path, name: &str) -> PathBuf {
    let parent = parent.to_string_lossy();
    if parent.is_empty() || parent.ends_with('\\') || parent.ends_with('/') {
        PathBuf::from(format!("{parent}{name}"))
    } else {
        PathBuf::from(format!("{parent}\\{name}"))
    }
}

/// Resolve one record and keep [`DirIndex`] current.
///
/// Returns `None` when the record cannot be linked (its parent is not indexed,
/// or the reason is unknown); the app keeps its old state and the next snapshot
/// reconciles.
pub fn delta_for(dirs: &mut DirIndex, record: &ChangeRecord) -> Option<Delta> {
    let is_dir = record.attributes & ATTR_DIRECTORY != 0;
    let parent = dirs.parent_path(record.parent_id)?;
    let path = join_path(&parent, &record.name);

    if record.reason & REASON_FILE_DELETE != 0 {
        if is_dir {
            dirs.remove(record.file_id);
        }
        return Some(Delta {
            action: DeltaAction::Removed,
            path,
        });
    }
    if record.reason & REASON_RENAME_OLD_NAME != 0 {
        // The directory map is updated by the paired new-name record; a rename
        // split across reads leaves the old name in place until it arrives.
        return Some(Delta {
            action: DeltaAction::RenamedOld,
            path,
        });
    }
    if record.reason & REASON_RENAME_NEW_NAME != 0 {
        if is_dir {
            dirs.rename(record.file_id, record.parent_id, record.name.clone());
        }
        return Some(Delta {
            action: DeltaAction::RenamedNew,
            path,
        });
    }
    if record.reason & REASON_FILE_CREATE != 0 {
        if is_dir {
            dirs.insert(record.file_id, record.parent_id, record.name.clone());
        }
        return Some(Delta {
            action: DeltaAction::Created,
            path,
        });
    }
    if record.reason & REASON_METADATA != 0 {
        return Some(Delta {
            action: DeltaAction::Modified,
            path,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `C:\` (5) -> `Users` (6) -> `docs` (7).
    fn tree() -> DirIndex {
        let mut dirs = DirIndex::new();
        dirs.insert_root(5, "C:");
        dirs.insert(6, 5, "Users");
        dirs.insert(7, 6, "docs");
        dirs
    }

    fn record(file_id: u64, parent_id: u64, name: &str, reason: u32) -> ChangeRecord {
        ChangeRecord {
            file_id,
            parent_id,
            name: name.into(),
            attributes: 0,
            reason,
        }
    }

    #[test]
    fn directory_paths_follow_the_parent_chain() {
        let dirs = tree();
        assert_eq!(dirs.path_of(5).unwrap().to_string_lossy(), "C:");
        assert_eq!(dirs.path_of(6).unwrap().to_string_lossy(), "C:\\Users");
        assert_eq!(
            dirs.path_of(7).unwrap().to_string_lossy(),
            "C:\\Users\\docs"
        );
        assert_eq!(dirs.path_of(999), None);
    }

    #[test]
    fn a_created_file_resolves_to_its_parent_directory() {
        let mut dirs = tree();
        let delta = delta_for(&mut dirs, &record(20, 7, "a.txt", REASON_FILE_CREATE)).unwrap();
        assert_eq!(delta.action, DeltaAction::Created);
        assert_eq!(delta.path.to_string_lossy(), "C:\\Users\\docs\\a.txt");
        // Files are not part of the directory map.
        assert_eq!(dirs.len(), 3);
    }

    #[test]
    fn a_created_directory_becomes_a_parent_for_later_records() {
        let mut dirs = tree();
        let mut created = record(21, 7, "nested", REASON_FILE_CREATE);
        created.attributes = ATTR_DIRECTORY;
        assert_eq!(
            delta_for(&mut dirs, &created)
                .unwrap()
                .path
                .to_string_lossy(),
            "C:\\Users\\docs\\nested"
        );
        let delta = delta_for(&mut dirs, &record(22, 21, "deep.txt", REASON_FILE_CREATE)).unwrap();
        assert_eq!(
            delta.path.to_string_lossy(),
            "C:\\Users\\docs\\nested\\deep.txt"
        );
    }

    #[test]
    fn deleting_a_directory_drops_its_later_children() {
        let mut dirs = tree();
        let mut deleted = record(6, 5, "Users", REASON_FILE_DELETE);
        deleted.attributes = ATTR_DIRECTORY;
        assert_eq!(
            delta_for(&mut dirs, &deleted).unwrap().action,
            DeltaAction::Removed
        );
        assert!(dirs.path_of(6).is_none());
        assert!(delta_for(&mut dirs, &record(30, 6, "x.txt", REASON_FILE_CREATE)).is_none());
    }

    #[test]
    fn a_directory_rename_moves_its_children() {
        let mut dirs = tree();
        let mut old = record(6, 5, "Users", REASON_RENAME_OLD_NAME);
        old.attributes = ATTR_DIRECTORY;
        let old_delta = delta_for(&mut dirs, &old).unwrap();
        assert_eq!(old_delta.action, DeltaAction::RenamedOld);
        assert_eq!(old_delta.path.to_string_lossy(), "C:\\Users");

        let mut new = record(6, 5, "Profiles", REASON_RENAME_NEW_NAME);
        new.attributes = ATTR_DIRECTORY;
        let new_delta = delta_for(&mut dirs, &new).unwrap();
        assert_eq!(new_delta.action, DeltaAction::RenamedNew);
        assert_eq!(new_delta.path.to_string_lossy(), "C:\\Profiles");
        assert_eq!(
            dirs.path_of(7).unwrap().to_string_lossy(),
            "C:\\Profiles\\docs"
        );
    }

    #[test]
    fn metadata_changes_are_refreshes_and_unknown_reasons_are_skipped() {
        let mut dirs = tree();
        let delta = delta_for(&mut dirs, &record(20, 7, "a.txt", REASON_DATA_EXTEND)).unwrap();
        assert_eq!(delta.action, DeltaAction::Modified);
        assert!(delta_for(&mut dirs, &record(20, 7, "a.txt", 0x0000_0000)).is_none());
    }

    #[test]
    fn a_record_under_an_unindexed_parent_is_skipped() {
        let mut dirs = tree();
        assert!(delta_for(&mut dirs, &record(30, 999, "x.txt", REASON_FILE_CREATE)).is_none());
    }

    #[test]
    fn the_volume_root_is_the_parent_for_id_zero() {
        let mut dirs = tree();
        let delta = delta_for(&mut dirs, &record(40, 0, "boot.ini", REASON_FILE_CREATE)).unwrap();
        assert_eq!(delta.path.to_string_lossy(), "C:\\boot.ini");
    }
}

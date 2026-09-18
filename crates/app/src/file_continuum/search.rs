//! Bounded folder suggestions for the file-dialog file continuumer.
//!
//! This module deliberately does not build a disk-wide index. Call it on a
//! worker thread: even a single metadata lookup can wait on a network share.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use steward_core_engine::{AppEntry, Engine};

const MAX_RESULTS: usize = 32;
const MAX_CANDIDATES: usize = 256;
// Limit entries visited, including files, so a folder containing many files
// cannot trigger an unbounded scan just to find a few subdirectories.
const MAX_DIRECTORY_ENTRIES: usize = 256;

/// Suggest existing folders for an editable, initially prefilled path.
/// `recent` is ordered newest first. Relative keywords search those folders
/// and standard user folders; absolute paths browse only one directory level.
pub(super) fn search_directories(query: &str, recent: &[PathBuf]) -> Vec<PathBuf> {
    let query = unquote(query);
    let path = Path::new(query);

    if !query.is_empty() && path.is_absolute() {
        if path.is_dir() {
            let mut results = vec![path.to_path_buf()];
            results.extend(child_directories(path));
            return deduplicate(results).into_iter().take(MAX_RESULTS).collect();
        }

        // `Path` retains drive and UNC share prefixes. Do not split on a
        // separator manually or interpret C:relative as an absolute path.
        if let (Some(parent), Some(leaf)) = (path.parent(), path.file_name()) {
            return fuzzy_directories(&leaf.to_string_lossy(), child_directories(parent), false);
        }
        return Vec::new();
    }

    let standard = [
        dirs::home_dir(),
        dirs::document_dir(),
        dirs::download_dir(),
        dirs::desktop_dir(),
    ];
    let candidates = deduplicate(
        recent
            .iter()
            .take(MAX_CANDIDATES)
            .cloned()
            .chain(standard.into_iter().flatten())
            .filter(|path| path.is_absolute() && path.is_dir()),
    );
    if query.is_empty() {
        // Avoid the matcher's sorting so clearing the input restores recency.
        return candidates.into_iter().take(MAX_RESULTS).collect();
    }
    fuzzy_directories(query, candidates, true)
}

fn unquote(query: &str) -> &str {
    let query = query.trim();
    if let Some(inner) = query.strip_prefix('"') {
        // Also allow editing a quoted path after its closing quote is erased.
        return inner.strip_suffix('"').unwrap_or(inner);
    }
    query
        .strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
        .unwrap_or(query)
}

fn child_directories(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .take(MAX_DIRECTORY_ENTRIES)
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    paths.sort_by_key(|path| path_key(path));
    paths
}

fn fuzzy_directories(query: &str, candidates: Vec<PathBuf>, include_path: bool) -> Vec<PathBuf> {
    let mut entries = Vec::new();
    for path in candidates.into_iter().take(MAX_CANDIDATES) {
        let name = path
            .file_name()
            .unwrap_or_else(|| path.as_os_str())
            .to_string_lossy()
            .into_owned();
        entries.push(AppEntry {
            name,
            path: path.clone(),
        });
        if include_path {
            // Separate entries let the engine score the folder name and full
            // path independently, including its existing pinyin variants.
            entries.push(AppEntry {
                name: path.to_string_lossy().into_owned(),
                path,
            });
        }
    }
    let mut engine = Engine::new();
    engine.set_entries(entries);
    deduplicate(
        engine
            .query(query, &|_| 0)
            .into_iter()
            .map(|entry| entry.path),
    )
    .into_iter()
    .take(MAX_RESULTS)
    .collect()
}

fn deduplicate(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path_key(path)))
        .collect()
}

fn path_key(path: &Path) -> OsString {
    // Components normalize separator spelling, trailing separators and `.`.
    let normalized: PathBuf = path.components().collect();
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        // Work in UTF-16 instead of lowercasing bytes or using a lossy string:
        // Chinese names, surrogate pairs and unpaired surrogates stay intact.
        let units: Vec<_> = normalized
            .as_os_str()
            .encode_wide()
            .map(|unit| match unit {
                65..=90 => unit + 32,
                47 => 92,
                _ => unit,
            })
            .collect();
        OsString::from_wide(&units)
    }
    #[cfg(not(windows))]
    normalized.into_os_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "steward-file-continuum-{}-{unique}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ));
            // Fail on a collision rather than taking ownership of old data.
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn folder(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir(&path).unwrap();
            path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn exact_directory_is_first_and_only_direct_children_are_suggested() {
        let root = TestDirectory::new();
        let child = root.folder("child");
        let nested = child.join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(root.0.join("file.txt"), b"example").unwrap();

        let results = search_directories(&root.0.to_string_lossy(), &[]);
        assert_eq!(results, vec![root.0.clone(), child]);
        assert!(!results.contains(&nested));
    }

    #[test]
    fn partial_absolute_path_matches_the_leaf_with_fuzzy_and_pinyin_search() {
        let root = TestDirectory::new();
        let project = root.folder("Project notes");
        let chinese = root.folder("项目资料");
        root.folder("Unrelated");
        for (query, expected) in [("Prjnt", project), ("xmzl", chinese)] {
            let path = root.0.join(query);
            assert_eq!(
                search_directories(&path.to_string_lossy(), &[]),
                vec![expected]
            );
        }
    }

    #[test]
    fn clearing_restores_recent_order_and_skips_files_missing_paths_and_duplicates() {
        let root = TestDirectory::new();
        let first = root.folder("Zulu");
        let second = root.folder("Alpha");
        let file = root.0.join("file.txt");
        std::fs::write(&file, b"example").unwrap();
        let recent = vec![
            first.clone(),
            file,
            root.0.join("missing"),
            second.clone(),
            first.clone(),
            first.join("."),
        ];
        let results = search_directories("   ", &recent);
        assert_eq!(&results[..2], &[first.clone(), second]);
        assert_eq!(results.iter().filter(|path| **path == first).count(), 1);
        assert!(results.iter().all(|path| path.is_dir()));
    }

    #[test]
    fn relative_keywords_search_recent_folder_names_and_ancestor_names() {
        let root = TestDirectory::new();
        let parent = root.folder("DistinctiveProjectName");
        let child = parent.join("设计文档");
        std::fs::create_dir(&child).unwrap();
        for query in ["shejiwendang", "sjwd", "设计", "DistinctiveProjectName"] {
            assert!(search_directories(query, std::slice::from_ref(&child)).contains(&child));
        }
    }

    #[test]
    fn quoted_paths_with_spaces_and_unicode_are_accepted() {
        let root = TestDirectory::new();
        let folder = root.folder("项目 notes 😀");
        for query in [
            format!("  \"{}\"  ", folder.display()),
            format!("'{}'", folder.display()),
            format!("\"{}", folder.display()),
        ] {
            assert_eq!(search_directories(&query, &[]), vec![folder.clone()]);
        }
    }

    #[test]
    fn results_are_capped_for_browsing_and_recent_search() {
        let root = TestDirectory::new();
        let recent: Vec<_> = (0..MAX_RESULTS + 8)
            .map(|i| root.folder(&format!("BoundedFolder{i:03}")))
            .collect();
        assert_eq!(search_directories("", &recent), recent[..MAX_RESULTS]);
        assert_eq!(
            search_directories("BoundedFolder", &recent).len(),
            MAX_RESULTS
        );
        assert_eq!(
            search_directories(&root.0.to_string_lossy(), &[]).len(),
            MAX_RESULTS
        );
    }

    #[test]
    fn directory_enumeration_is_bounded_before_result_truncation() {
        let root = TestDirectory::new();
        for i in 0..MAX_DIRECTORY_ENTRIES + 4 {
            root.folder(&format!("Folder{i:03}"));
        }
        assert_eq!(child_directories(&root.0).len(), MAX_DIRECTORY_ENTRIES);
    }

    #[test]
    fn nonexistent_absolute_parent_yields_no_suggestions() {
        let root = TestDirectory::new();
        let missing = root.0.join("missing").join("child");
        assert!(
            search_directories(&missing.to_string_lossy(), std::slice::from_ref(&root.0))
                .is_empty()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_keys_preserve_unicode_and_normalize_ascii_case_and_separators() {
        assert_eq!(
            path_key(Path::new("C:\\项目😀\\Notes\\")),
            path_key(Path::new("c:/项目😀/notes")),
        );
        assert_ne!(
            path_key(Path::new("C:\\项目")),
            path_key(Path::new("C:\\资料"))
        );
        let paths = deduplicate([
            PathBuf::from("C:\\项目😀\\Notes"),
            PathBuf::from("c:/项目😀/NOTES/"),
        ]);
        assert_eq!(paths.len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn unc_paths_keep_the_share_prefix_when_finding_a_partial_paths_parent() {
        let path = Path::new(r"\\server\share\项目\partial");
        assert!(path.is_absolute());
        assert_eq!(path.parent(), Some(Path::new(r"\\server\share\项目")));
        assert_eq!(path.file_name(), Some(std::ffi::OsStr::new("partial")));
        assert!(!Path::new(r"C:relative").is_absolute());
        assert_eq!(
            path_key(Path::new(r"\\Server\Share\项目\")),
            path_key(Path::new(r"\\server\share\项目")),
        );
    }
}

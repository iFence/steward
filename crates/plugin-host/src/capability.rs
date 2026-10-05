//! Filesystem capabilities for plugin `fs.read` / `fs.write`.
//!
//! A granted root is opened as a [`cap_std::fs::Dir`] handle, and every file
//! operation is performed *through* that handle with a relative path. Because
//! the path is resolved by the kernel relative to the handle, a symlink swapped
//! in after the check cannot redirect the operation outside the grant — the
//! check-then-use window the old string-prefix sandbox had.

use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

/// Which operation is being attempted; decides which roots are consulted and
/// whether a missing root is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// A granted directory handle plus the relative path to use against it.
pub struct Grant {
    dir: Dir,
    relative: PathBuf,
}

impl Grant {
    /// The directory every operation goes through.
    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    /// The path within the granted directory; never absolute, never `..`.
    pub fn relative(&self) -> &Path {
        &self.relative
    }
}

/// Resolve `path` inside the first root that contains it, returning a directory
/// handle and the relative path. `None` means "denied or unavailable".
pub fn open_in_roots(roots: &[String], path: &Path, access: Access) -> Option<Grant> {
    for root in roots {
        let root = Path::new(root);
        let relative = match resolve_relative(root, path) {
            Some(relative) => relative,
            None => continue,
        };

        if access == Access::Write && !root.exists() {
            // A write grant on a directory that does not exist yet is the host
            // saying that directory belongs to the plugin; making it is
            // honouring the grant, not exceeding it.
            let _ = std::fs::create_dir_all(root);
        }
        let Ok(dir) = Dir::open_ambient_dir(root, ambient_authority()) else {
            continue;
        };
        return Some(Grant { dir, relative });
    }
    None
}

/// The relative path to use against `root`, or `None` when `path` is outside it.
fn resolve_relative(root: &Path, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        let stripped = path.strip_prefix(root).ok()?;
        normalize_within(stripped)
    } else {
        normalize_within(path)
    }
}

/// Lexically normalize a relative path, rejecting anything that climbs out.
fn normalize_within(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            Component::Normal(segment) => out.push(segment),
            // A relative path must not contain a root or Windows prefix.
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "steward-cap-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn absolute_path_inside_a_root_resolves_relative() {
        let root = temp_root("inside");
        std::fs::write(root.join("a.txt"), b"hi").unwrap();
        let grant = open_in_roots(
            &[root.to_string_lossy().to_string()],
            &root.join("a.txt"),
            Access::Read,
        )
        .expect("inside the root");
        assert_eq!(grant.relative(), Path::new("a.txt"));
        assert_eq!(grant.dir().read(grant.relative()).unwrap(), b"hi");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_path_resolves_inside_its_root() {
        let root = temp_root("relative");
        let grant = open_in_roots(
            &[root.to_string_lossy().to_string()],
            Path::new("nested/out.txt"),
            Access::Write,
        )
        .expect("inside the root");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        grant.dir().write(grant.relative(), b"x").unwrap();
        assert!(root.join("nested/out.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn traversal_out_of_a_root_is_refused() {
        let root = temp_root("traversal");
        assert!(open_in_roots(
            &[root.to_string_lossy().to_string()],
            Path::new("../escape.txt"),
            Access::Write
        )
        .is_none());
        assert!(open_in_roots(
            &[root.to_string_lossy().to_string()],
            &root.join("..").join("escape.txt"),
            Access::Read
        )
        .is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_write_grant_creates_a_missing_root() {
        let root = temp_root("missing").join("not-yet");
        let grant = open_in_roots(
            &[root.to_string_lossy().to_string()],
            Path::new("file.txt"),
            Access::Write,
        )
        .expect("the write grant materializes its root");
        assert!(root.is_dir());
        grant.dir().write(grant.relative(), b"x").unwrap();
        assert!(root.join("file.txt").exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn no_roots_means_no_access() {
        assert!(open_in_roots(&[], Path::new("x"), Access::Read).is_none());
    }
}

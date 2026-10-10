//! Removing cache entries that are no longer used.
//!
//! The policy runs once, when the app finds KiCad, before any preview is
//! drawn:
//!
//! - Entries for another KiCad version, colour theme or cache layout are
//!   removed, because nothing will read them again.
//! - A library folder (`sym-<hash>`, `fp-<hash>`, `sym-blob-<hash>`, ...)
//!   not used for [`UNUSED_FOR`] is removed. An edited library gets a new
//!   hash, so its old folder stops being used and goes after that time.
//!
//! Every removed entry can be exported again, so removing one too early
//! only costs one export.

use super::staging;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a library folder may go unused before it is removed.
pub const UNUSED_FOR: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// A file in each library folder whose modification time says when the
/// folder was last used.
pub(super) const LAST_USED: &str = "last-used";

/// What to keep: the cache layout, KiCad version and theme folder names in
/// use now.
pub(super) struct InUse<'a> {
    pub layout: &'a str,
    pub kicad: &'a str,
    pub theme: &'a str,
}

/// Removes what [`InUse`] does not name and library folders unused for
/// [`UNUSED_FOR`]. Returns the folders it removed.
pub(super) fn evict(root: &Path, keep: &InUse<'_>) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    remove_children(root, &mut removed, |name, _| {
        is_layout_name(name) && name != keep.layout
    });
    let layout = root.join(keep.layout);
    remove_children(&layout, &mut removed, |name, _| {
        name.starts_with("kicad-") && name != keep.kicad
    });
    let kicad = layout.join(keep.kicad);
    remove_children(&kicad, &mut removed, |name, _| {
        name.starts_with("theme") && name != keep.theme
    });
    remove_children(&kicad.join(keep.theme), &mut removed, |_, path| {
        !used_within(path, UNUSED_FOR)
    });
    removed
}

/// Removes a preview cache from where an older version of the app kept it:
/// its layout folders and left-over staging folders, then the folder itself
/// if nothing else is in it. Returns the folders it removed.
pub fn remove_old_cache(old_root: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    remove_children(old_root, &mut removed, |name, _| is_layout_name(name));
    let staging_root = old_root.join("staging");
    removed.extend(staging::sweep(&staging_root));
    for empty in [&staging_root, old_root] {
        if fs::remove_dir(empty).is_ok() {
            removed.push(empty.to_path_buf());
        }
    }
    removed
}

/// `v1`, `v2`, ...: a cache layout folder.
fn is_layout_name(name: &str) -> bool {
    name.strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether the library folder was used within `age`: its [`LAST_USED`]
/// file, or the folder itself when it has none, is newer than that.
fn used_within(folder: &Path, age: Duration) -> bool {
    let marker = folder.join(LAST_USED);
    let checked = if marker.exists() {
        marker
    } else {
        folder.to_path_buf()
    };
    !staging::older_than(&checked, age)
}

fn remove_children(
    parent: &Path,
    removed: &mut Vec<PathBuf>,
    should_remove: impl Fn(&str, &Path) -> bool,
) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() && should_remove(&name, &path) && fs::remove_dir_all(&path).is_ok() {
            removed.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::staging::make_older;

    #[test]
    fn layout_names_are_recognised() {
        assert!(is_layout_name("v1") && is_layout_name("v12"));
        assert!(!is_layout_name("v") && !is_layout_name("staging") && !is_layout_name("vx"));
    }

    #[test]
    fn unused_versions_themes_and_libraries_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let theme = root.path().join("v1/kicad-10.0.4/theme_builtin_default");
        let used = theme.join("sym-aaaa");
        let unused = theme.join("fp-bbbb");
        let unused_but_marked = theme.join("sym-blob-cccc");
        let old_marker = theme.join("sym-dddd");
        for folder in [&used, &unused, &unused_but_marked, &old_marker] {
            fs::create_dir_all(folder.join("R-0123")).unwrap();
        }
        make_older(&unused, UNUSED_FOR + Duration::from_secs(60));
        make_older(&unused_but_marked, UNUSED_FOR * 2);
        fs::write(unused_but_marked.join(LAST_USED), "").unwrap();
        fs::write(old_marker.join(LAST_USED), "").unwrap();
        make_older(&old_marker.join(LAST_USED), UNUSED_FOR * 2);
        let other_theme = root.path().join("v1/kicad-10.0.4/theme_other/sym-aaaa");
        let other_kicad = root
            .path()
            .join("v1/kicad-10.0.3/theme_builtin_default/sym-aaaa");
        let other_layout = root.path().join("v0/anything");
        let staging = root.path().join("staging/run-1-0");
        for folder in [&other_theme, &other_kicad, &other_layout, &staging] {
            fs::create_dir_all(folder).unwrap();
        }

        let mut removed = evict(
            root.path(),
            &InUse {
                layout: "v1",
                kicad: "kicad-10.0.4",
                theme: "theme_builtin_default",
            },
        );
        removed.sort();
        let mut expected = vec![
            root.path().join("v0"),
            root.path().join("v1/kicad-10.0.3"),
            root.path().join("v1/kicad-10.0.4/theme_other"),
            unused.clone(),
            old_marker.clone(),
        ];
        expected.sort();
        assert_eq!(removed, expected);
        assert!(used.exists() && unused_but_marked.exists());
        // Staging folders belong to the startup sweep, not to eviction.
        assert!(staging.exists());
    }

    #[test]
    fn an_old_cache_location_is_removed_but_other_files_are_kept() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join("previews");
        fs::create_dir_all(old.join("v1/kicad-10.0.4/theme_builtin_default/sym-aaaa")).unwrap();
        fs::create_dir_all(old.join("staging/123-0")).unwrap();
        make_older(&old.join("staging/123-0"), Duration::from_secs(3600));
        remove_old_cache(&old);
        assert!(!old.exists());

        // A folder holding something else is emptied of cache folders only.
        fs::create_dir_all(old.join("v1")).unwrap();
        fs::write(old.join("notes.txt"), "keep").unwrap();
        remove_old_cache(&old);
        assert!(!old.join("v1").exists());
        assert!(old.join("notes.txt").exists());
    }
}

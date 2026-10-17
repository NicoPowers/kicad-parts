//! Writing library files while KiCad may read them: each file is written
//! in full to a temporary file in the same folder, then renamed over the
//! old one, so KiCad reads either the old file or the new one. The
//! temporary name starts with `.` and ends in `.tmp`, which KiCad never
//! reads as a library.
//!
//! Generated files are marked read-only. Only the update itself clears
//! the mark, to replace a file. KiCad 10 treats them differently:
//!
//! - A symbol library is one file, and KiCad checks whether that file can
//!   be written, so the symbol editor opens it as read-only.
//! - A footprint library is a `.pretty` folder, and KiCad checks whether
//!   the folder can be written (`pcb_io_kicad_sexpr.cpp`, lines 105 and
//!   3674-3680). The folder can be written, so the footprint editor treats
//!   the library as one it can change. Saving over a generated footprint
//!   fails, because that file is read-only (line 3495), but saving a new
//!   footprint into the folder works.
//!
//! So an update reports any file in the `symbols` and `footprints` folders
//! that the Catalog does not make, and leaves it alone.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Why a file could not be written.
#[derive(Debug)]
pub enum WriteError {
    /// Another program, such as KiCad, has the file open in a way that
    /// stops it being replaced. The old file is kept.
    InUse(PathBuf),
    /// Windows refused to replace the file, and no program seems to hold
    /// it open. The old file is kept.
    Denied(PathBuf),
    Io(PathBuf, io::Error),
}

/// Writes `bytes` to `path` through a temporary file and a rename, making
/// the folder first, and marks it read-only.
pub fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let failed = |error| WriteError::Io(path.to_path_buf(), error);
    let folder = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(folder).map_err(failed)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = folder.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        set_read_only(&temporary, true)
    })();
    if let Err(error) = written {
        let _ = remove(&temporary);
        return Err(failed(error));
    }
    let replaced = path.exists();
    if replaced {
        let _ = set_read_only(path, false);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = remove(&temporary);
        if replaced {
            let _ = set_read_only(path, true);
        }
        return Err(why_not_replaced(path, error));
    }
    Ok(())
}

/// Says why a rename over `path` failed: a sharing violation means a
/// program holds it open; access denied means so too only when the file
/// cannot be opened on its own now.
fn why_not_replaced(path: &Path, error: io::Error) -> WriteError {
    match error.raw_os_error() {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        Some(32 | 33) => WriteError::InUse(path.to_path_buf()),
        _ if error.kind() == io::ErrorKind::PermissionDenied => {
            if held_open(path) {
                WriteError::InUse(path.to_path_buf())
            } else {
                WriteError::Denied(path.to_path_buf())
            }
        }
        _ => WriteError::Io(path.to_path_buf(), error),
    }
}

/// Whether another program holds `path` open: opening it with no sharing
/// fails with a sharing violation.
#[cfg(windows)]
fn held_open(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    let opened = fs::OpenOptions::new().read(true).share_mode(0).open(path);
    matches!(opened, Err(error) if error.raw_os_error() == Some(32))
}

#[cfg(not(windows))]
fn held_open(_path: &Path) -> bool {
    false
}

/// Marks `path` read-only, or clears the mark.
pub fn set_read_only(path: &Path, read_only: bool) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    if permissions.readonly() != read_only {
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(read_only);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// Deletes a file, clearing its read-only mark first.
fn remove(path: &Path) -> io::Result<()> {
    let _ = set_read_only(path, false);
    fs::remove_file(path)
}

/// The bytes of `path`, or `None` if it does not exist.
pub fn read_if_there(path: &Path) -> Result<Option<Vec<u8>>, WriteError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(WriteError::Io(path.to_path_buf(), error)),
    }
}

/// Deletes the temporary files an update that stopped part way left in
/// `folder` and the folders below it. Returns how many.
pub fn remove_leftovers(folder: &Path) -> usize {
    let mut removed = 0;
    let Ok(entries) = fs::read_dir(folder) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            removed += remove_leftovers(&path);
        } else if name.starts_with('.') && name.ends_with(".tmp") && remove(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The lock an update holds on its folder, so only one update writes it
/// at a time, from this app or another. Released when dropped.
pub struct FolderLock {
    _file: fs::File,
}

/// The lock file's name.
pub const LOCK_FILE: &str = ".update.lock";

impl FolderLock {
    /// Takes the lock, or `None` if another update holds it.
    pub fn take(folder: &Path) -> io::Result<Option<Self>> {
        let path = folder.join(LOCK_FILE);
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // No sharing: a second open fails while this one is held.
            options.share_mode(0);
        }
        match options.open(&path) {
            Ok(file) => Ok(Some(Self { _file: file })),
            Err(error) if error.raw_os_error() == Some(32) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_read_only_files_and_leaves_no_temporary_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a").join("b.kicad_sym");
        write_whole(&path, b"one").unwrap();
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        // The update itself replaces a read-only file.
        write_whole(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["b.kicad_sym"]);
        assert_eq!(read_if_there(&folder.path().join("none")).unwrap(), None);
        set_read_only(&path, false).unwrap();
    }

    #[test]
    fn leftover_temporary_files_are_removed() {
        let folder = tempfile::tempdir().unwrap();
        let inner = folder.path().join("footprints").join("x.pretty");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join(".R_v1.kicad_mod.123.tmp"), b"half").unwrap();
        set_read_only(&inner.join(".R_v1.kicad_mod.123.tmp"), true).unwrap();
        fs::write(inner.join("R_v1.kicad_mod"), b"whole").unwrap();
        assert_eq!(remove_leftovers(folder.path()), 1);
        let names: Vec<_> = fs::read_dir(&inner)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["R_v1.kicad_mod"]);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_held_open_without_sharing_is_kept_and_named() {
        use std::os::windows::fs::OpenOptionsExt;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("team-parts.kicad_dbl");
        fs::write(&path, b"old").unwrap();
        // Open as a program that lets others read but not delete or
        // rename, as many do.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        match write_whole(&path, b"new") {
            Err(WriteError::InUse(file)) => assert_eq!(file, path),
            other => panic!("expected the file to be in use: {other:?}"),
        }
        drop(held);
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 1);
        write_whole(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        set_read_only(&path, false).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn one_update_holds_the_folder_at_a_time() {
        let folder = tempfile::tempdir().unwrap();
        let lock = FolderLock::take(folder.path()).unwrap();
        assert!(lock.is_some());
        assert!(FolderLock::take(folder.path()).unwrap().is_none());
        drop(lock);
        assert!(FolderLock::take(folder.path()).unwrap().is_some());
    }
}

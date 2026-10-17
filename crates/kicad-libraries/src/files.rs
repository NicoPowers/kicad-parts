//! Writing library files while KiCad may read them: each file is written
//! in full to a temporary file in the same folder, then renamed over the
//! old one, so KiCad reads either the old file or the new one. The
//! temporary name ends in `.tmp`, which KiCad never reads as a library.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Why a file could not be written.
#[derive(Debug)]
pub enum WriteError {
    /// Another program, such as KiCad, has the file open in a way that
    /// stops it being replaced. The old file is kept.
    InUse(PathBuf),
    Io(PathBuf, io::Error),
}

/// Writes `bytes` to `path` through a temporary file and a rename, making
/// the folder first.
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
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(failed(error));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(if in_use(&error) {
            WriteError::InUse(path.to_path_buf())
        } else {
            failed(error)
        });
    }
    Ok(())
}

/// Whether a rename failed because another program holds the file:
/// Windows says access denied or a sharing violation.
fn in_use(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// The bytes of `path`, or `None` if it does not exist.
pub fn read_if_there(path: &Path) -> Result<Option<Vec<u8>>, WriteError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(WriteError::Io(path.to_path_buf(), error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_files_and_leaves_no_temporary_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a").join("b.kicad_sym");
        write_whole(&path, b"one").unwrap();
        write_whole(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["b.kicad_sym"]);
        assert_eq!(read_if_there(&folder.path().join("none")).unwrap(), None);
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
    }
}

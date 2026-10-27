//! Folders under `target/` for the data of a running local team server.
//!
//! Each run gets `<base>/run-<process id>` and holds `in-use.lock` open,
//! without sharing, while it runs. If a run ends without cleaning up (a
//! hard kill, a power cut), nobody holds its lock any more, so the next run
//! deletes the folder. A folder whose lock is held belongs to a run that is
//! still going and is left alone. On systems other than Windows, where files
//! cannot be opened without sharing, only folders older than a day are
//! deleted.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

const LOCK: &str = "in-use.lock";

/// A run's folder. Dropping it deletes the folder.
pub struct RunFolder {
    path: PathBuf,
    lock: Option<File>,
}

impl RunFolder {
    /// Deletes stale run folders under `base`, then creates this run's.
    pub fn create(base: &Path) -> io::Result<Self> {
        fs::create_dir_all(base)?;
        sweep(base);
        let path = base.join(format!("run-{}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        let lock = open_exclusive(&path.join(LOCK), true)?;
        Ok(Self {
            path,
            lock: Some(lock),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Deletes the folder now; call after stopping the server.
    pub fn remove(mut self) -> io::Result<()> {
        self.lock.take();
        fs::remove_dir_all(&self.path)
    }
}

impl Drop for RunFolder {
    fn drop(&mut self) {
        if self.lock.take().is_some() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Deletes every `run-*` folder under `base` that no running process holds.
/// Returns the folders it deleted.
pub fn sweep(base: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(base) else {
        return removed;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_run = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("run-"));
        if !is_run || !path.is_dir() || in_use(&path) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

#[cfg(windows)]
fn in_use(folder: &Path) -> bool {
    match open_exclusive(&folder.join(LOCK), false) {
        Ok(_) => false,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

#[cfg(not(windows))]
fn in_use(folder: &Path) -> bool {
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    fs::metadata(folder)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|age| age < day)
}

#[cfg(windows)]
fn open_exclusive(path: &Path, create: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_exclusive(path: &Path, create: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .open(path)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn stale_folders_are_swept_and_live_ones_kept() {
        let base = tempfile::tempdir().unwrap();
        // A run that ended without cleaning up: no one holds its lock.
        let stale = base.path().join("run-1");
        fs::create_dir_all(stale.join("pb_data")).unwrap();
        fs::write(stale.join(LOCK), "").unwrap();
        fs::write(stale.join("pb_data").join("data.db"), "x").unwrap();
        // A folder without a lock (killed before it took one).
        let unlocked = base.path().join("run-2");
        fs::create_dir_all(&unlocked).unwrap();
        // A run still going.
        let live = RunFolder::create(base.path()).unwrap();
        assert!(!stale.exists() && !unlocked.exists());
        let other = base.path().join("run-3");
        fs::create_dir_all(&other).unwrap();
        let _held = open_exclusive(&other.join(LOCK), true).unwrap();
        assert_eq!(sweep(base.path()), Vec::<PathBuf>::new());
        assert!(live.path().exists() && other.exists());
        // Something else in the base folder is never touched.
        fs::write(base.path().join("notes.txt"), "keep").unwrap();
        let path = live.path().to_owned();
        live.remove().unwrap();
        assert!(!path.exists());
        assert!(base.path().join("notes.txt").exists());
    }
}

//! Staging folders, where kicad-cli writes an export before it is renamed
//! into the cache.
//!
//! Each preview cache gets `staging/run-<process id>-<n>` and holds
//! `in-use.lock` open, without sharing, while it lives. If the app ends
//! without cleaning up (a crash, a hard kill, a power cut), nobody holds the
//! lock any more, so the next start deletes the folder. A folder whose lock
//! is held belongs to an app that is still running and is left alone. On
//! systems other than Windows, where files cannot be opened without
//! sharing, only folders older than a day are deleted.
//!
//! The same pattern keeps local team server data in `tooling/catalog-import`
//! (`run_folder.rs`).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const LOCK: &str = "in-use.lock";

/// Folders from before staging folders were locked are named
/// `<process id>-<n>`. An export takes at most two minutes, so one older
/// than this is left over from a crash.
const UNLOCKED_FOLDER_MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// Tells apart several caches opened by one process, as tests do.
static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

/// This cache's own staging folder. Dropping it deletes the folder.
pub(super) struct RunFolder {
    path: PathBuf,
    lock: Option<File>,
    next_export: AtomicU64,
}

impl RunFolder {
    /// Creates this cache's folder under `staging_root`.
    pub(super) fn create(staging_root: &Path) -> io::Result<Self> {
        let path = staging_root.join(format!(
            "run-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::SeqCst)
        ));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        let lock = open_exclusive(&path.join(LOCK), true)?;
        Ok(Self {
            path,
            lock: Some(lock),
            next_export: AtomicU64::new(0),
        })
    }

    /// A new, empty folder for one export.
    pub(super) fn export_folder(&self) -> io::Result<PathBuf> {
        let folder = self
            .path
            .join(self.next_export.fetch_add(1, Ordering::SeqCst).to_string());
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder)?;
        Ok(folder)
    }
}

impl Drop for RunFolder {
    fn drop(&mut self) {
        if self.lock.take().is_some() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Deletes every staging folder under `staging_root` that no running app
/// holds. Returns the folders it deleted. Anything else is left alone.
pub(super) fn sweep(staging_root: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(staging_root) else {
        return removed;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let left_over = if name.starts_with("run-") {
            !in_use(&path)
        } else if is_unlocked_folder_name(&name) {
            older_than(&path, UNLOCKED_FOLDER_MAX_AGE)
        } else {
            false
        };
        if left_over && fs::remove_dir_all(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// `<process id>-<n>`, the name staging folders had before they were locked.
fn is_unlocked_folder_name(name: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    name.split_once('-')
        .is_some_and(|(pid, n)| digits(pid) && digits(n))
}

pub(super) fn older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
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
    !older_than(folder, Duration::from_secs(24 * 60 * 60))
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

/// Sets the modification time of a file or folder `by` into the past.
#[cfg(test)]
pub(super) fn make_older(path: &Path, by: Duration) {
    let mut options = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_BACKUP_SEMANTICS, which Windows needs to open a folder.
        options.write(true).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    options.read(true);
    let file = options.open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() - by)
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlocked_folder_names_are_recognised() {
        assert!(is_unlocked_folder_name("1234-0"));
        assert!(!is_unlocked_folder_name("run-1234-0"));
        assert!(!is_unlocked_folder_name("notes"));
        assert!(!is_unlocked_folder_name("12-"));
    }

    #[cfg(windows)]
    #[test]
    fn left_over_staging_folders_are_swept_and_live_ones_kept() {
        let root = tempfile::tempdir().unwrap();
        // An app that crashed: its lock file is there but nobody holds it.
        let crashed = root.path().join("run-1-0");
        fs::create_dir_all(crashed.join("7").join("out")).unwrap();
        fs::write(crashed.join(LOCK), "").unwrap();
        // Killed before it took its lock.
        let unlocked = root.path().join("run-2-0");
        fs::create_dir_all(&unlocked).unwrap();
        // An old-style folder from a crash long ago, and a recent one that
        // an older app may still be writing.
        let old_style = root.path().join("99-3");
        fs::create_dir_all(&old_style).unwrap();
        make_older(&old_style, Duration::from_secs(3600));
        let recent_old_style = root.path().join("99-4");
        fs::create_dir_all(&recent_old_style).unwrap();
        // Something else is never touched.
        fs::write(root.path().join("notes.txt"), "keep").unwrap();
        // An app still running.
        let live = RunFolder::create(root.path()).unwrap();
        let export = live.export_folder().unwrap();

        let mut removed = sweep(root.path());
        removed.sort();
        assert_eq!(
            removed,
            [old_style.clone(), crashed.clone(), unlocked.clone()]
        );
        assert!(live.path.exists() && export.exists());
        assert!(recent_old_style.exists());
        assert!(root.path().join("notes.txt").exists());

        // Dropping the folder deletes it.
        let path = live.path.clone();
        drop(live);
        assert!(!path.exists());
    }
}

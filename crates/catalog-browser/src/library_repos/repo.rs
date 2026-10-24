//! One library repo and the app's own copy of it: the seam every feature
//! that reads or writes a client's library goes through.
//!
//! The copy is a full clone in the app's local data folder, so any commit
//! can be read offline. It has no files checked out (`--no-checkout`):
//! the app reads files from commits, so a working tree would only double
//! the disk space a library with many 3D models takes. The person's own
//! clone is never used or written (mx-70700f).
//!
//! Several teams may link one repo, each with its own address for it
//! (ssh for one, https for another). Each step uses the address of the
//! [`LibraryRepo`] it runs for, so each team sees its own result.
//!
//! Each copy has one [`CopyLock`]. Every step that changes the copy holds
//! it, and reads never meet a copy being moved into place or deleted.
//!
//! Today: check access, make or update the copy, and read files and the
//! file list at a commit (kicad-parts-ab30). Scanning the repo
//! (kicad-parts-e9a3) reads through [`LibraryRepo::list_files`] and
//! [`LibraryRepo::read_files`]. Writing parts (kicad-parts-02fb) will add
//! "commit on a new branch and push" here, holding the same lock, with the
//! same steps and plain problems.

use super::address::RepoAddress;
use super::problems::{Step, explain, git_not_installed};
use crate::git_program::{Git, NotInstalled, Ran};
use desktop_contract::RepoProblem;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long each step may take before the app stops it.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Reading the repo or trying a push: a few seconds when the host
    /// answers. Long enough for a credential helper's sign-in window.
    pub check: Duration,
    /// Copying or fetching: a library with many 3D models may be large.
    pub transfer: Duration,
    /// Reading the app's own copy, on this computer.
    pub local: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            check: Duration::from_secs(90),
            transfer: Duration::from_secs(30 * 60),
            local: Duration::from_secs(60),
        }
    }
}

/// What trying a push found, once reading worked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanWrite {
    Yes,
    /// The repo has no commits yet, so there is nothing to try a push of.
    EmptyRepo,
}

/// One file in a commit, as listed: its path, its size in bytes, and its
/// git object ID, which stays the same while its bytes do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoFile {
    pub path: String,
    pub size: u64,
    pub blob: String,
}

/// The lock of one copy.
#[derive(Debug, Default)]
pub(crate) struct CopyLock {
    /// Held by every step that changes the copy: making, fetching,
    /// checking a push, deleting (and later committing and pushing).
    change: Mutex<()>,
    /// Written only while the copy's folder is moved into place or
    /// deleted; every read holds it to read.
    there: RwLock<()>,
}

/// Every copy's lock, by the copy's folder name, shared by every
/// [`LibraryRepo`] of one [`super::LibraryRepos`].
#[derive(Clone, Debug, Default)]
pub(crate) struct CopyLocks(Arc<Mutex<HashMap<String, Arc<CopyLock>>>>);

impl CopyLocks {
    pub(crate) fn of(&self, folder_name: &str) -> Arc<CopyLock> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(folder_name.to_owned())
            .or_default()
            .clone()
    }

    /// Deletes the copy or leftover at `path`, named `name`, unless a step
    /// is changing it now. Then it returns `false`, and the step's owner
    /// deletes it later.
    pub(crate) fn delete_if_idle(&self, path: &Path, name: &str) -> io::Result<bool> {
        let copy = name.strip_suffix(".partial").unwrap_or(name);
        let lock = self.of(copy);
        let Ok(_changing) = lock.change.try_lock() else {
            return Ok(false);
        };
        let _gone = lock
            .there
            .write()
            .unwrap_or_else(|error| error.into_inner());
        remove_folder(path).map(|()| true)
    }
}

/// A library repo at one address, and the app's copy of it.
#[derive(Clone, Debug)]
pub struct LibraryRepo {
    address: RepoAddress,
    /// The app's copy: `library-repos/<file-safe key>`.
    folder: PathBuf,
    git: Git,
    limits: Limits,
    lock: Arc<CopyLock>,
}

impl LibraryRepo {
    /// The repo at `address`, with the app's copy in `repos_folder`.
    pub(crate) fn new(
        git: Git,
        repos_folder: &Path,
        address: RepoAddress,
        limits: Limits,
        locks: &CopyLocks,
    ) -> Self {
        Self {
            folder: repos_folder.join(address.folder_name()),
            lock: locks.of(&address.folder_name()),
            address,
            git,
            limits,
        }
    }

    /// The same repo, with git asking the person nothing: no credential
    /// helper window (`GCM_INTERACTIVE=never`) and no askpass program for
    /// git or ssh. For checks the person did not ask for, such as at start.
    pub(crate) fn asking_nothing(&self) -> Self {
        let mut quiet = self.clone();
        quiet.git = quiet
            .git
            .with_env("GCM_INTERACTIVE", "never")
            .with_env("GIT_ASKPASS", "")
            .with_env("SSH_ASKPASS_REQUIRE", "never");
        quiet
    }

    pub fn address(&self) -> &RepoAddress {
        &self.address
    }

    /// The folder of the app's own copy.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Whether the app has a copy of the repo.
    pub fn is_copied(&self) -> bool {
        self.folder.join(".git").is_dir()
    }

    /// When the app's copy last matched the repo: its last fetch, or else
    /// when it was made.
    pub fn copied_at(&self) -> Option<SystemTime> {
        let git_dir = self.folder.join(".git");
        ["FETCH_HEAD", "HEAD"]
            .iter()
            .find_map(|name| fs::metadata(git_dir.join(name)).ok()?.modified().ok())
    }

    /// Checks the repo can be read, without copying anything: `git
    /// ls-remote`, asking only for the repo's `HEAD`.
    pub fn can_read(&self) -> Result<(), RepoProblem> {
        self.network(
            Step::Read,
            None,
            &["ls-remote", "--", self.address.url(), "HEAD"],
            self.limits.check,
        )
        .map(drop)
    }

    /// Makes the app's copy of the repo, or brings it up to date, from
    /// this repo's own address.
    ///
    /// A new copy is made in a folder of its own and moved into place only
    /// once git has finished, so a copy stopped halfway never looks done.
    /// Afterwards the copy records the repo's main branch again: git
    /// before 2.48 does not after copying an empty repo.
    pub fn copy_or_fetch(&self) -> Result<(), RepoProblem> {
        let _changing = self.changing();
        if self.is_copied() {
            self.use_own_address(Step::Fetch)?;
            self.network(
                Step::Fetch,
                Some(&self.folder),
                &["fetch", "--prune", "origin"],
                self.limits.transfer,
            )?;
        } else {
            self.make_copy()?;
        }
        self.record_main_branch();
        Ok(())
    }

    /// Checks the repo takes pushes, without changing it: `git push
    /// --dry-run` of the latest commit to a new branch name that cannot
    /// exist yet. The host checks write access when the push starts, so a
    /// refusal shows here, and a dry run never creates the branch. Hooks
    /// are skipped: this is a check, not a push. Needs the app's copy.
    pub fn can_write(&self) -> Result<CanWrite, RepoProblem> {
        let _changing = self.changing();
        let Some(commit) = self.latest_commit()? else {
            return Ok(CanWrite::EmptyRepo);
        };
        self.use_own_address(Step::Write)?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let target = format!("{commit}:refs/heads/kicad-parts-access-check-{nanos:x}");
        self.network(
            Step::Write,
            Some(&self.folder),
            &[
                "push",
                "--dry-run",
                "--porcelain",
                "--no-verify",
                "origin",
                &target,
            ],
            self.limits.check,
        )
        .map(|_| CanWrite::Yes)
    }

    /// The latest commit of the repo's main branch in the app's copy, as
    /// last fetched, or `None` while the repo has no commits. Works
    /// offline.
    pub fn latest_commit(&self) -> Result<Option<String>, RepoProblem> {
        let head = self.local(&[
            "rev-parse",
            "--verify",
            "-q",
            "refs/remotes/origin/HEAD^{commit}",
        ])?;
        let commit = head.stdout_text().trim().to_owned();
        Ok((head.succeeded && !commit.is_empty()).then_some(commit))
    }

    /// The bytes of the file at `path` (such as `symbols/Acme.kicad_sym`)
    /// in `commit`, from the app's copy. Works offline.
    pub fn read_file(&self, commit: &str, path: &str) -> Result<Vec<u8>, RepoProblem> {
        check_commit(commit)?;
        let ran = self.local(&["cat-file", "blob", &format!("{commit}:{path}")])?;
        if ran.succeeded {
            Ok(ran.stdout)
        } else {
            Err(self.read_problem(&format!("{path} at commit {commit}"), &ran.stderr))
        }
    }

    /// The bytes of each of `paths` in `commit`, in order, `None` for a
    /// path the commit does not hold as a file. One git run reads them
    /// all, however many. Works offline.
    pub fn read_files(
        &self,
        commit: &str,
        paths: &[&str],
    ) -> Result<Vec<Option<Vec<u8>>>, RepoProblem> {
        check_commit(commit)?;
        let mut input = Vec::new();
        for path in paths {
            if path.contains(['\n', '\r']) {
                return Err(self.read_problem(
                    &format!("the file {path:?}"),
                    "Its path holds a line break.",
                ));
            }
            input.extend_from_slice(format!("{commit}:{path}\n").as_bytes());
        }
        let what = format!("the files at commit {commit}");
        let ran = self.local_with_input(&["cat-file", "--batch"], input)?;
        if !ran.succeeded {
            return Err(self.read_problem(&what, &ran.stderr));
        }
        parse_batch(&ran.stdout, paths.len())
            .ok_or_else(|| self.read_problem(&what, "git's answer was cut short."))
    }

    /// Every file in `commit`, with its size and object ID, from the app's
    /// copy. Works offline.
    pub fn list_files(&self, commit: &str) -> Result<Vec<RepoFile>, RepoProblem> {
        check_commit(commit)?;
        let ran = self.local(&["ls-tree", "-r", "-l", "-z", commit])?;
        if !ran.succeeded {
            return Err(self.read_problem(&format!("the files at commit {commit}"), &ran.stderr));
        }
        Ok(parse_tree(&ran.stdout_text()))
    }

    /// Holds this copy's lock for a step that changes it.
    fn changing(&self) -> MutexGuard<'_, ()> {
        self.lock
            .change
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Points the copy's `origin` at this repo's own address, so a fetch
    /// or a push goes the way this team reaches the repo.
    fn use_own_address(&self, step: Step) -> Result<(), RepoProblem> {
        let ran = self.local(&["remote", "set-url", "origin", self.address.url()])?;
        if ran.succeeded {
            Ok(())
        } else {
            Err(explain(step, &ran, &self.address, self.limits.local, false))
        }
    }

    /// Makes a new copy in `<folder>.partial`, then moves it into place.
    fn make_copy(&self) -> Result<(), RepoProblem> {
        let partial = partial_folder(&self.folder);
        remove_folder(&partial).map_err(|error| self.local_problem(&partial, error))?;
        if let Some(parent) = partial.parent() {
            fs::create_dir_all(parent).map_err(|error| self.local_problem(parent, error))?;
        }
        let partial_text = partial.display().to_string();
        let copied = self.network(
            Step::Copy,
            None,
            &[
                "clone",
                "--no-checkout",
                "--origin",
                "origin",
                "--",
                self.address.url(),
                &partial_text,
            ],
            self.limits.transfer,
        );
        if let Err(problem) = copied {
            let _ = remove_folder(&partial);
            return Err(problem);
        }
        let _moving = self
            .lock
            .there
            .write()
            .unwrap_or_else(|error| error.into_inner());
        remove_folder(&self.folder)
            .and_then(|()| fs::rename(&partial, &self.folder))
            .map_err(|error| self.local_problem(&self.folder, error))
    }

    /// Asks the host which branch is the main one and records it as
    /// `origin/HEAD`. An empty repo has none yet; that is no problem.
    fn record_main_branch(&self) {
        let _ = self.git.run_network(
            Some(&self.folder),
            ["remote", "set-head", "origin", "--auto"],
            self.limits.check,
        );
    }

    /// Runs a step that reaches the repo's host.
    fn network(
        &self,
        step: Step,
        dir: Option<&Path>,
        args: &[&str],
        limit: Duration,
    ) -> Result<Ran, RepoProblem> {
        let ran = self
            .git
            .run_network(dir, args, limit)
            .map_err(|NotInstalled| git_not_installed(step))?;
        if ran.succeeded {
            Ok(ran)
        } else {
            // An https sign-in or the person's own ssh may have asked.
            let may_prompt = ran.timed_out
                && (self.address.is_web() || self.git.has_own_ssh_command(dir).unwrap_or(true));
            Err(explain(step, &ran, &self.address, limit, may_prompt))
        }
    }

    /// Runs git on the app's copy, offline, while the copy stays in place.
    fn local(&self, args: &[&str]) -> Result<Ran, RepoProblem> {
        let _there = self
            .lock
            .there
            .read()
            .unwrap_or_else(|error| error.into_inner());
        self.no_copy_yet()?;
        self.git
            .run(Some(&self.folder), args, self.limits.local)
            .map_err(|NotInstalled| git_not_installed(Step::Read))
    }

    /// Like [`LibraryRepo::local`], with `input` as git's standard input.
    fn local_with_input(&self, args: &[&str], input: Vec<u8>) -> Result<Ran, RepoProblem> {
        let _there = self
            .lock
            .there
            .read()
            .unwrap_or_else(|error| error.into_inner());
        self.no_copy_yet()?;
        self.git
            .run_with_input(Some(&self.folder), args, input, self.limits.local)
            .map_err(|NotInstalled| git_not_installed(Step::Read))
    }

    fn no_copy_yet(&self) -> Result<(), RepoProblem> {
        if self.is_copied() {
            return Ok(());
        }
        Err(RepoProblem {
            summary: "Can't read the repo: the app has no copy of it yet. Check again while you're online.".to_owned(),
            details: format!("The copy belongs in {}.", self.folder.display()),
            offline: false,
        })
    }

    fn read_problem(&self, what: &str, said: &str) -> RepoProblem {
        RepoProblem {
            summary: format!(
                "Can't read {what} in the app's copy of {}.",
                self.address.name()
            ),
            details: said.trim().to_owned(),
            offline: false,
        }
    }

    fn local_problem(&self, folder: &Path, error: io::Error) -> RepoProblem {
        RepoProblem {
            summary: format!(
                "Can't copy the repo to this computer: the app could not use the folder {}. Check that there is free disk space.",
                folder.display()
            ),
            details: error.to_string(),
            offline: false,
        }
    }
}

/// Refuses anything but a whole commit ID (40 or 64 hexadecimal digits),
/// so a commit can never be read as one of git's options.
fn check_commit(commit: &str) -> Result<(), RepoProblem> {
    let whole =
        matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit());
    if whole {
        Ok(())
    } else {
        Err(RepoProblem {
            summary: format!("{commit} is not a whole commit ID."),
            details: String::new(),
            offline: false,
        })
    }
}

/// `git cat-file --batch`'s answer for `count` objects. Each is either
/// `<id> <type> <size>`, a line break, the bytes and a line break, or
/// `<name> missing` and a line break. `None` if the answer is cut short.
fn parse_batch(mut out: &[u8], count: usize) -> Option<Vec<Option<Vec<u8>>>> {
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let end = out.iter().position(|&byte| byte == b'\n')?;
        let header = std::str::from_utf8(&out[..end]).ok()?;
        out = &out[end + 1..];
        let mut words = header.rsplitn(3, ' ');
        let last = words.next()?;
        if last == "missing" || last == "ambiguous" {
            files.push(None);
            continue;
        }
        let size: usize = last.parse().ok()?;
        let kind = words.next()?;
        let bytes = out.get(..size)?.to_vec();
        out = out.get(size + 1..)?;
        // A path naming a folder gives a tree: no file's bytes.
        files.push((kind == "blob").then_some(bytes));
    }
    Some(files)
}

/// `git ls-tree -r -l -z`'s answer: `<mode> <type> <id> <size>`, a tab and
/// the path for each entry, each ending in NUL. Only files: a submodule
/// has no bytes here.
fn parse_tree(out: &str) -> Vec<RepoFile> {
    out.split('\0')
        .filter_map(|entry| {
            let (about, path) = entry.split_once('\t')?;
            let mut words = about.split_whitespace();
            let (_mode, kind, blob, size) =
                (words.next()?, words.next()?, words.next()?, words.next()?);
            (kind == "blob").then(|| RepoFile {
                path: path.to_owned(),
                size: size.parse().unwrap_or(0),
                blob: blob.to_owned(),
            })
        })
        .collect()
}

/// Where a new copy is made before it is moved into place.
fn partial_folder(folder: &Path) -> PathBuf {
    let mut name = folder.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    folder.with_file_name(name)
}

/// Deletes `folder` and everything in it, read-only files too; nothing to
/// do if it is not there.
pub(crate) fn remove_folder(folder: &Path) -> io::Result<()> {
    match fs::remove_dir_all(folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            make_writable(folder);
            fs::remove_dir_all(folder)
        }
    }
}

/// Clears the read-only mark of every file under `folder`. Links are not
/// followed: only the app's own files are changed.
fn make_writable(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            make_writable(&entry.path());
        } else if kind.is_file() {
            if let Ok(metadata) = entry.metadata() {
                let mut permissions = metadata.permissions();
                if permissions.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    permissions.set_readonly(false);
                    let _ = fs::set_permissions(entry.path(), permissions);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_commit_ids_are_read() {
        assert!(check_commit(&"a".repeat(40)).is_ok());
        assert!(check_commit(&"0123456789abcdef".repeat(4)).is_ok());
        for refused in [
            "HEAD",
            "--output=x",
            "main",
            &"a".repeat(39),
            &"g".repeat(40),
        ] {
            assert!(check_commit(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn batch_answers_keep_missing_files_and_binary_bytes() {
        let mut out = b"aaa blob 3\n".to_vec();
        out.extend_from_slice(&[0, 1, b'\n', b'\n']);
        out.extend_from_slice(b"X:nope missing\nbbb blob 0\n\n");
        assert_eq!(
            parse_batch(&out, 3),
            Some(vec![Some(vec![0, 1, b'\n']), None, Some(Vec::new())])
        );
        // Cut short.
        assert_eq!(parse_batch(b"aaa blob 9\nabc", 1), None);
    }

    #[test]
    fn listings_keep_files_with_sizes_and_skip_submodules() {
        let out = "100644 blob e69de29b      12\tsymbols/A b.kicad_sym\x00160000 commit 1234abcd       -\tvendor\x00";
        assert_eq!(
            parse_tree(out),
            [RepoFile {
                path: "symbols/A b.kicad_sym".into(),
                size: 12,
                blob: "e69de29b".into()
            }]
        );
    }
}

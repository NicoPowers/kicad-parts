//! One library repo and the app's own copy of it: the seam every feature
//! that reads or writes a client's library goes through.
//!
//! The copy is a full clone in the app's local data folder, so any commit
//! can be read offline. It has no files checked out (`--no-checkout`):
//! the app reads files from commits, so a working tree would only double
//! the disk space a library with many 3D models takes. The person's own
//! clone is never used or written (mx-70700f).
//!
//! Today: check access, make or update the copy, and read files and the
//! file list at a commit (kicad-parts-ab30). Scanning the repo
//! (kicad-parts-e9a3) reads through [`LibraryRepo::read_file`] and
//! [`LibraryRepo::list_files`]. Writing parts (kicad-parts-02fb) will add
//! "commit on a new branch and push" here, with the same steps and plain
//! problems.

use super::address::RepoAddress;
use super::problems::{Step, explain, git_not_installed};
use crate::git_program::{Git, NotInstalled, Ran};
use desktop_contract::RepoProblem;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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

/// A library repo at one address, and the app's copy of it.
#[derive(Clone, Debug)]
pub struct LibraryRepo {
    address: RepoAddress,
    /// The app's copy: `library-repos/<file-safe key>`.
    folder: PathBuf,
    git: Git,
    limits: Limits,
}

impl LibraryRepo {
    /// The repo at `address`, with the app's copy in `repos_folder`.
    pub(crate) fn new(git: Git, repos_folder: &Path, address: RepoAddress, limits: Limits) -> Self {
        Self {
            folder: repos_folder.join(address.folder_name()),
            address,
            git,
            limits,
        }
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

    /// Makes the app's copy of the repo, or brings it up to date.
    ///
    /// A new copy is made in a folder of its own and moved into place only
    /// once git has finished, so a copy stopped halfway never looks done.
    pub fn copy_or_fetch(&self) -> Result<(), RepoProblem> {
        if self.is_copied() {
            return self
                .network(
                    Step::Fetch,
                    Some(&self.folder),
                    &["fetch", "--prune", "origin"],
                    self.limits.transfer,
                )
                .map(drop);
        }
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
        remove_folder(&self.folder)
            .and_then(|()| fs::rename(&partial, &self.folder))
            .map_err(|error| self.local_problem(&self.folder, error))
    }

    /// Checks the repo takes pushes, without changing it: `git push
    /// --dry-run` of the latest commit to a new branch name that cannot
    /// exist yet. The host checks write access when the push starts, so a
    /// refusal shows here, and a dry run never creates the branch. Hooks
    /// are skipped: this is a check, not a push. Needs the app's copy.
    pub fn can_write(&self) -> Result<CanWrite, RepoProblem> {
        let Some(commit) = self.latest_commit()? else {
            return Ok(CanWrite::EmptyRepo);
        };
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
        if head.succeeded {
            return Ok(Some(head.stdout_text().trim().to_owned()));
        }
        // An older git does not always record which branch is the main one.
        let any = self.local(&[
            "for-each-ref",
            "--count=1",
            "--format=%(objectname)",
            "refs/remotes/origin",
        ])?;
        let first = any.stdout_text().trim().to_owned();
        Ok((!first.is_empty()).then_some(first))
    }

    /// The bytes of the file at `path` (such as `symbols/Acme.kicad_sym`)
    /// in `commit`, from the app's copy. Works offline.
    pub fn read_file(&self, commit: &str, path: &str) -> Result<Vec<u8>, RepoProblem> {
        let ran = self.local(&["cat-file", "blob", &format!("{commit}:{path}")])?;
        if ran.succeeded {
            Ok(ran.stdout)
        } else {
            Err(self.read_problem(&format!("{path} at commit {commit}"), &ran))
        }
    }

    /// Every file's path in `commit`, from the app's copy. Works offline.
    pub fn list_files(&self, commit: &str) -> Result<Vec<String>, RepoProblem> {
        let ran = self.local(&["ls-tree", "-r", "-z", "--name-only", commit])?;
        if !ran.succeeded {
            return Err(self.read_problem(&format!("the files at commit {commit}"), &ran));
        }
        Ok(ran
            .stdout_text()
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect())
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
            Err(explain(step, &ran, &self.address.host(), limit))
        }
    }

    /// Runs git on the app's copy, offline.
    fn local(&self, args: &[&str]) -> Result<Ran, RepoProblem> {
        if !self.is_copied() {
            return Err(RepoProblem {
                summary: "Can't read the repo: the app has no copy of it yet. Check again while you're online.".to_owned(),
                details: format!("The copy belongs in {}.", self.folder.display()),
                offline: false,
            });
        }
        self.git
            .run(Some(&self.folder), args, self.limits.local)
            .map_err(|NotInstalled| git_not_installed(Step::Read))
    }

    fn read_problem(&self, what: &str, ran: &Ran) -> RepoProblem {
        RepoProblem {
            summary: format!(
                "Can't read {what} in the app's copy of {}.",
                self.address.name()
            ),
            details: ran.stderr.trim().to_owned(),
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

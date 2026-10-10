//! A disposable PocketBase carrying the repository's schema and hooks.

use catalog::CsvCatalog;
use catalog::csv_import::{CsvImport, ImportError};
use catalog::pocketbase::{ImportCommand, read_import_state};
use collaboration::PocketBaseClient;
use native_services::{
    PocketBaseOptions, PocketBaseServer, Superuser, pocketbase_binary, random_name,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Options for an instance whose state lives under `state` and whose
/// migrations and hooks are the repository's own. PocketBase only reads
/// them (`--automigrate=false`).
pub fn options(repository: &Path, state: &Path) -> PocketBaseOptions {
    PocketBaseOptions {
        data_dir: state.join("pb_data"),
        migrations_dir: repository.join("infra/pocketbase/pb_migrations"),
        hooks_dir: repository.join("infra/pocketbase/pb_hooks"),
        log_file: state.join(format!("pocketbase-{}.log", random_name(6))),
    }
}

/// Starts the pinned PocketBase on a random 127.0.0.1 port (with a
/// superuser upserted first, so it never opens its browser installer) and
/// signs in as that superuser.
pub fn start(
    repository: &Path,
    state: &Path,
) -> Result<(PocketBaseServer, PocketBaseClient, Superuser), String> {
    let binary = pocketbase_binary(repository).map_err(|error| error.to_string())?;
    let superuser = Superuser::generate();
    let server = PocketBaseServer::start(&binary, &options(repository, state), &superuser)
        .map_err(|error| error.to_string())?;
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .map_err(|error| error.to_string())?;
    server
        .check_no_installer()
        .map_err(|error| error.to_string())?;
    Ok((server, admin, superuser))
}

/// A team member account added to a disposable instance.
#[derive(Clone)]
pub struct Member {
    pub name: String,
    pub email: String,
    pub password: String,
    pub can_edit_catalog: bool,
}

impl std::fmt::Debug for Member {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Member")
            .field("email", &self.email)
            .field("can_edit_catalog", &self.can_edit_catalog)
            .finish_non_exhaustive()
    }
}

/// Adds a verified team member with a random password, marked as able to
/// edit the Catalog or not.
pub fn add_member(
    admin: &PocketBaseClient,
    name: &str,
    email: &str,
    can_edit_catalog: bool,
) -> Result<Member, String> {
    let password = native_services::random_token(20);
    admin
        .post_json(
            "/api/collections/users/records",
            &serde_json::json!({
                "email": email,
                "name": name,
                "password": password,
                "passwordConfirm": password,
                "verified": true,
                "can_edit_catalog": can_edit_catalog,
            }),
        )
        .and_then(|reply| reply.success())
        .map_err(|error| format!("cannot add {email}: {error}"))?;
    Ok(Member {
        name: name.to_owned(),
        email: email.to_owned(),
        password,
        can_edit_catalog,
    })
}

/// Marks a source commit whose inputs differ from it.
pub const DIRTY_SUFFIX: &str = "+dirty";

/// The git program the tooling runs.
pub const GIT: &str = "git";

/// What git said, or that it could not be started.
#[derive(Debug)]
pub(crate) enum GitAnswer {
    /// git ran; its exit code and trimmed standard output.
    Ran { code: Option<i32>, stdout: String },
    /// git could not be started: it is not installed or not on the PATH.
    Missing,
}

/// Runs `program` (git) in `repository`, windowless and inside a
/// kill-on-close job like every service.
pub(crate) fn run_git(program: &str, repository: &Path, args: &[&str]) -> GitAnswer {
    let mut command = Command::new(program);
    command.arg("-C").arg(repository).args(args);
    match native_services::run_hidden("git", &mut command) {
        Err(_) => GitAnswer::Missing,
        Ok(output) => GitAnswer::Ran {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        },
    }
}

/// git's trimmed standard output when it succeeds.
#[cfg(test)]
fn git(repository: &Path, args: &[&str]) -> Option<String> {
    match run_git(GIT, repository, args) {
        GitAnswer::Ran {
            code: Some(0),
            stdout,
        } => Some(stdout),
        _ => None,
    }
}

/// Where the files an import reads came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceCommit {
    /// A commit, ending in [`DIRTY_SUFFIX`] if any input differs from it.
    Commit(String),
    /// The folder is not inside a git repository (or has no commit yet).
    NotARepository,
    /// git is not installed or not on the PATH, so nobody can tell.
    GitMissing,
}

/// Where `inputs` (repository paths) under `repository` came from: the
/// `HEAD` commit, with [`DIRTY_SUFFIX`] appended if any of them is modified,
/// staged or untracked relative to it, so a dirty tree is never recorded as
/// a clean commit.
pub fn read_source_commit(repository: &Path, inputs: &[&str]) -> SourceCommit {
    read_source_commit_with(GIT, repository, inputs)
}

pub(crate) fn read_source_commit_with(
    program: &str,
    repository: &Path,
    inputs: &[&str],
) -> SourceCommit {
    let commit = match run_git(program, repository, &["rev-parse", "HEAD"]) {
        GitAnswer::Missing => return SourceCommit::GitMissing,
        GitAnswer::Ran {
            code: Some(0),
            stdout,
        } if stdout.len() == 40 => stdout,
        GitAnswer::Ran { .. } => return SourceCommit::NotARepository,
    };
    let mut status = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    status.extend_from_slice(inputs);
    match run_git(program, repository, &status) {
        GitAnswer::Missing => SourceCommit::GitMissing,
        GitAnswer::Ran {
            code: Some(0),
            stdout,
        } => SourceCommit::Commit(if stdout.is_empty() {
            commit
        } else {
            format!("{commit}{DIRTY_SUFFIX}")
        }),
        GitAnswer::Ran { .. } => SourceCommit::NotARepository,
    }
}

/// The source commit of `inputs`, as [`read_source_commit`] finds it, or
/// `None` if there is none (no repository, or git is missing).
pub fn source_commit(repository: &Path, inputs: &[&str]) -> Option<String> {
    match read_source_commit(repository, inputs) {
        SourceCommit::Commit(commit) => Some(commit),
        SourceCommit::NotARepository | SourceCommit::GitMissing => None,
    }
}

/// Plans the import of `csv_file`, recording the source commit of exactly
/// the files the import reads.
pub fn plan(repository: &Path, csv_file: &str) -> Result<CsvImport, ImportError> {
    let catalog = CsvCatalog::new(repository);
    let probe = catalog.import(csv_file, None)?;
    let inputs: Vec<&str> = probe
        .sources
        .iter()
        .map(|source| source.path.as_str())
        .collect();
    catalog.import(csv_file, source_commit(repository, &inputs))
}

/// Reads the team catalog and returns the command for `import`, expecting
/// each part's current revision as read. Refuses, with a plain-language
/// reason, if the import's checkout (`repository`) does not include the team
/// catalog's latest imports, unless `allow_older_checkout` is set; the run's
/// audit entry then records that it was allowed.
pub fn prepare_import<'a>(
    client: &PocketBaseClient,
    repository: &Path,
    import: &'a CsvImport,
    reason: &str,
    allow_older_checkout: bool,
) -> Result<ImportCommand<'a>, String> {
    let state = read_import_state(client).map_err(|error| error.to_string())?;
    if !allow_older_checkout {
        crate::checkout::check_checkout(repository, import, &state)?;
    }
    Ok(ImportCommand::new(import, reason)
        .expecting(&state)
        .allowing_older_checkout(allow_older_checkout))
}

/// This repository's root.
pub fn repository_root() -> PathBuf {
    native_services::repository_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dirty_input_is_recorded_as_dirty() {
        let repository = tempfile::tempdir().unwrap();
        let hooks = tempfile::tempdir().unwrap();
        let root = repository.path();
        assert_eq!(source_commit(root, &["a.csv"]), None);
        // A throwaway repository: no signing or hooks from the user's
        // configuration apply to its fixture commit.
        let hooks_path = format!("core.hooksPath={}", hooks.path().display());
        let fixture = |args: &[&str]| {
            let mut all = vec![
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                hooks_path.as_str(),
            ];
            all.extend_from_slice(args);
            git(root, &all).unwrap()
        };
        fixture(&["init", "-q"]);
        std::fs::write(root.join("a.csv"), "x\n").unwrap();
        std::fs::write(root.join("b.csv"), "y\n").unwrap();
        fixture(&["add", "a.csv", "b.csv"]);
        fixture(&["commit", "-q", "-m", "fixture"]);
        let clean = source_commit(root, &["a.csv", "b.csv"]).unwrap();
        assert_eq!(clean.len(), 40);
        std::fs::write(root.join("b.csv"), "changed\n").unwrap();
        assert_eq!(source_commit(root, &["a.csv"]).unwrap(), clean);
        assert_eq!(
            source_commit(root, &["a.csv", "b.csv"]).unwrap(),
            format!("{clean}{DIRTY_SUFFIX}")
        );
        std::fs::write(root.join("new.csv"), "z\n").unwrap();
        assert!(
            source_commit(root, &["new.csv"])
                .unwrap()
                .ends_with(DIRTY_SUFFIX)
        );
    }

    #[test]
    fn a_missing_git_is_told_apart_from_a_folder_outside_git() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(
            read_source_commit(folder.path(), &["a.csv"]),
            SourceCommit::NotARepository
        );
        assert_eq!(
            read_source_commit_with("kicad-parts-no-such-git", &repository_root(), &[]),
            SourceCommit::GitMissing
        );
        assert!(matches!(
            read_source_commit(&repository_root(), &[]),
            SourceCommit::Commit(_)
        ));
    }
}

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
    /// git ran; its exit code and its standard output without the
    /// whitespace at its end.
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
            // Only the end is trimmed: `git status --porcelain` starts
            // lines with a space that matters.
            stdout: String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_owned(),
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

/// What an import is allowed to do although it is normally refused. Each
/// one that is used is recorded on the run's audit entry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Allow {
    /// Import from a checkout that does not include the team catalog's
    /// latest imports. This makes the checkout's older symbols, footprints
    /// and 3D models the current revisions of their CAD items, for every
    /// part that uses them (see `ImportCommand::allowing_older_checkout`).
    pub older_checkout: bool,
    /// Import files that have changes that are not committed. Until they
    /// are committed, a later import from a checkout without them undoes
    /// them.
    pub uncommitted_changes: bool,
}

/// Reads the team catalog and returns the command for `import`, expecting
/// each part's current revision as read. Refuses, with a plain-language
/// reason:
///
/// - before sending anything, if the files `import` reads have changes
///   that are not committed, unless `allow.uncommitted_changes` is set
///   (see [`check_committed`]);
/// - if the import's checkout (`repository`) does not include the team
///   catalog's latest imports, unless `allow.older_checkout` is set.
///
/// The run's audit entry records each one that was allowed and used.
pub fn prepare_import<'a>(
    client: &PocketBaseClient,
    repository: &Path,
    import: &'a CsvImport,
    reason: &str,
    allow: Allow,
) -> Result<ImportCommand<'a>, String> {
    check_committed(repository, import, allow)?;
    let state = read_import_state(client).map_err(|error| error.to_string())?;
    if !allow.older_checkout {
        crate::checkout::check_checkout(repository, import, &state)?;
    }
    Ok(ImportCommand::new(import, reason)
        .expecting(&state)
        .allowing_older_checkout(allow.older_checkout)
        .allowing_uncommitted_changes(allow.uncommitted_changes && has_uncommitted_changes(import)))
}

/// Refuses `import` if the files it reads have changes that are not
/// committed, unless `allow.uncommitted_changes` is set. Only the files the
/// import reads count (its CSV file, substitutes.csv, and the symbols,
/// footprints and 3D models its parts use), so other work in the checkout
/// never blocks an import. The reason names the changed files (at most 10).
pub fn check_committed(repository: &Path, import: &CsvImport, allow: Allow) -> Result<(), String> {
    if allow.uncommitted_changes || !has_uncommitted_changes(import) {
        return Ok(());
    }
    let inputs: Vec<&str> = import
        .sources
        .iter()
        .map(|source| source.path.as_str())
        .collect();
    let mut changed = uncommitted_files(repository, &inputs);
    let named = if changed.is_empty() {
        // The files were changed back after the import was planned.
        "the files this import reads".to_owned()
    } else if changed.len() > 10 {
        let more = changed.len() - 10;
        changed.truncate(10);
        format!("{} and {more} more", changed.join(", "))
    } else {
        changed.join(", ")
    };
    Err(format!(
        "{named} {} changes that are not committed. Commit them first and import again, so the team catalog matches a commit everyone can check out. To import them anyway, allow uncommitted changes (--allow-uncommitted-changes); until they are committed, a later import from a checkout without them will undo them.",
        if changed.len() == 1 { "has" } else { "have" }
    ))
}

/// Whether `import` was read from files with changes that are not
/// committed: its source commit ends in [`DIRTY_SUFFIX`].
pub fn has_uncommitted_changes(import: &CsvImport) -> bool {
    import
        .source_commit
        .as_deref()
        .is_some_and(|commit| commit.ends_with(DIRTY_SUFFIX))
}

/// The paths among `inputs` that are modified, staged or untracked in
/// `repository`, as git names them, sorted. Empty if git cannot tell.
pub fn uncommitted_files(repository: &Path, inputs: &[&str]) -> Vec<String> {
    // Paths are taken literally, never as patterns.
    let mut status = vec![
        "--literal-pathspecs",
        "status",
        "--porcelain",
        "-z",
        "--untracked-files=all",
        "--",
    ];
    status.extend_from_slice(inputs);
    let GitAnswer::Ran {
        code: Some(0),
        stdout,
    } = run_git(GIT, repository, &status)
    else {
        return Vec::new();
    };
    // Each entry is two status letters, a space and the path, ending in a
    // NUL; a rename or copy is followed by its old path, also ending in a
    // NUL, which is skipped.
    let mut paths = Vec::new();
    let mut entries = stdout.split('\0');
    while let Some(entry) = entries.next() {
        let Some(path) = entry.get(3..) else {
            continue;
        };
        if entry.starts_with('R') || entry.starts_with('C') {
            entries.next();
        }
        paths.push(path.to_owned());
    }
    paths.sort();
    paths.dedup();
    paths
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

    /// Every file the resistor import reads.
    const RESISTOR_INPUTS: [&str; 4] = [
        "database/g-res.csv",
        "database/substitutes.csv",
        "symbols/g-pas.kicad_sym",
        "footprints/g-res.pretty/R_0603_1608Metric.kicad_mod",
    ];

    /// A throwaway git repository holding a committed copy of the resistor
    /// inputs, so tests can change files without touching this repository.
    /// Returns the repository and the folder its (empty) hooks live in.
    fn committed_resistors() -> (tempfile::TempDir, tempfile::TempDir) {
        let repository = tempfile::tempdir().unwrap();
        let hooks = tempfile::tempdir().unwrap();
        let root = repository.path();
        for path in RESISTOR_INPUTS {
            let target = root.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(repository_root().join(path), target).unwrap();
        }
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
                "core.autocrlf=false",
                "-c",
                hooks_path.as_str(),
            ];
            all.extend_from_slice(args);
            git(root, &all).unwrap()
        };
        fixture(&["init", "-q"]);
        let mut add = vec!["add", "--"];
        add.extend_from_slice(&RESISTOR_INPUTS);
        fixture(&add);
        fixture(&["commit", "-q", "-m", "fixture"]);
        (repository, hooks)
    }

    /// A client for an address where nothing listens, so any request it
    /// sends fails with a transport error.
    fn nobody_listening() -> PocketBaseClient {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        PocketBaseClient::new(&format!("http://{address}"))
    }

    #[test]
    fn uncommitted_changes_are_refused_before_anything_is_sent() {
        let (repository, _hooks) = committed_resistors();
        let root = repository.path();
        let symbols = root.join("symbols/g-pas.kicad_sym");
        let text = std::fs::read_to_string(&symbols).unwrap();
        std::fs::write(&symbols, format!("{text}\n")).unwrap();
        let substitutes = root.join("database/substitutes.csv");
        let header = std::fs::read_to_string(&substitutes).unwrap();
        std::fs::write(
            &substitutes,
            format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-10K-ND\n"),
        )
        .unwrap();
        let import = plan(root, "g-res").unwrap();
        assert!(has_uncommitted_changes(&import));
        // Nothing listens at the client's address: a request would fail
        // with a transport error, not with this reason.
        let refused =
            prepare_import(&nobody_listening(), root, &import, "r", Allow::default()).unwrap_err();
        assert!(
            refused.starts_with(
                "database/substitutes.csv, symbols/g-pas.kicad_sym have changes that are not committed. Commit them first"
            ) && refused.contains("--allow-uncommitted-changes"),
            "{refused}"
        );
        // Allowing an older checkout does not allow uncommitted changes.
        let older = Allow {
            older_checkout: true,
            ..Allow::default()
        };
        assert!(
            check_committed(root, &import, older)
                .unwrap_err()
                .contains("not committed")
        );
        let allowed = Allow {
            uncommitted_changes: true,
            ..Allow::default()
        };
        assert_eq!(check_committed(root, &import, allowed), Ok(()));
    }

    #[test]
    fn only_changes_to_the_files_an_import_reads_count() {
        let (repository, _hooks) = committed_resistors();
        let root = repository.path();
        let clean = plan(root, "g-res").unwrap();
        assert!(!has_uncommitted_changes(&clean));
        assert_eq!(check_committed(root, &clean, Allow::default()), Ok(()));
        // Other work in the checkout: a new file and a file the resistor
        // import does not read.
        std::fs::write(root.join("notes.txt"), "unrelated\n").unwrap();
        std::fs::write(root.join("database/g-cap.csv"), "IPN\n").unwrap();
        let unrelated = plan(root, "g-res").unwrap();
        assert_eq!(unrelated.source_commit, clean.source_commit);
        assert_eq!(check_committed(root, &unrelated, Allow::default()), Ok(()));
        // A file it reads, changed and staged, then a new file under a
        // name with a space, are named as git names them.
        let csv = root.join("database/g-res.csv");
        let text = std::fs::read_to_string(&csv).unwrap();
        std::fs::write(&csv, format!("{text}\n")).unwrap();
        git(root, &["add", "--", "database/g-res.csv"]).unwrap();
        std::fs::write(root.join("a new.txt"), "x\n").unwrap();
        assert_eq!(
            uncommitted_files(
                root,
                &[
                    "database/g-res.csv",
                    "a new.txt",
                    "database/substitutes.csv"
                ]
            ),
            ["a new.txt", "database/g-res.csv"]
        );
    }
}

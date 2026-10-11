//! A disposable PocketBase carrying the repository's schema and hooks.

use catalog::CsvCatalog;
use catalog::csv_import::{CsvImport, ImportError};
use catalog::pocketbase::{ImportCommand, read_import_state};
use collaboration::PocketBaseClient;
use native_services::{
    PocketBaseOptions, PocketBaseServer, Superuser, pocketbase_binary, random_name,
};
use std::collections::BTreeMap;
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
            // Only the end is trimmed, so the first entry of a list keeps
            // its first character even if that is a space.
            stdout: String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_owned(),
        },
    }
}

/// Where the files an import reads came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceCommit {
    /// A commit, ending in [`DIRTY_SUFFIX`] if any input differs from it.
    Commit(String),
    /// The folder is not the top folder of a git repository (or has no
    /// commit yet).
    NotARepository,
    /// git is not installed or not on the PATH, so nobody can tell.
    GitMissing,
}

/// Where an import's inputs under `repository` came from: the `HEAD`
/// commit, with [`DIRTY_SUFFIX`] appended if any input differs from it, so a
/// dirty tree is never recorded as a clean commit. `read` are the
/// repository paths the import read and `missing` those it looked for but
/// did not find (see [`changed_inputs`]).
pub fn read_source_commit(repository: &Path, read: &[&str], missing: &[&str]) -> SourceCommit {
    read_source_commit_with(GIT, repository, read, missing)
}

pub(crate) fn read_source_commit_with(
    program: &str,
    repository: &Path,
    read: &[&str],
    missing: &[&str],
) -> SourceCommit {
    match compare_with_head(program, repository, read, missing) {
        Ok((commit, changed)) if changed.is_empty() => SourceCommit::Commit(commit),
        Ok((commit, _)) => SourceCommit::Commit(format!("{commit}{DIRTY_SUFFIX}")),
        Err(no_commit) => no_commit,
    }
}

/// The source commit of an import's inputs, as [`read_source_commit`]
/// finds it, or `None` if there is none (not a repository's top folder, or
/// git is missing).
pub fn source_commit(repository: &Path, read: &[&str], missing: &[&str]) -> Option<String> {
    match read_source_commit(repository, read, missing) {
        SourceCommit::Commit(commit) => Some(commit),
        SourceCommit::NotARepository | SourceCommit::GitMissing => None,
    }
}

/// Plans the import of `csv_file`, recording the source commit of exactly
/// the files the import reads and looks for.
pub fn plan(repository: &Path, csv_file: &str) -> Result<CsvImport, ImportError> {
    let catalog = CsvCatalog::new(repository);
    let probe = catalog.import(csv_file, None)?;
    let (read, missing) = input_paths(&probe);
    catalog.import(csv_file, source_commit(repository, &read, &missing))
}

/// The paths `import` read, and the paths it looked for but did not find.
fn input_paths(import: &CsvImport) -> (Vec<&str>, Vec<&str>) {
    let read = import
        .sources
        .iter()
        .map(|source| source.path.as_str())
        .collect();
    let missing = import.missing_inputs.iter().map(String::as_str).collect();
    (read, missing)
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
    /// are committed and imported again, a later import from a checkout
    /// without them undoes them.
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

/// Refuses `import` if the files it uses have changes that are not
/// committed, unless `allow.uncommitted_changes` is set. Only the files the
/// import reads or looks for count (its CSV file, substitutes.csv, and the
/// symbols, footprints and 3D models its parts name), so other work in the
/// checkout never blocks an import. The reason names the changed files (at
/// most 10).
pub fn check_committed(repository: &Path, import: &CsvImport, allow: Allow) -> Result<(), String> {
    if allow.uncommitted_changes || !has_uncommitted_changes(import) {
        return Ok(());
    }
    let mut changed = uncommitted_files(repository, import);
    let named = if changed.is_empty() {
        // The files were changed back after the import was planned.
        "the files this import uses".to_owned()
    } else if changed.len() > 10 {
        let more = changed.len() - 10;
        changed.truncate(10);
        format!("{} and {more} more", changed.join(", "))
    } else {
        changed.join(", ")
    };
    Err(format!(
        "{named} {} changes that are not committed. Commit them first and import again, so the team catalog matches a commit everyone can check out. To import them anyway, allow uncommitted changes (--allow-uncommitted-changes). Until you commit these changes and import again, a later import from a checkout without them will undo them.",
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

/// The paths among `import`'s inputs that differ from the `HEAD` commit of
/// `repository` (see [`changed_inputs`]), sorted. Empty if git cannot tell.
pub fn uncommitted_files(repository: &Path, import: &CsvImport) -> Vec<String> {
    let (read, missing) = input_paths(import);
    compare_with_head(GIT, repository, &read, &missing)
        .map(|(_, changed)| changed)
        .unwrap_or_default()
}

/// The `HEAD` commit of `repository` and the inputs that differ from it:
///
/// - a file the import read whose bytes are not the file in `HEAD`, or
///   which `HEAD` does not have (a new file);
/// - a file the import looked for but did not find, which `HEAD` has (a
///   deleted file, or one renamed, even only in its letter case).
///
/// The bytes on disk are compared with the commit, not git's idea of the
/// working tree, so files git is told to ignore changes to
/// (`skip-worktree`, `assume-unchanged`) count too. The inputs are stored
/// byte for byte (`-text` in .gitattributes), so equal files hash equally.
/// Refuses a folder that is not the repository's top folder, such as a copy
/// inside an ignored folder, whose paths are not the commit's.
fn compare_with_head(
    program: &str,
    repository: &Path,
    read: &[&str],
    missing: &[&str],
) -> Result<(String, Vec<String>), SourceCommit> {
    let top = git_output(program, repository, &["rev-parse", "--show-toplevel"])?;
    if !same_folder(Path::new(&top), repository) {
        return Err(SourceCommit::NotARepository);
    }
    let commit = git_output(program, repository, &["rev-parse", "HEAD"])?;
    if commit.len() != 40 {
        return Err(SourceCommit::NotARepository);
    }
    let mut committed: BTreeMap<String, String> = BTreeMap::new();
    for paths in read
        .chunks(PATHS_PER_RUN)
        .chain(missing.chunks(PATHS_PER_RUN))
    {
        // Paths are taken literally, never as patterns.
        let mut args = vec!["--literal-pathspecs", "ls-tree", "-r", "-z", "HEAD", "--"];
        args.extend_from_slice(paths);
        // Each entry is `<mode> <type> <object ID>`, a tab and the path,
        // ending in a NUL.
        for entry in git_output(program, repository, &args)?.split('\0') {
            if let Some((about, path)) = entry.split_once('\t') {
                if let Some(object) = about.split(' ').nth(2) {
                    committed.insert(path.to_owned(), object.to_owned());
                }
            }
        }
    }
    let mut changed = Vec::new();
    for paths in read.chunks(PATHS_PER_RUN) {
        let mut args = vec!["hash-object", "--no-filters", "--"];
        args.extend_from_slice(paths);
        let output = git_output(program, repository, &args)?;
        let objects: Vec<&str> = output.lines().collect();
        if objects.len() != paths.len() {
            return Err(SourceCommit::NotARepository);
        }
        for (path, object) in paths.iter().zip(objects) {
            if committed.get(*path).map(String::as_str) != Some(object) {
                changed.push((*path).to_owned());
            }
        }
    }
    changed.extend(
        missing
            .iter()
            .filter(|path| committed.contains_key(**path))
            .map(|path| (*path).to_owned()),
    );
    changed.sort();
    changed.dedup();
    Ok((commit, changed))
}

/// How many paths one git command is given, so its command line stays
/// well under the Windows limit of 32,767 characters.
const PATHS_PER_RUN: usize = 100;

/// git's output when it succeeds; otherwise why there is no source commit.
fn git_output(program: &str, repository: &Path, args: &[&str]) -> Result<String, SourceCommit> {
    match run_git(program, repository, args) {
        GitAnswer::Missing => Err(SourceCommit::GitMissing),
        GitAnswer::Ran {
            code: Some(0),
            stdout,
        } => Ok(stdout),
        GitAnswer::Ran { .. } => Err(SourceCommit::NotARepository),
    }
}

/// Whether `a` and `b` name the same folder.
fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// This repository's root.
pub fn repository_root() -> PathBuf {
    native_services::repository_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs git in a throwaway repository, with no signing, hooks or line
    /// ending conversion from the user's configuration, and returns its
    /// output.
    fn fixture_git(root: &Path, hooks: &Path, args: &[&str]) -> String {
        let hooks_path = format!("core.hooksPath={}", hooks.display());
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
        match run_git(GIT, root, &all) {
            GitAnswer::Ran {
                code: Some(0),
                stdout,
            } => stdout,
            other => panic!("git {args:?}: {other:?}"),
        }
    }

    #[test]
    fn a_changed_new_or_deleted_input_is_recorded_as_dirty() {
        let repository = tempfile::tempdir().unwrap();
        let hooks = tempfile::tempdir().unwrap();
        let root = repository.path();
        assert_eq!(source_commit(root, &["a.csv"], &[]), None);
        let git = |args: &[&str]| fixture_git(root, hooks.path(), args);
        git(&["init", "-q"]);
        std::fs::write(root.join("a.csv"), "x\n").unwrap();
        std::fs::write(root.join("b.csv"), "y\n").unwrap();
        std::fs::write(root.join("R1.kicad_mod"), "r\n").unwrap();
        git(&["add", "a.csv", "b.csv", "R1.kicad_mod"]);
        git(&["commit", "-q", "-m", "fixture"]);
        let clean = source_commit(root, &["a.csv", "b.csv"], &[]).unwrap();
        assert_eq!(clean.len(), 40);
        let dirty = format!("{clean}{DIRTY_SUFFIX}");
        // A file looked for and missing in the commit too is no change.
        assert_eq!(
            source_commit(root, &["a.csv"], &["gone.csv"]).unwrap(),
            clean
        );
        std::fs::write(root.join("b.csv"), "changed\n").unwrap();
        assert_eq!(source_commit(root, &["a.csv"], &[]).unwrap(), clean);
        assert_eq!(
            source_commit(root, &["a.csv", "b.csv"], &[]).unwrap(),
            dirty
        );
        // Staged but not committed is still not the commit.
        git(&["add", "b.csv"]);
        assert_eq!(source_commit(root, &["b.csv"], &[]).unwrap(), dirty);
        std::fs::write(root.join("new.csv"), "z\n").unwrap();
        assert_eq!(source_commit(root, &["new.csv"], &[]).unwrap(), dirty);
        // A file the commit has but the import did not find: deleted.
        std::fs::remove_file(root.join("a.csv")).unwrap();
        assert_eq!(source_commit(root, &[], &["a.csv"]).unwrap(), dirty);
        // Paths are not patterns: `R[1]` does not name `R1`.
        assert_eq!(
            source_commit(root, &[], &["R[1].kicad_mod"]).unwrap(),
            clean
        );
    }

    #[test]
    fn a_change_git_is_told_to_ignore_still_counts() {
        let repository = tempfile::tempdir().unwrap();
        let hooks = tempfile::tempdir().unwrap();
        let root = repository.path();
        let git = |args: &[&str]| fixture_git(root, hooks.path(), args);
        git(&["init", "-q"]);
        std::fs::write(root.join("a.csv"), "x\n").unwrap();
        std::fs::write(root.join("b.csv"), "y\n").unwrap();
        git(&["add", "a.csv", "b.csv"]);
        git(&["commit", "-q", "-m", "fixture"]);
        git(&["update-index", "--skip-worktree", "a.csv"]);
        git(&["update-index", "--assume-unchanged", "b.csv"]);
        std::fs::write(root.join("a.csv"), "changed\n").unwrap();
        std::fs::write(root.join("b.csv"), "changed\n").unwrap();
        // git status sees nothing...
        assert_eq!(git(&["status", "--porcelain"]), "");
        // ...but the bytes are not the commit's.
        for path in ["a.csv", "b.csv"] {
            assert!(
                source_commit(root, &[path], &[])
                    .unwrap()
                    .ends_with(DIRTY_SUFFIX),
                "{path}"
            );
        }
    }

    #[test]
    fn a_folder_that_is_not_the_top_of_a_repository_has_no_commit() {
        let repository = tempfile::tempdir().unwrap();
        let hooks = tempfile::tempdir().unwrap();
        let root = repository.path();
        let git = |args: &[&str]| fixture_git(root, hooks.path(), args);
        git(&["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "copy/\n").unwrap();
        git(&["add", ".gitignore"]);
        git(&["commit", "-q", "-m", "fixture"]);
        // A copy inside an ignored folder of the outer repository.
        let copy = root.join("copy");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("a.csv"), "x\n").unwrap();
        assert_eq!(
            read_source_commit(&copy, &["a.csv"], &[]),
            SourceCommit::NotARepository
        );
        assert!(matches!(
            read_source_commit(root, &[".gitignore"], &[]),
            SourceCommit::Commit(_)
        ));
    }

    #[test]
    fn a_missing_git_is_told_apart_from_a_folder_outside_git() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(
            read_source_commit(folder.path(), &["a.csv"], &[]),
            SourceCommit::NotARepository
        );
        assert_eq!(
            read_source_commit_with("kicad-parts-no-such-git", &repository_root(), &[], &[]),
            SourceCommit::GitMissing
        );
        assert!(matches!(
            read_source_commit(&repository_root(), &[], &[]),
            SourceCommit::Commit(_)
        ));
    }

    /// Every file the resistor import reads.
    const RESISTOR_INPUTS: [&str; 4] = [
        "database/g-res.csv",
        "database/substitutes.csv",
        "symbols/g-pas.kicad_sym",
        FOOTPRINT,
    ];
    const FOOTPRINT: &str = "footprints/g-res.pretty/R_0603_1608Metric.kicad_mod";

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
        let git = |args: &[&str]| fixture_git(root, hooks.path(), args);
        git(&["init", "-q"]);
        let mut add = vec!["add", "--"];
        add.extend_from_slice(&RESISTOR_INPUTS);
        git(&add);
        git(&["commit", "-q", "-m", "fixture"]);
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
            ) && refused.contains(
                "(--allow-uncommitted-changes). Until you commit these changes and import again, a later import"
            ),
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
    fn a_deleted_or_renamed_footprint_is_an_uncommitted_change() {
        let (repository, hooks) = committed_resistors();
        let root = repository.path();
        let clean = plan(root, "g-res").unwrap();
        assert!(clean.missing_inputs.is_empty());
        // Deleted: every part that uses it would lose its footprint.
        let footprint = root.join(FOOTPRINT);
        let bytes = std::fs::read(&footprint).unwrap();
        std::fs::remove_file(&footprint).unwrap();
        let deleted = plan(root, "g-res").unwrap();
        assert_eq!(deleted.missing_inputs, [FOOTPRINT]);
        assert!(
            has_uncommitted_changes(&deleted),
            "{:?}",
            deleted.source_commit
        );
        let refused = check_committed(root, &deleted, Allow::default()).unwrap_err();
        assert!(
            refused.starts_with(&format!("{FOOTPRINT} has changes that are not committed")),
            "{refused}"
        );
        // Renamed only in its letter case: Windows would still open it under
        // the old name, but the import does not find it.
        let renamed = root.join("footprints/g-res.pretty/r_0603_1608metric.kicad_mod");
        std::fs::write(&renamed, &bytes).unwrap();
        let import = plan(root, "g-res").unwrap();
        assert_eq!(import.missing_inputs, [FOOTPRINT]);
        assert!(has_uncommitted_changes(&import));
        // Once the deletion is committed, it is the commit's state.
        std::fs::remove_file(&renamed).unwrap();
        let git = |args: &[&str]| fixture_git(root, hooks.path(), args);
        git(&["rm", "-q", "--cached", "--", FOOTPRINT]);
        git(&["commit", "-q", "-m", "remove the footprint"]);
        let committed = plan(root, "g-res").unwrap();
        assert!(!has_uncommitted_changes(&committed));
    }

    #[test]
    fn only_changes_to_the_files_an_import_uses_count() {
        let (repository, hooks) = committed_resistors();
        let root = repository.path();
        let clean = plan(root, "g-res").unwrap();
        assert!(!has_uncommitted_changes(&clean));
        assert_eq!(check_committed(root, &clean, Allow::default()), Ok(()));
        // Other work in the checkout: a new file, a file the resistor import
        // does not read, and a staged rename of a file it reads, copied back
        // with the same bytes.
        std::fs::write(root.join("notes.txt"), "unrelated\n").unwrap();
        std::fs::write(root.join("database/g-cap.csv"), "IPN\n").unwrap();
        let git = |args: &[&str]| fixture_git(root, hooks.path(), args);
        git(&["mv", "database/substitutes.csv", "database/subs.csv"]);
        std::fs::copy(
            root.join("database/subs.csv"),
            root.join("database/substitutes.csv"),
        )
        .unwrap();
        let unrelated = plan(root, "g-res").unwrap();
        assert_eq!(unrelated.source_commit, clean.source_commit);
        assert_eq!(check_committed(root, &unrelated, Allow::default()), Ok(()));
        // A file it reads, changed, is named.
        let csv = root.join("database/g-res.csv");
        let text = std::fs::read_to_string(&csv).unwrap();
        std::fs::write(&csv, format!("{text}\n")).unwrap();
        let changed = plan(root, "g-res").unwrap();
        assert_eq!(uncommitted_files(root, &changed), ["database/g-res.csv"]);
    }
}

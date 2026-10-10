//! Refuses to import from a checkout that does not include the team
//! catalog's latest imports.
//!
//! The import route compares each part only with its current revision, so
//! importing files from an older checkout would add revisions that undo
//! newer imported part details. Before importing, [`check_checkout`] makes
//! sure that, for every CSV file in the team catalog, the files being
//! imported include what its latest import run read:
//!
//! - if that run came from a clean commit X, the commit being imported must
//!   be X or a descendant of it (an import at `Y+dirty` counts as Y, since it
//!   adds local changes on top);
//! - if that run came from uncommitted changes (`X+dirty`), the commit being
//!   imported must still be X or a descendant, and every file both imports
//!   read must either have the hash that run recorded or, for a descendant
//!   Y, have changed in git between X and Y. Then a commit of those changes
//!   (or a later change to the file) still imports, but a clean checkout, an
//!   unrelated later commit or other local changes cannot undo them. Files
//!   only one of the imports reads (such as another CSV file) do not
//!   matter.

use crate::services::{self, DIRTY_SUFFIX, GIT, GitAnswer, SourceCommit};
use catalog::csv_import::CsvImport;
use catalog::pocketbase::{ImportState, LatestImport};
use std::collections::BTreeMap;
use std::path::Path;

/// Checks that `import` (read from the checkout `repository`) includes what
/// every latest import run in `state` read. Returns a plain-language reason
/// to refuse.
pub fn check_checkout(
    repository: &Path,
    import: &CsvImport,
    state: &ImportState,
) -> Result<(), String> {
    let mut files: Vec<(String, String)> = import
        .sources
        .iter()
        .map(|source| (source.path.clone(), source.byte_sha256.clone()))
        .collect();
    files.sort();
    check_commit(repository, import.source_commit.as_deref(), &files, state)
}

/// [`check_checkout`] for an import whose source commit is `importing` and
/// whose files are `files` (`(path, byte_sha256)`).
pub fn check_commit(
    repository: &Path,
    importing: Option<&str>,
    files: &[(String, String)],
    state: &ImportState,
) -> Result<(), String> {
    check_commit_with(GIT, repository, importing, files, state)
}

const GIT_MISSING: &str = "git is not installed or not on the PATH, so this cannot check whether the checkout includes the team catalog's latest imports. Install git and import again, or allow an older checkout to import anyway.";

/// Why one or more latest import runs are not included, grouped so that many
/// CSV files sharing a commit give one line.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Refusal {
    /// The checkout does not include this commit.
    NotIncluded(String),
    /// The checkout does not have this commit, so git cannot tell.
    Unknown(String),
    /// The run came from uncommitted changes on this commit, and importing
    /// these files (comma-separated) would undo them.
    UndoesLocalChanges(String, String),
    /// The run records a source commit that is not a commit ID.
    NotACommit(usize),
}

/// [`check_commit`] running `program` as git, so tests can name a git that
/// does not exist.
fn check_commit_with(
    program: &str,
    repository: &Path,
    importing: Option<&str>,
    files: &[(String, String)],
    state: &ImportState,
) -> Result<(), String> {
    let recorded: Vec<&LatestImport> = state
        .latest_imports
        .iter()
        .filter(|run| !run.source_commit.is_empty())
        .collect();
    if recorded.is_empty() {
        return Ok(());
    }
    let Some(importing_commit) = importing else {
        if services::read_source_commit_with(program, repository, &[]) == SourceCommit::GitMissing {
            return Err(GIT_MISSING.to_owned());
        }
        let run = recorded[recorded.len() - 1];
        return Err(format!(
            "cannot tell whether these files include the team catalog's latest imports: they were not read from a git commit, and the latest import of {} came from commit {}. Import from a git checkout, or allow an older checkout to import anyway.",
            csv_path(run),
            short(&run.source_commit)
        ));
    };
    let importing_base = base_commit(importing_commit);
    let git = Git {
        program,
        repository,
    };
    let mut refusals: BTreeMap<Refusal, Vec<String>> = BTreeMap::new();
    for run in recorded {
        let newest = base_commit(&run.source_commit);
        let from_local_changes = run.source_commit.ends_with(DIRTY_SUFFIX);
        let refusal = if !is_commit_id(newest) {
            Some(Refusal::NotACommit(newest.len()))
        } else if newest == importing_base {
            let undone = changed_shared_files(run, files);
            (from_local_changes && !undone.is_empty())
                .then(|| Refusal::UndoesLocalChanges(newest.to_owned(), undone.join(", ")))
        } else {
            match git.is_ancestor(newest, importing_base) {
                Answer::Yes if from_local_changes => {
                    // A shared file that differs from the uncommitted one is
                    // fine only if git changed it since: then it is a
                    // commit of the change or a later change, not an older
                    // copy.
                    let mut undone = Vec::new();
                    for path in changed_shared_files(run, files) {
                        match git.changed_between(newest, importing_base, &path) {
                            Answer::Yes => {}
                            Answer::No | Answer::Unknown => undone.push(path),
                            Answer::GitMissing => return Err(GIT_MISSING.to_owned()),
                        }
                    }
                    (!undone.is_empty())
                        .then(|| Refusal::UndoesLocalChanges(newest.to_owned(), undone.join(", ")))
                }
                Answer::Yes => None,
                Answer::No => Some(Refusal::NotIncluded(newest.to_owned())),
                Answer::Unknown => Some(Refusal::Unknown(newest.to_owned())),
                Answer::GitMissing => return Err(GIT_MISSING.to_owned()),
            }
        };
        if let Some(refusal) = refusal {
            refusals.entry(refusal).or_default().push(csv_path(run));
        }
    }
    if refusals.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = refusals
        .into_iter()
        .map(|(refusal, paths)| describe(&refusal, &paths.join(", "), importing_commit))
        .collect();
    Err(lines.join("\n"))
}

/// The paths that both `run` and the import read, but with different
/// hashes.
fn changed_shared_files(run: &LatestImport, files: &[(String, String)]) -> Vec<String> {
    let recorded: BTreeMap<&str, &str> = run
        .file_hashes
        .iter()
        .map(|(path, hash)| (path.as_str(), hash.as_str()))
        .collect();
    files
        .iter()
        .filter(|(path, hash)| recorded.get(path.as_str()).is_some_and(|old| old != hash))
        .map(|(path, _)| path.clone())
        .collect()
}

fn describe(refusal: &Refusal, paths: &str, importing: &str) -> String {
    match refusal {
        Refusal::NotIncluded(commit) => format!(
            "this checkout (commit {}) does not include commit {}, which the latest import of {paths} came from. Importing could undo the part details that import added, and would make its older symbols, footprints and 3D models the current revisions for every part that uses them. Update the checkout to include that commit and import again, or allow an older checkout if you mean to go back.",
            short(importing),
            short(commit)
        ),
        Refusal::Unknown(commit) => format!(
            "the latest import of {paths} came from commit {}, which this checkout does not have, so it cannot tell whether this checkout includes it. Fetch that commit and import again, or allow an older checkout to import anyway.",
            short(commit)
        ),
        Refusal::UndoesLocalChanges(commit, files) => format!(
            "the latest import of {paths} came from uncommitted changes on commit {}, and this checkout has other copies of {files} that do not include those changes, so importing them would undo them, including making older symbols, footprints or 3D models current again for every part that uses them. Commit those changes and import from that commit, or allow an older checkout if you mean to go back.",
            short(commit)
        ),
        Refusal::NotACommit(length) => format!(
            "the latest import of {paths} records a source commit that is not a commit ID ({length} characters), so it cannot be checked. Allow an older checkout to import anyway."
        ),
    }
}

/// A source commit without its `+dirty` mark.
fn base_commit(commit: &str) -> &str {
    commit.strip_suffix(DIRTY_SUFFIX).unwrap_or(commit)
}

/// 40 hex digits: safe to hand to git as an argument.
fn is_commit_id(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

fn csv_path(run: &LatestImport) -> String {
    format!("database/{}.csv", run.csv_file)
}

/// What git answered to a yes-or-no question.
enum Answer {
    Yes,
    No,
    /// git answered with an error, for example because a commit is not in
    /// this repository.
    Unknown,
    GitMissing,
}

/// git in one repository, run windowless inside a kill-on-close job like
/// every service.
struct Git<'a> {
    program: &'a str,
    repository: &'a Path,
}

impl Git<'_> {
    fn ask(&self, args: &[&str]) -> Answer {
        match services::run_git(self.program, self.repository, args) {
            GitAnswer::Missing => Answer::GitMissing,
            GitAnswer::Ran { code: Some(0), .. } => Answer::Yes,
            GitAnswer::Ran { code: Some(1), .. } => Answer::No,
            GitAnswer::Ran { .. } => Answer::Unknown,
        }
    }

    /// Whether `ancestor` is `descendant` or one of its ancestors.
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Answer {
        if !is_commit_id(descendant) {
            return Answer::Unknown;
        }
        self.ask(&["merge-base", "--is-ancestor", ancestor, descendant])
    }

    /// Whether `path` differs between commits `from` and `to`. The path is
    /// taken literally, never as a pattern.
    fn changed_between(&self, from: &str, to: &str, path: &str) -> Answer {
        match self.ask(&[
            "--literal-pathspecs",
            "diff",
            "--quiet",
            from,
            to,
            "--",
            path,
        ]) {
            // `git diff --quiet` exits 1 when there are differences.
            Answer::Yes => Answer::No,
            Answer::No => Answer::Yes,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(csv_file: &str, source_commit: &str, file_hashes: &[(&str, &str)]) -> LatestImport {
        LatestImport {
            csv_file: csv_file.to_owned(),
            csv_import_number: 1,
            source_commit: source_commit.to_owned(),
            audit_entry: "a".repeat(15),
            file_hashes: pairs(file_hashes),
        }
    }

    fn state(runs: Vec<LatestImport>) -> ImportState {
        ImportState {
            latest_imports: runs,
            ..ImportState::default()
        }
    }

    fn pairs(files: &[(&str, &str)]) -> Vec<(String, String)> {
        files
            .iter()
            .map(|(path, hash)| ((*path).to_owned(), hash.repeat(64)))
            .collect()
    }

    /// This repository's HEAD commit (read only).
    fn head() -> String {
        match services::run_git(GIT, &services::repository_root(), &["rev-parse", "HEAD"]) {
            GitAnswer::Ran { stdout, .. } => stdout,
            GitAnswer::Missing => panic!("git is missing"),
        }
    }

    const RES: &str = "database/g-res.csv";
    const CAP: &str = "database/g-cap.csv";
    const IND: &str = "database/g-ind.csv";
    const SUBSTITUTES: &str = "database/substitutes.csv";

    #[test]
    fn an_empty_catalog_or_the_same_commit_is_accepted() {
        let root = services::repository_root();
        let files = pairs(&[(RES, "a")]);
        assert_eq!(check_commit(&root, None, &files, &state(vec![])), Ok(()));
        assert_eq!(
            check_commit(&root, None, &files, &state(vec![run("g-res", "", &[])])),
            Ok(())
        );
        let head = head();
        let dirty = format!("{head}{DIRTY_SUFFIX}");
        // Local changes on top of the latest import's clean commit.
        let clean_run = state(vec![run("g-res", &head, &[(RES, "b")])]);
        assert_eq!(
            check_commit(&root, Some(&dirty), &files, &clean_run),
            Ok(())
        );
        // The same uncommitted files again.
        let dirty_run = state(vec![run("g-res", &dirty, &[(RES, "a")])]);
        assert_eq!(
            check_commit(&root, Some(&dirty), &files, &dirty_run),
            Ok(())
        );
    }

    #[test]
    fn a_clean_or_different_import_of_a_dirty_commit_is_refused() {
        let root = services::repository_root();
        let head = head();
        let dirty = format!("{head}{DIRTY_SUFFIX}");
        let latest = state(vec![run(
            "g-res",
            &dirty,
            &[(RES, "a"), (SUBSTITUTES, "s")],
        )]);
        for importing in [&head, &dirty] {
            let files = pairs(&[(RES, "b"), (SUBSTITUTES, "s")]);
            let refused = check_commit(&root, Some(importing), &files, &latest).unwrap_err();
            assert!(
                refused.contains("came from uncommitted changes on commit")
                    && refused.contains(&head[..12])
                    && refused.contains("other copies of database/g-res.csv"),
                "{refused}"
            );
        }
    }

    #[test]
    fn only_files_both_imports_read_are_compared() {
        let root = services::repository_root();
        let head = head();
        let dirty = format!("{head}{DIRTY_SUFFIX}");
        // g-res was imported from uncommitted changes to g-res.csv.
        let latest = state(vec![run(
            "g-res",
            &dirty,
            &[(RES, "a"), (SUBSTITUTES, "s")],
        )]);
        // Another CSV file from the same dirty tree imports...
        let g_cap = pairs(&[(CAP, "c"), (SUBSTITUTES, "s")]);
        assert_eq!(check_commit(&root, Some(&dirty), &g_cap, &latest), Ok(()));
        // ...and so does one from a clean checkout that shares no changed
        // file.
        let g_ind = pairs(&[(IND, "i"), (SUBSTITUTES, "s")]);
        assert_eq!(check_commit(&root, Some(&head), &g_ind, &latest), Ok(()));
        // A shared file that differs is refused, whichever CSV file it is.
        let changed_shared = pairs(&[(IND, "i"), (SUBSTITUTES, "t")]);
        let refused = check_commit(&root, Some(&head), &changed_shared, &latest).unwrap_err();
        assert!(
            refused.contains("other copies of database/substitutes.csv"),
            "{refused}"
        );
    }

    #[test]
    fn a_commit_that_cannot_be_checked_is_refused() {
        let root = services::repository_root();
        let head = head();
        let any = pairs(&[(RES, "a")]);
        // A commit this repository does not have.
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let refused = check_commit(
            &root,
            Some(&head),
            &any,
            &state(vec![run("g-cap", unknown, &[])]),
        )
        .unwrap_err();
        assert!(
            refused.contains("which this checkout does not have")
                && refused.contains("database/g-cap.csv")
                && refused.contains("0123456789ab"),
            "{refused}"
        );
        // Files not read from a git commit.
        let refused =
            check_commit(&root, None, &any, &state(vec![run("g-res", &head, &[])])).unwrap_err();
        assert!(refused.contains("not read from a git commit"), "{refused}");
        // A recorded commit that is not a commit ID never reaches git.
        let refused = check_commit(
            &root,
            Some(&head),
            &any,
            &state(vec![run("g-res", "--all", &[])]),
        )
        .unwrap_err();
        assert!(
            refused.contains("not a commit ID (5 characters)"),
            "{refused}"
        );
    }

    #[test]
    fn a_missing_git_is_said_plainly() {
        let root = services::repository_root();
        let head = head();
        let any = pairs(&[(RES, "a")]);
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let latest = state(vec![run("g-res", unknown, &[])]);
        for importing in [None, Some(head.as_str())] {
            let refused =
                check_commit_with("kicad-parts-no-such-git", &root, importing, &any, &latest)
                    .unwrap_err();
            assert!(refused.starts_with("git is not installed"), "{refused}");
        }
    }

    #[test]
    fn files_sharing_a_commit_give_one_line() {
        let root = services::repository_root();
        let head = head();
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let latest = state(vec![run("g-cap", unknown, &[]), run("g-res", unknown, &[])]);
        let refused = check_commit(&root, Some(&head), &pairs(&[(RES, "a")]), &latest).unwrap_err();
        assert_eq!(refused.lines().count(), 1, "{refused}");
        assert!(
            refused.contains("database/g-cap.csv, database/g-res.csv"),
            "{refused}"
        );
    }
}

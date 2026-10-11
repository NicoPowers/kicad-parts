//! Refuses to import from a checkout that does not include the team
//! catalog's latest imports.
//!
//! The import route compares each part only with its current revision, so
//! importing files from an older checkout would add revisions that undo
//! newer imported part details. Before importing, [`check_checkout`] makes
//! sure that, for every CSV file in the team catalog, the commit being
//! imported is the commit its latest import run came from, or a descendant
//! of it.
//!
//! An import from uncommitted changes (`X+dirty`) counts as its commit X,
//! both when it is the latest run and when it is the import being checked.
//! Such an import is refused unless it is allowed explicitly (see
//! `services::check_committed`). Until its changes are committed, a later
//! import from a checkout without them undoes them, like an import from an
//! older checkout; this check does not try to stop that, because whoever
//! allows uncommitted changes is told to commit them.

use crate::services::{self, DIRTY_SUFFIX, GIT, GitAnswer, SourceCommit};
use catalog::csv_import::CsvImport;
use catalog::pocketbase::{ImportState, LatestImport};
use std::collections::BTreeMap;
use std::path::Path;

/// Checks that `import` (read from the checkout `repository`) includes the
/// commit of every latest import run in `state`. Returns a plain-language
/// reason to refuse.
pub fn check_checkout(
    repository: &Path,
    import: &CsvImport,
    state: &ImportState,
) -> Result<(), String> {
    check_commit(repository, import.source_commit.as_deref(), state)
}

/// [`check_checkout`] for an import whose source commit is `importing`.
pub fn check_commit(
    repository: &Path,
    importing: Option<&str>,
    state: &ImportState,
) -> Result<(), String> {
    check_commit_with(GIT, repository, importing, state)
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
    /// The run records a source commit that is not a commit ID.
    NotACommit(usize),
}

/// [`check_commit`] running `program` as git, so tests can name a git that
/// does not exist.
fn check_commit_with(
    program: &str,
    repository: &Path,
    importing: Option<&str>,
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
        let refusal = if !is_commit_id(newest) {
            Some(Refusal::NotACommit(newest.len()))
        } else if newest == importing_base {
            None
        } else {
            match git.is_ancestor(newest, importing_base) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(csv_file: &str, source_commit: &str) -> LatestImport {
        LatestImport {
            csv_file: csv_file.to_owned(),
            csv_import_number: 1,
            source_commit: source_commit.to_owned(),
            audit_entry: "a".repeat(15),
        }
    }

    fn state(runs: Vec<LatestImport>) -> ImportState {
        ImportState {
            latest_imports: runs,
            ..ImportState::default()
        }
    }

    /// This repository's HEAD commit (read only).
    fn head() -> String {
        match services::run_git(GIT, &services::repository_root(), &["rev-parse", "HEAD"]) {
            GitAnswer::Ran { stdout, .. } => stdout,
            GitAnswer::Missing => panic!("git is missing"),
        }
    }

    #[test]
    fn an_empty_catalog_or_the_same_commit_is_accepted() {
        let root = services::repository_root();
        assert_eq!(check_commit(&root, None, &state(vec![])), Ok(()));
        assert_eq!(
            check_commit(&root, None, &state(vec![run("g-res", "")])),
            Ok(())
        );
        let head = head();
        let dirty = format!("{head}{DIRTY_SUFFIX}");
        // Uncommitted changes count as their commit, on either side.
        for (latest, importing) in [
            (&head, &head),
            (&head, &dirty),
            (&dirty, &head),
            (&dirty, &dirty),
        ] {
            assert_eq!(
                check_commit(&root, Some(importing), &state(vec![run("g-res", latest)])),
                Ok(()),
                "{latest} then {importing}"
            );
        }
    }

    /// Runs git in a throwaway repository, with no signing or hooks from the
    /// user's configuration, and returns its output.
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
            hooks_path.as_str(),
        ];
        all.extend_from_slice(args);
        match services::run_git(GIT, root, &all) {
            GitAnswer::Ran {
                code: Some(0),
                stdout,
            } => stdout,
            other => panic!("git {args:?}: {other:?}"),
        }
    }

    #[test]
    fn a_descendant_is_accepted_and_an_ancestor_refused() {
        let repository = tempfile::tempdir().unwrap();
        let hooks = tempfile::tempdir().unwrap();
        let root = repository.path();
        let git = |args: &[&str]| fixture_git(root, hooks.path(), args);
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "parent"]);
        let parent = git(&["rev-parse", "HEAD"]);
        git(&["commit", "-q", "--allow-empty", "-m", "head"]);
        let head = git(&["rev-parse", "HEAD"]);
        let dirty_parent = format!("{parent}{DIRTY_SUFFIX}");
        assert_eq!(
            check_commit(root, Some(&head), &state(vec![run("g-res", &dirty_parent)])),
            Ok(())
        );
        for latest in [&head, &format!("{head}{DIRTY_SUFFIX}")] {
            let refused = check_commit(
                root,
                Some(&dirty_parent),
                &state(vec![run("g-res", latest)]),
            )
            .unwrap_err();
            assert!(
                refused.contains(&format!("does not include commit {}", &head[..12]))
                    && refused.contains("database/g-res.csv"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_commit_that_cannot_be_checked_is_refused() {
        let root = services::repository_root();
        let head = head();
        // A commit this repository does not have.
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let refused =
            check_commit(&root, Some(&head), &state(vec![run("g-cap", unknown)])).unwrap_err();
        assert!(
            refused.contains("which this checkout does not have")
                && refused.contains("database/g-cap.csv")
                && refused.contains("0123456789ab"),
            "{refused}"
        );
        // Files not read from a git commit.
        let refused = check_commit(&root, None, &state(vec![run("g-res", &head)])).unwrap_err();
        assert!(refused.contains("not read from a git commit"), "{refused}");
        // A recorded commit that is not a commit ID never reaches git.
        let refused =
            check_commit(&root, Some(&head), &state(vec![run("g-res", "--all")])).unwrap_err();
        assert!(
            refused.contains("not a commit ID (5 characters)"),
            "{refused}"
        );
    }

    #[test]
    fn a_missing_git_is_said_plainly() {
        let root = services::repository_root();
        let head = head();
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let latest = state(vec![run("g-res", unknown)]);
        for importing in [None, Some(head.as_str())] {
            let refused = check_commit_with("kicad-parts-no-such-git", &root, importing, &latest)
                .unwrap_err();
            assert!(refused.starts_with("git is not installed"), "{refused}");
        }
    }

    #[test]
    fn files_sharing_a_commit_give_one_line() {
        let root = services::repository_root();
        let head = head();
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let latest = state(vec![run("g-cap", unknown), run("g-res", unknown)]);
        let refused = check_commit(&root, Some(&head), &latest).unwrap_err();
        assert_eq!(refused.lines().count(), 1, "{refused}");
        assert!(
            refused.contains("database/g-cap.csv, database/g-res.csv"),
            "{refused}"
        );
    }
}

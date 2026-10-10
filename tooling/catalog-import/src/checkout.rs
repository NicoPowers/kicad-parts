//! Refuses to import from a checkout that does not include the team
//! catalog's latest imports.
//!
//! The import route compares each part only with its current revision, so
//! importing files from an older checkout would add revisions that undo
//! newer imported part details. Before importing, [`check_checkout`] makes
//! sure that, for every CSV file in the team catalog, the files being
//! imported include what its latest import run read:
//!
//! - if that run came from a clean commit, the commit being imported must be
//!   the same commit or a descendant of it (a `+dirty` import counts as its
//!   base commit here, since it adds local changes on top);
//! - if that run came from uncommitted changes (`<commit>+dirty`), an import
//!   from the same commit must read exactly the same files, because a clean
//!   checkout of that commit, or other local changes, would undo them. A
//!   descendant commit is accepted, as with a clean run.

use crate::services::DIRTY_SUFFIX;
use catalog::csv_import::CsvImport;
use catalog::pocketbase::{ImportState, LatestImport};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

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

/// Why one or more latest import runs are not included, grouped so that many
/// CSV files sharing a commit give one line.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Refusal {
    /// The checkout does not include this commit.
    NotIncluded(String),
    /// The checkout does not have this commit, so git cannot tell.
    Unknown(String),
    /// The run came from uncommitted changes on this commit, and these files
    /// differ from them.
    DirtyDiffers(String),
    /// The run records a source commit that is not a commit ID.
    NotACommit(usize),
}

/// [`check_checkout`] for an import whose source commit is `importing` and
/// whose files are `files` (`(path, byte_sha256)`, sorted by path).
pub fn check_commit(
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
        let run = recorded[recorded.len() - 1];
        return Err(format!(
            "cannot tell whether these files include the team catalog's latest imports: they were not read from a git commit, and the latest import of {} came from commit {}. Import from a git checkout, or allow an older checkout to import anyway.",
            csv_path(run),
            short(&run.source_commit)
        ));
    };
    let importing_base = base_commit(importing_commit);
    let mut refusals: BTreeMap<Refusal, Vec<String>> = BTreeMap::new();
    for run in recorded {
        let newest = base_commit(&run.source_commit);
        let refusal = if !is_commit_id(newest) {
            Some(Refusal::NotACommit(newest.len()))
        } else if newest == importing_base {
            let from_local_changes = run.source_commit.ends_with(DIRTY_SUFFIX);
            (from_local_changes && run.file_hashes != files)
                .then(|| Refusal::DirtyDiffers(newest.to_owned()))
        } else {
            match is_ancestor(repository, newest, importing_base) {
                Ancestry::Included => None,
                Ancestry::NotIncluded => Some(Refusal::NotIncluded(newest.to_owned())),
                Ancestry::Unknown => Some(Refusal::Unknown(newest.to_owned())),
                Ancestry::GitMissing => {
                    return Err(
                        "git is not installed or not on the PATH, so this cannot check whether the checkout includes the team catalog's latest imports. Install git and import again, or allow an older checkout to import anyway."
                            .to_owned(),
                    );
                }
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
            "this checkout (commit {}) does not include commit {}, which the latest import of {paths} came from. Importing could undo the part details that import added. Update the checkout to include that commit and import again, or allow an older checkout if you mean to go back.",
            short(importing),
            short(commit)
        ),
        Refusal::Unknown(commit) => format!(
            "the latest import of {paths} came from commit {}, which this checkout does not have, so it cannot tell whether this checkout includes it. Fetch that commit and import again, or allow an older checkout to import anyway.",
            short(commit)
        ),
        Refusal::DirtyDiffers(commit) => format!(
            "the latest import of {paths} came from uncommitted changes on commit {}, and these files differ from those changes, so importing them would undo them. Commit those changes and import from that commit, or allow an older checkout if you mean to go back.",
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

enum Ancestry {
    Included,
    NotIncluded,
    /// git answered with an error, for example because the commit is not
    /// in this repository.
    Unknown,
    /// git could not be started.
    GitMissing,
}

/// Whether `ancestor` is `descendant` or one of its ancestors, asked of git
/// windowless and inside a kill-on-close job like every service.
fn is_ancestor(repository: &Path, ancestor: &str, descendant: &str) -> Ancestry {
    if !is_commit_id(descendant) {
        return Ancestry::Unknown;
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repository)
        .args(["merge-base", "--is-ancestor", ancestor, descendant]);
    match native_services::run_hidden("git", &mut command) {
        Err(_) => Ancestry::GitMissing,
        Ok(output) => match output.status.code() {
            Some(0) => Ancestry::Included,
            Some(1) => Ancestry::NotIncluded,
            _ => Ancestry::Unknown,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(csv_file: &str, source_commit: &str) -> ImportState {
        state_with_files(csv_file, source_commit, &[])
    }

    fn state_with_files(
        csv_file: &str,
        source_commit: &str,
        file_hashes: &[(String, String)],
    ) -> ImportState {
        ImportState {
            current_revisions: Default::default(),
            latest_imports: vec![LatestImport {
                csv_file: csv_file.to_owned(),
                csv_import_number: 1,
                source_commit: source_commit.to_owned(),
                audit_entry: "a".repeat(15),
                file_hashes: file_hashes.to_vec(),
            }],
        }
    }

    /// This repository's HEAD commit (read only).
    fn head() -> String {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(crate::services::repository_root())
            .args(["rev-parse", "HEAD"]);
        let output = native_services::run_hidden("git", &mut command).unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn files(hash: &str) -> Vec<(String, String)> {
        vec![("database/g-res.csv".to_owned(), hash.repeat(64))]
    }

    #[test]
    fn an_empty_catalog_or_the_same_commit_is_accepted() {
        let root = crate::services::repository_root();
        let none = files("a");
        assert_eq!(
            check_commit(&root, None, &none, &ImportState::default()),
            Ok(())
        );
        assert_eq!(
            check_commit(&root, None, &none, &state("g-res", "")),
            Ok(())
        );
        let head = head();
        let dirty = format!("{head}{DIRTY_SUFFIX}");
        // Local changes on top of the latest import's clean commit.
        assert_eq!(
            check_commit(&root, Some(&dirty), &none, &state("g-res", &head)),
            Ok(())
        );
        // The same uncommitted files again.
        assert_eq!(
            check_commit(
                &root,
                Some(&dirty),
                &none,
                &state_with_files("g-res", &dirty, &none)
            ),
            Ok(())
        );
    }

    #[test]
    fn a_clean_or_different_import_of_a_dirty_commit_is_refused() {
        let root = crate::services::repository_root();
        let head = head();
        let dirty = format!("{head}{DIRTY_SUFFIX}");
        let latest = state_with_files("g-res", &dirty, &files("a"));
        for importing in [&head, &dirty] {
            let refused = check_commit(&root, Some(importing), &files("b"), &latest).unwrap_err();
            assert!(
                refused.contains("came from uncommitted changes on commit")
                    && refused.contains(&head[..12]),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_commit_that_cannot_be_checked_is_refused() {
        let root = crate::services::repository_root();
        let head = head();
        let any = files("a");
        // A commit this repository does not have.
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let refused = check_commit(&root, Some(&head), &any, &state("g-cap", unknown)).unwrap_err();
        assert!(
            refused.contains("which this checkout does not have")
                && refused.contains("database/g-cap.csv")
                && refused.contains("0123456789ab"),
            "{refused}"
        );
        // Files not read from a git commit.
        let refused = check_commit(&root, None, &any, &state("g-res", &head)).unwrap_err();
        assert!(refused.contains("not read from a git commit"), "{refused}");
        // A recorded commit that is not a commit ID never reaches git.
        let refused = check_commit(&root, Some(&head), &any, &state("g-res", "--all")).unwrap_err();
        assert!(
            refused.contains("not a commit ID (5 characters)"),
            "{refused}"
        );
    }

    #[test]
    fn files_sharing_a_commit_give_one_line() {
        let root = crate::services::repository_root();
        let head = head();
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let mut latest = state("g-cap", unknown);
        let mut second = latest.latest_imports[0].clone();
        second.csv_file = "g-res".into();
        latest.latest_imports.push(second);
        let refused = check_commit(&root, Some(&head), &files("a"), &latest).unwrap_err();
        assert_eq!(refused.lines().count(), 1, "{refused}");
        assert!(
            refused.contains("database/g-cap.csv, database/g-res.csv"),
            "{refused}"
        );
    }
}

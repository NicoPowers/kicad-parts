//! Refuses to import from a checkout that is older than the team catalog.
//!
//! The import route compares each part only with its current revision, so
//! importing files from an older checkout would add revisions that undo
//! newer imported part details. Before importing, [`check_checkout`] makes
//! sure the commit being imported is the same as, or a descendant of, the
//! commit of the latest import run of every CSV file in the team catalog.
//! A commit ending in `+dirty` counts as its base commit.

use crate::services::DIRTY_SUFFIX;
use catalog::pocketbase::{ImportState, LatestImport};
use std::path::Path;
use std::process::Command;

/// Checks that `importing` (the source commit of the import, as
/// [`crate::services::source_commit`] records it) includes the commit of
/// every latest import run in `state`. `repository` is the checkout the
/// files were read from. Returns a plain-language reason to refuse.
pub fn check_checkout(
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
    let Some(importing) = importing.map(base_commit) else {
        let run = recorded[recorded.len() - 1];
        return Err(format!(
            "cannot tell whether these files are older than the team catalog: they were not read from a git commit, and the latest import of {} came from commit {}. Import from a git checkout, or allow an older checkout to import anyway.",
            csv_path(run),
            short(&run.source_commit)
        ));
    };
    let mut reasons = Vec::new();
    for run in recorded {
        let newest = base_commit(&run.source_commit);
        if newest == importing {
            continue;
        }
        if !is_commit_id(newest) {
            reasons.push(format!(
                "the latest import of {} records a source commit that is not a commit ID ({} characters)",
                csv_path(run),
                newest.len()
            ));
            continue;
        }
        match is_ancestor(repository, newest, importing) {
            Ancestry::Included => {}
            Ancestry::NotIncluded => reasons.push(format!(
                "this checkout (commit {}) is older than the team catalog: the latest import of {} came from commit {}, which this checkout does not include. Importing would undo newer imported part details. Update the checkout to include that commit and import again, or allow an older checkout if you mean to go back.",
                short(importing),
                csv_path(run),
                short(newest)
            )),
            Ancestry::Unknown => reasons.push(format!(
                "the latest import of {} came from commit {}, which this checkout does not have, so it cannot tell whether this checkout is older. Fetch that commit and import again, or allow an older checkout to import anyway.",
                csv_path(run),
                short(newest)
            )),
        }
    }
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons.join("\n"))
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
    /// git could not answer, for example because the commit is not in this
    /// repository.
    Unknown,
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
    match native_services::run_hidden("git", &mut command).map(|output| output.status.code()) {
        Ok(Some(0)) => Ancestry::Included,
        Ok(Some(1)) => Ancestry::NotIncluded,
        _ => Ancestry::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(csv_file: &str, source_commit: &str) -> ImportState {
        ImportState {
            current_revisions: Default::default(),
            latest_imports: vec![LatestImport {
                csv_file: csv_file.to_owned(),
                csv_import_number: 1,
                source_commit: source_commit.to_owned(),
                audit_entry: "a".repeat(15),
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

    #[test]
    fn an_empty_catalog_or_the_same_commit_is_accepted() {
        let root = crate::services::repository_root();
        assert_eq!(check_checkout(&root, None, &ImportState::default()), Ok(()));
        assert_eq!(check_checkout(&root, None, &state("g-res", "")), Ok(()));
        let head = head();
        let dirty = format!("{head}{DIRTY_SUFFIX}");
        assert_eq!(
            check_checkout(&root, Some(&head), &state("g-res", &dirty)),
            Ok(())
        );
        assert_eq!(
            check_checkout(&root, Some(&dirty), &state("g-res", &head)),
            Ok(())
        );
    }

    #[test]
    fn a_commit_that_cannot_be_checked_is_refused() {
        let root = crate::services::repository_root();
        let head = head();
        // A commit this repository does not have.
        let unknown = "0123456789abcdef0123456789abcdef01234567";
        let refused = check_checkout(&root, Some(&head), &state("g-cap", unknown)).unwrap_err();
        assert!(
            refused.contains("which this checkout does not have")
                && refused.contains("database/g-cap.csv")
                && refused.contains("0123456789ab"),
            "{refused}"
        );
        // Files not read from a git commit.
        let refused = check_checkout(&root, None, &state("g-res", &head)).unwrap_err();
        assert!(refused.contains("not read from a git commit"), "{refused}");
        // A recorded commit that is not a commit ID never reaches git.
        let refused = check_checkout(&root, Some(&head), &state("g-res", "--all")).unwrap_err();
        assert!(
            refused.contains("not a commit ID (5 characters)"),
            "{refused}"
        );
    }
}

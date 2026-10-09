//! A disposable PocketBase carrying the repository's schema and hooks.

use catalog::CsvCatalog;
use catalog::csv_import::{CsvImport, ImportError};
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

/// Marks a source commit whose inputs differ from it.
pub const DIRTY_SUFFIX: &str = "+dirty";

/// Runs git in `repository`, windowless and inside a kill-on-close job like
/// every service, and returns its trimmed standard output on success.
fn git(repository: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repository).args(args);
    let output = native_services::run_hidden("git", &mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

/// The `HEAD` commit the import read `inputs` (repository paths) at, with
/// [`DIRTY_SUFFIX`] appended if any of them is modified, staged or untracked
/// relative to it, so a dirty tree is never recorded as a clean commit.
/// `None` outside a git repository.
pub fn source_commit(repository: &Path, inputs: &[&str]) -> Option<String> {
    let commit = git(repository, &["rev-parse", "HEAD"])?;
    if commit.len() != 40 {
        return None;
    }
    let mut status = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    status.extend_from_slice(inputs);
    let changes = git(repository, &status)?;
    Some(if changes.is_empty() {
        commit
    } else {
        format!("{commit}{DIRTY_SUFFIX}")
    })
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
}

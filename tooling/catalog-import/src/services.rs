//! A disposable PocketBase carrying the repository's schema and hooks.

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

/// The repository's `HEAD` commit, if git can tell.
pub fn source_commit(repository: &Path) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "HEAD"]);
    // Windowless and inside a kill-on-close job, like every service.
    let output = native_services::run_hidden("git rev-parse", &mut command).ok()?;
    let commit = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && commit.len() == 40).then_some(commit)
}

/// This repository's root.
pub fn repository_root() -> PathBuf {
    native_services::repository_root()
}

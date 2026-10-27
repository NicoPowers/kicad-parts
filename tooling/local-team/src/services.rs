//! A disposable PocketBase carrying the repository's schema and hooks.

use collaboration::PocketBaseClient;
use native_services::{
    PocketBaseOptions, PocketBaseServer, Superuser, pocketbase_binary, random_name,
};
use std::path::{Path, PathBuf};

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

/// This repository's root.
pub fn repository_root() -> PathBuf {
    native_services::repository_root()
}

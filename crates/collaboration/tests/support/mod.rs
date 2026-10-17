//! Shared setup for the PocketBase integration tests: disposable instances
//! of the pinned release, started through `native-services`.

#![allow(dead_code)]

use collaboration::PocketBaseClient;
use native_services::{
    PocketBaseOptions, PocketBaseServer, Superuser, pocketbase_binary, random_name, repository_root,
};
use std::fs;
use std::path::{Path, PathBuf};

pub const INIT_MIGRATION: &str = "1791400000_collab_init.js";
pub const CATEGORY_MIGRATION: &str = "1791400100_collab_record_category.js";

pub fn binary() -> PathBuf {
    pocketbase_binary(&repository_root()).expect("the pinned PocketBase is available")
}

/// Options for an instance under `root`: `root/pb_data`, a migrations
/// directory holding copies of `migrations` from the repository (so a test
/// controls which ones exist), and the repository hooks.
pub fn options(root: &Path, migrations: &[&str]) -> PocketBaseOptions {
    let repository = repository_root();
    let migrations_dir = root.join("pb_migrations");
    fs::create_dir_all(&migrations_dir).unwrap();
    for name in migrations {
        fs::copy(
            repository.join("infra/pocketbase/pb_migrations").join(name),
            migrations_dir.join(name),
        )
        .unwrap();
    }
    PocketBaseOptions {
        data_dir: root.join("pb_data"),
        migrations_dir,
        hooks_dir: repository.join("infra/pocketbase/pb_hooks"),
        log_file: root.join(format!("pocketbase-{}.log", random_name(6))),
    }
}

/// Every migration in the repository, in the order PocketBase applies them.
pub fn all_migrations() -> Vec<String> {
    let mut names: Vec<String> =
        fs::read_dir(repository_root().join("infra/pocketbase/pb_migrations"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".js"))
            .collect();
    names.sort();
    names
}

/// Starts an instance (upserting `superuser` first) and signs in as it.
/// `PocketBaseServer::start` already fails if PocketBase showed its
/// browser installer.
pub fn start(
    options: &PocketBaseOptions,
    superuser: &Superuser,
) -> (PocketBaseServer, PocketBaseClient) {
    let server = PocketBaseServer::start(&binary(), options, superuser).unwrap();
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    (server, admin)
}

/// A team member's credentials.
#[derive(Clone)]
pub struct Member {
    pub email: String,
    pub password: String,
    pub id: String,
}

pub fn create_member(admin: &PocketBaseClient, name: &str) -> Member {
    let email = format!("{name}-{}@example.test", random_name(6));
    let password = native_services::random_token(24);
    let created: serde_json::Value = admin
        .post_json(
            "/api/collections/users/records",
            &serde_json::json!({
                "email": email,
                "password": password,
                "passwordConfirm": password,
                "name": name,
                "verified": true,
            }),
        )
        .unwrap()
        .json()
        .unwrap();
    Member {
        email,
        password,
        id: created["id"].as_str().unwrap().to_owned(),
    }
}

pub fn sign_in(server_url: &str, member: &Member) -> PocketBaseClient {
    PocketBaseClient::new(server_url)
        .authenticate("users", &member.email, &member.password)
        .unwrap()
}

pub fn sha256(bytes: &[u8]) -> String {
    native_services::sha256_hex(bytes)
}

/// Every record of `collection` as JSON, sorted by ID, with
/// `collectionId`/`collectionName` (which a restore preserves anyway)
/// left in, so the comparison is of everything the API returns.
pub fn records(admin: &PocketBaseClient, collection: &str) -> Vec<serde_json::Value> {
    admin.list_records(collection, "", "id").unwrap()
}

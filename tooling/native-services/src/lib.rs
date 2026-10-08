//! Real PocketBase and MinIO services for tests, run as native processes.
//!
//! Every binary comes from `infra/versions.lock`, the single source of truth
//! shared with the Docker lab: PocketBase is the official release asset, and
//! MinIO is built from its pinned source archive with the pinned Go toolchain
//! because MinIO no longer publishes community binaries. Downloads are
//! verified by SHA-256 and cached by checksum under `target/native-services`,
//! never in the repository.
//!
//! Each service a test starts is disposable:
//! - it uses a caller-owned data directory (normally a temp dir);
//! - it listens on a random free port on 127.0.0.1 only, so Windows Firewall
//!   never prompts;
//! - its credentials are generated per run and kept in memory;
//! - it runs without a console window (`CREATE_NO_WINDOW`) inside a
//!   kill-on-close job object, so it dies with the test process even if the
//!   test is killed.

mod archive;
mod fetch;
mod minio;
mod pins;
mod pocketbase;
mod process;
pub mod s3;

use std::fmt;
use std::path::PathBuf;

pub use archive::extract_zip;
pub use fetch::{Cache, sha256_file, sha256_hex};
pub use minio::{MinioServer, minio_binary};
pub use pins::{GoToolchain, MinioSource, NativePins, PinnedAsset, Platform};
pub use pocketbase::{
    PocketBaseOptions, PocketBaseServer, Superuser, pocketbase_binary, shows_installer_banner,
    upsert_superuser,
};
pub use process::{ServiceProcess, free_loopback_port, run_hidden};

/// A failure fetching, building, starting or talking to a native service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError(pub String);

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ServiceError {}

pub type ServiceResult<T> = Result<T, ServiceError>;

pub(crate) fn error(message: impl Into<String>) -> ServiceError {
    ServiceError(message.into())
}

/// The repository root, found from this crate's manifest directory.
pub fn repository_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists");
    without_verbatim_prefix(root)
}

/// Turns Windows' `\\?\C:\...` (what `canonicalize` returns) into
/// `C:\...`. Child programs need the plain form: Go, for one, reads the `?`
/// as a glob wildcard, and its go:embed patterns then match nothing.
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => path,
    }
}

/// A random string of ASCII letters and digits from the OS random source,
/// for per-run credentials and names.
pub fn random_token(length: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = vec![0u8; length];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    // 248 is the largest multiple of 62 below 256; rejecting larger bytes
    // keeps every character equally likely.
    let mut token = String::with_capacity(length);
    let mut pool = bytes;
    while token.len() < length {
        for byte in &pool {
            if *byte < 248 && token.len() < length {
                token.push(ALPHABET[(*byte % 62) as usize] as char);
            }
        }
        pool = vec![0u8; length];
        getrandom::fill(&mut pool).expect("the OS random source is available");
    }
    token
}

/// A random lowercase name, valid as an S3 bucket name suffix.
pub fn random_name(length: usize) -> String {
    random_token(length).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_removed() {
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\C:\repo")),
            PathBuf::from(r"C:\repo")
        );
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
        assert_eq!(
            without_verbatim_prefix(PathBuf::from("/srv/repo")),
            PathBuf::from("/srv/repo")
        );
        assert!(!repository_root().to_string_lossy().contains('?'));
    }

    #[test]
    fn random_tokens_have_the_requested_length_and_alphabet() {
        let token = random_token(40);
        assert_eq!(token.len(), 40);
        assert!(token.chars().all(|ch| ch.is_ascii_alphanumeric()));
        assert_ne!(token, random_token(40));
        let name = random_name(12);
        assert_eq!(name.len(), 12);
        assert!(
            name.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        );
    }
}

//! Checksum-addressed downloads and unpacked archives under `target/`.

use crate::archive;
use crate::pins::PinnedAsset;
use crate::process::run_hidden;
use crate::{ServiceResult, error};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The download and build cache, `target/native-services` by default.
#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn for_repository(repository: &Path) -> Self {
        Self::new(repository.join("target").join("native-services"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns a verified local copy of `asset`, downloading it when the
    /// cache has none. A cached file that no longer matches its checksum is
    /// replaced.
    pub fn download(&self, asset: &PinnedAsset) -> ServiceResult<PathBuf> {
        let file_name = asset
            .url
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty() && !name.contains(['\\', ':']))
            .ok_or_else(|| error(format!("`{}` does not name a file", asset.url)))?;
        let directory = self.root.join("downloads").join(&asset.sha256);
        let target = directory.join(file_name);
        if target.is_file() {
            if sha256_file(&target)? == asset.sha256 {
                return Ok(target);
            }
            fs::remove_file(&target)
                .map_err(|io| error(format!("cannot replace `{}`: {io}", target.display())))?;
        }
        create_dir(&directory)?;
        let partial = directory.join(format!("{file_name}.{}.partial", unique_suffix()));
        let mut command = Command::new("curl");
        command
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--retry",
                "3",
                "--output",
            ])
            .arg(&partial)
            .arg(&asset.url);
        let output = run_hidden("curl", &mut command)?;
        if !output.status.success() {
            let _ = fs::remove_file(&partial);
            return Err(error(format!(
                "downloading {} failed: {}",
                asset.url,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let actual = sha256_file(&partial)?;
        if actual != asset.sha256 {
            let _ = fs::remove_file(&partial);
            return Err(error(format!(
                "{} has SHA-256 {actual}, but infra/versions.lock pins {}",
                asset.url, asset.sha256
            )));
        }
        publish(&partial, &target)?;
        Ok(target)
    }

    /// Unpacks a verified archive once into `unpacked/<label>-<sha>` and
    /// returns that directory. A `.complete` marker makes partial unpacks
    /// (from a killed run) start over.
    pub fn unpacked(&self, label: &str, asset: &PinnedAsset) -> ServiceResult<PathBuf> {
        let directory = self
            .root
            .join("unpacked")
            .join(format!("{label}-{}", &asset.sha256[..16]));
        let marker = directory.join(".complete");
        if fs::read_to_string(&marker).ok().as_deref() == Some(asset.sha256.as_str()) {
            return Ok(directory);
        }
        let archive_path = self.download(asset)?;
        let staging = self.root.join("unpacked").join(format!(
            "{label}-{}.{}",
            &asset.sha256[..16],
            unique_suffix()
        ));
        create_dir(&staging)?;
        let unpacked = if asset.url.ends_with(".zip") {
            archive::extract_zip(&archive_path, &staging)
        } else if asset.url.ends_with(".tar.gz") {
            archive::extract_tar_gz(&archive_path, &staging, 1)
        } else {
            Err(error(format!("unsupported archive type: {}", asset.url)))
        };
        if let Err(failure) = unpacked {
            let _ = fs::remove_dir_all(&staging);
            return Err(failure);
        }
        fs::write(staging.join(".complete"), &asset.sha256)
            .map_err(|io| error(format!("cannot mark `{}`: {io}", staging.display())))?;
        if directory.exists() {
            // A previous unpack without a marker, or one that finished
            // concurrently; prefer a complete one.
            if fs::read_to_string(&marker).ok().as_deref() == Some(asset.sha256.as_str()) {
                let _ = fs::remove_dir_all(&staging);
                return Ok(directory);
            }
            fs::remove_dir_all(&directory)
                .map_err(|io| error(format!("cannot clear `{}`: {io}", directory.display())))?;
        }
        match fs::rename(&staging, &directory) {
            Ok(()) => Ok(directory),
            Err(_) if marker.is_file() => {
                let _ = fs::remove_dir_all(&staging);
                Ok(directory)
            }
            Err(io) => Err(error(format!(
                "cannot move `{}` into place: {io}",
                staging.display()
            ))),
        }
    }
}

/// Moves a finished file into place; if another process got there first
/// with the same content, keeps theirs.
pub(crate) fn publish(partial: &Path, target: &Path) -> ServiceResult<()> {
    match fs::rename(partial, target) {
        Ok(()) => Ok(()),
        Err(_) if target.is_file() => {
            let _ = fs::remove_file(partial);
            Ok(())
        }
        Err(io) => Err(error(format!(
            "cannot move `{}` into place: {io}",
            partial.display()
        ))),
    }
}

pub(crate) fn create_dir(path: &Path) -> ServiceResult<()> {
    fs::create_dir_all(path)
        .map_err(|io| error(format!("cannot create `{}`: {io}", path.display())))
}

pub(crate) fn unique_suffix() -> String {
    format!("{}-{}", std::process::id(), crate::random_name(8))
}

/// The lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> ServiceResult<String> {
    let mut file =
        File::open(path).map_err(|io| error(format!("cannot open `{}`: {io}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|io| error(format!("cannot read `{}`: {io}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// The lowercase hex SHA-256 of bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_known_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("abc");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(sha256_file(&path).unwrap(), sha256_hex(b"abc"));
    }

    #[test]
    fn a_cached_download_is_reused_only_while_its_checksum_matches() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = Cache::new(temporary.path());
        let asset = PinnedAsset {
            version: "1".into(),
            // Never fetched: the cached copy matches.
            url: "https://example.invalid/asset.bin".into(),
            sha256: sha256_hex(b"cached"),
        };
        let directory = temporary.path().join("downloads").join(&asset.sha256);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("asset.bin"), b"cached").unwrap();
        assert_eq!(cache.download(&asset).unwrap(), directory.join("asset.bin"));
        // A tampered cache entry is discarded and fetched again, which
        // fails here because the host does not exist.
        fs::write(directory.join("asset.bin"), b"tampered").unwrap();
        assert!(cache.download(&asset).is_err());
        assert!(!directory.join("asset.bin").exists());
    }

    #[test]
    fn urls_must_name_a_file() {
        let temporary = tempfile::tempdir().unwrap();
        let asset = PinnedAsset {
            version: "1".into(),
            url: "https://example.invalid/".into(),
            sha256: sha256_hex(b""),
        };
        assert!(Cache::new(temporary.path()).download(&asset).is_err());
    }
}

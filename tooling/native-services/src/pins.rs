//! The native service pins, read from `infra/versions.lock`.
//!
//! The Docker lab and these native services read the same typed, strictly
//! parsed lock, so a version exists once and the Linux and Windows assets
//! cannot drift apart: the lock rejects a Windows URL that does not encode
//! the same version as its Linux sibling.

use crate::{ServiceResult, error};
use afk_lab::VersionsLock;
use std::path::Path;

/// The platforms the native services support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    WindowsX64,
    LinuxX64,
}

impl Platform {
    pub fn host() -> ServiceResult<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => Ok(Self::WindowsX64),
            ("linux", "x86_64") => Ok(Self::LinuxX64),
            (os, arch) => Err(error(format!(
                "native PocketBase and MinIO services are pinned for windows/x86_64 and linux/x86_64, not {os}/{arch}"
            ))),
        }
    }

    pub fn executable(self, name: &str) -> String {
        match self {
            Self::WindowsX64 => format!("{name}.exe"),
            Self::LinuxX64 => name.to_owned(),
        }
    }

    pub(crate) fn goos(self) -> &'static str {
        match self {
            Self::WindowsX64 => "windows",
            Self::LinuxX64 => "linux",
        }
    }
}

/// A downloadable archive with its expected SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedAsset {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoToolchain {
    pub archive: PinnedAsset,
}

/// The exact MinIO source and the build stamps the Docker lab also uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinioSource {
    pub archive: PinnedAsset,
    pub commit: String,
    pub short_commit: String,
    pub build_version: String,
    pub copyright_year: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePins {
    pub platform: Platform,
    pub pocketbase: PinnedAsset,
    pub go: GoToolchain,
    pub minio: MinioSource,
}

impl NativePins {
    /// Loads and validates the repository lock and selects the host assets.
    pub fn load(repository: &Path) -> ServiceResult<Self> {
        let lock = VersionsLock::load(
            &repository.join("infra/versions.lock"),
            &repository.join("rust-toolchain.toml"),
        )
        .map_err(|lab| error(format!("infra/versions.lock: {lab}")))?;
        Ok(Self::select(&lock, Platform::host()?))
    }

    /// Picks the assets for `platform` from a validated lock.
    pub fn select(lock: &VersionsLock, platform: Platform) -> Self {
        let pocketbase = &lock.components.pocketbase;
        let toolchain = &lock.toolchain;
        let minio = &lock.components.minio;
        let (pocketbase_url, pocketbase_sha256, go_url, go_sha256) = match platform {
            Platform::WindowsX64 => (
                &pocketbase.windows_amd64_url,
                &pocketbase.windows_amd64_sha256,
                &toolchain.go_windows_x64_url,
                &toolchain.go_windows_x64_sha256,
            ),
            Platform::LinuxX64 => (
                &pocketbase.url,
                &pocketbase.sha256,
                &toolchain.go_url,
                &toolchain.go_linux_x64_sha256,
            ),
        };
        Self {
            platform,
            pocketbase: PinnedAsset {
                version: pocketbase.version.clone(),
                url: pocketbase_url.clone(),
                sha256: pocketbase_sha256.clone(),
            },
            go: GoToolchain {
                archive: PinnedAsset {
                    version: toolchain.go.clone(),
                    url: go_url.clone(),
                    sha256: go_sha256.clone(),
                },
            },
            minio: MinioSource {
                archive: PinnedAsset {
                    version: minio.version.clone(),
                    url: minio.source_url.clone(),
                    sha256: minio.source_sha256.clone(),
                },
                commit: minio.commit.clone(),
                short_commit: minio.short_commit.clone(),
                build_version: minio.build_version.clone(),
                copyright_year: minio.copyright_year.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository_root;

    #[test]
    fn windows_and_linux_pins_share_one_version() {
        let root = repository_root();
        let lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        let windows = NativePins::select(&lock, Platform::WindowsX64);
        let linux = NativePins::select(&lock, Platform::LinuxX64);
        assert_eq!(windows.pocketbase.version, linux.pocketbase.version);
        assert_eq!(windows.go.archive.version, linux.go.archive.version);
        assert_eq!(windows.minio, linux.minio);
        assert!(windows.pocketbase.url.ends_with("_windows_amd64.zip"));
        assert!(linux.pocketbase.url.ends_with("_linux_amd64.zip"));
        assert_ne!(windows.pocketbase.sha256, linux.pocketbase.sha256);
        assert!(windows.go.archive.url.ends_with(".windows-amd64.zip"));
        assert_eq!(linux.go.archive.url, lock.toolchain.go_url);
        // The Linux asset is the one the Docker lab already verifies.
        assert_eq!(linux.pocketbase.sha256, lock.components.pocketbase.sha256);
    }

    #[test]
    fn host_pins_load_from_the_repository() {
        if let Ok(platform) = Platform::host() {
            let pins = NativePins::load(&repository_root()).unwrap();
            assert_eq!(pins.platform, platform);
        }
    }
}

use crate::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionsLock {
    pub schema: u32,
    pub locked_at: String,
    pub toolchain: ToolchainLock,
    pub images: ImageLocks,
    pub components: ComponentLocks,
    pub ubuntu_gui: UbuntuGuiLock,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainLock {
    pub rust: String,
    pub rust_toolchain_dir: String,
    pub node: String,
    pub npm: String,
    pub node_linux_x64_sha256: String,
    pub node_url: String,
    pub tauri_cli: String,
    pub tauri_cli_integrity: String,
    pub tauri_cli_sha512: String,
    pub tauri_cli_url: String,
    pub tauri_cli_linux_x64_gnu_integrity: String,
    pub tauri_cli_linux_x64_gnu_sha512: String,
    pub tauri_cli_linux_x64_gnu_url: String,
    pub debian_snapshot: String,
    pub webkit2gtk_driver: String,
    pub libwebkit2gtk: String,
    pub xvfb: String,
    pub runner_uid: u32,
    pub runner_gid: u32,
    pub go: String,
    pub go_linux_x64_sha256: String,
    pub go_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageLocks {
    pub kicad: ImageLock,
    pub busybox: ImageLock,
    pub rust: ImageLock,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageLock {
    pub version: String,
    pub reference: String,
    pub digest: String,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentLocks {
    pub pocketbase: PocketBaseLock,
    pub minio: MinioLock,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PocketBaseLock {
    pub version: String,
    pub url: String,
    pub checksums_url: String,
    pub release_api: String,
    pub asset_digest: String,
    pub sha256: String,
    pub expected_output: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MinioLock {
    pub version: String,
    pub commit: String,
    pub short_commit: String,
    pub build_version: String,
    pub copyright_year: String,
    pub source_url: String,
    pub source_sha256: String,
    pub release_url: String,
    pub registry_status: String,
    pub expected_arch: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UbuntuGuiLock {
    pub runner: String,
    pub ppa: String,
    pub packages_index: String,
    pub ubuntu_packages_source: String,
    pub kicad_package: UbuntuPackagePin,
    pub stock_packages: Vec<UbuntuPackagePin>,
    pub webkit_library: UbuntuPackagePin,
    pub webkit_driver: UbuntuPackagePin,
    pub xvfb: UbuntuPackagePin,
    pub x11_utils: UbuntuPackagePin,
    pub imagemagick: UbuntuPackagePin,
    pub coredump_tool: UbuntuPackagePin,
    pub window_manager: UbuntuPackagePin,
    pub stock_symbol: String,
    pub stock_footprint: String,
    pub stock_3d_model: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UbuntuPackagePin {
    pub name: String,
    pub version: String,
}

impl VersionsLock {
    pub fn load(path: &Path, rust_toolchain_path: &Path) -> LabResult<Self> {
        let text = fs::read_to_string(path)
            .map_err(|error| LabError(format!("cannot read `{}`: {error}", path.display())))?;
        let lock: Self = toml::from_str(&text)?;
        lock.validate()?;
        let toolchain_text = fs::read_to_string(rust_toolchain_path).map_err(|error| {
            LabError(format!(
                "cannot read `{}`: {error}",
                rust_toolchain_path.display()
            ))
        })?;
        let toolchain: toml::Value = toml::from_str(&toolchain_text)?;
        let channel = toolchain
            .get("toolchain")
            .and_then(|value| value.get("channel"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| LabError("rust-toolchain.toml has no toolchain.channel".into()))?;
        if channel != lock.toolchain.rust {
            return Err(LabError(format!(
                "rust-toolchain.toml `{channel}` does not match versions.lock `{}`",
                lock.toolchain.rust
            )));
        }
        Ok(lock)
    }

    pub fn validate(&self) -> LabResult<()> {
        if self.schema != 1 {
            return Err(LabError("versions.lock schema must be 1".into()));
        }
        if self.locked_at.len() != 10
            || self.locked_at.as_bytes().get(4) != Some(&b'-')
            || self.locked_at.as_bytes().get(7) != Some(&b'-')
        {
            return Err(LabError(
                "versions.lock locked_at must be YYYY-MM-DD".into(),
            ));
        }
        let required = self.required_strings();
        for (field, value) in &required {
            if value.trim().is_empty() {
                return Err(LabError(format!("versions.lock `{field}` is empty")));
            }
            let lower = value.to_ascii_lowercase();
            if lower == "latest" || lower.contains(":latest") || lower.contains("@main") {
                return Err(LabError(format!("versions.lock `{field}` is floating")));
            }
        }
        for (field, value) in [
            (
                "toolchain.node_linux_x64_sha256",
                self.toolchain.node_linux_x64_sha256.as_str(),
            ),
            (
                "toolchain.go_linux_x64_sha256",
                self.toolchain.go_linux_x64_sha256.as_str(),
            ),
            (
                "toolchain.tauri_cli_sha512",
                self.toolchain.tauri_cli_sha512.as_str(),
            ),
            (
                "toolchain.tauri_cli_linux_x64_gnu_sha512",
                self.toolchain.tauri_cli_linux_x64_gnu_sha512.as_str(),
            ),
            (
                "components.pocketbase.sha256",
                self.components.pocketbase.sha256.as_str(),
            ),
            (
                "components.minio.source_sha256",
                self.components.minio.source_sha256.as_str(),
            ),
        ] {
            let length = if field.ends_with("sha512") { 128 } else { 64 };
            validate_hex(value, length, field)?;
        }
        for (name, image) in [
            ("images.kicad", &self.images.kicad),
            ("images.busybox", &self.images.busybox),
            ("images.rust", &self.images.rust),
        ] {
            validate_digest(&image.digest, &format!("{name}.digest"))?;
            if !image.reference.ends_with(&format!("@{}", image.digest)) {
                return Err(LabError(format!(
                    "{name}.reference is not pinned to its declared digest"
                )));
            }
            if !image.source.starts_with("https://") {
                return Err(LabError(format!("{name}.source must use HTTPS")));
            }
        }
        validate_digest(
            &self.components.pocketbase.asset_digest,
            "components.pocketbase.asset_digest",
        )?;
        if self.components.pocketbase.asset_digest
            != format!("sha256:{}", self.components.pocketbase.sha256)
        {
            return Err(LabError(
                "PocketBase asset_digest and sha256 disagree".into(),
            ));
        }
        for (field, url) in required.iter().filter(|(field, _)| {
            field.ends_with("url") || field.ends_with("source") || field.ends_with("index")
        }) {
            if !url.starts_with("https://") {
                return Err(LabError(format!("versions.lock `{field}` must use HTTPS")));
            }
        }
        if self.components.minio.commit.len() != 40
            || !self
                .components
                .minio
                .commit
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
            || self.components.minio.short_commit != self.components.minio.commit[..12]
        {
            return Err(LabError("MinIO commit/short_commit is invalid".into()));
        }
        if self.toolchain.runner_uid == 0 || self.toolchain.runner_gid == 0 {
            return Err(LabError("runner UID/GID must be non-root".into()));
        }
        if self.ubuntu_gui.runner != "ubuntu-24.04" {
            return Err(LabError(
                "Ubuntu GUI runner must remain ubuntu-24.04".into(),
            ));
        }
        let expected_packages = [
            ("kicad", &self.ubuntu_gui.kicad_package),
            ("libwebkit2gtk-4.1-0", &self.ubuntu_gui.webkit_library),
            ("webkit2gtk-driver", &self.ubuntu_gui.webkit_driver),
            ("xvfb", &self.ubuntu_gui.xvfb),
            ("x11-utils", &self.ubuntu_gui.x11_utils),
            ("imagemagick-6.q16", &self.ubuntu_gui.imagemagick),
            ("systemd-coredump", &self.ubuntu_gui.coredump_tool),
            ("openbox", &self.ubuntu_gui.window_manager),
        ];
        for (expected_name, package) in expected_packages {
            validate_ubuntu_package(package, expected_name)?;
        }
        let wanted_stock = ["kicad-symbols", "kicad-footprints", "kicad-packages3d"];
        if self.ubuntu_gui.stock_packages.len() != wanted_stock.len() {
            return Err(LabError(
                "Ubuntu GUI stock package lock must contain exactly three packages".into(),
            ));
        }
        for (package, expected_name) in self.ubuntu_gui.stock_packages.iter().zip(wanted_stock) {
            validate_ubuntu_package(package, expected_name)?;
            if package.version != self.ubuntu_gui.kicad_package.version {
                return Err(LabError(format!(
                    "Ubuntu GUI stock package `{expected_name}` must match the KiCad package version"
                )));
            }
        }
        if self.ubuntu_gui.webkit_driver.version != self.ubuntu_gui.webkit_library.version {
            return Err(LabError(
                "Ubuntu GUI WebKit driver and library versions must match".into(),
            ));
        }
        let expected_kicad_gui = format!("{}~ubuntu24.04.1", self.images.kicad.version);
        if self.ubuntu_gui.kicad_package.version != expected_kicad_gui
            || self.ubuntu_gui.webkit_library.version != "2.52.3-0ubuntu0.24.04.1"
            || self.ubuntu_gui.xvfb.version != "2:21.1.12-1ubuntu1.5"
            || self.ubuntu_gui.x11_utils.version != "7.7+6build2"
            || self.ubuntu_gui.imagemagick.version != "8:6.9.12.98+dfsg1-5.2build2"
            || self.ubuntu_gui.coredump_tool.version != "255.4-1ubuntu8.16"
            || self.ubuntu_gui.window_manager.version != "3.6.1-12build5"
            || self.ubuntu_gui.ubuntu_packages_source != "https://packages.ubuntu.com/noble/amd64"
        {
            return Err(LabError(
                "Ubuntu GUI package snapshot differs from authoritative Noble/PPA locks".into(),
            ));
        }
        for (artifact, integrity, sha512) in [
            (
                "Tauri CLI wrapper",
                self.toolchain.tauri_cli_integrity.as_str(),
                self.toolchain.tauri_cli_sha512.as_str(),
            ),
            (
                "Tauri CLI Linux x64 GNU binding",
                self.toolchain.tauri_cli_linux_x64_gnu_integrity.as_str(),
                self.toolchain.tauri_cli_linux_x64_gnu_sha512.as_str(),
            ),
        ] {
            if !integrity.starts_with("sha512-") {
                return Err(LabError(format!("{artifact} npm integrity must be sha512")));
            }
            let expected = format!("sha512-{}", base64_encode(&hex_bytes(sha512)?));
            if integrity != expected {
                return Err(LabError(format!(
                    "{artifact} npm integrity and SHA-512 hex disagree"
                )));
            }
        }
        for (field, url, needle) in [
            (
                "toolchain.node_url",
                self.toolchain.node_url.as_str(),
                format!("/v{0}/node-v{0}-linux-x64.tar.xz", self.toolchain.node),
            ),
            (
                "toolchain.tauri_cli_url",
                self.toolchain.tauri_cli_url.as_str(),
                format!("cli-{}.tgz", self.toolchain.tauri_cli),
            ),
            (
                "toolchain.tauri_cli_linux_x64_gnu_url",
                self.toolchain.tauri_cli_linux_x64_gnu_url.as_str(),
                format!("cli-linux-x64-gnu-{}.tgz", self.toolchain.tauri_cli),
            ),
            (
                "toolchain.go_url",
                self.toolchain.go_url.as_str(),
                format!("go{}.linux-amd64.tar.gz", self.toolchain.go),
            ),
            (
                "components.pocketbase.url",
                self.components.pocketbase.url.as_str(),
                format!(
                    "/v{0}/pocketbase_{0}_linux_amd64.zip",
                    self.components.pocketbase.version
                ),
            ),
            (
                "components.minio.source_url",
                self.components.minio.source_url.as_str(),
                self.components.minio.commit.clone(),
            ),
        ] {
            if !url.contains(&needle) {
                return Err(LabError(format!(
                    "versions.lock `{field}` does not encode its locked version/commit"
                )));
            }
        }
        for (name, image) in [
            ("kicad", &self.images.kicad),
            ("busybox", &self.images.busybox),
            ("rust", &self.images.rust),
        ] {
            if !image.reference.contains(&image.version) {
                return Err(LabError(format!(
                    "images.{name}.reference does not encode its version"
                )));
            }
        }
        if self.toolchain.rust_toolchain_dir
            != format!("{}-x86_64-unknown-linux-gnu", self.toolchain.rust)
            || self.toolchain.debian_snapshot.len() != 16
            || !self.toolchain.debian_snapshot.ends_with('Z')
            || !self
                .ubuntu_gui
                .kicad_package
                .version
                .starts_with(&self.images.kicad.version)
        {
            return Err(LabError(
                "toolchain directory, snapshot, or GUI package version is inconsistent".into(),
            ));
        }
        let release_from_build = format!(
            "RELEASE.{}",
            self.components.minio.build_version.replace(':', "-")
        );
        if self.components.minio.version != release_from_build {
            return Err(LabError(
                "MinIO build_version and release version disagree".into(),
            ));
        }
        for path in [
            &self.ubuntu_gui.stock_symbol,
            &self.ubuntu_gui.stock_footprint,
            &self.ubuntu_gui.stock_3d_model,
        ] {
            if !path.starts_with("/usr/share/kicad/")
                || !path
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "/._-".contains(ch))
            {
                return Err(LabError(
                    "stock assertion path is outside /usr/share/kicad or unsafe".into(),
                ));
            }
        }
        Ok(())
    }

    fn required_strings(&self) -> BTreeMap<&'static str, &str> {
        BTreeMap::from([
            ("toolchain.rust", self.toolchain.rust.as_str()),
            (
                "toolchain.rust_toolchain_dir",
                self.toolchain.rust_toolchain_dir.as_str(),
            ),
            ("toolchain.node", self.toolchain.node.as_str()),
            ("toolchain.npm", self.toolchain.npm.as_str()),
            ("toolchain.node_url", self.toolchain.node_url.as_str()),
            ("toolchain.tauri_cli", self.toolchain.tauri_cli.as_str()),
            (
                "toolchain.tauri_cli_url",
                self.toolchain.tauri_cli_url.as_str(),
            ),
            (
                "toolchain.tauri_cli_linux_x64_gnu_url",
                self.toolchain.tauri_cli_linux_x64_gnu_url.as_str(),
            ),
            (
                "toolchain.debian_snapshot",
                self.toolchain.debian_snapshot.as_str(),
            ),
            (
                "toolchain.webkit2gtk_driver",
                self.toolchain.webkit2gtk_driver.as_str(),
            ),
            (
                "toolchain.libwebkit2gtk",
                self.toolchain.libwebkit2gtk.as_str(),
            ),
            ("toolchain.xvfb", self.toolchain.xvfb.as_str()),
            ("toolchain.go", self.toolchain.go.as_str()),
            ("toolchain.go_url", self.toolchain.go_url.as_str()),
            (
                "components.pocketbase.version",
                self.components.pocketbase.version.as_str(),
            ),
            (
                "components.pocketbase.url",
                self.components.pocketbase.url.as_str(),
            ),
            (
                "components.pocketbase.checksums_url",
                self.components.pocketbase.checksums_url.as_str(),
            ),
            (
                "components.pocketbase.release_api",
                self.components.pocketbase.release_api.as_str(),
            ),
            (
                "components.pocketbase.expected_output",
                self.components.pocketbase.expected_output.as_str(),
            ),
            (
                "components.minio.version",
                self.components.minio.version.as_str(),
            ),
            (
                "components.minio.build_version",
                self.components.minio.build_version.as_str(),
            ),
            (
                "components.minio.copyright_year",
                self.components.minio.copyright_year.as_str(),
            ),
            (
                "components.minio.source_url",
                self.components.minio.source_url.as_str(),
            ),
            (
                "components.minio.release_url",
                self.components.minio.release_url.as_str(),
            ),
            (
                "components.minio.registry_status",
                self.components.minio.registry_status.as_str(),
            ),
            (
                "components.minio.expected_arch",
                self.components.minio.expected_arch.as_str(),
            ),
            ("ubuntu_gui.runner", self.ubuntu_gui.runner.as_str()),
            ("ubuntu_gui.ppa", self.ubuntu_gui.ppa.as_str()),
            (
                "ubuntu_gui.packages_index",
                self.ubuntu_gui.packages_index.as_str(),
            ),
            (
                "ubuntu_gui.ubuntu_packages_source",
                self.ubuntu_gui.ubuntu_packages_source.as_str(),
            ),
            (
                "ubuntu_gui.kicad_package.name",
                self.ubuntu_gui.kicad_package.name.as_str(),
            ),
            (
                "ubuntu_gui.kicad_package.version",
                self.ubuntu_gui.kicad_package.version.as_str(),
            ),
            (
                "ubuntu_gui.webkit_library.name",
                self.ubuntu_gui.webkit_library.name.as_str(),
            ),
            (
                "ubuntu_gui.webkit_library.version",
                self.ubuntu_gui.webkit_library.version.as_str(),
            ),
            (
                "ubuntu_gui.webkit_driver.name",
                self.ubuntu_gui.webkit_driver.name.as_str(),
            ),
            (
                "ubuntu_gui.webkit_driver.version",
                self.ubuntu_gui.webkit_driver.version.as_str(),
            ),
            ("ubuntu_gui.xvfb.name", self.ubuntu_gui.xvfb.name.as_str()),
            (
                "ubuntu_gui.xvfb.version",
                self.ubuntu_gui.xvfb.version.as_str(),
            ),
            (
                "ubuntu_gui.x11_utils.name",
                self.ubuntu_gui.x11_utils.name.as_str(),
            ),
            (
                "ubuntu_gui.x11_utils.version",
                self.ubuntu_gui.x11_utils.version.as_str(),
            ),
            (
                "ubuntu_gui.imagemagick.name",
                self.ubuntu_gui.imagemagick.name.as_str(),
            ),
            (
                "ubuntu_gui.imagemagick.version",
                self.ubuntu_gui.imagemagick.version.as_str(),
            ),
            (
                "ubuntu_gui.coredump_tool.name",
                self.ubuntu_gui.coredump_tool.name.as_str(),
            ),
            (
                "ubuntu_gui.coredump_tool.version",
                self.ubuntu_gui.coredump_tool.version.as_str(),
            ),
            (
                "ubuntu_gui.window_manager.name",
                self.ubuntu_gui.window_manager.name.as_str(),
            ),
            (
                "ubuntu_gui.window_manager.version",
                self.ubuntu_gui.window_manager.version.as_str(),
            ),
            (
                "ubuntu_gui.stock_symbol",
                self.ubuntu_gui.stock_symbol.as_str(),
            ),
            (
                "ubuntu_gui.stock_footprint",
                self.ubuntu_gui.stock_footprint.as_str(),
            ),
            (
                "ubuntu_gui.stock_3d_model",
                self.ubuntu_gui.stock_3d_model.as_str(),
            ),
        ])
    }
}

fn validate_ubuntu_package(package: &UbuntuPackagePin, expected_name: &str) -> LabResult<()> {
    if package.name != expected_name
        || !package
            .name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ".+-".contains(ch))
    {
        return Err(LabError(format!(
            "Ubuntu GUI package name must be exactly `{expected_name}`"
        )));
    }
    if package.version.is_empty()
        || package.version.eq_ignore_ascii_case("latest")
        || !package
            .version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ".+~:-".contains(ch))
    {
        return Err(LabError(format!(
            "Ubuntu GUI package `{expected_name}` has an unsafe or floating version"
        )));
    }
    Ok(())
}

fn hex_bytes(value: &str) -> LabResult<Vec<u8>> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|error| LabError(error.to_string()))?;
            u8::from_str_radix(text, 16).map_err(|error| LabError(error.to_string()))
        })
        .collect()
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let value = (a << 16) | (b << 8) | c;
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

pub fn validate_hex(value: &str, length: usize, field: &str) -> LabResult<()> {
    if value.len() != length || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(LabError(format!(
            "`{field}` must be {length} hex characters"
        )));
    }
    Ok(())
}

pub fn validate_digest(value: &str, field: &str) -> LabResult<()> {
    let digest = value
        .strip_prefix("sha256:")
        .ok_or_else(|| LabError(format!("`{field}` must be a sha256 digest")))?;
    validate_hex(digest, 64, field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn repository() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn checked_in_lock_is_typed_exhaustive_and_consistent() {
        let root = repository();
        VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
    }

    #[test]
    fn unknown_or_missing_lock_fields_fail_closed() {
        let root = repository();
        let text = fs::read_to_string(root.join("infra/versions.lock")).unwrap();
        let unknown = format!("{text}\nunexpected = \"value\"\n");
        assert!(toml::from_str::<VersionsLock>(&unknown).is_err());
        let missing = text.replace("npm = \"11.16.0\"\n", "");
        assert!(toml::from_str::<VersionsLock>(&missing).is_err());
    }

    #[test]
    fn every_ubuntu_gui_package_pin_is_required_and_snapshot_exact() {
        let root = repository();
        let text = fs::read_to_string(root.join("infra/versions.lock")).unwrap();
        for line_prefix in [
            "kicad_package = ",
            "webkit_library = ",
            "webkit_driver = ",
            "xvfb = ",
            "x11_utils = ",
            "imagemagick = ",
            "coredump_tool = ",
            "window_manager = ",
        ] {
            let filtered = text
                .lines()
                .filter(|line| !line.starts_with(line_prefix))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                toml::from_str::<VersionsLock>(&filtered).is_err(),
                "accepted missing {line_prefix}"
            );
        }
        for stock_name in ["kicad-symbols", "kicad-footprints", "kicad-packages3d"] {
            let filtered = text
                .lines()
                .filter(|line| !line.contains(&format!("name = \"{stock_name}\"")))
                .collect::<Vec<_>>()
                .join("\n");
            let lock: VersionsLock = toml::from_str(&filtered).unwrap();
            assert!(lock.validate().is_err(), "accepted missing {stock_name}");
        }

        for (from, to) in [
            (
                "version = \"2.52.3-0ubuntu0.24.04.1\"",
                "version = \"2.44.0-2\"",
            ),
            (
                "version = \"2:21.1.12-1ubuntu1.5\"",
                "version = \"2:21.1.12-1ubuntu1\"",
            ),
            ("version = \"7.7+6build2\"", "version = \"7.7+5build2\""),
            (
                "version = \"8:6.9.12.98+dfsg1-5.2build2\"",
                "version = \"8:6.9.12.98+dfsg1-5.2build1\"",
            ),
            (
                "version = \"255.4-1ubuntu8.16\"",
                "version = \"255.4-1ubuntu8.15\"",
            ),
            (
                "version = \"3.6.1-12build5\"",
                "version = \"3.6.1-12build4\"",
            ),
            ("name = \"openbox\"", "name = \"open-box\""),
            (
                "name = \"systemd-coredump\"",
                "name = \"systemd-core-dump\"",
            ),
            ("name = \"kicad-symbols\"", "name = \"kicad-symbol\""),
        ] {
            let changed = text.replace(from, to);
            let lock: VersionsLock = toml::from_str(&changed).unwrap();
            assert!(lock.validate().is_err(), "accepted changed pin `{from}`");
        }
    }

    #[test]
    fn checksum_integrity_commit_and_reference_mismatches_fail() {
        let root = repository();
        let mut lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        lock.toolchain.node_linux_x64_sha256 = "short".into();
        assert!(lock.validate().is_err());
        let mut lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        lock.toolchain.tauri_cli_integrity = "sha512-wrong".into();
        assert!(lock.validate().is_err());
        let mut lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        lock.toolchain.tauri_cli_linux_x64_gnu_integrity = "sha512-wrong".into();
        assert!(lock.validate().is_err());
        let mut lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        lock.toolchain.tauri_cli_linux_x64_gnu_sha512 = "short".into();
        assert!(lock.validate().is_err());
        let mut lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        lock.toolchain.tauri_cli_linux_x64_gnu_url = lock
            .toolchain
            .tauri_cli_linux_x64_gnu_url
            .replace("2.11.4", "2.11.3");
        assert!(lock.validate().is_err());
        let mut lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        lock.components.minio.short_commit = "000000000000".into();
        assert!(lock.validate().is_err());
        let mut lock = VersionsLock::load(
            &root.join("infra/versions.lock"),
            &root.join("rust-toolchain.toml"),
        )
        .unwrap();
        lock.images.kicad.reference = "docker.io/kicad/kicad:latest".into();
        assert!(lock.validate().is_err());
    }

    #[test]
    fn rust_toolchain_file_mismatch_is_rejected() {
        let root = repository();
        let temporary = tempdir().unwrap();
        let toolchain = temporary.path().join("rust-toolchain.toml");
        fs::write(&toolchain, "[toolchain]\nchannel = \"1.84.0\"\n").unwrap();
        assert!(VersionsLock::load(&root.join("infra/versions.lock"), &toolchain).is_err());
    }
}

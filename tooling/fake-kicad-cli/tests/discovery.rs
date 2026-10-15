//! Version handling in kicad-integration's discovery, driven by the fake
//! kicad-cli this package builds.

use kicad_integration::{
    CliSettingsFolder, Discovery, DiscoveryOptions, SearchLocations, discover,
};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn an_older_kicad_is_reported_as_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), Some("9.0.7"));
    let options = DiscoveryOptions {
        user_override: Some(fake.clone()),
        locations: SearchLocations::Standard,
        settings: CliSettingsFolder::new(dir.path().join("settings")),
    };
    match discover(&options) {
        Discovery::Unsupported { cli, version, .. } => {
            assert_eq!(cli, fake);
            assert_eq!(version.to_string(), "9.0.7");
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

/// The fake kicad-cli, copied into `dir` under the real file name, reporting
/// `version` (or failing like a damaged install when `None`).
fn fake_cli(dir: &Path, version: Option<&str>) -> PathBuf {
    let name = if cfg!(windows) {
        "kicad-cli.exe"
    } else {
        "kicad-cli"
    };
    let path = dir.join(name);
    fs::copy(env!("CARGO_BIN_EXE_fake-kicad-cli"), &path).unwrap();
    if let Some(version) = version {
        fs::write(dir.join("fake-version.txt"), version).unwrap();
    }
    path
}

#[test]
fn a_kicad_cli_that_cannot_report_its_version_is_broken() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), None);
    let options = DiscoveryOptions {
        user_override: Some(dir.path().to_path_buf()),
        locations: SearchLocations::Only(Vec::new()),
        settings: CliSettingsFolder::new(dir.path().join("settings")),
    };
    match discover(&options) {
        Discovery::Broken { cli, message, .. } => {
            assert_eq!(cli, fake);
            assert!(message.contains("damaged"), "{message}");
        }
        other => panic!("expected Broken, got {other:?}"),
    }
}

#[test]
fn a_kicad_nightly_is_reported_as_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    fake_cli(dir.path(), Some("10.99.0-1234-gdeadbeef"));
    let options = DiscoveryOptions {
        user_override: Some(dir.path().to_path_buf()),
        locations: SearchLocations::Only(Vec::new()),
        settings: CliSettingsFolder::new(dir.path().join("settings")),
    };
    assert!(matches!(discover(&options), Discovery::Unsupported { .. }));
}

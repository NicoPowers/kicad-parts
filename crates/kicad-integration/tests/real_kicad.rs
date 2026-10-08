//! These tests run the installed KiCad 10 kicad-cli. KiCad is a required
//! dependency for development and CI (see docs/operations).

use kicad_integration::{
    CacheOutcome, Discovery, DiscoveryOptions, InstallSource, KicadInstall, LibraryOrigin,
    LibraryResolver, PreviewCache, PreviewError, SearchLocations, discover,
};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .canonicalize()
        .unwrap()
}

fn installed_kicad() -> KicadInstall {
    match discover(&DiscoveryOptions::default()) {
        Discovery::Ready(install) => install,
        other => panic!(
            "KiCad 10 is required for these tests; install it from https://www.kicad.org/download/ ({other:?})"
        ),
    }
}

fn resolver(kicad: &KicadInstall) -> LibraryResolver {
    LibraryResolver::new(Some(repo_root()), kicad.share_dir.clone())
}

#[test]
fn finds_installed_kicad_10_in_a_standard_location() {
    let kicad = installed_kicad();
    assert_eq!(kicad.version.major, 10);
    assert_eq!(kicad.source, InstallSource::Discovered);
    let share = kicad
        .share_dir
        .expect("bundled libraries next to kicad-cli");
    assert!(share.join("symbols").join("Device.kicad_sym").is_file());
}

#[test]
fn user_override_accepts_the_cli_its_bin_directory_or_install_root() {
    let kicad = installed_kicad();
    let bin = kicad.cli.parent().unwrap().to_path_buf();
    let root = bin.parent().unwrap().to_path_buf();
    for path in [kicad.cli.clone(), bin, root] {
        let options = DiscoveryOptions {
            user_override: Some(path.clone()),
            locations: SearchLocations::Only(Vec::new()),
        };
        match discover(&options) {
            Discovery::Ready(found) => {
                assert_eq!(found.source, InstallSource::UserOverride, "{path:?}");
                assert_eq!(found.version, kicad.version);
            }
            other => panic!("override {path:?} not accepted: {other:?}"),
        }
    }
}

#[test]
fn search_roots_shaped_like_program_files_find_kicad() {
    let kicad = installed_kicad();
    // <ProgramFiles>\KiCad\10.0\bin\kicad-cli.exe -> <ProgramFiles>
    let program_files = kicad.cli.ancestors().nth(4).unwrap().to_path_buf();
    let options = DiscoveryOptions {
        user_override: None,
        locations: SearchLocations::Only(vec![program_files]),
    };
    assert!(matches!(discover(&options), Discovery::Ready(_)));
}

#[test]
fn an_older_kicad_is_reported_as_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "9.0.7");
    let options = DiscoveryOptions {
        user_override: Some(fake.clone()),
        locations: SearchLocations::Standard,
    };
    match discover(&options) {
        Discovery::Unsupported { cli, version, .. } => {
            assert_eq!(cli, fake);
            assert_eq!(version.to_string(), "9.0.7");
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[cfg(windows)]
fn fake_cli(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join("kicad-cli.cmd");
    fs::write(&path, format!("@echo {version}\r\n")).unwrap();
    path
}

#[cfg(not(windows))]
fn fake_cli(dir: &Path, version: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("kicad-cli");
    fs::write(&path, format!("#!/bin/sh\necho {version}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn exports_every_symbol_unit_once_then_serves_the_cache() {
    let kicad = installed_kicad();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let symbol = resolver(&kicad).symbol("g-mpu:JETSON_NANO").unwrap();
    assert_eq!(symbol.origin, LibraryOrigin::Project);

    let first = cache.symbol(&kicad, &symbol).unwrap();
    assert_eq!(first.cache, CacheOutcome::Exported);
    let units: Vec<_> = first.images.iter().map(|image| image.unit).collect();
    assert_eq!(units, [Some(1), Some(2), Some(3)]);
    for image in &first.images {
        assert!(image.svg.contains("<svg"), "{:?}", image.file);
        assert!(image.file.starts_with(cache_dir.path()));
    }
    let library_bytes = fs::read(&symbol.library).unwrap();
    assert_eq!(first.library_sha256.len(), 64);
    assert!(!library_bytes.is_empty());
    let entry = first.images[0].file.to_string_lossy().into_owned();
    assert!(
        entry.contains(&format!("kicad-{}", kicad.version)),
        "{entry}"
    );
    assert!(entry.contains(&first.library_sha256), "{entry}");

    let second = cache.symbol(&kicad, &symbol).unwrap();
    assert_eq!(second.cache, CacheOutcome::Hit);
    assert_eq!(second.images, first.images);
    assert_eq!(
        cache.export_count(),
        1,
        "a repeat view must not run kicad-cli"
    );
}

#[test]
fn exports_project_and_bundled_footprints() {
    let kicad = installed_kicad();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let resolver = resolver(&kicad);
    for (reference, origin) in [
        ("g-res:R_0603_1608Metric", LibraryOrigin::Project),
        (
            "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
            LibraryOrigin::KicadBundled,
        ),
    ] {
        let footprint = resolver.footprint(reference).unwrap();
        assert_eq!(footprint.origin, origin);
        let preview = cache.footprint(&kicad, &footprint).unwrap();
        assert_eq!(preview.cache, CacheOutcome::Exported);
        assert_eq!(preview.images.len(), 1, "{reference}");
        assert!(preview.images[0].svg.contains("<svg"));
        assert_eq!(
            cache.footprint(&kicad, &footprint).unwrap().cache,
            CacheOutcome::Hit
        );
    }
    assert_eq!(cache.export_count(), 2);
}

#[test]
fn a_bundled_symbol_resolves_against_the_installed_kicad() {
    let kicad = installed_kicad();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let symbol = resolver(&kicad).symbol("Device:R").unwrap();
    assert_eq!(symbol.origin, LibraryOrigin::KicadBundled);
    let preview = cache.symbol(&kicad, &symbol).unwrap();
    assert_eq!(preview.images.len(), 1);
}

#[test]
fn a_failed_export_carries_the_kicad_cli_message_and_is_not_cached() {
    let kicad = installed_kicad();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let symbol = resolver(&kicad).symbol("g-pas:NO_SUCH_SYMBOL").unwrap();
    for attempt in 1..=2 {
        match cache.symbol(&kicad, &symbol) {
            Err(PreviewError::ExportFailed {
                command,
                cli_message,
            }) => {
                assert!(command.starts_with("kicad-cli sym export svg"), "{command}");
                assert!(
                    cli_message.contains("no symbol selected"),
                    "unexpected kicad-cli message: {cli_message}"
                );
            }
            other => panic!("expected ExportFailed, got {other:?}"),
        }
        assert_eq!(cache.export_count(), attempt);
    }
}

#[test]
fn editing_a_library_changes_the_cache_key() {
    let kicad = installed_kicad();
    let project = tempfile::tempdir().unwrap();
    let pretty = project.path().join("footprints").join("t.pretty");
    fs::create_dir_all(&pretty).unwrap();
    let source = repo_root()
        .join("footprints")
        .join("g-res.pretty")
        .join("R_0603_1608Metric.kicad_mod");
    let target = pretty.join("R_0603_1608Metric.kicad_mod");
    fs::copy(&source, &target).unwrap();

    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let resolver = LibraryResolver::new(Some(project.path().to_path_buf()), None);
    let footprint = resolver.footprint("t:R_0603_1608Metric").unwrap();
    let before = cache.footprint(&kicad, &footprint).unwrap();

    let mut text = fs::read_to_string(&target).unwrap();
    text.push('\n');
    fs::write(&target, text).unwrap();
    let after = cache.footprint(&kicad, &footprint).unwrap();
    assert_ne!(before.library_sha256, after.library_sha256);
    assert_eq!(after.cache, CacheOutcome::Exported);
    assert_eq!(cache.export_count(), 2);
}

//! These tests run the installed KiCad 10 kicad-cli. KiCad is a required
//! dependency for development and CI (see docs/operations).

use kicad_integration::{
    CacheOutcome, CliSettingsFolder, Discovery, DiscoveryOptions, InstallSource, KicadInstall,
    LibraryOrigin, LibraryResolver, PreviewCache, PreviewError, SearchLocations, discover,
};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .canonicalize()
        .unwrap()
}

/// A settings folder of the tests' own, so kicad-cli never loads or saves
/// the person's KiCad settings. Every test run shares it.
fn cli_settings() -> CliSettingsFolder {
    CliSettingsFolder::new(
        std::env::temp_dir()
            .join("kicad-parts-tests")
            .join("kicad-cli-settings"),
    )
}

fn installed_kicad() -> KicadInstall {
    let options = DiscoveryOptions {
        user_override: None,
        locations: SearchLocations::Standard,
        settings: cli_settings(),
    };
    match discover(&options) {
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
            settings: cli_settings(),
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
        settings: cli_settings(),
    };
    assert!(matches!(discover(&options), Discovery::Ready(_)));
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
        ("g-Resistor_SMD:R_0603_1608Metric", LibraryOrigin::Project),
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
    let symbol = resolver(&kicad).symbol("g-res:NO_SUCH_SYMBOL").unwrap();
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
        .join("g-Resistor_SMD.pretty")
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

#[test]
fn de_morgan_body_styles_are_labelled_and_follow_their_unit() {
    let kicad = installed_kicad();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let symbol = resolver(&kicad).symbol("74xx:74LS00").unwrap();
    let preview = cache.symbol(&kicad, &symbol).unwrap();
    let order: Vec<_> = preview
        .images
        .iter()
        .map(|image| (image.unit, image.demorgan))
        .collect();
    let expected: Vec<_> = (1..=5)
        .flat_map(|unit| [(Some(unit), false), (Some(unit), true)])
        .collect();
    assert_eq!(order, expected);
    for image in &preview.images {
        let name = image.file.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("74LS00_unit"), "{name}");
        assert_eq!(name.ends_with("_demorgan.svg"), image.demorgan, "{name}");
    }
}

#[test]
fn references_match_case_exactly_like_kicad() {
    let kicad = installed_kicad();
    let resolver = resolver(&kicad);
    assert!(resolver.symbol("Device:R").is_ok());
    assert!(matches!(
        resolver.symbol("device:R"),
        Err(kicad_integration::ResolveError::LibraryNotFound { .. })
    ));
    assert!(
        resolver
            .footprint("Package_SO:SOIC-8_3.9x4.9mm_P1.27mm")
            .is_ok()
    );
    assert!(matches!(
        resolver.footprint("Package_SO:soic-8_3.9x4.9mm_p1.27mm"),
        Err(kicad_integration::ResolveError::FootprintNotFound { .. })
    ));
    assert!(matches!(
        resolver.footprint("package_so:SOIC-8_3.9x4.9mm_P1.27mm"),
        Err(kicad_integration::ResolveError::LibraryNotFound { .. })
    ));

    // A symbol name in the wrong case reaches kicad-cli, which is
    // case-sensitive and refuses it.
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let wrong_case = resolver.symbol("Device:r").unwrap();
    assert!(matches!(
        cache.symbol(&kicad, &wrong_case),
        Err(PreviewError::ExportFailed { .. })
    ));
}

#[test]
fn names_that_look_like_options_never_reach_kicad_cli() {
    let kicad = installed_kicad();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let resolver = resolver(&kicad);
    for reference in ["power:-12V", "Device:--black-and-white"] {
        let symbol = resolver.symbol(reference).unwrap();
        match cache.symbol(&kicad, &symbol) {
            Err(PreviewError::Unexportable(message)) => assert!(
                message.contains("kicad-cli cannot export names beginning with '-'"),
                "{message}"
            ),
            other => panic!("{reference}: expected Unexportable, got {other:?}"),
        }
    }
    let project = tempfile::tempdir().unwrap();
    let pretty = project.path().join("footprints").join("t.pretty");
    fs::create_dir_all(&pretty).unwrap();
    fs::write(pretty.join("-x.kicad_mod"), b"").unwrap();
    let footprint = LibraryResolver::new(Some(project.path().to_path_buf()), None)
        .footprint("t:-x")
        .unwrap();
    assert!(matches!(
        cache.footprint(&kicad, &footprint),
        Err(PreviewError::Unexportable(_))
    ));
    assert_eq!(cache.export_count(), 0);
}

#[test]
fn names_kicad_cli_writes_with_spaces_still_export() {
    // kicad-cli writes `A*B` as `A B_unit1.svg`; a synthetic library keeps
    // the repository catalog out of it.
    let kicad = installed_kicad();
    let project = tempfile::tempdir().unwrap();
    let symbols = project.path().join("symbols");
    fs::create_dir_all(&symbols).unwrap();
    let mut library = String::from(
        "(kicad_symbol_lib\n\t(version 20241209)\n\t(generator \"kicad-parts-test\")\n",
    );
    for name in ["A*B", "A?B", "A|B"] {
        library.push_str(&format!(
            "\t(symbol \"{name}\"\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\
             \t\t(property \"Reference\" \"U\" (at 0 3.81 0) (effects (font (size 1.27 1.27))))\n\
             \t\t(property \"Value\" \"{name}\" (at 0 -3.81 0) (effects (font (size 1.27 1.27))))\n\
             \t\t(symbol \"{name}_1_1\"\n\
             \t\t\t(rectangle (start -2.54 2.54) (end 2.54 -2.54) (stroke (width 0.254) (type default)) (fill (type background)))\n\
             \t\t)\n\t)\n"
        ));
    }
    library.push_str(")\n");
    fs::write(symbols.join("t.kicad_sym"), library).unwrap();

    let resolver = LibraryResolver::new(Some(project.path().to_path_buf()), None);
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    for name in ["A*B", "A?B", "A|B"] {
        let symbol = resolver.symbol(&format!("t:{name}")).unwrap();
        let preview = cache
            .symbol(&kicad, &symbol)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(preview.images.len(), 1, "{name}");
        assert_eq!(preview.images[0].unit, Some(1));
        assert_eq!(
            preview.images[0]
                .file
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "A B_unit1.svg"
        );
    }
}

/// The SVG without its `<title>`, which holds the export time.
fn drawing(svg: &str) -> String {
    svg.lines()
        .filter(|line| !line.trim_start().starts_with("<title>"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn stored_bytes_are_drawn_even_after_the_local_library_changes() {
    use kicad_integration::library_file::SymbolLibrary;
    use kicad_integration::{LibRef, StoredItem, sha256_hex};

    let kicad = installed_kicad();
    // What the team catalog stored: the R_US entry exactly as the
    // repository's library has it.
    let original = fs::read(repo_root().join("symbols").join("g-res.kicad_sym")).unwrap();
    let library = SymbolLibrary::parse(&original).unwrap();
    let kicad_integration::library_file::SymbolLookup::Found(entry) = library.find("R_US") else {
        panic!("g-res has R_US");
    };
    let stored_bytes = original[entry.span.clone()].to_vec();
    let item = StoredItem {
        lib_ref: LibRef::parse("g-res:R_US").unwrap(),
        blob_sha256: sha256_hex(&stored_bytes),
        format_version: library.version.clone(),
    };

    // A temporary copy of the library where someone has since redrawn R_US.
    let project = tempfile::tempdir().unwrap();
    let symbols = project.path().join("symbols");
    fs::create_dir_all(&symbols).unwrap();
    let text = String::from_utf8(original.clone()).unwrap();
    let start = entry.span.start;
    let redrawn = format!(
        "{}{}{}",
        &text[..start],
        text[start..entry.span.end].replacen(
            "(symbol \"R_US_0_1\"",
            "(symbol \"R_US_0_1\"\n\t\t\t(circle (center 0 0) (radius 5) (stroke (width 0.5) (type default)) (fill (type none)))",
            1
        ),
        &text[entry.span.end..]
    );
    assert_ne!(redrawn, text, "the test changes the drawing");
    fs::write(symbols.join("g-res.kicad_sym"), redrawn).unwrap();

    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let changed_local = LibraryResolver::new(Some(project.path().to_path_buf()), None)
        .symbol("g-res:R_US")
        .unwrap();
    let local = cache.symbol(&kicad, &changed_local).unwrap();
    let stored = cache
        .stored_symbol(&kicad, &item, || Ok(stored_bytes.clone()), None)
        .unwrap();
    let unchanged = cache
        .symbol(&kicad, &resolver(&kicad).symbol("g-res:R_US").unwrap())
        .unwrap();

    assert_eq!(stored.library_sha256, item.blob_sha256);
    assert_eq!(stored.images.len(), 1);
    assert_ne!(
        drawing(&stored.images[0].svg),
        drawing(&local.images[0].svg)
    );
    assert_eq!(
        drawing(&stored.images[0].svg),
        drawing(&unchanged.images[0].svg)
    );
}

#[test]
fn stored_footprints_are_drawn_from_their_bytes() {
    use kicad_integration::{LibRef, StoredItem, sha256_hex};

    let kicad = installed_kicad();
    let file = repo_root()
        .join("footprints")
        .join("g-Resistor_SMD.pretty")
        .join("R_0603_1608Metric.kicad_mod");
    let bytes = fs::read(&file).unwrap();
    let item = StoredItem {
        lib_ref: LibRef::parse("g-Resistor_SMD:R_0603_1608Metric").unwrap(),
        blob_sha256: sha256_hex(&bytes),
        format_version: None,
    };
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PreviewCache::new(cache_dir.path().to_path_buf());
    let stored = cache
        .stored_footprint(&kicad, &item, || Ok(bytes.clone()), None)
        .unwrap();
    let local = cache
        .footprint(
            &kicad,
            &resolver(&kicad)
                .footprint("g-Resistor_SMD:R_0603_1608Metric")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(stored.images.len(), 1);
    assert_eq!(
        drawing(&stored.images[0].svg),
        drawing(&local.images[0].svg)
    );
}

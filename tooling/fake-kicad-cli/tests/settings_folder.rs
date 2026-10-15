//! Every kicad-cli run keeps its settings in the folder our code chose,
//! never in the person's KiCad folders, which kicad-cli can overwrite.
//! One test, so no other test in this process reads the environment while
//! it changes it.

use kicad_integration::lib_table::is_inside;
use kicad_integration::{
    CliSettingsFolder, Discovery, DiscoveryOptions, LibRef, LibraryOrigin, PreviewCache,
    PreviewError, ResolvedSymbol, SearchLocations, discover,
};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn kicad_cli_runs_get_our_settings_folder_and_never_the_persons() {
    let person = tempfile::tempdir().unwrap();
    let theirs = [person.path().join("config"), person.path().join("cache")];
    // SAFETY: this is the only test in this process.
    unsafe {
        std::env::set_var("KICAD_CONFIG_HOME", &theirs[0]);
        std::env::set_var("KICAD_CACHE_HOME", &theirs[1]);
    }

    let app = tempfile::tempdir().unwrap();
    let bin = app.path().join("bin");
    let cli = fake_cli(&bin);
    let ours = app.path().join("kicad-cli-settings");
    let install = match discover(&options(&cli, &ours)) {
        Discovery::Ready(install) => install,
        other => panic!("expected Ready, got {other:?}"),
    };
    assert_eq!(install.settings.path(), ours);

    // A preview export runs the same kicad-cli; the fake cannot export.
    let library = app.path().join("t.kicad_sym");
    fs::write(&library, "(kicad_symbol_lib)").unwrap();
    let symbol = ResolvedSymbol {
        lib_ref: LibRef {
            library: "t".into(),
            item: "R".into(),
        },
        library,
        origin: LibraryOrigin::Project,
    };
    let cache = PreviewCache::new(app.path().join("previews"));
    assert!(matches!(
        cache.symbol(&install, &symbol),
        Err(PreviewError::ExportFailed { .. })
    ));

    let recorded = runs(&bin);
    let commands: Vec<&str> = recorded.iter().map(|run| run[0].as_str()).collect();
    assert_eq!(commands, ["version", "sym"]);
    let appdata = std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("kicad"));
    for run in &recorded {
        let (config, cache) = (Path::new(&run[1]), Path::new(&run[2]));
        assert_eq!(config, ours.join("config"), "{run:?}");
        assert_eq!(cache, ours.join("cache"), "{run:?}");
        for folder in theirs.iter().chain(&appdata) {
            assert!(!is_inside(config, folder), "{run:?} uses {folder:?}");
            assert!(!is_inside(cache, folder), "{run:?} uses {folder:?}");
        }
    }

    // A settings folder inside the person's KiCad settings is refused before
    // kicad-cli starts, with a message about the folder, not about KiCad.
    match discover(&options(&cli, &theirs[0].join("10.0").join("ours"))) {
        Discovery::SettingsFolder { message, .. } => {
            assert!(message.contains("overlaps your KiCad folder"), "{message}")
        }
        other => panic!("expected SettingsFolder, got {other:?}"),
    }
    assert_eq!(runs(&bin).len(), 2, "the refused run must not start");
}

fn options(cli: &Path, settings: &Path) -> DiscoveryOptions {
    DiscoveryOptions {
        user_override: Some(cli.to_path_buf()),
        locations: SearchLocations::Only(Vec::new()),
        settings: CliSettingsFolder::new(settings.to_path_buf()),
    }
}

/// The fake kicad-cli, copied into `dir` under the real file name, reporting
/// KiCad 10.
fn fake_cli(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(if cfg!(windows) {
        "kicad-cli.exe"
    } else {
        "kicad-cli"
    });
    fs::copy(env!("CARGO_BIN_EXE_fake-kicad-cli"), &path).unwrap();
    fs::write(dir.join("fake-version.txt"), "10.0.4").unwrap();
    path
}

/// Each run the fake recorded: its first argument, `KICAD_CONFIG_HOME` and
/// `KICAD_CACHE_HOME`.
fn runs(dir: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(dir.join("fake-runs.txt"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect()
}

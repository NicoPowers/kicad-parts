//! End-to-end use cases over the real legacy catalog and the installed
//! KiCad 10, without the desktop window.

use catalog_browser::{BrowserConfig, CatalogBrowser};
use desktop_contract::{KicadStatus, PreviewKind, PreviewOutcome, PreviewProblemReason};
use kicad_integration::SearchLocations;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn open_browser(dir: &Path, locations: SearchLocations) -> CatalogBrowser {
    CatalogBrowser::open(BrowserConfig {
        catalog_root: repo_root(),
        cache_dir: dir.join("cache"),
        settings_file: dir.join("config").join("settings.json"),
        kicad_locations: locations,
    })
}

fn ready(outcome: PreviewOutcome) -> desktop_contract::PreviewSet {
    match outcome {
        PreviewOutcome::Ready(set) => set,
        PreviewOutcome::Problem(problem) => panic!("preview failed: {problem:?}"),
    }
}

#[test]
fn search_select_and_preview_a_multi_unit_part() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path(), SearchLocations::Standard);
    let status = browser.status();
    assert!(status.kicad.is_ready(), "{:?}", status.kicad);
    assert_eq!((status.catalog.categories, status.catalog.parts), (21, 178));

    let results = browser.search("stm32h723", 50);
    assert_eq!(results.total, 1);
    let ipn = results.hits[0].ipn.clone();
    let part = browser.part(&ipn).unwrap();
    assert_eq!(part.symbol.as_deref(), Some("g-mcu:STM32H723ZG"));

    let symbol = ready(browser.preview(&ipn, PreviewKind::Symbol));
    assert!(!symbol.from_kicad_bundle);
    let units: Vec<_> = symbol.images.iter().map(|image| image.unit).collect();
    assert!(units.len() > 1, "{units:?}");
    assert!(
        units.iter().zip(1..).all(|(unit, n)| *unit == Some(n)),
        "{units:?}"
    );
    assert!(!symbol.from_cache);
    assert!(
        symbol.images[0]
            .data_uri
            .starts_with("data:image/svg+xml;base64,")
    );
    let footprint = ready(browser.preview(&ipn, PreviewKind::Footprint));
    assert!(footprint.from_kicad_bundle);
    assert_eq!(footprint.reference, "Package_QFP:LQFP-144_20x20mm_P0.5mm");
    assert_eq!(footprint.images.len(), 1);
    assert_eq!(footprint.images[0].unit, None);
    let exports = browser.export_count();

    // A repeat view, even from a fresh session, is served from the cache.
    let again = open_browser(dir.path(), SearchLocations::Standard);
    assert!(ready(again.preview(&ipn, PreviewKind::Symbol)).from_cache);
    assert!(ready(again.preview(&ipn, PreviewKind::Footprint)).from_cache);
    assert_eq!(again.export_count(), 0);
    assert_eq!(browser.export_count(), exports);
}

#[test]
fn missing_kicad_is_explained_and_fixed_by_a_user_override() {
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let only_empty = SearchLocations::Only(vec![empty.path().to_path_buf()]);
    let browser = open_browser(dir.path(), only_empty.clone());
    match browser.status().kicad {
        KicadStatus::Missing { searched } => assert_eq!(searched.len(), 1),
        other => panic!("expected missing, got {other:?}"),
    }
    let ipn = browser.search("RC0603FR-0747RL", 1).hits[0].ipn.clone();
    match browser.preview(&ipn, PreviewKind::Symbol) {
        PreviewOutcome::Problem(problem) => {
            assert_eq!(problem.reason, PreviewProblemReason::KicadUnavailable)
        }
        other => panic!("expected a problem, got {other:?}"),
    }

    let installed = match browser_with_standard(dir.path()).status().kicad {
        KicadStatus::Ready { cli_path, .. } => cli_path,
        other => panic!("KiCad 10 is required for this test: {other:?}"),
    };
    let status = browser.set_kicad_override(Some(installed.clone())).unwrap();
    assert!(matches!(
        status.kicad,
        KicadStatus::Ready {
            from_override: true,
            ..
        }
    ));
    assert_eq!(status.kicad_override.as_deref(), Some(installed.as_str()));
    ready(browser.preview(&ipn, PreviewKind::Symbol));

    // The override is a saved setting.
    let reopened = open_browser(dir.path(), only_empty);
    assert!(reopened.status().kicad.is_ready());
    let cleared = reopened.set_kicad_override(None).unwrap();
    assert!(matches!(cleared.kicad, KicadStatus::Missing { .. }));
    assert_eq!(cleared.kicad_override, None);
}

fn browser_with_standard(dir: &Path) -> CatalogBrowser {
    open_browser(&dir.join("standard"), SearchLocations::Standard)
}

#[test]
fn real_catalog_problems_are_reported_with_their_cause() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path(), SearchLocations::Standard);

    // The catalog names a symbol its library does not contain; kicad-cli's
    // own message reaches the UI.
    match browser.preview("ICS-0041-0001", PreviewKind::Symbol) {
        PreviewOutcome::Problem(problem) => {
            assert_eq!(problem.reason, PreviewProblemReason::ExportFailed);
            assert_eq!(problem.reference.as_deref(), Some("g-ics:TP4333"));
            let message = problem.cli_message.unwrap();
            assert!(message.contains("no symbol selected"), "{message}");
            assert!(problem.command.unwrap().contains("sym export svg"));
        }
        other => panic!("expected an export failure, got {other:?}"),
    }

    // A footprint list uses its first entry and keeps the alternates.
    let listed = ready(browser.preview("MCU-0002-0001", PreviewKind::Footprint));
    assert_eq!(
        listed.reference,
        "Package_DFN_QFN:UFQFPN-32-1EP_5x5mm_P0.5mm_EP3.5x3.5mm"
    );
    assert_eq!(listed.alternates.len(), 1);

    match browser.preview("PCB-0000-0001", PreviewKind::Symbol) {
        PreviewOutcome::Problem(problem) => {
            assert_eq!(problem.reason, PreviewProblemReason::Unassigned)
        }
        other => panic!("expected unassigned, got {other:?}"),
    }
}

#[test]
fn an_invalid_override_is_explained_and_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path(), SearchLocations::Standard);
    let settings = dir.path().join("config").join("settings.json");
    let status = browser
        .set_kicad_override(Some(dir.path().join("nope").display().to_string()))
        .unwrap();
    let rejected = status.override_rejected.expect("the rejection is reported");
    assert!(rejected.contains("does not exist"), "{rejected}");
    // The working discovery and the saved settings are untouched.
    assert!(status.kicad.is_ready(), "{:?}", status.kicad);
    assert_eq!(status.kicad_override, None);
    assert!(!settings.exists());

    // A later successful search clears the message.
    assert_eq!(browser.rediscover_kicad().override_rejected, None);
}

#[test]
fn a_corrupt_settings_file_is_reported_and_replaced_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    fs::create_dir_all(&config).unwrap();
    let settings = config.join("settings.json");
    fs::write(&settings, b"{ not json").unwrap();

    let browser = open_browser(dir.path(), SearchLocations::Standard);
    let status = browser.status();
    let error = status
        .settings_error
        .expect("corrupt settings are reported");
    assert!(error.contains("not valid settings JSON"), "{error}");
    assert!(status.kicad.is_ready(), "defaults are in use");

    let installed = match status.kicad {
        KicadStatus::Ready { cli_path, .. } => cli_path,
        other => panic!("KiCad 10 is required for this test: {other:?}"),
    };
    let saved = browser.set_kicad_override(Some(installed.clone())).unwrap();
    assert_eq!(saved.settings_error, None);
    let written: serde_json::Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(written["kicad_cli_override"], installed.as_str());
    let leftovers: Vec<_> = fs::read_dir(&config)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        ["settings.json"],
        "no temporary file is left behind"
    );
}

/// Exports every symbol and footprint the real catalog references and
/// prints a summary. Slow (hundreds of kicad-cli runs), so run on demand:
/// `cargo test -p catalog-browser --test browse -- --ignored --nocapture`.
#[test]
#[ignore = "exports the whole catalog; run on demand"]
fn audit_every_catalog_preview() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path(), SearchLocations::Standard);
    let parts = browser.search("", 500).hits;
    assert_eq!(parts.len(), 178);
    let mut ready_count = 0;
    let mut problems = Vec::new();
    for part in &parts {
        for kind in [PreviewKind::Symbol, PreviewKind::Footprint] {
            match browser.preview(&part.ipn, kind) {
                PreviewOutcome::Ready(_) => ready_count += 1,
                PreviewOutcome::Problem(problem) => problems.push(format!(
                    "{} {:?} {:?}: {} {}",
                    part.ipn,
                    kind,
                    problem.reason,
                    problem.message,
                    problem.cli_message.unwrap_or_default().replace('\n', " | ")
                )),
            }
        }
    }
    println!("{ready_count} previews ready, {} problems", problems.len());
    for problem in &problems {
        println!("  {problem}");
    }
}

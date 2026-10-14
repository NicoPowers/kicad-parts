//! End-to-end use cases over the real CSV catalog and the installed
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
        kicad_config_dir: None,
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
    assert_eq!((status.catalog.csv_files, status.catalog.parts), (21, 178));

    let results = browser.search("stm32h723", 50);
    assert_eq!(results.total, 1);
    let ipn = results.hits().next().unwrap().ipn.clone();
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
    let ipn = browser
        .search("RC0603FR-0747RL", 1)
        .hits()
        .next()
        .unwrap()
        .ipn
        .clone();
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
    let parts: Vec<_> = browser.search("", 500).hits().cloned().collect();
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

#[test]
fn search_groups_the_parts_by_category() {
    use catalog_browser::PartValues;
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let browser = open_browser(
        dir.path(),
        SearchLocations::Only(vec![empty.path().to_path_buf()]),
    );

    // Every CSV file with parts is one section, by plain name, holding all
    // its parts; g-mec.csv has none, so 20 of the 21 make a section.
    let all = browser.search("", 500);
    assert_eq!((all.total, all.shown(), all.sections.len()), (178, 178, 20));
    assert!(all.sections.iter().all(|s| s.category != "mec"));
    let names: Vec<&str> = all.sections.iter().map(|s| s.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_by_key(|name| name.to_lowercase());
    assert_eq!(names, sorted);
    let resistors = all.sections.iter().find(|s| s.category == "res").unwrap();
    assert_eq!(resistors.name, "Resistors");
    assert_eq!(resistors.total, resistors.hits.len());
    assert!(resistors.hits.iter().all(|hit| hit.csv_file == "g-res.csv"));

    // A search keeps only the sections with matches, each with its count.
    let found = browser.search("0603", 500);
    assert!(found.sections.len() < 20);
    assert!(
        found
            .sections
            .iter()
            .all(|s| s.total > 0 && s.total == s.hits.len())
    );
    assert_eq!(found.shown(), found.total);

    // A section shows at most `limit` parts but counts every match.
    let limited = browser.search("", 2);
    let resistors = limited
        .sections
        .iter()
        .find(|s| s.category == "res")
        .unwrap();
    assert_eq!(resistors.hits.len(), 2);
    assert!(resistors.total > 2);

    // The team catalog's category wins, a part only it has is in its
    // category's section, and its names are used.
    let moved = resistors.hits[0].ipn.clone();
    let current: BTreeMap<String, PartValues> = [
        (
            moved.clone(),
            PartValues {
                category: Some("cap".to_owned()),
                ..PartValues::default()
            },
        ),
        (
            "RES-9999-0001".to_owned(),
            PartValues {
                category: Some("res".to_owned()),
                description: Some("New resistor".to_owned()),
                ..PartValues::default()
            },
        ),
    ]
    .into();
    let names: BTreeMap<String, String> = [("cap".to_owned(), "Capacitors (C)".to_owned())].into();
    let team = browser.search_with("", 500, &current, &names);
    assert_eq!(team.total, 179);
    let section_of = |ipn: &str| {
        team.sections
            .iter()
            .find(|s| s.hits.iter().any(|hit| hit.ipn == ipn))
            .map(|s| s.name.as_str())
    };
    assert_eq!(section_of(&moved), Some("Capacitors (C)"));
    assert_eq!(section_of("RES-9999-0001"), Some("Resistors"));
}

#[test]
fn search_answers_one_section_or_counts_only_and_always_shows_the_selected_part() {
    use desktop_contract::SearchRequest;
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let browser = open_browser(
        dir.path(),
        SearchLocations::Only(vec![empty.path().to_path_buf()]),
    );
    let (none, no_names) = (BTreeMap::new(), BTreeMap::new());
    let request =
        |query: &str, limit: usize, selected: Option<&str>, category: Option<&str>| SearchRequest {
            query: query.to_owned(),
            limit,
            selected: selected.map(str::to_owned),
            category: category.map(str::to_owned),
        };

    // Counts only, as the empty search asks while every section is closed.
    let counts = browser.search_request(&request("", 0, None, None), &none, &no_names);
    assert_eq!(
        (counts.total, counts.shown(), counts.sections.len()),
        (178, 0, 20)
    );
    assert_eq!(counts.selected, None);

    // One section's parts, as opening it asks.
    let resistors = browser.search_request(&request("", 200, None, Some("res")), &none, &no_names);
    assert_eq!(resistors.sections.len(), 1);
    let section = &resistors.sections[0];
    assert_eq!(
        (section.category.as_str(), section.hits.len()),
        ("res", section.total)
    );
    assert_eq!(
        resistors.total, 178,
        "the count line still counts every match"
    );

    // The selected part is always among its section's parts, and the answer
    // says which section holds it.
    let last = section.hits.last().unwrap().ipn.clone();
    let found = browser.search_request(&request("", 1, Some(&last), None), &none, &no_names);
    assert_eq!(found.selected.as_deref(), Some("res"));
    let res = found.sections.iter().find(|s| s.category == "res").unwrap();
    assert!(res.hits.iter().any(|hit| hit.ipn == last), "{:?}", res.hits);
    // A selected part the search does not match is in no section.
    let hidden = browser.search_request(
        &request("stm32h723", 25, Some(&last), None),
        &none,
        &no_names,
    );
    assert_eq!(hidden.selected, None);

    // A trailing space is the same search.
    assert_eq!(
        browser.search("stm32h723 ", 25).sections,
        browser.search("stm32h723", 25).sections
    );
}

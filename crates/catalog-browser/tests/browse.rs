//! End-to-end use cases over the sample library (fixtures/sample-library)
//! and the installed KiCad 10, without the desktop window: the part list
//! of a team that has the sample parts, and previews of them drawn from the
//! sample library, which stands for our own libraries.

use catalog_browser::team::KnownParts;
use catalog_browser::{BrowserConfig, CatalogBrowser, PartValues};
use desktop_contract::{
    GroupBy, KicadStatus, PartDetail, PartKey, PreviewKind, PreviewOutcome, PreviewProblemReason,
    SearchRequest,
};
use kicad_integration::SearchLocations;
use sample_library::Sample;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// The team server address of the sample team.
const TEAM: &str = "https://ma.example.com";

fn open_browser(dir: &Path, locations: SearchLocations) -> CatalogBrowser {
    CatalogBrowser::open(BrowserConfig {
        catalog_root: Some(sample_library::root()),
        cache_dir: dir.join("cache"),
        settings_file: dir.join("config").join("settings.json"),
        kicad_locations: locations,
        kicad_config_dir: None,
        kicad_documents_dir: None,
    })
}

/// Every sample part as a team the app follows has it, as
/// `TeamCatalog::current_values` gives them.
fn sample_values() -> BTreeMap<String, PartValues> {
    let sample = Sample::load();
    sample
        .parts
        .iter()
        .map(|part| {
            let category = sample.category(&part.category);
            let main = part.manufacturer_parts.first();
            let values = PartValues {
                category: Some(part.category.clone()),
                fields: Some(
                    part.detail_fields(category)
                        .into_iter()
                        .map(|field| (field.name, field.value))
                        .collect(),
                ),
                mpn: Some(main.map(|(_, mpn)| mpn.clone()).unwrap_or_default()),
                manufacturer: Some(main.map(|(maker, _)| maker.clone()).unwrap_or_default()),
                description: Some(part.field("Description").to_owned()),
                value: Some(part.field("Value").to_owned()),
                symbol: Some(part.symbol.clone()),
                footprint: Some(part.footprint.clone()),
            };
            (part.ipn.clone(), values)
        })
        .collect()
}

/// The sample categories' plain names, by code.
fn sample_names() -> BTreeMap<String, String> {
    Sample::load()
        .categories
        .iter()
        .map(|category| (category.code.clone(), category.name.clone()))
        .collect()
}

/// The team that has every sample part.
fn sample_team() -> [Arc<KnownParts>; 1] {
    [Arc::new(KnownParts::new(
        TEAM,
        "MA",
        sample_values(),
        sample_names(),
    ))]
}

fn key(ipn: &str) -> PartKey {
    PartKey {
        team: TEAM.to_owned(),
        ipn: ipn.to_owned(),
    }
}

/// The part panel's view of the sample part `ipn`.
fn part(browser: &CatalogBrowser, ipn: &str) -> PartDetail {
    browser
        .part_in_list(&sample_team(), &key(ipn))
        .unwrap_or_else(|| panic!("{ipn} is in the list"))
}

/// The preview of the part `ipn`'s symbol or footprint, drawn from the
/// sample library, as the app draws a team part's.
fn preview(browser: &CatalogBrowser, ipn: &str, kind: PreviewKind) -> PreviewOutcome {
    let detail = part(browser, ipn);
    let field = match kind {
        PreviewKind::Symbol => detail.symbol,
        PreviewKind::Footprint => detail.footprint,
    };
    browser.preview_field(ipn, kind, field.as_deref().unwrap_or_default())
}

fn ready(outcome: PreviewOutcome) -> desktop_contract::PreviewSet {
    match outcome {
        PreviewOutcome::Ready(set) => set,
        PreviewOutcome::Problem(problem) => panic!("preview failed: {problem:?}"),
    }
}

fn request(query: &str, limit: usize) -> SearchRequest {
    SearchRequest {
        query: query.to_owned(),
        limit,
        selected: None,
        section: None,
        uses: None,
        team: None,
        group_by: GroupBy::Category,
    }
}

#[test]
fn search_select_and_preview_a_multi_unit_part() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path(), SearchLocations::Standard);
    let status = browser.status();
    assert!(status.kicad.is_ready(), "{:?}", status.kicad);

    let results = browser.search_with("sample-mcu", 50, &sample_team());
    assert_eq!(results.total, 1);
    let ipn = results.hits().next().unwrap().ipn.clone();
    assert_eq!(
        part(&browser, &ipn).symbol.as_deref(),
        Some("g-mcu:SAMPLE_MCU")
    );

    let symbol = ready(preview(&browser, &ipn, PreviewKind::Symbol));
    assert!(!symbol.from_kicad_bundle);
    let units: Vec<_> = symbol.images.iter().map(|image| image.unit).collect();
    assert_eq!(units, [Some(1), Some(2)]);
    assert!(!symbol.from_cache);
    assert!(
        symbol.images[0]
            .data_uri
            .starts_with("data:image/svg+xml;base64,")
    );
    let footprint = ready(preview(&browser, &ipn, PreviewKind::Footprint));
    assert!(footprint.from_kicad_bundle);
    assert_eq!(
        footprint.reference,
        "Package_DFN_QFN:QFN-32-1EP_5x5mm_P0.5mm_EP3.45x3.45mm"
    );
    assert_eq!(footprint.images.len(), 1);
    assert_eq!(footprint.images[0].unit, None);
    let exports = browser.export_count();

    // A repeat view, even from a fresh session, is served from the cache.
    let again = open_browser(dir.path(), SearchLocations::Standard);
    assert!(ready(preview(&again, &ipn, PreviewKind::Symbol)).from_cache);
    assert!(ready(preview(&again, &ipn, PreviewKind::Footprint)).from_cache);
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
        .search_with("RC0603FR-0747RL", 1, &sample_team())
        .hits()
        .next()
        .unwrap()
        .ipn
        .clone();
    match preview(&browser, &ipn, PreviewKind::Symbol) {
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
    ready(preview(&browser, &ipn, PreviewKind::Symbol));

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
fn preview_problems_are_reported_with_their_cause() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path(), SearchLocations::Standard);

    // A symbol its library does not contain; kicad-cli's own message
    // reaches the UI.
    match preview(&browser, "ICS-0002-0001", PreviewKind::Symbol) {
        PreviewOutcome::Problem(problem) => {
            assert_eq!(problem.reason, PreviewProblemReason::ExportFailed);
            assert_eq!(problem.reference.as_deref(), Some("Device:TP4333"));
            let message = problem.cli_message.unwrap();
            assert!(message.contains("no symbol selected"), "{message}");
            assert!(problem.command.unwrap().contains("sym export svg"));
        }
        other => panic!("expected an export failure, got {other:?}"),
    }
    // A footprint library that is nowhere.
    match preview(&browser, "ICS-0003-0001", PreviewKind::Footprint) {
        PreviewOutcome::Problem(problem) => {
            assert_eq!(problem.reason, PreviewProblemReason::Unresolved);
            assert_eq!(problem.reference.as_deref(), Some("Sample_Missing:SOT-353"));
        }
        other => panic!("expected an unresolved library, got {other:?}"),
    }

    // A footprint list uses its first entry and keeps the alternates.
    let listed = ready(preview(&browser, "MCU-0000-0001", PreviewKind::Footprint));
    assert_eq!(
        listed.reference,
        "Package_DFN_QFN:QFN-32-1EP_5x5mm_P0.5mm_EP3.45x3.45mm"
    );
    assert_eq!(listed.alternates.len(), 1);

    match preview(&browser, "PCB-0000-0001", PreviewKind::Symbol) {
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

/// Exports every symbol and footprint the sample parts reference and
/// prints a summary. Run on demand:
/// `cargo test -p catalog-browser --test browse -- --ignored --nocapture`.
#[test]
#[ignore = "exports every sample preview; run on demand"]
fn audit_every_sample_preview() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path(), SearchLocations::Standard);
    let parts: Vec<_> = browser
        .search_with("", 500, &sample_team())
        .hits()
        .cloned()
        .collect();
    assert_eq!(parts.len(), Sample::load().parts.len());
    let mut ready_count = 0;
    let mut problems = Vec::new();
    for part in &parts {
        for kind in [PreviewKind::Symbol, PreviewKind::Footprint] {
            match preview(&browser, &part.ipn, kind) {
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
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let browser = open_browser(
        dir.path(),
        SearchLocations::Only(vec![empty.path().to_path_buf()]),
    );
    let sample = Sample::load();
    let team = sample_team();

    // Every category with parts is one section, by plain name, holding all
    // its parts.
    let all = browser.search_with("", 500, &team);
    assert_eq!(
        (all.total, all.shown(), all.sections.len()),
        (
            sample.parts.len(),
            sample.parts.len(),
            sample.categories.len()
        )
    );
    let names: Vec<&str> = all.sections.iter().map(|s| s.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_by_key(|name| name.to_lowercase());
    assert_eq!(names, sorted);
    let resistors = all.sections.iter().find(|s| s.key == "res").unwrap();
    assert_eq!(resistors.name, "Resistors");
    assert_eq!(resistors.total, resistors.hits.len());
    assert_eq!(resistors.total, sample.parts_in(&["res"]).count());
    assert!(resistors.hits.iter().all(|hit| hit.team == TEAM));

    // A search keeps only the sections with matches, each with its count.
    let found = browser.search_with("0603", 500, &team);
    assert!(found.sections.len() < sample.categories.len());
    assert!(
        found
            .sections
            .iter()
            .all(|s| s.total > 0 && s.total == s.hits.len())
    );
    assert_eq!(found.shown(), found.total);

    // A section shows at most `limit` parts but counts every match.
    let limited = browser.search_with("", 2, &team);
    let resistors = limited.sections.iter().find(|s| s.key == "res").unwrap();
    assert_eq!(resistors.hits.len(), 2);
    assert!(resistors.total > 2);

    // A part is in its category's section, and the team's names are used.
    let moved = resistors.hits[0].ipn.clone();
    let mut values = sample_values();
    values.get_mut(&moved).unwrap().category = Some("cap".to_owned());
    values.insert(
        "RES-9999-0001".to_owned(),
        PartValues {
            category: Some("res".to_owned()),
            description: Some("New resistor".to_owned()),
            ..PartValues::default()
        },
    );
    let mut names = sample_names();
    names.insert("cap".to_owned(), "Capacitors (C)".to_owned());
    let parts = KnownParts::new(TEAM, "MA", values, names);
    let changed = browser.search_with("", 500, &[Arc::new(parts)]);
    assert_eq!(changed.total, sample.parts.len() + 1);
    let section_of = |ipn: &str| {
        changed
            .sections
            .iter()
            .find(|s| s.hits.iter().any(|hit| hit.ipn == ipn))
            .map(|s| s.name.as_str())
    };
    assert_eq!(section_of(&moved), Some("Capacitors (C)"));
    assert_eq!(section_of("RES-9999-0001"), Some("Resistors"));
}

#[test]
fn search_answers_one_section_or_counts_only_and_always_shows_the_selected_part() {
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let browser = open_browser(
        dir.path(),
        SearchLocations::Only(vec![empty.path().to_path_buf()]),
    );
    let sample = Sample::load();
    let team = sample_team();
    let ask = |query: &str, limit: usize, selected: Option<&str>, section: Option<&str>| {
        let mut asked = request(query, limit);
        asked.selected = selected.map(key);
        asked.section = section.map(str::to_owned);
        browser.search_request(&asked, &team)
    };

    // Counts only, as the empty search asks while every section is closed.
    let counts = ask("", 0, None, None);
    assert_eq!(
        (counts.total, counts.shown(), counts.sections.len()),
        (sample.parts.len(), 0, sample.categories.len())
    );
    assert_eq!(counts.selected, None);

    // One section's parts, as opening it asks.
    let resistors = ask("", 200, None, Some("res"));
    assert_eq!(resistors.sections.len(), 1);
    let section = &resistors.sections[0];
    assert_eq!(
        (section.key.as_str(), section.hits.len()),
        ("res", section.total)
    );
    assert_eq!(
        resistors.total,
        sample.parts.len(),
        "the count line still counts every match"
    );

    // The selected part is always among its section's parts, and the answer
    // says which section holds it.
    let last = section.hits.last().unwrap().ipn.clone();
    let found = ask("", 1, Some(&last), None);
    assert_eq!(found.selected.as_deref(), Some("res"));
    let res = found.sections.iter().find(|s| s.key == "res").unwrap();
    assert!(res.hits.iter().any(|hit| hit.ipn == last), "{:?}", res.hits);
    // A selected part the search does not match is in no section.
    let hidden = ask("sample-mcu", 25, Some(&last), None);
    assert_eq!(hidden.selected, None);

    // A trailing space is the same search.
    assert_eq!(
        browser.search_with("sample-mcu ", 25, &team).sections,
        browser.search_with("sample-mcu", 25, &team).sections
    );
}

#[test]
fn shows_only_the_parts_using_a_reference_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let browser = open_browser(
        dir.path(),
        SearchLocations::Only(vec![empty.path().to_path_buf()]),
    );
    let team = sample_team();
    let uses = |reference: &str, query: &str, section: Option<&str>| {
        let mut asked = request(query, 500);
        asked.uses = Some(reference.to_owned());
        asked.section = section.map(str::to_owned);
        browser.search_request(&asked, &team)
    };
    // A footprint: every part listing it as a choice, and no other.
    let footprint = "g-Resistor_SMD:R_0603_1608Metric";
    let found = uses(footprint, "", None);
    assert_eq!(found.uses.as_deref(), Some(footprint));
    assert_eq!(found.total, 5);
    assert_eq!(found.total, found.shown());
    for hit in found.hits() {
        let choices = part(&browser, &hit.ipn).footprint.unwrap_or_default();
        assert!(
            choices.split(';').any(|choice| choice.trim() == footprint),
            "{choices}"
        );
    }
    // The free-text search finds it too.
    let text = browser.search_with("R_0603_1608Metric", 500, &team);
    assert!(text.total >= found.total);
    // A near name finds nothing; a search narrows further; one section.
    assert_eq!(uses("g-Resistor_SMD:R_0603", "", None).total, 0);
    let narrowed = uses(footprint, "47", None);
    assert!(narrowed.total >= 1 && narrowed.total < found.total);
    assert_eq!(uses(footprint, "", Some("res")).sections.len(), 1);
    // A symbol, compared exactly.
    let symbol = uses("g-res:R_US", "", None);
    assert_eq!(symbol.total, 5);
    for hit in symbol.hits() {
        assert_eq!(
            part(&browser, &hit.ipn).symbol.as_deref(),
            Some("g-res:R_US")
        );
    }
}

#[test]
fn the_part_panel_shows_a_part_as_its_row_in_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let browser = open_browser(
        dir.path(),
        SearchLocations::Only(vec![empty.path().to_path_buf()]),
    );
    let ipn = "RES-0000-0470".to_owned();
    let values = PartValues {
        category: Some("res".into()),
        value: Some("47R".into()),
        fields: Some(vec![("Value".into(), "47R".into())]),
        ..PartValues::default()
    };
    let team = |server: &str| {
        Arc::new(KnownParts::new(
            server,
            "AC",
            [(ipn.clone(), values.clone())].into(),
            BTreeMap::new(),
        ))
    };
    let key = |team: &str| PartKey {
        team: team.into(),
        ipn: ipn.clone(),
    };
    let teams = [
        team("https://ac.example.com"),
        team("https://be.example.com"),
    ];
    // The team's last-known values, as its row shows them.
    let detail = browser
        .part_in_list(&teams, &key("https://ac.example.com"))
        .unwrap();
    assert_eq!(detail.summary.value, "47R");
    assert_eq!(detail.summary.team, "");
    // A part of no team is in no list; another team's part is not this
    // one.
    assert!(browser.part_in_list(&teams, &key("")).is_none());
    assert!(
        browser
            .part_in_list(&teams, &key("https://other.example.com"))
            .is_none()
    );
}

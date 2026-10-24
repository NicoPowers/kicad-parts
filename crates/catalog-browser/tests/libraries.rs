//! The Libraries browser's index (kicad-parts-d152) over fixture libraries
//! and fixture KiCad library tables. These tests never read the
//! developer's own KiCad settings: they pass a folder of fixture tables, or
//! none. Only the last test reads KiCad's installed libraries (read only).

#![allow(clippy::format_collect)]

use catalog_browser::team::KnownParts;
use catalog_browser::{BrowserConfig, CatalogBrowser, PartValues};
use desktop_contract::{
    LibraryFilters, LibraryGroup, LibraryItemRequest, LibraryItemsRequest, LibraryKey, MountFilter,
    PreviewKind, PreviewOutcome, PreviewProblemReason,
};
use kicad_integration::SearchLocations;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(120);

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn symbol(name: &str, reference: &str, filters: &str, footprint: &str, pins: &[&str]) -> String {
    let pins: String = pins
        .iter()
        .map(|number| {
            format!("(pin passive line (at 0 0 0) (length 1) (name \"~\") (number \"{number}\"))")
        })
        .collect();
    format!(
        "(symbol \"{name}\" (property \"Reference\" \"{reference}\") (property \"Value\" \"{name}\") (property \"Footprint\" \"{footprint}\") (property \"Description\" \"{name} for tests\") (property \"ki_keywords\" \"test {name}\") (property \"ki_fp_filters\" \"{filters}\") (symbol \"{name}_1_1\" {pins}))"
    )
}

fn library(symbols: &[String]) -> String {
    format!(
        "(kicad_symbol_lib (version 20241209) (generator \"test\") {})",
        symbols.join(" ")
    )
}

fn footprint(name: &str, attr: &str, pads: &[&str]) -> String {
    let pads: String = pads
        .iter()
        .map(|number| format!("(pad \"{number}\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\"))"))
        .collect();
    format!(
        "(footprint \"{name}\" (version 20240108) (descr \"{name} footprint\") (tags \"test\") (attr {attr}) {pads})"
    )
}

/// A CSV catalog folder, a fake KiCad install and fixture KiCad settings.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    catalog: PathBuf,
    install: PathBuf,
    share: PathBuf,
    config: PathBuf,
    mine: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let catalog = root.join("catalog");
    write(
        &catalog.join("database").join("g-res.csv"),
        "IPN,MPN,Description,Symbol,Footprint\nRES-1,RC1,Resistor one,g-pas:R_US,g-Resistor_SMD:R_0603\n",
    );
    write(
        &catalog.join("symbols").join("g-pas.kicad_sym"),
        &library(&[
            symbol("R_US", "R", "R_*", "g-Resistor_SMD:R_0603", &["1", "2"]),
            symbol("C_Unused", "C", "C_*", "", &["1", "2"]),
        ]),
    );
    write(
        &catalog
            .join("footprints")
            .join("g-Resistor_SMD.pretty")
            .join("R_0603.kicad_mod"),
        &footprint("R_0603", "smd", &["1", "2"]),
    );
    // A fake KiCad 10 install: kicad-cli cannot run, but the data folder
    // is found.
    let install = root.join("install");
    let kicad = install.join("KiCad").join("10.0");
    let cli = if cfg!(windows) {
        "kicad-cli.exe"
    } else {
        "kicad-cli"
    };
    write(&kicad.join("bin").join(cli), "");
    let share = kicad.join("share").join("kicad");
    write(
        &share.join("symbols").join("Device.kicad_sym"),
        &library(&[
            symbol("R", "R", "R_*", "", &["1", "2"]),
            symbol("Q_NPN", "Q", "SOT?23* TO?92*", "", &["1", "2", "3"]),
            "(symbol \"GND\" (power) (property \"Reference\" \"#PWR\") (symbol \"GND_0_1\" (pin power_in line (number \"1\"))))".to_owned(),
        ]),
    );
    write(
        &share.join("template").join("sym-lib-table"),
        "(sym_lib_table (version 7) (lib (name \"Device\")(type \"KiCad\")(uri \"${KICAD10_SYMBOL_DIR}/Device.kicad_sym\")(options \"\")(descr \"Generic symbols\")))",
    );
    let pretty = share.join("footprints");
    write(
        &pretty
            .join("Resistor_SMD.pretty")
            .join("R_0402_1005Metric.kicad_mod"),
        &footprint("R_0402_1005Metric", "smd", &["1", "2"]),
    );
    write(
        &pretty.join("Resistor_THT.pretty").join("R_Axial.kicad_mod"),
        &footprint("R_Axial", "through_hole", &["1", "2"]),
    );
    write(
        &pretty
            .join("Package_TO_SOT_SMD.pretty")
            .join("SOT-23.kicad_mod"),
        &footprint("SOT-23", "smd", &["1", "2", "3"]),
    );
    write(
        &pretty
            .join("Capacitor_SMD.pretty")
            .join("C_0402_1005Metric.kicad_mod"),
        &footprint("C_0402_1005Metric", "smd", &["1", "2"]),
    );
    // The person's KiCad settings: a symbol folder library under a
    // nickname that differs from its folder, a nested table, a database
    // library, a hidden one, and KiCad's own rows (already listed).
    let mine = root.join("mine");
    write(
        &mine.join("Mine.kicad_symdir").join("Base.kicad_sym"),
        &library(&[symbol("Base", "U", "SOT?23*", "", &["1", "2", "3"])]),
    );
    write(
        &mine.join("Mine.kicad_symdir").join("Child.kicad_sym"),
        "(kicad_symbol_lib (version 20241209) (symbol \"Child\" (extends \"Base\") (property \"Reference\" \"U\") (property \"Description\" \"Child of base\")))",
    );
    write(
        &mine.join("Shared.pretty").join("Mine_SOT-23.kicad_mod"),
        &footprint("Mine_SOT-23", "smd", &["1", "2", "3"]),
    );
    let config = root.join("kicad-config");
    write(
        &config.join("kicad_common.json"),
        &format!(
            "{{\"environment\": {{\"vars\": {{\"MY_LIBS\": {}}}}}}}",
            serde_json::to_string(&mine.to_string_lossy()).unwrap()
        ),
    );
    write(
        &config.join("sym-lib-table"),
        "(sym_lib_table (version 7)
  (lib (name \"Device\")(type \"KiCad\")(uri \"${KICAD10_SYMBOL_DIR}/Device.kicad_sym\")(options \"\")(descr \"\"))
  (lib (name \"MySymbols\")(type \"KiCad\")(uri \"${MY_LIBS}/Mine.kicad_symdir\")(options \"\")(descr \"My own symbols\"))
  (lib (name \"Parts DB\")(type \"Database\")(uri \"${MY_LIBS}/parts.kicad_dbl\")(options \"\")(descr \"\"))
  (lib (name \"Hidden\")(type \"KiCad\")(uri \"${MY_LIBS}/hidden.kicad_sym\")(options \"\")(descr \"\")(hidden))
  (lib (name \"Gone\")(type \"KiCad\")(uri \"${MY_LIBS}/gone.kicad_sym\")(options \"\")(descr \"\")))",
    );
    write(
        &config.join("fp-lib-table"),
        "(fp_lib_table (version 7) (lib (name \"Team\")(type \"Table\")(uri \"${MY_LIBS}/fp-lib-table\")(options \"\")(descr \"\")))",
    );
    write(
        &mine.join("fp-lib-table"),
        "(fp_lib_table (version 7) (lib (name \"MyFootprints\")(type \"KiCad\")(uri \"${MY_LIBS}/Shared.pretty\")(options \"\")(descr \"\")))",
    );
    Fixture {
        _dir: dir,
        root,
        catalog,
        install,
        share,
        config,
        mine,
    }
}

fn open(fixture: &Fixture, config: Option<&Path>) -> CatalogBrowser {
    CatalogBrowser::open(BrowserConfig {
        catalog_root: fixture.catalog.clone(),
        cache_dir: fixture.root.join("cache"),
        settings_file: fixture.root.join("settings.json"),
        kicad_locations: SearchLocations::Only(vec![fixture.install.clone()]),
        kicad_config_dir: config.map(Path::to_path_buf),
        kicad_documents_dir: None,
    })
}

fn indexed(fixture: &Fixture) -> CatalogBrowser {
    let browser = open(fixture, Some(&fixture.config));
    browser.libraries(true);
    assert!(browser.wait_for_libraries(WAIT));
    browser
}

fn key(group: LibraryGroup, nickname: &str) -> LibraryKey {
    LibraryKey {
        group,
        nickname: nickname.to_owned(),
        team: String::new(),
    }
}

fn request(kind: PreviewKind) -> LibraryItemsRequest {
    LibraryItemsRequest {
        kind,
        library: None,
        query: String::new(),
        filters: LibraryFilters::default(),
        for_symbol: None,
        limit: 200,
    }
}

fn names(browser: &CatalogBrowser, request: &LibraryItemsRequest) -> Vec<String> {
    browser
        .library_items(request, &[], None)
        .rows
        .into_iter()
        .map(|row| row.reference())
        .collect()
}

#[test]
fn lists_ours_kicads_and_installed_libraries() {
    let fixture = fixture();
    let browser = indexed(&fixture);
    let view = browser.libraries(false);
    assert!(view.is_ready(), "{view:?}");
    let listed =
        |entries: &[desktop_contract::LibraryEntry]| -> Vec<(LibraryGroup, String, Option<usize>)> {
            entries
                .iter()
                .map(|entry| (entry.key.group, entry.key.nickname.clone(), entry.items))
                .collect()
        };
    assert_eq!(
        listed(&view.symbols),
        [
            (LibraryGroup::Ours, "g-pas".to_owned(), Some(2)),
            (LibraryGroup::Kicad, "Device".to_owned(), Some(3)),
            (LibraryGroup::Installed, "Gone".to_owned(), None),
            (LibraryGroup::Installed, "MySymbols".to_owned(), Some(2)),
        ]
    );
    let device = &view.symbols[1];
    assert_eq!(device.description, "Generic symbols");
    assert!(
        view.symbols[2]
            .problem
            .as_deref()
            .unwrap()
            .contains("not found")
    );
    assert_eq!(view.symbols[3].description, "My own symbols");
    assert_eq!(
        listed(&view.footprints)
            .iter()
            .map(|(group, name, _)| (*group, name.as_str()))
            .collect::<Vec<_>>(),
        [
            (LibraryGroup::Ours, "g-Resistor_SMD"),
            (LibraryGroup::Kicad, "Capacitor_SMD"),
            (LibraryGroup::Kicad, "Package_TO_SOT_SMD"),
            (LibraryGroup::Kicad, "Resistor_SMD"),
            (LibraryGroup::Kicad, "Resistor_THT"),
            // From the nested table.
            (LibraryGroup::Installed, "MyFootprints"),
        ]
    );
    assert_eq!(view.notes.len(), 1, "{:?}", view.notes);
    assert!(view.notes[0].contains("1 library"), "{:?}", view.notes);
    // Without KiCad settings, nothing is installed.
    let plain = open(&fixture, None);
    plain.libraries(true);
    assert!(plain.wait_for_libraries(WAIT));
    let view = plain.libraries(false);
    assert!(
        view.symbols
            .iter()
            .all(|e| e.key.group != LibraryGroup::Installed)
    );
}

#[test]
fn searches_and_filters_symbols() {
    let fixture = fixture();
    let browser = indexed(&fixture);
    let all = request(PreviewKind::Symbol);
    assert_eq!(
        names(&browser, &all),
        [
            "g-pas:C_Unused",
            "g-pas:R_US",
            "Device:GND",
            "Device:Q_NPN",
            "Device:R",
            "MySymbols:Base",
            "MySymbols:Child"
        ]
    );
    // The name first, then description and keywords; every word must match.
    let mut search = all.clone();
    search.query = "r".into();
    assert_eq!(
        names(&browser, &search)[..2],
        ["Device:R".to_owned(), "g-pas:R_US".to_owned()]
    );
    search.query = "child base".into();
    assert_eq!(names(&browser, &search), ["MySymbols:Child"]);
    // One library.
    let mut one = all.clone();
    one.library = Some(key(LibraryGroup::Kicad, "Device"));
    assert_eq!(
        names(&browser, &one),
        ["Device:GND", "Device:Q_NPN", "Device:R"]
    );
    one.filters.hide_power = true;
    assert_eq!(names(&browser, &one), ["Device:Q_NPN", "Device:R"]);
    // Pin count, exact or a range; a derived symbol has its parent's pins.
    let mut pins = all.clone();
    pins.filters.pins_min = Some(3);
    pins.filters.pins_max = Some(3);
    assert_eq!(
        names(&browser, &pins),
        ["Device:Q_NPN", "MySymbols:Base", "MySymbols:Child"]
    );
    let mut letter = all.clone();
    letter.filters.reference = "r".into();
    assert_eq!(names(&browser, &letter), ["g-pas:R_US", "Device:R"]);
    let mut with = all.clone();
    with.filters.with_footprint = true;
    assert_eq!(names(&browser, &with), ["g-pas:R_US"]);
    // Rows are capped, with the count of every match.
    let mut capped = all.clone();
    capped.limit = 2;
    let items = browser.library_items(&capped, &[], None);
    assert_eq!((items.total, items.rows.len()), (7, 2));
    assert!(!items.reading);
}

#[test]
fn filters_footprints_for_a_symbol_like_kicad() {
    let fixture = fixture();
    let browser = indexed(&fixture);
    let all = request(PreviewKind::Footprint);
    let mut smd = all.clone();
    smd.filters.mount = MountFilter::ThroughHole;
    assert_eq!(names(&browser, &smd), ["Resistor_THT:R_Axial"]);
    let mut pads = all.clone();
    pads.filters.pads_min = Some(3);
    assert_eq!(
        names(&browser, &pads),
        ["Package_TO_SOT_SMD:SOT-23", "MyFootprints:Mine_SOT-23"]
    );
    // For Device:R: R_* and two pins.
    let mut for_r = all.clone();
    for_r.for_symbol = Some("Device:R".into());
    for_r.filters.match_footprint_filters = true;
    for_r.filters.match_pin_count = true;
    let items = browser.library_items(&for_r, &[], None);
    let symbol = items.for_symbol.clone().unwrap();
    assert!(symbol.found);
    assert_eq!(
        (symbol.footprint_filters.clone(), symbol.pin_count),
        (vec!["R_*".to_owned()], 2)
    );
    assert_eq!(
        names(&browser, &for_r),
        [
            "g-Resistor_SMD:R_0603",
            "Resistor_SMD:R_0402_1005Metric",
            "Resistor_THT:R_Axial"
        ]
    );
    // Only the pin count.
    for_r.filters.match_footprint_filters = false;
    assert_eq!(names(&browser, &for_r).len(), 4);
    // An installed symbol in a folder library: SOT?23* and three pins.
    let mut for_base = all.clone();
    for_base.for_symbol = Some("MySymbols:Base".into());
    for_base.filters.match_footprint_filters = true;
    for_base.filters.match_pin_count = true;
    assert_eq!(names(&browser, &for_base), ["Package_TO_SOT_SMD:SOT-23"]);
    // A symbol that is not found filters nothing.
    for_base.for_symbol = Some("Nope:X".into());
    let items = browser.library_items(&for_base, &[], None);
    assert!(!items.for_symbol.unwrap().found);
    assert_eq!(items.total, 6);
}

#[test]
fn marks_items_the_catalog_uses_and_lists_its_parts() {
    let fixture = fixture();
    let browser = indexed(&fixture);
    let items = browser.library_items(&request(PreviewKind::Symbol), &[], None);
    let in_catalog: Vec<String> = items
        .rows
        .iter()
        .filter(|row| row.in_catalog)
        .map(|row| row.reference())
        .collect();
    assert_eq!(in_catalog, ["g-pas:R_US"]);
    let detail = browser
        .library_item(
            &LibraryItemRequest {
                kind: PreviewKind::Symbol,
                library: key(LibraryGroup::Ours, "g-pas"),
                name: "R_US".into(),
                selection: None,
            },
            &[],
            None,
        )
        .unwrap();
    assert_eq!(detail.reference, "g-pas:R_US");
    assert_eq!(detail.count, 2);
    assert_eq!(detail.reference_letter, "R");
    assert_eq!(detail.default_footprint, "g-Resistor_SMD:R_0603");
    assert_eq!(detail.footprint_filters, ["R_*"]);
    assert_eq!(detail.used_by.len(), 1);
    assert_eq!(detail.used_by[0].ipn, "RES-1");
    // The team catalog's current values count: a part moved to Device:R.
    let current = BTreeMap::from([(
        "RES-1".to_owned(),
        PartValues {
            symbol: Some("Device:R".into()),
            ..PartValues::default()
        },
    )]);
    let team = Arc::new(KnownParts::new(
        "https://ma.example.com",
        "MA",
        current,
        BTreeMap::new(),
    ));
    let items = browser.library_items(&request(PreviewKind::Symbol), &[team.clone()], None);
    let in_catalog: Vec<String> = items
        .rows
        .iter()
        .filter(|row| row.in_catalog)
        .map(|row| row.reference())
        .collect();
    assert_eq!(in_catalog, ["Device:R"]);
    let footprint = browser
        .library_item(
            &LibraryItemRequest {
                kind: PreviewKind::Footprint,
                library: key(LibraryGroup::Kicad, "Resistor_THT"),
                name: "R_Axial".into(),
                selection: None,
            },
            &[],
            None,
        )
        .unwrap();
    assert_eq!(
        (footprint.mount.as_str(), footprint.count),
        ("Through-hole", 2)
    );
    assert!(footprint.used_by.is_empty());
    // The part using Device:R now is the team's part: its team comes with it.
    let device_r = browser
        .library_item(
            &LibraryItemRequest {
                kind: PreviewKind::Symbol,
                library: key(LibraryGroup::Kicad, "Device"),
                name: "R".into(),
                selection: None,
            },
            &[team.clone()],
            None,
        )
        .unwrap();
    let used: Vec<(&str, &str)> = device_r
        .used_by
        .iter()
        .map(|part| (part.team.as_str(), part.ipn.as_str()))
        .collect();
    assert_eq!(used, [("https://ma.example.com", "RES-1")]);
    // Another team chosen in the part list: MA's parts mark nothing.
    let other = browser.library_items(
        &request(PreviewKind::Symbol),
        &[team],
        Some("https://ac.example.com"),
    );
    assert!(other.rows.iter().all(|row| !row.in_catalog));
    // KiCad cannot run here, so nothing is drawn, plainly.
    let PreviewOutcome::Problem(problem) = browser.library_preview(&LibraryItemRequest {
        kind: PreviewKind::Symbol,
        library: key(LibraryGroup::Kicad, "Device"),
        name: "R".into(),
        selection: None,
    }) else {
        panic!("kicad-cli cannot run in this fixture");
    };
    assert_eq!(problem.reason, PreviewProblemReason::KicadUnavailable);
}

#[test]
fn reads_a_library_again_only_when_it_changed() {
    let fixture = fixture();
    let browser = indexed(&fixture);
    let cache = cache_folder(&fixture);
    let files: Vec<PathBuf> = fs::read_dir(&cache)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(files.len(), 9, "one cache file per library read");
    // A new app reads unchanged libraries from the cache: mark the cached
    // Device library and see the mark.
    let device = files
        .iter()
        .find(|file| fs::read_to_string(file).unwrap().contains("Q_NPN"))
        .unwrap();
    let marked = fs::read_to_string(device)
        .unwrap()
        .replace("Q_NPN", "Q_FROM_CACHE");
    fs::write(device, marked).unwrap();
    let again = indexed(&fixture);
    let mut device_items = request(PreviewKind::Symbol);
    device_items.library = Some(key(LibraryGroup::Kicad, "Device"));
    assert_eq!(
        names(&again, &device_items),
        ["Device:GND", "Device:Q_FROM_CACHE", "Device:R"]
    );
    // Changing the file (its size) reads it again, in the same app too.
    let file = fixture.share.join("symbols").join("Device.kicad_sym");
    fs::write(
        &file,
        library(&[
            symbol("R", "R", "R_*", "", &["1", "2"]),
            symbol("C", "C", "C_*", "", &["1", "2"]),
        ]),
    )
    .unwrap();
    again.libraries(true);
    assert!(again.wait_for_libraries(WAIT));
    assert_eq!(names(&again, &device_items), ["Device:C", "Device:R"]);
    // A new footprint in a folder library is found on the next refresh.
    write(
        &fixture.mine.join("Shared.pretty").join("Mine_R.kicad_mod"),
        &footprint("Mine_R", "smd", &["1", "2"]),
    );
    browser.libraries(true);
    assert!(browser.wait_for_libraries(WAIT));
    let mut shared = request(PreviewKind::Footprint);
    shared.library = Some(key(LibraryGroup::Installed, "MyFootprints"));
    assert_eq!(
        names(&browser, &shared),
        ["MyFootprints:Mine_R", "MyFootprints:Mine_SOT-23"]
    );
    // A broken library says why and keeps the others.
    fs::write(&file, "(kicad_symbol_lib (symbol \"R\"").unwrap();
    browser.libraries(true);
    assert!(browser.wait_for_libraries(WAIT));
    let view = browser.libraries(false);
    let device = view
        .symbols
        .iter()
        .find(|e| e.key.nickname == "Device")
        .unwrap();
    assert!(
        device
            .problem
            .as_deref()
            .unwrap()
            .contains("could not be read"),
        "{device:?}"
    );
    assert_eq!(device.items, None);
}

#[test]
fn never_follows_links_out_of_our_libraries() {
    let fixture = fixture();
    let outside = fixture.root.join("outside.kicad_mod");
    write(&outside, &footprint("Outside", "smd", &["1"]));
    let link = fixture
        .catalog
        .join("footprints")
        .join("g-Resistor_SMD.pretty")
        .join("Outside.kicad_mod");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&outside, &link).is_ok();
    #[cfg(not(windows))]
    let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
    if !made {
        eprintln!("links cannot be made here; skipped");
        return;
    }
    let browser = indexed(&fixture);
    let mut ours = request(PreviewKind::Footprint);
    ours.library = Some(key(LibraryGroup::Ours, "g-Resistor_SMD"));
    assert_eq!(names(&browser, &ours), ["g-Resistor_SMD:R_0603"]);
}

/// KiCad's own libraries (read only) and the installed kicad-cli: how
/// long the index takes from nothing and from its cache, and a drawing of
/// a KiCad symbol, a KiCad footprint and a symbol in a folder library.
#[test]
fn indexes_and_draws_kicads_own_libraries() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let open_real = || {
        CatalogBrowser::open(BrowserConfig {
            catalog_root: repo.clone(),
            cache_dir: dir.path().join("cache"),
            settings_file: dir.path().join("settings.json"),
            kicad_locations: SearchLocations::Standard,
            kicad_config_dir: Some(fixture.config.clone()),
            kicad_documents_dir: None,
        })
    };
    let browser = open_real();
    if !browser.status().kicad.is_ready() {
        eprintln!("KiCad 10 is not installed; skipped");
        return;
    }
    let started = Instant::now();
    browser.libraries(true);
    assert!(browser.wait_for_libraries(Duration::from_secs(600)));
    let cold = started.elapsed();
    let view = browser.libraries(false);
    let count = |entries: &[desktop_contract::LibraryEntry], group| -> usize {
        entries
            .iter()
            .filter(|entry| entry.key.group == group)
            .filter_map(|entry| entry.items)
            .sum()
    };
    let symbols = count(&view.symbols, LibraryGroup::Kicad);
    let footprints = count(&view.footprints, LibraryGroup::Kicad);
    assert!(
        symbols > 10_000 && footprints > 10_000,
        "{symbols} {footprints}"
    );
    let started = Instant::now();
    let warm = open_real();
    warm.libraries(true);
    assert!(warm.wait_for_libraries(Duration::from_secs(600)));
    let from_cache = started.elapsed();
    let started = Instant::now();
    let mut search = request(PreviewKind::Symbol);
    search.query = "op amp".into();
    let found = warm.library_items(&search, &[], None);
    let searched = started.elapsed();
    assert!(found.total > 10, "{}", found.total);
    eprintln!(
        "KiCad: {symbols} symbols, {footprints} footprints; index from nothing {cold:?}, from its cache {from_cache:?}; a search of every symbol {searched:?}"
    );
    let draw =
        |kind, group, nickname: &str, name: &str| match warm.library_preview(&LibraryItemRequest {
            kind,
            library: key(group, nickname),
            name: name.into(),
            selection: None,
        }) {
            PreviewOutcome::Ready(set) => set,
            PreviewOutcome::Problem(problem) => panic!("{nickname}:{name}: {problem:?}"),
        };
    let r = draw(PreviewKind::Symbol, LibraryGroup::Kicad, "Device", "R");
    assert!(r.from_kicad_bundle && !r.images.is_empty());
    let footprint = draw(
        PreviewKind::Footprint,
        LibraryGroup::Kicad,
        "Resistor_SMD",
        "R_0603_1608Metric",
    );
    assert_eq!(footprint.images.len(), 1);
    let child = draw(
        PreviewKind::Symbol,
        LibraryGroup::Installed,
        "MySymbols",
        "Child",
    );
    assert!(child.installed_library && !child.images.is_empty());
}

#[test]
fn ranks_names_and_keywords_before_descriptions() {
    let fixture = fixture();
    let symbol_with = |name: &str, description: &str, keywords: &str| {
        format!(
            "(symbol \"{name}\" (property \"Reference\" \"R\") (property \"Description\" \"{description}\") (property \"ki_keywords\" \"{keywords}\") (symbol \"{name}_1_1\" (pin passive line (number \"1\"))))"
        )
    };
    write(
        &fixture.share.join("symbols").join("Device.kicad_sym"),
        &library(&[
            symbol_with("Heater", "Heater, resistor wire", "heater"),
            symbol_with("R_Small", "Resistor, small symbol", "R res resistor"),
            symbol_with("R", "Resistor", "R res resistor"),
            symbol_with("Thermistor", "Temperature dependent resistor", "thermistor"),
            symbol_with("Varistor", "Voltage dependent", "varistor"),
            symbol_with("Resistor_Bridge", "Bridge", "bridge"),
        ]),
    );
    let browser = indexed(&fixture);
    let mut search = request(PreviewKind::Symbol);
    search.library = Some(key(LibraryGroup::Kicad, "Device"));
    search.query = "resistor".into();
    // A name starting with it, then keywords equal to it (shorter name
    // first), then a description word starting with it.
    assert_eq!(
        names(&browser, &search),
        [
            "Device:Resistor_Bridge",
            "Device:R",
            "Device:R_Small",
            "Device:Heater",
            "Device:Thermistor",
        ]
    );
    // A word of the name: "small" finds R_Small first.
    search.query = "small".into();
    assert_eq!(names(&browser, &search)[0], "Device:R_Small");
}

/// The one cache folder of the index.
fn cache_folder(fixture: &Fixture) -> PathBuf {
    let folders: Vec<PathBuf> = fs::read_dir(fixture.root.join("cache").join("library-index"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(folders.len(), 1, "{folders:?}");
    folders[0].clone()
}

#[test]
fn a_bad_file_is_skipped_and_the_rest_of_its_library_kept() {
    let fixture = fixture();
    let pretty = fixture.share.join("footprints").join("Resistor_SMD.pretty");
    write(&pretty.join("Broken.kicad_mod"), "(footprint \"Broken\"");
    write(&pretty.join("Other.kicad_mod"), "(kicad_symbol_lib)");
    let symdir = fixture.mine.join("Mine.kicad_symdir");
    write(
        &symdir.join("Broken.kicad_sym"),
        "(kicad_symbol_lib (symbol",
    );
    let browser = indexed(&fixture);
    let view = browser.libraries(false);
    let resistors = view
        .footprints
        .iter()
        .find(|entry| entry.key.nickname == "Resistor_SMD")
        .unwrap();
    assert_eq!(resistors.items, Some(1));
    assert_eq!(resistors.problem, None);
    let note = resistors.note.as_deref().unwrap();
    assert!(note.starts_with("2 files could not be read."), "{note}");
    assert!(note.contains("Broken.kicad_mod"), "{note}");
    let mine = view
        .symbols
        .iter()
        .find(|entry| entry.key.nickname == "MySymbols")
        .unwrap();
    assert_eq!(mine.items, Some(2));
    assert!(mine.note.as_deref().unwrap().contains("Broken.kicad_sym"));
    // The cache keeps the note.
    let again = indexed(&fixture);
    let view = again.libraries(false);
    let resistors = view
        .footprints
        .iter()
        .find(|entry| entry.key.nickname == "Resistor_SMD")
        .unwrap();
    assert!(resistors.note.is_some());
}

#[test]
fn says_when_kicads_settings_are_not_found() {
    let fixture = fixture();
    let missing = fixture.root.join("no-kicad-settings");
    let browser = open(&fixture, Some(&missing));
    browser.libraries(true);
    assert!(browser.wait_for_libraries(WAIT));
    let note = browser.libraries(false).installed_note.unwrap();
    assert!(
        note.contains("were not found at") && note.contains("no-kicad-settings"),
        "{note}"
    );
    let none = open(&fixture, None);
    none.libraries(true);
    assert!(none.wait_for_libraries(WAIT));
    assert!(
        none.libraries(false)
            .installed_note
            .unwrap()
            .contains("was not found")
    );
    // Settings with no other libraries say so too.
    let empty = fixture.root.join("empty-settings");
    fs::create_dir_all(&empty).unwrap();
    let browser = open(&fixture, Some(&empty));
    browser.libraries(true);
    assert!(browser.wait_for_libraries(WAIT));
    assert_eq!(
        browser.libraries(false).installed_note.as_deref(),
        Some("Your KiCad library tables add no other libraries.")
    );
    // With installed libraries there is no note.
    assert_eq!(indexed(&fixture).libraries(false).installed_note, None);
}

#[test]
fn removes_old_cache_layouts_and_left_over_temporary_files() {
    let fixture = fixture();
    let index = fixture.root.join("cache").join("library-index");
    write(&index.join("v1").join("old.json"), "{}");
    let browser = indexed(&fixture);
    drop(browser);
    let current = cache_folder(&fixture);
    write(&current.join("abc.json.123.tmp"), "half");
    let again = indexed(&fixture);
    drop(again);
    assert!(!index.join("v1").exists());
    assert!(!current.join("abc.json.123.tmp").exists());
    assert!(current.read_dir().unwrap().count() > 0);
}

#[test]
fn lists_each_linked_teams_reviewed_libraries_in_a_group_of_its_own() {
    use catalog_browser::library_repos::TeamLibraries;
    use kicad_integration::library_source::{LibraryList, SourceLibrary, SourceProfile};
    let fixture = fixture();
    // The files the app wrote out of a client repo's copy, at repo paths.
    let files = fixture.root.join("acme-files");
    write(
        &files.join("lib").join("Device.kicad_sym"),
        &library(&[symbol("Acme_R", "R", "", "Acme_FP:R_0603", &["1", "2"])]),
    );
    write(
        &files
            .join("lib")
            .join("Acme.pretty")
            .join("R_0603.kicad_mod"),
        &footprint("R_0603", "smd", &["1", "2"]),
    );
    let listed = |nickname: &str, path: &str| LibraryList::Listed {
        libraries: vec![SourceLibrary {
            nickname: nickname.into(),
            path: path.into(),
        }],
    };
    let acme = TeamLibraries {
        server_url: "https://acme.example.test".into(),
        code: "AC".into(),
        colour: "blue".into(),
        repo_name: "acme/kicad-lib".into(),
        profile: SourceProfile {
            // A client's own Device library, beside KiCad's.
            symbols: listed("Device", "lib/Device.kicad_sym"),
            footprints: listed("Acme_FP", "lib/Acme.pretty"),
            model_folders: vec![],
            path_variables: vec![],
            database_libraries: vec![],
            models_by_library: None,
            own_library_start: None,
        },
        folder: files,
    };
    let browser = open(&fixture, None);
    browser.libraries_with_teams(true, || (vec![acme.clone()], Vec::new()));
    assert!(browser.wait_for_libraries(WAIT));
    let view = browser.libraries(false);
    assert_eq!(view.teams.len(), 1);
    assert_eq!(view.teams[0].code, "AC");
    assert_eq!(view.teams[0].repo_name, "acme/kicad-lib");
    let symbols: Vec<(LibraryGroup, &str, &str, Option<usize>)> = view
        .symbols
        .iter()
        .map(|entry| {
            (
                entry.key.group,
                entry.key.nickname.as_str(),
                entry.key.team.as_str(),
                entry.items,
            )
        })
        .collect();
    // After ours and before KiCad's, with the repo's own nickname.
    assert_eq!(
        symbols,
        [
            (LibraryGroup::Ours, "g-pas", "", Some(2)),
            (
                LibraryGroup::Team,
                "Device",
                "https://acme.example.test",
                Some(1)
            ),
            (LibraryGroup::Kicad, "Device", "", Some(3)),
        ]
    );
    let team_key = view.symbols[1].key.clone();
    let only_acme = LibraryItemsRequest {
        library: Some(team_key),
        ..request(PreviewKind::Symbol)
    };
    assert_eq!(names(&browser, &only_acme), ["Device:Acme_R"]);
    // A later refresh with no teams lists none.
    browser.libraries_with_teams(true, || (Vec::new(), vec!["A note".into()]));
    assert!(browser.wait_for_libraries(WAIT));
    let view = browser.libraries_with_teams(false, || unreachable!());
    assert!(view.teams.is_empty());
    assert!(view.notes.contains(&"A note".to_owned()));
}

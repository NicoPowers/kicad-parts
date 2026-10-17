//! The layout of the libraries: names, groups, fields, model links, KiCad
//! library items, links that survive a revision becoming older or
//! archived, the same bytes from the same Catalog, and revision files that
//! never change.

use super::{Cad, Fixture, resistors, test_folder};
use crate::{DATABASE_FILE, DBL_FILE, LibraryKind, Outcome, UpdateError, update};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn run(fixture: &Fixture, folder: &Path) -> Result<Outcome, UpdateError> {
    let mut blobs = fixture.blobs();
    update(folder, &fixture.catalog, &mut blobs)
}

/// Every row of `table`, each as column name to value.
fn rows(folder: &Path, table: &str) -> Vec<BTreeMap<String, String>> {
    let connection = rusqlite::Connection::open(folder.join(DATABASE_FILE)).unwrap();
    let mut statement = connection
        .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY \"Entry\""))
        .unwrap();
    let names: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    statement
        .query_map([], |row| {
            Ok(names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let value: rusqlite::types::Value = row.get(index).unwrap();
                    let text = match value {
                        rusqlite::types::Value::Integer(number) => number.to_string(),
                        rusqlite::types::Value::Text(text) => text,
                        other => format!("{other:?}"),
                    };
                    (name.clone(), text)
                })
                .collect())
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn entries(folder: &Path, table: &str) -> Vec<String> {
    rows(folder, table)
        .into_iter()
        .map(|row| row["Entry"].clone())
        .collect()
}

fn dbl(folder: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(folder.join(DBL_FILE)).unwrap()).unwrap()
}

/// Every file under `folder`, by path relative to it with `/`.
fn files(folder: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, folder: &Path, found: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, found);
            } else {
                let relative = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                found.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(folder, folder, &mut found);
    found
}

fn text(folder: &Path, relative: &str) -> String {
    fs::read_to_string(folder.join(relative)).unwrap()
}

#[test]
fn each_revision_gets_an_entry_and_each_cad_revision_a_name() {
    let folder = test_folder("names");
    let fixture = resistors();
    let outcome = run(&fixture, &folder).unwrap();
    assert_eq!(outcome.problems, Vec::<String>::new());
    assert_eq!(
        (outcome.current, outcome.older, outcome.archived),
        (2, 1, 0)
    );
    assert!(outcome.database_changed && outcome.layout_changed);

    // The current revisions are under their category; the older one is in
    // its own group.
    assert_eq!(
        entries(&folder, "category_res"),
        ["RES-0000-0100_v1", "RES-0000-0470_v2"]
    );
    assert_eq!(entries(&folder, "older_revisions"), ["RES-0000-0470_v1"]);
    assert!(entries(&folder, "archived_revisions").is_empty());

    // Symbols are named with their revision, units too, in their library.
    let symbols = text(&folder, "symbols/g-res.kicad_sym");
    assert!(symbols.starts_with("(kicad_symbol_lib\n\t(version 20241209)\n"));
    for name in [
        "R_US_v1",
        "R_US_v1_0_1",
        "R_US_v1_1_1",
        "R_US_v2",
        "R_US_v2_1_1",
    ] {
        assert!(symbols.contains(&format!("(symbol \"{name}\"")), "{name}");
    }
    assert!(!symbols.contains("(symbol \"R_US\""));
    // Footprints and models too, beside their library.
    let footprint = text(
        &folder,
        "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric_v1.kicad_mod",
    );
    let model_path = format!(
        "{}/footprints/g-Resistor_SMD.3dshapes/R_0603_1608Metric_v1.step",
        folder.to_string_lossy().replace('\\', "/")
    );
    assert!(
        footprint.contains(&format!("(model \"{model_path}\"")),
        "{footprint}"
    );
    assert_eq!(fs::read(&model_path).unwrap(), b"ISO-10303-21; a model\n");
    // Only the model link differs from the stored footprint.
    let stored = super::footprint_bytes(
        "R_0603_1608Metric",
        &["${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step"],
    );
    assert_eq!(
        footprint.replace(
            &model_path,
            "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step"
        ),
        String::from_utf8(stored).unwrap()
    );
    // KiCad's own footprint is linked, not copied.
    assert!(!folder.join("footprints/Resistor_SMD.pretty").exists());

    let libraries: Vec<(LibraryKind, &str)> = outcome
        .libraries
        .iter()
        .map(|library| (library.kind, library.nickname.as_str()))
        .collect();
    assert_eq!(
        libraries,
        [
            (LibraryKind::Parts, "team-parts"),
            (LibraryKind::Symbols, "g-res"),
            (LibraryKind::Footprints, "g-Resistor_SMD"),
        ]
    );
}

#[test]
fn rows_hold_the_fields_and_link_exact_revisions() {
    let folder = test_folder("fields");
    let fixture = resistors();
    run(&fixture, &folder).unwrap();
    let current = rows(&folder, "category_res");
    let row = &current[1];
    assert_eq!(row["Entry"], "RES-0000-0470_v2");
    // The IPN stays plain; the revision has its own field.
    assert_eq!(row["IPN"], "RES-0000-0470");
    assert_eq!(row["Revision"], "2");
    assert_eq!(row["Symbol"], "g-res:R_US_v2");
    assert_eq!(row["Footprint"], "g-Resistor_SMD:R_0603_1608Metric_v1");
    assert_eq!(row["Value"], "47");
    assert_eq!(row["Manufacturer"], "Yageo");
    assert_eq!(row["MPN"], "RC0603FR-0747RL");
    assert_eq!(row["Description"], "RES 47 OHM 1% 1/10W 0603");
    assert_eq!(row["Datasheet"], "https://example.test/47.pdf");
    assert_eq!(row["Tolerance"], "1%");
    assert_eq!(row["Exclude from BOM"], "0");
    // A KiCad footprint stays as written; a blank value falls back to the
    // MPN; Excluded from BOM is a number KiCad reads.
    let kicad = &current[0];
    assert_eq!(kicad["Footprint"], "Resistor_SMD:R_0402_1005Metric");
    assert_eq!(kicad["Symbol"], "g-res:R_US_v1");
    assert_eq!(kicad["Value"], "RC0402JR-0710RL");
    assert_eq!(kicad["Exclude from BOM"], "1");
    // Older revisions keep the fields every category shares.
    let older = &rows(&folder, "older_revisions")[0];
    assert_eq!(older["Symbol"], "g-res:R_US_v1");
    assert_eq!(older["Revision"], "1");
    assert!(!older.contains_key("Tolerance"));

    let dbl = dbl(&folder);
    assert_eq!(dbl["globally_unique_keys"], true);
    assert_eq!(dbl["meta"]["version"], 1);
    assert_eq!(
        dbl["source"]["connection_string"],
        "DRIVER={SQLite3 ODBC Driver};DATABASE=${CWD}/team-parts.sqlite"
    );
    let libraries = dbl["libraries"].as_array().unwrap();
    let groups: Vec<&str> = libraries
        .iter()
        .map(|library| library["name"].as_str().unwrap())
        .collect();
    // The regulators have no parts, so they show no empty group.
    assert_eq!(
        groups,
        ["Resistors", "Older revisions", "Archived revisions"]
    );
    let resistors = &libraries[0];
    assert_eq!(resistors["key"], "Entry");
    assert_eq!(resistors["symbols"], "Symbol");
    assert_eq!(resistors["footprints"], "Footprint");
    assert_eq!(
        resistors["properties"]["exclude_from_bom"],
        "Exclude from BOM"
    );
    let fields: Vec<(&str, bool, bool)> = resistors["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| {
            (
                field["name"].as_str().unwrap(),
                field["visible_on_add"].as_bool().unwrap(),
                field["visible_in_chooser"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("IPN", false, true),
            ("Revision", false, true),
            ("Value", true, false),
            ("Manufacturer", false, true),
            ("MPN", false, true),
            ("Description", false, true),
            ("Datasheet", false, true),
            ("Tolerance", false, true),
        ]
    );
}

#[test]
fn category_fields_follow_the_old_database_library() {
    let regulators = catalog::CategoryDetails::starting(
        "reg",
        ["Description", "Voltage", "Sim_Library", "value", "Current"]
            .map(str::to_owned)
            .to_vec(),
    );
    let fields: Vec<(String, String, bool)> = crate::plan::extra_fields(&regulators)
        .into_iter()
        .map(|field| (field.column, field.field, field.shown_on_add))
        .collect();
    assert_eq!(
        fields,
        [
            ("Voltage".to_owned(), "Voltage".to_owned(), true),
            ("Sim_Library".to_owned(), "Sim.Library".to_owned(), false),
            ("Current".to_owned(), "Current".to_owned(), true),
        ]
    );
}

#[test]
fn links_stay_the_same_when_a_revision_becomes_older_or_archived() {
    let folder = test_folder("links");
    let mut fixture = resistors();
    run(&fixture, &folder).unwrap();
    let all = |folder: &Path| {
        let mut all: Vec<String> = ["category_res", "older_revisions", "archived_revisions"]
            .iter()
            .flat_map(|table| entries(folder, table))
            .collect();
        all.sort();
        all
    };
    let before = all(&folder);
    let dbl_before = fs::read(folder.join(DBL_FILE)).unwrap();

    // Archiving revision 2 makes revision 1 current again.
    fixture.archive("RES-0000-0470", 2, true);
    let outcome = run(&fixture, &folder).unwrap();
    assert_eq!(
        (outcome.current, outcome.older, outcome.archived),
        (2, 0, 1)
    );
    assert!(outcome.database_changed);
    // Every link still resolves in the same library: the entry names are
    // the same, only their groups changed, and KiCad needs no new group.
    assert_eq!(all(&folder), before);
    assert_eq!(
        entries(&folder, "category_res"),
        ["RES-0000-0100_v1", "RES-0000-0470_v1"]
    );
    assert_eq!(entries(&folder, "archived_revisions"), ["RES-0000-0470_v2"]);
    assert!(!outcome.layout_changed);
    assert_eq!(fs::read(folder.join(DBL_FILE)).unwrap(), dbl_before);
    assert_eq!(outcome.written, Vec::<String>::new());

    // Unarchiving moves it back.
    fixture.archive("RES-0000-0470", 2, false);
    run(&fixture, &folder).unwrap();
    assert_eq!(entries(&folder, "older_revisions"), ["RES-0000-0470_v1"]);
    assert_eq!(all(&folder), before);
}

#[test]
fn the_same_catalog_gives_the_same_bytes() {
    let fixture = resistors();
    let first = test_folder("same-1");
    let second = test_folder("same-2");
    run(&fixture, &first).unwrap();
    run(&fixture, &second).unwrap();
    let path_text = |folder: &PathBuf| folder.to_string_lossy().replace('\\', "/");
    let (one, two) = (files(&first), files(&second));
    assert_eq!(
        one.keys().collect::<Vec<_>>(),
        two.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in &one {
        // Footprints point at their models by full path, which names the
        // folder; nothing else may differ.
        let a = String::from_utf8_lossy(bytes).replace(&path_text(&first), "FOLDER");
        let b = String::from_utf8_lossy(&two[name]).replace(&path_text(&second), "FOLDER");
        assert!(a == b, "{name} differs");
    }
    assert!(one.contains_key(DATABASE_FILE));
    // Updating again with nothing new writes nothing.
    let again = run(&fixture, &first).unwrap();
    assert_eq!(again.written, Vec::<String>::new());
    assert!(!again.database_changed && !again.layout_changed);
    assert_eq!(files(&first), one);
}

#[test]
fn a_revisions_files_never_change_across_updates() {
    let folder = test_folder("never-change");
    let mut fixture = resistors();
    run(&fixture, &folder).unwrap();
    let before = files(&folder);
    let symbol_entries = |folder: &Path| -> BTreeMap<String, Vec<u8>> {
        let bytes = fs::read(folder.join("symbols/g-res.kicad_sym")).unwrap();
        let library = kicad_integration::library_file::SymbolLibrary::parse(&bytes).unwrap();
        library
            .entries
            .into_iter()
            .map(|entry| (entry.name, bytes[entry.span].to_vec()))
            .collect()
    };
    let entries_before = symbol_entries(&folder);

    // A third symbol revision, a second footprint revision with a new
    // model, and a part revision using them.
    let symbol = fixture.symbol("g-res:R_US", "\n\t\t(embedded_fonts yes)");
    let model = fixture.model(
        "3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step",
        b"ISO-10303-21; a better model\n",
    );
    let footprint = fixture.footprint(
        "g-Resistor_SMD:R_0603_1608Metric",
        &[(
            "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step",
            Some(&model),
        )],
    );
    fixture.revision(
        "RES-0000-0470",
        Cad::Bound(&symbol),
        &[Cad::Bound(&footprint)],
        false,
    );
    let outcome = run(&fixture, &folder).unwrap();
    assert_eq!(
        outcome.written,
        [
            "footprints/g-Resistor_SMD.3dshapes/R_0603_1608Metric_v2.step",
            "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric_v2.kicad_mod",
            "symbols/g-res.kicad_sym",
        ]
    );
    let after = files(&folder);
    for (name, bytes) in &before {
        if name.ends_with(".step") || name.ends_with(".kicad_mod") {
            assert_eq!(&after[name], bytes, "{name} changed");
        }
    }
    let entries_after = symbol_entries(&folder);
    for (name, bytes) in &entries_before {
        assert_eq!(&entries_after[name], bytes, "{name} changed");
    }
    assert!(entries_after.contains_key("R_US_v3"));
    assert_eq!(
        entries(&folder, "older_revisions"),
        ["RES-0000-0470_v1", "RES-0000-0470_v2"]
    );
}

#[test]
fn a_file_changed_outside_the_app_is_kept_and_named() {
    let folder = test_folder("changed");
    let fixture = resistors();
    run(&fixture, &folder).unwrap();
    let footprint = folder.join("footprints/g-Resistor_SMD.pretty/R_0603_1608Metric_v1.kicad_mod");
    let edited = fs::read_to_string(&footprint)
        .unwrap()
        .replace("0.825", "0.9");
    fs::write(&footprint, &edited).unwrap();
    match run(&fixture, &folder) {
        Err(UpdateError::Changed(file)) => assert_eq!(file, footprint),
        other => panic!("expected the changed footprint to be named: {other:?}"),
    }
    assert_eq!(fs::read_to_string(&footprint).unwrap(), edited);
    // A symbol edited in place is found too.
    fs::remove_file(&footprint).unwrap();
    run(&fixture, &folder).unwrap();
    let symbols = folder.join("symbols/g-res.kicad_sym");
    let edited = fs::read_to_string(&symbols)
        .unwrap()
        .replace("(length 1.27)", "(length 2.54)");
    fs::write(&symbols, &edited).unwrap();
    assert!(matches!(
        run(&fixture, &folder),
        Err(UpdateError::Changed(_))
    ));
}

#[test]
fn names_windows_refuses_are_left_out_or_encoded() {
    let folder = test_folder("windows-names");
    let mut fixture = Fixture::new();
    let symbol = fixture.symbol("CON:R", "");
    // A footprint named CON.x would be the file CON.x_v1.kicad_mod.
    let refused = fixture.footprint("g-Resistor_SMD:CON.x", &[]);
    // A library named NUL is kept, in an encoded folder.
    let kept = fixture.footprint("NUL:R_0603", &[]);
    fixture.part("RES-0000-0001", "res", &[("Value", "1")], "X");
    fixture.revision(
        "RES-0000-0001",
        Cad::Bound(&symbol),
        &[Cad::Bound(&kept), Cad::Bound(&refused)],
        false,
    );
    let outcome = run(&fixture, &folder).unwrap();
    assert_eq!(
        outcome.problems,
        [
            "The footprint g-Resistor_SMD:CON.x revision 1 was left out: Windows refuses the file name CON.x_v1.kicad_mod because Windows keeps it for a device, like CON or COM1."
        ]
    );
    assert!(folder.join("symbols/%43ON.kicad_sym").is_file());
    assert!(
        folder
            .join("footprints/%4EUL.pretty/R_0603_v1.kicad_mod")
            .is_file()
    );
    let nicknames: Vec<(&str, &str)> = outcome
        .libraries
        .iter()
        .map(|library| {
            (
                library.nickname.as_str(),
                library.path.rsplit('/').next().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        nicknames,
        [
            ("team-parts", "team-parts.kicad_dbl"),
            ("CON", "%43ON.kicad_sym"),
            ("NUL", "%4EUL.pretty"),
            ("g-Resistor_SMD", "g-Resistor_SMD.pretty"),
        ]
    );
    // The row still links every footprint by its KiCad name.
    let row = &rows(&folder, "category_res")[0];
    assert_eq!(row["Symbol"], "CON:R_v1");
    assert_eq!(row["Footprint"], "NUL:R_0603_v1;g-Resistor_SMD:CON.x_v1");
}

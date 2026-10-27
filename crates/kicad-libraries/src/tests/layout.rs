//! The layout of the libraries: names, groups, fields, model links, KiCad
//! library items, links that survive a revision becoming older or
//! archived, the same bytes from the same Catalog wherever they are
//! written, revision files that never change, and what an update does
//! with files changed outside the app and with folders of other teams.

use super::{Cad, Fixture, clear_read_only, resistors, team, test_folder};
use crate::files::{FolderLock, set_read_only};
use crate::{LOCK_FILE, LibraryKind, Naming, Outcome, STATE_FILE, Team, UpdateError, update};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const DATABASE: &str = "MA-Parts.sqlite";
const DBL: &str = "MA-Parts.kicad_dbl";
const SYMBOLS: &str = "symbols/MA-res-20241209.kicad_sym";
const FOOTPRINT: &str = "footprints/MA-Resistor_SMD.pretty/R_0603_1608Metric_v1.kicad_mod";
const MODEL: &str = "footprints/MA-Resistor_SMD.3dshapes/R_0603_1608Metric_v1.step";

/// Updates the libraries under `root`; they are written to `root/MA`.
fn run(fixture: &Fixture, root: &Path) -> Result<Outcome, UpdateError> {
    let mut blobs = fixture.blobs();
    update(root, &fixture.catalog, &team(), &mut blobs)
}

/// Every row of `table`, each as column name to value.
fn rows(folder: &Path, table: &str) -> Vec<BTreeMap<String, String>> {
    let connection = rusqlite::Connection::open(folder.join(DATABASE)).unwrap();
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
    serde_json::from_slice(&fs::read(folder.join(DBL)).unwrap()).unwrap()
}

fn groups(folder: &Path) -> Vec<String> {
    dbl(folder)["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|library| library["name"].as_str().unwrap().to_owned())
        .collect()
}

/// Every library file under `folder`, by path relative to it with `/`:
/// the state and lock files, which say when and who, are left out.
pub(super) fn files(folder: &Path) -> BTreeMap<String, Vec<u8>> {
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
                if relative != STATE_FILE && relative != LOCK_FILE {
                    found.insert(relative, fs::read(&path).unwrap());
                }
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

/// Changes a generated file as someone outside the app might.
fn edit(path: &Path, change: impl FnOnce(String) -> String) -> String {
    set_read_only(path, false).unwrap();
    let edited = change(fs::read_to_string(path).unwrap());
    fs::write(path, &edited).unwrap();
    edited
}

#[test]
fn each_revision_gets_an_entry_and_each_cad_revision_a_name() {
    let root = test_folder("names");
    let folder = root.join("MA");
    let fixture = resistors();
    let outcome = run(&fixture, &root).unwrap();
    assert_eq!(outcome.folder, folder);
    assert_eq!(outcome.problems, Vec::<String>::new());
    assert_eq!(
        (outcome.current, outcome.older, outcome.archived),
        (2, 1, 0)
    );
    assert!(outcome.database_changed && !outcome.layout_changed);
    assert!(outcome.written.contains(&DBL.to_owned()));

    // The current revisions are under their category; the older one is in
    // its own group.
    assert_eq!(
        entries(&folder, "category_res"),
        ["RES-0000-0100_v1", "RES-0000-0470_v2"]
    );
    assert_eq!(entries(&folder, "older_revisions"), ["RES-0000-0470_v1"]);
    assert!(entries(&folder, "archived_revisions").is_empty());

    // Symbols are named with their revision, units too, in a library named
    // with the team's code, the category and the file version.
    let symbols = text(&folder, SYMBOLS);
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
    // Footprints and models too, beside their library, with the link
    // relative to it, so the folder can move.
    let footprint = text(&folder, FOOTPRINT);
    let link = "../MA-Resistor_SMD.3dshapes/R_0603_1608Metric_v1.step";
    assert!(
        footprint.contains(&format!("(model \"{link}\"")),
        "{footprint}"
    );
    assert_eq!(
        fs::read(folder.join(MODEL)).unwrap(),
        b"ISO-10303-21; a model\n"
    );
    // Only the model link differs from the stored footprint.
    let stored = super::footprint_bytes(
        "R_0603_1608Metric",
        &["${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step"],
    );
    assert_eq!(
        footprint.replace(
            link,
            "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step"
        ),
        String::from_utf8(stored).unwrap()
    );
    // KiCad's own footprint is linked, not copied.
    assert!(
        !folder
            .join("footprints/MA-Resistor_SMD.pretty/R_0402_1005Metric.kicad_mod")
            .exists()
    );
    assert_eq!(fs::read_dir(folder.join("footprints")).unwrap().count(), 2);

    let libraries: Vec<(LibraryKind, &str)> = outcome
        .libraries
        .iter()
        .map(|library| (library.kind, library.nickname.as_str()))
        .collect();
    assert_eq!(
        libraries,
        [
            (LibraryKind::Parts, "MA-Parts"),
            (LibraryKind::Symbols, "MA-res-20241209"),
            (LibraryKind::Footprints, "MA-Resistor_SMD"),
        ]
    );
}

#[test]
fn generated_files_are_read_only() {
    let root = test_folder("read-only");
    let folder = root.join("MA");
    run(&resistors(), &root).unwrap();
    for file in [SYMBOLS, FOOTPRINT, MODEL, DBL] {
        assert!(
            fs::metadata(folder.join(file))
                .unwrap()
                .permissions()
                .readonly(),
            "{file}"
        );
    }
    // The database stays writable: the next update changes it in place.
    assert!(
        !fs::metadata(folder.join(DATABASE))
            .unwrap()
            .permissions()
            .readonly()
    );
    clear_read_only(&root);
}

#[test]
fn rows_hold_the_fields_and_link_exact_revisions() {
    let root = test_folder("fields");
    let folder = root.join("MA");
    let fixture = resistors();
    run(&fixture, &root).unwrap();
    let current = rows(&folder, "category_res");
    let row = &current[1];
    assert_eq!(row["Entry"], "RES-0000-0470_v2");
    // The IPN stays plain; the revision has its own field.
    assert_eq!(row["IPN"], "RES-0000-0470");
    assert_eq!(row["Revision"], "2");
    assert_eq!(row["Symbol"], "MA-res-20241209:R_US_v2");
    assert_eq!(row["Footprint"], "MA-Resistor_SMD:R_0603_1608Metric_v1");
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
    assert_eq!(kicad["Symbol"], "MA-res-20241209:R_US_v1");
    assert_eq!(kicad["Value"], "RC0402JR-0710RL");
    assert_eq!(kicad["Exclude from BOM"], "1");
    // Older revisions keep the fields every category shares.
    let older = &rows(&folder, "older_revisions")[0];
    assert_eq!(older["Symbol"], "MA-res-20241209:R_US_v1");
    assert_eq!(older["Revision"], "1");
    assert!(!older.contains_key("Tolerance"));

    let dbl = dbl(&folder);
    assert_eq!(dbl["name"], "MA-Parts");
    assert_eq!(dbl["globally_unique_keys"], true);
    assert_eq!(dbl["meta"]["version"], 1);
    assert_eq!(
        dbl["source"]["connection_string"],
        "DRIVER={SQLite3 ODBC Driver};DATABASE=${CWD}/MA-Parts.sqlite"
    );
    // Every category is listed, even with no parts yet, so a part moving
    // into it never needs a group a running KiCad has not loaded.
    assert_eq!(
        groups(&folder),
        [
            "Voltage regulators",
            "Resistors",
            "Older revisions",
            "Archived revisions"
        ]
    );
    let resistors = &dbl["libraries"][1];
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
fn category_fields_become_database_library_columns() {
    let regulators = catalog::CategoryDetails {
        fields: [
            "Description",
            "Voltage",
            "Sim_Library",
            "value",
            "Size \"A\"",
            "Current",
        ]
        .map(str::to_owned)
        .to_vec(),
        ..catalog::CategoryDetails::starting("reg")
    };
    let (fields, refused) = crate::plan::extra_fields(&regulators);
    let fields: Vec<(String, String, bool)> = fields
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
    assert_eq!(refused, ["Size \"A\""]);
}

#[test]
fn a_field_name_with_a_double_quote_is_left_out_and_reported() {
    let root = test_folder("quote");
    let folder = root.join("MA");
    let mut fixture = resistors();
    fixture.category(
        "res",
        vec!["Description", "Value", "Size \"A\"", "Tolerance"],
    );
    let outcome = run(&fixture, &root).unwrap();
    assert_eq!(
        outcome.problems,
        [
            "The field Size \"A\" of Resistors was left out of KiCad, because KiCad cannot read a field name with a double quote in it."
        ]
    );
    assert!(!rows(&folder, "category_res")[0].contains_key("Size \"A\""));
    assert!(
        !fs::read_to_string(folder.join(DBL))
            .unwrap()
            .contains("Size")
    );
    assert_eq!(rows(&folder, "category_res")[1]["Tolerance"], "1%");
}

#[test]
fn links_stay_the_same_when_a_revision_becomes_older_or_archived() {
    let root = test_folder("links");
    let folder = root.join("MA");
    let mut fixture = resistors();
    run(&fixture, &root).unwrap();
    let all = |folder: &Path| {
        let mut all: Vec<String> = ["category_res", "older_revisions", "archived_revisions"]
            .iter()
            .flat_map(|table| entries(folder, table))
            .collect();
        all.sort();
        all
    };
    let before = all(&folder);
    let dbl_before = fs::read(folder.join(DBL)).unwrap();

    // Archiving revision 2 makes revision 1 current again.
    fixture.archive("RES-0000-0470", 2, true);
    let outcome = run(&fixture, &root).unwrap();
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
    assert_eq!(fs::read(folder.join(DBL)).unwrap(), dbl_before);
    assert_eq!(outcome.written, Vec::<String>::new());

    // Unarchiving moves it back.
    fixture.archive("RES-0000-0470", 2, false);
    run(&fixture, &root).unwrap();
    assert_eq!(entries(&folder, "older_revisions"), ["RES-0000-0470_v1"]);
    assert_eq!(all(&folder), before);
}

#[test]
fn the_same_catalog_gives_the_same_bytes_wherever_it_is_written() {
    let fixture = resistors();
    let first = test_folder("same-1");
    let second = test_folder("same-2");
    run(&fixture, &first).unwrap();
    run(&fixture, &second).unwrap();
    let one = files(&first.join("MA"));
    assert!(one.contains_key(DATABASE));
    assert!(one == files(&second.join("MA")), "the two folders differ");
    // Updating again with nothing new writes nothing.
    let again = run(&fixture, &first).unwrap();
    assert_eq!(again.written, Vec::<String>::new());
    assert!(!again.database_changed && !again.layout_changed);
    assert_eq!(files(&first.join("MA")), one);
    // A moved folder needs nothing written again: the model links are
    // relative, so every file is still what the Catalog stores.
    let moved = test_folder("same-moved");
    fs::rename(second.join("MA"), moved.join("MA")).unwrap();
    let after_move = run(&fixture, &moved).unwrap();
    assert_eq!(after_move.written, Vec::<String>::new());
    assert_eq!(after_move.problems, Vec::<String>::new());
}

#[test]
fn a_revisions_files_never_change_across_updates() {
    let root = test_folder("never-change");
    let folder = root.join("MA");
    let mut fixture = resistors();
    run(&fixture, &root).unwrap();
    let before = files(&folder);
    let symbol_entries = |folder: &Path| -> BTreeMap<String, Vec<u8>> {
        let bytes = fs::read(folder.join(SYMBOLS)).unwrap();
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
    let outcome = run(&fixture, &root).unwrap();
    assert_eq!(
        outcome.written,
        [
            "footprints/MA-Resistor_SMD.3dshapes/R_0603_1608Metric_v2.step",
            "footprints/MA-Resistor_SMD.pretty/R_0603_1608Metric_v2.kicad_mod",
            SYMBOLS,
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
fn a_file_changed_outside_the_app_is_kept_and_reported_and_the_rest_goes_on() {
    let root = test_folder("changed");
    let folder = root.join("MA");
    let mut fixture = resistors();
    run(&fixture, &root).unwrap();
    let footprint = folder.join(FOOTPRINT);
    let edited = edit(&footprint, |text| text.replace("0.825", "0.9"));
    let symbols = folder.join(SYMBOLS);
    let edited_symbols = edit(&symbols, |text| {
        text.replace("(length 1.27)", "(length 2.54)")
    });
    // A new part revision, with a new symbol revision, arrives meanwhile.
    let symbol = fixture.symbol("g-res:R_US", "\n\t\t(embedded_fonts yes)");
    fixture.revision("RES-0000-0100", Cad::Bound(&symbol), &[], false);
    let outcome = run(&fixture, &root).unwrap();
    assert!(outcome.problems.contains(&format!(
        "{FOOTPRINT} was changed outside this app, so it was kept as it is. Delete that file, then update again to write it from the team catalog."
    )));
    assert!(outcome.problems.contains(&format!(
        "{SYMBOLS} was changed outside this app, so it was kept as it is. Delete that file, then update again to write it from the team catalog."
    )));
    assert!(outcome.problems.contains(
        &"RES-0000-0100_v2 uses the symbol MA-res-20241209:R_US_v3, which is not in the KiCad libraries, so KiCad shows it as missing.".to_owned()
    ));
    assert_eq!(fs::read_to_string(&footprint).unwrap(), edited);
    assert_eq!(fs::read_to_string(&symbols).unwrap(), edited_symbols);
    // The rest went on: the new revision has its row.
    assert_eq!(
        entries(&folder, "category_res"),
        ["RES-0000-0100_v2", "RES-0000-0470_v2"]
    );
    // Deleting the changed files writes them again.
    fs::remove_file(&footprint).unwrap();
    fs::remove_file(&symbols).unwrap();
    let fixed = run(&fixture, &root).unwrap();
    assert_eq!(fixed.problems, Vec::<String>::new());
    assert!(
        fs::read_to_string(&symbols)
            .unwrap()
            .contains("(symbol \"R_US_v3\"")
    );
}

#[test]
fn a_file_saved_into_the_libraries_from_outside_is_left_alone_and_reported() {
    let root = test_folder("saved-in");
    let folder = root.join("MA");
    let fixture = resistors();
    run(&fixture, &root).unwrap();
    // KiCad lets someone save into a footprint library folder, and a file
    // can be copied in by hand.
    let footprint = "footprints/MA-Resistor_SMD.pretty/My_Resistor.kicad_mod";
    let symbols = "symbols/mine.kicad_sym";
    fs::write(folder.join(footprint), "(footprint \"My_Resistor\")\n").unwrap();
    fs::write(folder.join(symbols), "(kicad_symbol_lib)\n").unwrap();
    // Hidden files and File Explorer's are not reported.
    fs::write(folder.join("symbols/desktop.ini"), "").unwrap();
    let outcome = run(&fixture, &root).unwrap();
    let said = |file: &str| {
        format!(
            "{file} did not come from the team catalog. This app leaves it alone, but no one else on the team gets it. Keep your own symbols and footprints in a library of your own."
        )
    };
    assert_eq!(outcome.problems, [said(footprint), said(symbols)]);
    assert_eq!(text(&folder, footprint), "(footprint \"My_Resistor\")\n");
    assert_eq!(text(&folder, symbols), "(kicad_symbol_lib)\n");
}

#[test]
fn symbols_saved_in_different_kicad_versions_never_share_a_file() {
    let root = test_folder("versions");
    let folder = root.join("MA");
    let mut fixture = resistors();
    fixture.symbol_version = Some("20251024".into());
    let newer = fixture.symbol("g-res:R_US", "\n\t\t(embedded_fonts yes)");
    fixture.symbol_version = Some("20260101".into());
    let too_new = fixture.symbol("g-res:R_Small", "");
    fixture.revision("RES-0000-0470", Cad::Bound(&newer), &[], false);
    fixture.revision("RES-0000-0100", Cad::Bound(&too_new), &[], false);
    let outcome = run(&fixture, &root).unwrap();
    // Each file is read by its own version.
    let old = text(&folder, SYMBOLS);
    let new = text(&folder, "symbols/MA-res-20251024.kicad_sym");
    assert!(old.contains("(version 20241209)") && old.contains("(symbol \"R_US_v2\""));
    assert!(!old.contains("R_US_v3"));
    assert!(new.contains("(version 20251024)") && new.contains("(symbol \"R_US_v3\""));
    assert!(!new.contains("R_US_v1"));
    assert_eq!(
        rows(&folder, "category_res")[1]["Symbol"],
        "MA-res-20251024:R_US_v3"
    );
    // A symbol saved by a KiCad newer than 10 is left out, and its row
    // says so.
    assert!(!folder.join("symbols/MA-res-20260101.kicad_sym").exists());
    assert_eq!(
        outcome.problems,
        [
            "The symbol g-res:R_Small revision 1 was left out, because it was saved by a KiCad newer than 10 (file version 20260101), which KiCad 10 cannot read.",
            "RES-0000-0100_v2 uses g-res:R_Small revision 1, which is not in the KiCad libraries, so KiCad shows it as missing. The symbol g-res:R_Small revision 1 was left out, because it was saved by a KiCad newer than 10 (file version 20260101), which KiCad 10 cannot read.",
        ]
    );
    let libraries: Vec<&str> = outcome
        .libraries
        .iter()
        .map(|library| library.nickname.as_str())
        .collect();
    assert_eq!(
        libraries,
        [
            "MA-Parts",
            "MA-res-20241209",
            "MA-res-20251024",
            "MA-Resistor_SMD"
        ]
    );
}

#[test]
fn a_case_clash_keeps_the_older_library_and_drops_nothing() {
    let root = test_folder("case-clash");
    let folder = root.join("MA");
    let mut fixture = Fixture::new();
    // g-res is saved first; g-Res sorts before it but comes later.
    let older = fixture.symbol("g-res:R_US", "");
    let newer = fixture.symbol("g-Res:Q", "");
    let footprint = fixture.footprint("g-Resistor_SMD:R_0603", &[]);
    let clashing = fixture.footprint("g-resistor_SMD:R_0805", &[]);
    let same_name = fixture.footprint("g-Resistor_SMD:r_0603", &[]);
    fixture.part("RES-0000-0001", "res", &[("Value", "1")], "X");
    fixture.revision(
        "RES-0000-0001",
        Cad::Bound(&older),
        &[Cad::Bound(&footprint), Cad::Bound(&clashing)],
        false,
    );
    fixture.part("RES-0000-0002", "res", &[("Value", "2")], "Y");
    fixture.revision(
        "RES-0000-0002",
        Cad::Bound(&newer),
        &[Cad::Bound(&same_name)],
        false,
    );
    let outcome = run(&fixture, &root).unwrap();
    // Both symbol libraries keep their symbols, in files of their own.
    assert!(text(&folder, SYMBOLS).contains("(symbol \"R_US_v1\""));
    assert!(text(&folder, "symbols/MA-g-Res-20241209.kicad_sym").contains("(symbol \"Q_v1\""));
    assert!(
        folder
            .join("footprints/MA-Resistor_SMD.pretty/R_0603_v1.kicad_mod")
            .is_file()
    );
    assert!(
        folder
            .join("footprints/MA-g-resistor_SMD.pretty/R_0805_v1.kicad_mod")
            .is_file()
    );
    let rows = rows(&folder, "category_res");
    assert_eq!(rows[0]["Symbol"], "MA-res-20241209:R_US_v1");
    assert_eq!(
        rows[0]["Footprint"],
        "MA-Resistor_SMD:R_0603_v1;MA-g-resistor_SMD:R_0805_v1"
    );
    assert_eq!(rows[1]["Symbol"], "MA-g-Res-20241209:Q_v1");
    // A footprint whose name differs only in case from an older one in its
    // library cannot have a file of its own on Windows: it is left out,
    // and its row says so.
    assert_eq!(
        outcome.problems,
        [
            "The footprint g-Resistor_SMD:r_0603 revision 1 was left out, because an older footprint of g-Resistor_SMD has the same name apart from upper and lower case, and Windows sees one file.",
            "RES-0000-0002_v1 uses g-Resistor_SMD:r_0603 revision 1, which is not in the KiCad libraries, so KiCad shows it as missing. The footprint g-Resistor_SMD:r_0603 revision 1 was left out, because an older footprint of g-Resistor_SMD has the same name apart from upper and lower case, and Windows sees one file.",
        ]
    );
}

#[test]
fn names_windows_refuses_are_left_out_or_encoded() {
    let root = test_folder("windows-names");
    let folder = root.join("MA");
    let mut fixture = Fixture::new();
    let symbol = fixture.symbol("g-res:R", "");
    // A footprint named CON.x would be the file CON.x_v1.kicad_mod.
    let refused = fixture.footprint("g-Resistor_SMD:CON.x", &[]);
    // A library named with a ? is kept, in an encoded folder.
    let kept = fixture.footprint("g-a?b:R_0603", &[]);
    fixture.part("RES-0000-0001", "res", &[("Value", "1")], "X");
    fixture.revision(
        "RES-0000-0001",
        Cad::Bound(&symbol),
        &[Cad::Bound(&kept), Cad::Bound(&refused)],
        false,
    );
    let outcome = run(&fixture, &root).unwrap();
    assert_eq!(
        outcome.problems,
        [
            "The footprint g-Resistor_SMD:CON.x revision 1 was left out: Windows refuses the file name CON.x_v1.kicad_mod because Windows keeps it for a device, like CON or COM1.",
            "RES-0000-0001_v1 uses g-Resistor_SMD:CON.x revision 1, which is not in the KiCad libraries, so KiCad shows it as missing. The footprint g-Resistor_SMD:CON.x revision 1 was left out: Windows refuses the file name CON.x_v1.kicad_mod because Windows keeps it for a device, like CON or COM1.",
        ]
    );
    assert!(
        folder
            .join("footprints/MA-a%3Fb.pretty/R_0603_v1.kicad_mod")
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
            ("MA-Parts", "MA-Parts.kicad_dbl"),
            ("MA-res-20241209", "MA-res-20241209.kicad_sym"),
            ("MA-Resistor_SMD", "MA-Resistor_SMD.pretty"),
            ("MA-a?b", "MA-a%3Fb.pretty"),
        ]
    );
    // The row still links every footprint by its KiCad name.
    let row = &rows(&folder, "category_res")[0];
    assert_eq!(
        row["Footprint"],
        "MA-a?b:R_0603_v1;MA-Resistor_SMD:CON.x_v1"
    );
}

#[test]
fn rows_and_symbols_are_kept_when_the_catalog_no_longer_lists_them() {
    let root = test_folder("never-deleted");
    let folder = root.join("MA");
    let mut fixture = resistors();
    run(&fixture, &root).unwrap();
    // A reading in which one part's details cannot be read, or which
    // missed a record.
    let part = fixture
        .catalog
        .parts
        .iter()
        .position(|part| part.ipn == "RES-0000-0470")
        .unwrap();
    fixture.catalog.parts.remove(part);
    let outcome = run(&fixture, &root).unwrap();
    assert_eq!(
        outcome.problems,
        [
            "RES-0000-0470_v1 is not in this reading of the team catalog, so its row was kept as it was.",
            "RES-0000-0470_v2 is not in this reading of the team catalog, so its row was kept as it was.",
        ]
    );
    assert_eq!(
        entries(&folder, "category_res"),
        ["RES-0000-0100_v1", "RES-0000-0470_v2"]
    );
    assert_eq!(entries(&folder, "older_revisions"), ["RES-0000-0470_v1"]);
}

#[test]
fn one_update_writes_a_folder_at_a_time() {
    let root = test_folder("busy");
    let folder = root.join("MA");
    let fixture = resistors();
    run(&fixture, &root).unwrap();
    let held = FolderLock::take(&folder).unwrap().unwrap();
    assert!(matches!(run(&fixture, &root), Err(UpdateError::Busy)));
    drop(held);
    run(&fixture, &root).unwrap();
}

#[test]
fn each_team_has_its_own_folder_and_another_teams_folder_is_never_written() {
    let root = test_folder("teams");
    let fixture = resistors();
    run(&fixture, &root).unwrap();
    let other = Team {
        id: "team-settings-sl".into(),
        server: "http://127.0.0.1:8091".into(),
        naming: Naming::new("SL").unwrap(),
    };
    let mut blobs = fixture.blobs();
    let outcome = update(&root, &fixture.catalog, &other, &mut blobs).unwrap();
    assert_eq!(outcome.folder, root.join("SL"));
    assert!(root.join("SL/SL-Parts.kicad_dbl").is_file());
    assert!(root.join("SL/symbols/SL-res-20241209.kicad_sym").is_file());
    let folders: Vec<(String, String)> = crate::team_folders(&root)
        .into_iter()
        .map(|(_, state)| (state.code, state.team_id))
        .collect();
    assert_eq!(
        folders,
        [
            ("MA".to_owned(), "team-settings-ma".to_owned()),
            ("SL".to_owned(), "team-settings-sl".to_owned())
        ]
    );
    // A third team that chose MA too finds the folder taken.
    let clash = Team {
        id: "team-settings-other".into(),
        server: "http://127.0.0.1:8092".into(),
        naming: Naming::new("MA").unwrap(),
    };
    let before = files(&root.join("MA"));
    let mut blobs = fixture.blobs();
    match update(&root, &fixture.catalog, &clash, &mut blobs) {
        Err(error @ UpdateError::OtherTeam { .. }) => {
            let message = error.message(&root);
            assert!(
                message.starts_with("Two teams on this computer use the code MA, and KiCad can't hold both. The folder ")
                    && message.contains(" holds the MA libraries of the team on http://127.0.0.1:8090, so it was not changed. To fix this, a superuser of http://127.0.0.1:8092 changes this team's code before any board uses it. Then sign in again."),
                "{message}"
            );
        }
        other => panic!("expected the folder to be refused: {other:?}"),
    }
    assert_eq!(files(&root.join("MA")), before);
    assert_eq!(
        crate::team::code_used_by_other_team(&root, "MA", "team-settings-other")
            .map(|state| state.server),
        Some("http://127.0.0.1:8090".to_owned())
    );
    assert_eq!(
        crate::team::code_used_by_other_team(&root, "MA", "team-settings-ma"),
        None
    );
}

#[test]
fn a_folder_the_odbc_driver_cannot_read_is_refused() {
    for name in ["semi;colon", "brace{d}"] {
        let root: PathBuf = test_folder("odbc-path").join(name);
        match run(&resistors(), &root) {
            Err(error @ UpdateError::Folder(_)) => assert!(
                error.to_string().contains("has a ; or a brace in its path"),
                "{error}"
            ),
            other => panic!("expected {name} to be refused: {other:?}"),
        }
        assert!(!root.exists());
    }
}

#[test]
fn a_new_category_says_kicad_needs_a_restart() {
    let root = test_folder("new-group");
    let folder = root.join("MA");
    let mut fixture = resistors();
    let first = run(&fixture, &root).unwrap();
    assert_eq!(first.layout_changed_at_ms, None);
    // A part in an existing category with no parts yet changes no group.
    let symbol = fixture.symbol("g-reg:LDO", "");
    fixture.part(
        "REG-0000-0001",
        "reg",
        &[("Voltage", "3.3V"), ("Current", "1A")],
        "AP2112K-3.3",
    );
    fixture.revision("REG-0000-0001", Cad::Bound(&symbol), &[], false);
    let outcome = run(&fixture, &root).unwrap();
    assert!(!outcome.layout_changed);
    let row = &rows(&folder, "category_reg")[0];
    assert_eq!(
        (row["Voltage"].as_str(), row["Value"].as_str()),
        ("3.3V", "AP2112K-3.3")
    );
    // A new category is a new group, which a running KiCad sees only
    // after a restart; the time is kept for later updates.
    fixture.category("cap", vec!["Value"]);
    let changed = run(&fixture, &root).unwrap();
    assert!(changed.layout_changed);
    let at = changed.layout_changed_at_ms.unwrap();
    assert_eq!(groups(&folder)[0], "Capacitors");
    let later = run(&fixture, &root).unwrap();
    assert!(!later.layout_changed);
    assert_eq!(later.layout_changed_at_ms, Some(at));
    // Two new libraries since the last update are named for the setup.
    let footprint = fixture.footprint("g-Capacitor_SMD:C_0603", &[]);
    let cap = fixture.symbol("g-cap:C", "");
    fixture.part("CAP-0000-0001", "cap", &[("Value", "1u")], "C1");
    fixture.revision(
        "CAP-0000-0001",
        Cad::Bound(&cap),
        &[Cad::Bound(&footprint)],
        false,
    );
    let added = run(&fixture, &root).unwrap();
    let new: Vec<&str> = added
        .new_libraries
        .iter()
        .map(|library| library.nickname.as_str())
        .collect();
    assert_eq!(new, ["MA-cap-20241209", "MA-Capacitor_SMD"]);
}

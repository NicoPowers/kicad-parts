//! The loop the guide gives a hardware engineer
//! (`docs/operations/MOVE_THE_CSV_CATALOG.md`) works: fix a value in a CSV
//! file, run the full-catalog proof, record the change in the CSV catalog
//! fixture, and run it again. The first run stops at once and says how to
//! record the change; the second passes, although the frozen
//! `parts.sqlite` still has the old value.
//!
//! It works on a copy of the CSV catalog under `target/`, imported into
//! disposable PocketBase servers on 127.0.0.1. The repository's own files
//! are only read. It takes about 45 seconds.

use catalog_import::full_proof;
use catalog_import::services::{self, Allow};
use std::fs;
use std::path::Path;

const FOLDERS: [&str; 5] = [
    "database",
    "symbols",
    "footprints",
    "3d-models",
    "fixtures/import/csv-catalog",
];

fn copy_folder(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_folder(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

#[test]
fn an_engineers_csv_fix_passes_once_the_fixture_records_it() {
    let root = services::repository_root();
    let folder = root.join("target").join("full-proof-tests").join("csv-fix");
    let _ = fs::remove_dir_all(&folder);
    let copy = folder.join("repository");
    for name in FOLDERS {
        copy_folder(&root.join(name), &copy.join(name));
    }
    let manifest = copy.join("fixtures/import/csv-catalog/manifest.json");
    let allow = Allow {
        uncommitted_changes: true,
        ..Allow::default()
    };

    // The engineer adds "4-PAD" to one crystal's description.
    let osc = copy.join("database/g-osc.csv");
    let text = fs::read_to_string(&osc).unwrap();
    let fixed = text.replacen(
        "CRYSTAL 25.0000MHZ 18PF SMD,",
        "CRYSTAL 25.0000MHZ 18PF SMD 4-PAD,",
        1,
    );
    assert_ne!(fixed, text);
    fs::write(&osc, fixed).unwrap();

    // The proof stops at once and says how to record the change.
    let old = csv_catalog_fixture::load_manifest(&manifest).unwrap();
    let error = full_proof::run(&copy, &old, &folder.join("first"), allow)
        .err()
        .expect("a change the fixture has not recorded stops the proof");
    assert!(
        error.contains("has changed since its fixture was recorded")
            && error.contains("csv-catalog-fixture -- write ."),
        "{error}"
    );

    // Recorded, the proof passes with the new value, and says why it
    // differs from the frozen parts.sqlite.
    let recorded = csv_catalog_fixture::build_manifest(&copy).unwrap();
    fs::write(
        &manifest,
        csv_catalog_fixture::manifest_json(&recorded).unwrap(),
    )
    .unwrap();
    let proof = full_proof::run(&copy, &recorded, &folder.join("second"), allow).unwrap();
    assert_eq!(proof.failures(), Vec::<String>::new());
    assert!(
        proof
            .comparison
            .differences
            .iter()
            .any(|found| found.rule == Some("parts-sqlite-is-older")
                && found.difference.place() == "osc OSC-0001-0256 Description"),
        "{:#?}",
        proof.comparison.differences.len()
    );
    assert!(
        proof
            .whole
            .not_imported
            .iter()
            .any(|item| item.what.contains("sqlite-cell-differs-from-csv")),
        "{:#?}",
        proof.whole.not_imported
    );
    let _ = fs::remove_dir_all(&folder);
}

//! The loop the guide gives a hardware engineer
//! (`docs/operations/MOVE_THE_CSV_CATALOG.md`) works: fix a value in a CSV
//! file, run the full-catalog proof, record the change in the CSV catalog
//! fixture, and run it again.
//!
//! No team server is needed to show it. The first run stops before it
//! starts one, so `check_fixture_is_current` stands for it. After the
//! fixture is recorded, what matters is that the fixture's own record of
//! the change is read back as a place where `parts.sqlite` is older, and
//! explained. That reading is what the unit tests in
//! `csv_kicad_library/tests.rs` cannot show, because they plant the
//! record by hand; the rule itself, and that the generated library must
//! then have the new value, are tested there. `full-pilot` in the gate runs
//! the whole proof on the real catalog.
//!
//! It works on a copy of the CSV catalog under `target/`; the repository's
//! own files are only read. It takes a few seconds.

use catalog_import::csv_kicad_library::{self, Difference, GeneratedSide};
use catalog_import::full_proof;
use catalog_import::services;
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

    // The proof stops before it starts a team server, and says how to
    // record the change.
    let old =
        csv_catalog_fixture::load_manifest(&copy.join("fixtures/import/csv-catalog/manifest.json"))
            .unwrap();
    let error = full_proof::check_fixture_is_current(&copy, &old)
        .expect_err("a change the fixture has not recorded stops the proof");
    assert!(
        error.contains("has changed since its fixture was recorded")
            && error.contains("csv-catalog-fixture -- write ."),
        "{error}"
    );

    // Recorded, the proof may start, and the fixture's record of the
    // change explains why parts.sqlite is older at exactly that place.
    let recorded = csv_catalog_fixture::build_manifest(&copy).unwrap();
    full_proof::check_fixture_is_current(&copy, &recorded).unwrap();
    let csv = csv_kicad_library::load::csv_side(&copy, &recorded).unwrap();
    let comparison = csv_kicad_library::compare(&csv, &GeneratedSide::default());
    let older: Vec<(String, Option<&str>)> = comparison
        .differences
        .iter()
        .filter(|found| matches!(found.difference, Difference::PartsSqliteIsOlder { .. }))
        .map(|found| (found.difference.place(), found.rule))
        .collect();
    assert_eq!(
        older,
        [(
            "osc OSC-0001-0256 Description".to_owned(),
            Some("parts-sqlite-is-older")
        )]
    );
    let _ = fs::remove_dir_all(&folder);
}

//! The library check in a CSV import (kicad-parts-da65), on a real,
//! disposable PocketBase: an import with only old problems (clean-up)
//! goes ahead, and one whose footprint was changed in KiCad to link a 3D
//! model outside our library is refused before anything is sent, with a
//! plain list of what to fix.
//!
//! The test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; every changed input is a temp copy (not a git checkout, so the
//! commit-first and checkout checks have nothing to compare with).

use catalog::CsvCatalog;
use catalog::library_check::Group;
use catalog::pocketbase::{ImportOutcome, read_back, submit_import};
use catalog_import::services::{self, Allow};
use std::fs;
use std::path::Path;

const REASON: &str = "Import the resistors from database/g-res.csv";
const FOOTPRINT: &str = "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod";
const KICAD_MODEL: &str = "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step";

/// Copies the files the resistor import reads into `target`.
fn copy_inputs(target: &Path) {
    for path in [
        "database/g-res.csv",
        "database/substitutes.csv",
        "symbols/g-res.kicad_sym",
        FOOTPRINT,
    ] {
        let destination = target.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(services::repository_root().join(path), destination).unwrap();
    }
}

/// Points the copy's footprint's 3D model at `link`.
fn link_model(root: &Path, link: &str) {
    let file = root.join(FOOTPRINT);
    let text = fs::read_to_string(&file).unwrap();
    let from = if text.contains(KICAD_MODEL) {
        KICAD_MODEL.to_owned()
    } else {
        let start = text.find("(model \"").unwrap() + "(model \"".len();
        let end = start + text[start..].find('"').unwrap();
        text[start..end].to_owned()
    };
    fs::write(&file, text.replacen(&from, link, 1)).unwrap();
}

#[test]
fn old_problems_import_and_a_new_bad_3d_model_link_is_refused_before_sending() {
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    copy_inputs(root);
    // The footprint already links a model in someone's home folder before
    // the first import: an old problem, so clean-up.
    link_model(root, "/home/jeffs/Workspace/Cliff/3d-models/R_0603.step");
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) =
        services::start(&services::repository_root(), state.path()).unwrap();
    let import = CsvCatalog::new(root).import("g-res", None).unwrap();
    let (command, check) =
        services::prepare_checked_import(&admin, root, &import, REASON, Allow::default()).unwrap();
    assert_eq!(check.count(Group::ToFix), 0, "{}", check.report());
    assert_eq!(check.count(Group::CleanUp), 1, "{}", check.report());
    let reply = submit_import(&admin, &command).unwrap();
    assert!(
        matches!(reply.outcome, ImportOutcome::Applied { .. }),
        "{}",
        reply.body
    );
    // Imported again unchanged: still only clean-up, and it goes ahead.
    let (_, again) =
        services::prepare_checked_import(&admin, root, &import, REASON, Allow::default()).unwrap();
    assert_eq!(again.count(Group::ToFix), 0, "{}", again.report());
    let clean_up: Vec<&str> = again
        .in_group(Group::CleanUp)
        .map(|finding| finding.message.as_str())
        .collect();
    assert!(
        clean_up[0].ends_with("The Catalog already has the footprint like this."),
        "{clean_up:?}"
    );

    // Someone changes the footprint in KiCad and links a model on their
    // own drive: the import is refused before anything is sent.
    link_model(root, "C:/Users/me/models/R_0603.step");
    let changed = CsvCatalog::new(root).import("g-res", None).unwrap();
    let before = read_back(&admin).unwrap();
    let Err(refused) = services::prepare_import(&admin, root, &changed, REASON, Allow::default())
    else {
        panic!("not refused");
    };
    assert_eq!(
        refused,
        format!(
            "The library check found a problem in files this import uses, so nothing was sent. Fix it in KiCad, commit, and import again:\n- {FOOTPRINT}: The footprint links the 3D model C:/Users/me/models/R_0603.step, which is outside our library, so it only works on one computer. Put the file in 3d-models/g-Resistor_SMD.3dshapes/ and link it as ${{GITPLM_PARTS}}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step. The footprint changed since the last import."
        )
    );
    // Allowing uncommitted changes or an older checkout does not allow it.
    let everything = Allow {
        older_checkout: true,
        uncommitted_changes: true,
    };
    assert!(services::prepare_import(&admin, root, &changed, REASON, everything).is_err());
    assert!(read_back(&admin).unwrap() == before, "nothing was sent");

    // Linked as the check says, with the model in our library: it imports.
    link_model(
        root,
        "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step",
    );
    let model = root.join("3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step");
    fs::create_dir_all(model.parent().unwrap()).unwrap();
    fs::write(&model, "ISO-10303-21;\nEND-ISO-10303-21;\n").unwrap();
    let fixed = CsvCatalog::new(root).import("g-res", None).unwrap();
    let (command, check) =
        services::prepare_checked_import(&admin, root, &fixed, REASON, Allow::default()).unwrap();
    assert_eq!(check.count(Group::ToFix), 0, "{}", check.report());
    let reply = submit_import(&admin, &command).unwrap();
    assert!(
        matches!(reply.outcome, ImportOutcome::Applied { .. }),
        "{}",
        reply.body
    );
    server.stop();
}

/// Copies the CSV catalog's folders into `target`.
fn copy_catalog(target: &Path) {
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let destination = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        copy_tree(&services::repository_root().join(tree), &target.join(tree));
    }
}

/// Today's CSV catalog, imported one CSV file at a time into one team
/// catalog: no import stops, because a CSV file's first import takes the
/// footprints it uses as they are (their old problems are clean-up).
#[test]
fn todays_csv_files_import_one_by_one() {
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    copy_catalog(root);
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) =
        services::start(&services::repository_root(), state.path()).unwrap();
    let mut clean_up = 0;
    for csv_file in services::csv_files(root).unwrap() {
        let import = CsvCatalog::new(root).import(&csv_file, None).unwrap();
        let reason = format!("Import the parts from database/{csv_file}.csv");
        let (command, check) =
            services::prepare_checked_import(&admin, root, &import, &reason, Allow::default())
                .unwrap_or_else(|refusal| panic!("{csv_file}: {refusal}"));
        assert_eq!(
            check.count(Group::ToFix),
            0,
            "{csv_file}: {}",
            check.report()
        );
        clean_up = clean_up.max(check.count(Group::CleanUp));
        let reply = submit_import(&admin, &command).unwrap();
        assert!(
            matches!(reply.outcome, ImportOutcome::Applied { .. }),
            "{csv_file}: {}",
            reply.body
        );
    }
    // Today's old problems: 3D model links outside the library, and two
    // missing model files.
    assert!(clean_up >= 14, "{clean_up}");
    server.stop();
}

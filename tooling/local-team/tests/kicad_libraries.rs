//! The KiCad libraries made from a real team catalog (kicad-parts-f397),
//! checked by the installed KiCad 10:
//!
//! - kicad-cli draws every generated symbol and footprint;
//! - its electrical rules check resolves database library entries through
//!   the SQLite3 ODBC driver, including revisions that became older or
//!   archived, and finds each one's real symbol: the schematic saves an
//!   exact copy of what the library gives, and KiCad finds no difference;
//! - its design rules check finds a generated footprint by its link;
//! - its STEP export finds the footprint's 3D model by its relative link
//!   after the folder has moved.
//!
//! The libraries are written to a folder whose name is not plain ASCII, as
//! a Windows user name may be. kicad-cli runs only through run_kicad_cli,
//! with a settings folder of the test's own. A disposable PocketBase on
//! 127.0.0.1 has the sample library's resistors and capacitors; the
//! libraries are written under target/.

use catalog::PartRevision;
use catalog::pocketbase::{
    ArchiveCommand, CatalogReadback, EditCommand, read_back, read_team, submit_archive,
    submit_edit, submit_team_code,
};
use collaboration::PocketBaseClient;
use kicad_integration::library_file::model_references;
use kicad_integration::{CliSettingsFolder, KicadInstall};
use kicad_libraries::{Library, Naming, Team, setup, update};
use local_team::services;
use std::fs;
use std::path::Path;

mod support;
use support::kicad::{
    draw_every_footprint, draw_every_symbol, erc, installed_kicad, plain_box, run, saved_copy,
    schematic, target_folder,
};

/// Changes the symbol of `ipn` to `symbol`, which adds a revision.
fn change_symbol(admin: &PocketBaseClient, ipn: &str, symbol: &str) {
    let readback = read_back(admin).unwrap();
    let part = readback.part(ipn).unwrap();
    let revision: PartRevision = serde_json::from_str(
        &readback
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut edit = EditCommand::new(ipn, &format!("Use {symbol}"))
        .editing_cad(&revision, part.current_revision_number);
    assert!(edit.set_field("Symbol", symbol));
    let reply = submit_edit(admin, &edit).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
}

#[test]
fn kicad_loads_every_generated_library_and_resolves_every_entry() {
    let folder = target_folder("real-kicad");
    let settings = CliSettingsFolder::new(folder.join("kicad-cli-settings"));
    let Some(kicad) = installed_kicad(&settings) else {
        return;
    };
    let share = kicad
        .share_dir
        .clone()
        .expect("KiCad's own libraries next to kicad-cli");
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    sample_library::seed(&admin, &["res", "cap"]).unwrap();
    submit_team_code(&admin, "MA").unwrap();
    // RES-0000-0470 gets revision 2, so revision 1 is older.
    change_symbol(&admin, "RES-0000-0470", "Device:R");
    // RES-0000-1000 gets revision 2, which is archived, so revision 1 is
    // current again.
    change_symbol(&admin, "RES-0000-1000", "Device:R_Small");
    let reply = submit_archive(
        &admin,
        &ArchiveCommand::new("RES-0000-1000", 2, true, "Archive the trial symbol"),
    )
    .unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let readback: CatalogReadback = read_back(&admin).unwrap();
    let settings_record = read_team(&admin).unwrap();
    let server_url = server.url().to_owned();
    server.stop();
    let team = Team {
        id: settings_record.id,
        server: server_url,
        naming: Naming::new(&settings_record.code).unwrap(),
    };

    // A folder name that is not plain ASCII, through the real ODBC driver.
    let libraries_root = folder.join("bibliothèques");
    let outcome = update(&libraries_root, &readback, &team, &mut |blob| {
        Ok(blob.bytes.clone())
    })
    .unwrap();
    let libraries = outcome.folder.clone();
    assert_eq!(outcome.problems, Vec::<String>::new());
    assert!(outcome.older >= 1 && outcome.archived == 1, "{outcome:?}");

    let out = folder.join("svg");
    let symbols = draw_every_symbol(&kicad, &settings, &outcome.libraries, &out);
    let (footprints, with_model) =
        draw_every_footprint(&kicad, &settings, &outcome.libraries, &out);
    assert!(
        symbols >= 2 && footprints >= 2,
        "{symbols} symbols, {footprints} footprints"
    );
    let (footprint_link, footprint_file) =
        with_model.expect("the capacitor footprint has a 3D model from the team catalog");
    check_footprint_link(
        &kicad,
        &settings,
        &folder,
        &outcome.libraries,
        &footprint_link,
        &footprint_file,
    );
    // The folder moves: KiCad still finds the model by its relative link.
    let moved = folder.join("moved").join("MA");
    copy_folder(&libraries, &moved);
    check_model_found(&kicad, &settings, &folder, &moved, &footprint_link);

    // A project whose symbol library table is the setup text, plus KiCad's
    // Device library for the revisions that use KiCad's own symbols. Device
    // goes first, before the database library that draws from it.
    let project = folder.join("project");
    fs::create_dir_all(&project).unwrap();
    let device = share.join("symbols").join("Device.kicad_sym");
    let table = setup::symbol_table(&outcome.libraries).replacen(
        "  (version 7)\n",
        &format!(
            "  (version 7)\n  (lib (name \"Device\")(type \"KiCad\")(uri \"{}\")(options \"\")(descr \"\"))\n",
            device.to_string_lossy().replace('\\', "/")
        ),
        1,
    );
    fs::write(project.join("sym-lib-table"), table).unwrap();
    fs::write(project.join("check.kicad_pro"), "{}\n").unwrap();
    let real = [
        // Older: revision 1 after revision 2 was added.
        "MA-Parts:RES-0000-0470_v1",
        // Current, using KiCad's own symbol.
        "MA-Parts:RES-0000-0470_v2",
        // Archived.
        "MA-Parts:RES-0000-1000_v2",
        // Current again after its newer revision was archived.
        "MA-Parts:RES-0000-1000_v1",
        // A capacitor, in another category.
        "MA-Parts:CAP-0000-0020_v1",
    ];
    let mut placed: Vec<(&str, String)> = real
        .iter()
        .map(|link| (*link, saved_copy(&libraries, &share, link)))
        .collect();
    // A wrong copy, which KiCad must tell apart from the library's symbol,
    // and an entry that does not exist.
    placed.push((
        "MA-Parts:CAP-0000-0100_v1",
        plain_box("MA-Parts:CAP-0000-0100_v1"),
    ));
    placed.push((
        "MA-Parts:RES-0000-9999_v1",
        plain_box("MA-Parts:RES-0000-9999_v1"),
    ));
    let sheet = project.join("check.kicad_sch");
    fs::write(&sheet, schematic(&placed)).unwrap();
    let found = erc(&kicad, &settings, &sheet);
    let said = |kind: &str, text: &str| {
        found
            .iter()
            .any(|(found_kind, description)| found_kind == kind && description.contains(text))
    };
    // KiCad found every real entry and its exact symbol: the copy the
    // schematic saved matches what the library gives.
    for link in real {
        let entry = link.split_once(':').unwrap().1;
        assert!(
            !said("lib_symbol_issues", &format!("'{entry}'"))
                && !said("lib_symbol_mismatch", &format!("'{entry}'")),
            "{link}: {found:#?}"
        );
    }
    // A wrong copy differs, and a missing entry is not found.
    assert!(
        said(
            "lib_symbol_mismatch",
            "Symbol 'CAP-0000-0100_v1' doesn't match copy in library 'MA-Parts'"
        ),
        "{found:#?}"
    );
    assert!(
        said(
            "lib_symbol_issues",
            "Symbol 'RES-0000-9999_v1' not found in symbol library 'MA-Parts'"
        ),
        "{found:#?}"
    );
}

/// Copies `from` to `to`, files and folders.
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

/// A board holding the generated footprint in `file` under the link a
/// design stores, `placed`.
fn board_with(file: &Path, placed: &str) -> Vec<u8> {
    let text = fs::read_to_string(file).unwrap();
    let name = file.file_stem().unwrap().to_string_lossy().into_owned();
    let saved_name = name.rsplit_once("_v").unwrap().0;
    let placed_text = text.replacen(
        &format!("(footprint \"{saved_name}\""),
        &format!("(footprint \"{placed}\""),
        1,
    );
    assert_ne!(placed_text, text, "{name} starts with its own name");
    kicad_integration::board::one_footprint_board(
        placed_text.as_bytes(),
        &[],
        [-3.0, -3.0, 3.0, 3.0],
    )
    .unwrap()
}

/// KiCad's design rules check finds the footprint `link` (such as
/// `MA-Inductor_SMD:L_..._v1`) in the generated footprint library, the
/// same as the file `file`, 3D model link included, and says plainly when
/// a link names no footprint.
fn check_footprint_link(
    kicad: &KicadInstall,
    settings: &CliSettingsFolder,
    folder: &Path,
    libraries: &[Library],
    link: &str,
    file: &Path,
) {
    let board = folder.join("board");
    fs::create_dir_all(&board).unwrap();
    fs::write(board.join("check.kicad_pro"), "{}\n").unwrap();
    fs::write(
        board.join("fp-lib-table"),
        setup::footprint_table(libraries),
    )
    .unwrap();
    let (library, _) = link.split_once(':').unwrap();
    for (placed, expected) in [
        (link.to_owned(), None),
        (
            format!("{library}:No_such_footprint_v1"),
            Some(format!(
                "Footprint 'No_such_footprint_v1' not found in library '{library}'"
            )),
        ),
    ] {
        let pcb_file = board.join("check.kicad_pcb");
        fs::write(&pcb_file, board_with(file, &placed)).unwrap();
        let report_file = board.join("drc.json");
        let _ = fs::remove_file(&report_file);
        run(
            kicad,
            settings,
            &[
                "pcb".as_ref(),
                "drc".as_ref(),
                "--format".as_ref(),
                "json".as_ref(),
                "--severity-all".as_ref(),
                "--output".as_ref(),
                report_file.as_os_str(),
                pcb_file.as_os_str(),
            ],
        );
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(&report_file).unwrap()).unwrap();
        let library_problems: Vec<String> = report["violations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|violation| {
                violation["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("lib_footprint"))
            })
            .map(|violation| {
                violation["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        match expected {
            None => assert_eq!(library_problems, Vec::<String>::new(), "{placed}"),
            Some(message) => assert_eq!(library_problems, [message], "{placed}"),
        }
    }
}

/// KiCad's STEP export finds the 3D model of the footprint `link` in the
/// moved folder `moved`, by the link relative to its library, and says so
/// when a link names a model that is not there.
fn check_model_found(
    kicad: &KicadInstall,
    settings: &CliSettingsFolder,
    folder: &Path,
    moved: &Path,
    link: &str,
) {
    let (library, name) = link.split_once(':').unwrap();
    let pretty = moved.join("footprints").join(format!("{library}.pretty"));
    let file = pretty.join(format!("{name}.kicad_mod"));
    let project = folder.join("moved-board");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("check.kicad_pro"), "{}\n").unwrap();
    fs::write(
        project.join("fp-lib-table"),
        format!(
            "(fp_lib_table\n  (version 7)\n  (lib (name \"{library}\")(type \"KiCad\")(uri \"{}\")(options \"\")(descr \"\"))\n)\n",
            pretty.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();
    let board = board_with(&file, link);
    let text = String::from_utf8(board).unwrap();
    let missing = model_references(&text)
        .into_iter()
        .find(|reference| reference.starts_with("../"))
        .expect("the footprint's model link is relative");
    for (case, board_text) in [
        ("found", text.clone()),
        (
            "missing",
            text.replace(&missing, "../nowhere.3dshapes/none.step"),
        ),
    ] {
        let pcb_file = project.join("check.kicad_pcb");
        fs::write(&pcb_file, &board_text).unwrap();
        let step = project.join(format!("{case}.step"));
        let message = run(
            kicad,
            settings,
            &[
                "pcb".as_ref(),
                "export".as_ref(),
                "step".as_ref(),
                "--force".as_ref(),
                "--output".as_ref(),
                step.as_os_str(),
                pcb_file.as_os_str(),
            ],
        );
        let not_found = message.contains("Could not add 3D model");
        if case == "found" {
            assert!(!not_found, "the moved model was not found: {message}");
        } else {
            assert!(not_found, "a missing model was not reported: {message}");
        }
    }
}

//! The KiCad libraries made from a real team catalog (kicad-parts-f397),
//! checked by the installed KiCad 10: kicad-cli loads every generated
//! symbol and footprint, and its electrical rules check resolves database
//! library entries through the SQLite3 ODBC driver, including revisions
//! that became older or archived. kicad-cli runs only through
//! run_kicad_cli, with a settings folder of the test's own.
//!
//! The resistors and inductors are imported into a disposable PocketBase on
//! 127.0.0.1; the libraries are written under target/.

use catalog::PartRevision;
use catalog::pocketbase::{
    ArchiveCommand, CatalogReadback, EditCommand, ImportCommand, read_back, read_import_state,
    submit_archive, submit_edit, submit_import,
};
use catalog_import::services;
use collaboration::PocketBaseClient;
use kicad_integration::library_file::{SymbolLibrary, model_references};
use kicad_integration::{
    CliSettingsFolder, Discovery, DiscoveryOptions, KicadInstall, SearchLocations, discover,
    run_kicad_cli,
};
use kicad_libraries::{Library, LibraryKind, setup, update};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(180);

fn target_folder(name: &str) -> PathBuf {
    let folder = services::repository_root()
        .join("target")
        .join("kicad-libraries-tests")
        .join(name);
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).unwrap();
    folder
}

/// The installed KiCad 10, or `None` with a message saying the test is
/// skipped.
fn installed_kicad(settings: &CliSettingsFolder) -> Option<KicadInstall> {
    match discover(&DiscoveryOptions {
        user_override: None,
        locations: SearchLocations::Standard,
        settings: settings.clone(),
    }) {
        Discovery::Ready(install) => Some(install),
        other => {
            eprintln!(
                "SKIPPED: KiCad 10 is not installed, so kicad-cli cannot check the generated libraries ({other:?})."
            );
            None
        }
    }
}

/// Imports `csv_files` one after another, as the local team server does.
fn import(admin: &PocketBaseClient, csv_files: &[&str]) {
    let root = services::repository_root();
    for csv_file in csv_files {
        let plan = services::plan(&root, csv_file).unwrap();
        let state = read_import_state(admin).unwrap();
        let command = ImportCommand::new(&plan, &format!("Import {csv_file}"))
            .expecting(&state)
            .allowing_uncommitted_changes(services::has_uncommitted_changes(&plan));
        let reply = submit_import(admin, &command).unwrap();
        assert!(reply.status == 200, "{}", reply.body);
    }
}

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

fn run(kicad: &KicadInstall, settings: &CliSettingsFolder, args: &[&std::ffi::OsStr]) -> String {
    let output = run_kicad_cli(&kicad.cli, settings, args, TIMEOUT)
        .unwrap_or_else(|error| panic!("{}", error.message(&kicad.cli)));
    assert!(output.success(), "kicad-cli {args:?}: {}", output.message());
    output.message()
}

/// A schematic placing one symbol from each of `links`, each with a plain
/// box as its saved copy.
fn schematic(links: &[&str]) -> String {
    let root = "11111111-2222-3333-4444-555555555555";
    let mut lib_symbols = String::new();
    let mut symbols = String::new();
    for (index, link) in links.iter().enumerate() {
        let name = link.split_once(':').unwrap().1;
        lib_symbols.push_str(&format!(
            "\t\t(symbol \"{link}\"\n\t\t\t(exclude_from_sim no)\n\t\t\t(in_bom yes)\n\t\t\t(on_board yes)\n\t\t\t(property \"Reference\" \"R\"\n\t\t\t\t(at 0 0 0)\n\t\t\t\t(effects\n\t\t\t\t\t(font\n\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t\t(property \"Value\" \"{name}\"\n\t\t\t\t(at 0 0 0)\n\t\t\t\t(effects\n\t\t\t\t\t(font\n\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t\t(symbol \"{name}_0_1\"\n\t\t\t\t(rectangle\n\t\t\t\t\t(start -1.27 -1.27)\n\t\t\t\t\t(end 1.27 1.27)\n\t\t\t\t\t(stroke\n\t\t\t\t\t\t(width 0)\n\t\t\t\t\t\t(type default)\n\t\t\t\t\t)\n\t\t\t\t\t(fill\n\t\t\t\t\t\t(type none)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n"
        ));
        let x = 50 + 20 * index;
        symbols.push_str(&format!(
            "\t(symbol\n\t\t(lib_id \"{link}\")\n\t\t(at {x} 50 0)\n\t\t(unit 1)\n\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\t\t(dnp no)\n\t\t(uuid \"00000000-0000-0000-0000-{index:012}\")\n\t\t(property \"Reference\" \"R{index}\"\n\t\t\t(at {x} 45 0)\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t\t(property \"Value\" \"{name}\"\n\t\t\t(at {x} 55 0)\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t\t(instances\n\t\t\t(project \"check\"\n\t\t\t\t(path \"/{root}\"\n\t\t\t\t\t(reference \"R{index}\")\n\t\t\t\t\t(unit 1)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t)\n"
        ));
    }
    format!(
        "(kicad_sch\n\t(version 20250114)\n\t(generator \"eeschema\")\n\t(generator_version \"9.0\")\n\t(uuid \"{root}\")\n\t(paper \"A4\")\n\t(lib_symbols\n{lib_symbols}\t)\n{symbols}\t(sheet_instances\n\t\t(path \"/\"\n\t\t\t(page \"1\")\n\t\t)\n\t)\n)\n"
    )
}

/// The ERC report's violations as (type, description, the items' text).
fn violations(report: &serde_json::Value) -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    for sheet in report["sheets"].as_array().into_iter().flatten() {
        for violation in sheet["violations"].as_array().into_iter().flatten() {
            let items: Vec<String> = violation["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| item["description"].as_str().unwrap_or_default().to_owned())
                .collect();
            found.push((
                violation["type"].as_str().unwrap_or_default().to_owned(),
                violation["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                items.join(" | "),
            ));
        }
    }
    found
}

fn libraries_of(outcome: &[Library], kind: LibraryKind) -> Vec<&Library> {
    outcome
        .iter()
        .filter(|library| library.kind == kind)
        .collect()
}

#[test]
fn kicad_loads_every_generated_library_and_resolves_every_entry() {
    let folder = target_folder("real-kicad");
    let settings = CliSettingsFolder::new(folder.join("kicad-cli-settings"));
    let Some(kicad) = installed_kicad(&settings) else {
        return;
    };
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    import(&admin, &["g-res", "g-ind"]);
    // RES-0000-0470 gets revision 2, so revision 1 is older.
    change_symbol(&admin, "RES-0000-0470", "Device:R");
    // RES-0000-0000 gets revision 2, which is archived, so revision 1 is
    // current again.
    change_symbol(&admin, "RES-0000-0000", "Device:R_Small");
    let reply = submit_archive(
        &admin,
        &ArchiveCommand::new("RES-0000-0000", 2, true, "Archive the trial symbol"),
    )
    .unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let readback: CatalogReadback = read_back(&admin).unwrap();
    server.stop();

    let libraries = folder.join("libraries");
    let outcome = update(&libraries, &readback, &mut |blob| Ok(blob.bytes.clone())).unwrap();
    assert_eq!(outcome.problems, Vec::<String>::new());
    assert!(outcome.older >= 1 && outcome.archived == 1, "{outcome:?}");

    // kicad-cli draws every symbol of every generated symbol library.
    let out = folder.join("svg");
    let mut symbols = 0;
    for library in libraries_of(&outcome.libraries, LibraryKind::Symbols) {
        let target = out.join("symbols").join(&library.nickname);
        fs::create_dir_all(&target).unwrap();
        let path = PathBuf::from(&library.path);
        run(
            &kicad,
            &settings,
            &[
                "sym".as_ref(),
                "export".as_ref(),
                "svg".as_ref(),
                "--output".as_ref(),
                target.as_os_str(),
                path.as_os_str(),
            ],
        );
        let drawn: Vec<String> = fs::read_dir(&target)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_lowercase())
            .collect();
        let bytes = fs::read(&path).unwrap();
        for entry in SymbolLibrary::parse(&bytes).unwrap().entries {
            let stem = format!("{}_unit", entry.name.to_lowercase());
            assert!(
                drawn.iter().any(|file| file.starts_with(&stem)),
                "kicad-cli drew no {} from {}: {drawn:?}",
                entry.name,
                library.path
            );
            symbols += 1;
        }
    }
    // kicad-cli draws every generated footprint, and every 3D model link
    // that was rewritten names a file that is there.
    let mut footprints = 0;
    // A footprint whose 3D model link now points beside its library.
    let mut with_model: Option<(String, PathBuf)> = None;
    for library in libraries_of(&outcome.libraries, LibraryKind::Footprints) {
        let target = out.join("footprints").join(&library.nickname);
        fs::create_dir_all(&target).unwrap();
        let pretty = PathBuf::from(&library.path);
        run(
            &kicad,
            &settings,
            &[
                "fp".as_ref(),
                "export".as_ref(),
                "svg".as_ref(),
                "--output".as_ref(),
                target.as_os_str(),
                pretty.as_os_str(),
            ],
        );
        for entry in fs::read_dir(&pretty).unwrap() {
            let path = entry.unwrap().path();
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            assert!(
                target.join(format!("{stem}.svg")).is_file(),
                "kicad-cli drew no {stem} from {}",
                library.path
            );
            footprints += 1;
            let text = fs::read_to_string(&path).unwrap();
            for link in model_references(&text) {
                let inside = libraries.to_string_lossy().replace('\\', "/");
                if link.starts_with(&inside) {
                    assert!(
                        Path::new(&link).is_file(),
                        "{stem} links to a missing {link}"
                    );
                    with_model
                        .get_or_insert((format!("{}:{stem}", library.nickname), path.clone()));
                }
            }
        }
    }
    assert!(
        symbols >= 2 && footprints >= 2,
        "{symbols} symbols, {footprints} footprints"
    );
    let (footprint_link, footprint_file) =
        with_model.expect("an inductor footprint has a 3D model from the team catalog");
    check_footprint_link(
        &kicad,
        &settings,
        &folder,
        &outcome.libraries,
        &footprint_link,
        &footprint_file,
    );

    // A project whose symbol library table is the setup text, plus KiCad's
    // Device library for the revisions that use KiCad's own symbols.
    let project = folder.join("project");
    fs::create_dir_all(&project).unwrap();
    let device = kicad
        .share_dir
        .as_ref()
        .expect("KiCad's own libraries next to kicad-cli")
        .join("symbols")
        .join("Device.kicad_sym");
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
    let placed = [
        // Older: revision 1 after revision 2 was added.
        "team-parts:RES-0000-0470_v1",
        // Current, using KiCad's own symbol.
        "team-parts:RES-0000-0470_v2",
        // Archived.
        "team-parts:RES-0000-0000_v2",
        // Current again after its newer revision was archived.
        "team-parts:RES-0000-0000_v1",
        // An inductor, in another category.
        "team-parts:IND-0000-0221_v1",
        // No such entry: the check below must tell it apart.
        "team-parts:RES-0000-9999_v1",
        "g-res:R_US_v1",
        "Device:R",
    ];
    let sheet = project.join("check.kicad_sch");
    fs::write(&sheet, schematic(&placed)).unwrap();
    let report_file = project.join("erc.json");
    let output = run_kicad_cli(
        &kicad.cli,
        &settings,
        [
            "sch".as_ref(),
            "erc".as_ref(),
            "--format".as_ref(),
            "json".as_ref(),
            "--severity-all".as_ref(),
            "--output".as_ref(),
            report_file.as_os_str(),
            sheet.as_os_str(),
        ],
        TIMEOUT,
    )
    .unwrap();
    let report: serde_json::Value = serde_json::from_slice(
        &fs::read(&report_file)
            .unwrap_or_else(|_| panic!("ERC wrote no report: {}", output.message())),
    )
    .unwrap();
    let found = violations(&report);
    eprintln!("{}", output.message());
    let said = |kind: &str, text: &str| {
        found
            .iter()
            .any(|(found_kind, description, _)| found_kind == kind && description.contains(text))
    };
    // KiCad found every real entry: it compared each with the plain box
    // the schematic saved as its copy, and said they differ. It found no
    // entry that does not exist.
    for link in &placed[..5] {
        let entry = link.split_once(':').unwrap().1;
        assert!(
            said(
                "lib_symbol_mismatch",
                &format!("Symbol '{entry}' doesn't match copy in library 'team-parts'")
            ),
            "{link} was not compared: {found:#?}"
        );
        assert!(
            !said("lib_symbol_issues", &format!("'{entry}'")),
            "{found:#?}"
        );
    }
    assert!(
        said(
            "lib_symbol_issues",
            "Symbol 'RES-0000-9999_v1' not found in symbol library 'team-parts'"
        ),
        "{found:#?}"
    );
    // The team symbol libraries and KiCad's own load beside it.
    assert!(said("lib_symbol_mismatch", "library 'g-res'"), "{found:#?}");
    assert!(
        said("lib_symbol_mismatch", "library 'Device'"),
        "{found:#?}"
    );
}

/// KiCad's design rules check finds the footprint `link` (such as
/// `g-Diode_SMD:D_SOD-323_v1`) in the generated footprint library, the
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
    fs::write(
        board.join("check.kicad_pro"),
        "{}
",
    )
    .unwrap();
    fs::write(
        board.join("fp-lib-table"),
        setup::footprint_table(libraries),
    )
    .unwrap();
    let text = fs::read_to_string(file).unwrap();
    let name = file.file_stem().unwrap().to_string_lossy().into_owned();
    let saved_name = name.rsplit_once("_v").unwrap().0;
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
        // The board keeps the footprint as the library has it, under the
        // link a design would store.
        let placed_text = text.replacen(
            &format!("(footprint \"{saved_name}\""),
            &format!("(footprint \"{placed}\""),
            1,
        );
        assert_ne!(placed_text, text, "{name} starts with its own name");
        let pcb = kicad_integration::board::one_footprint_board(
            placed_text.as_bytes(),
            &[],
            [-3.0, -3.0, 3.0, 3.0],
        )
        .unwrap();
        let pcb_file = board.join("check.kicad_pcb");
        fs::write(&pcb_file, pcb).unwrap();
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

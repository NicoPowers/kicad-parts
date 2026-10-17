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
//! with a settings folder of the test's own. The resistors and inductors
//! are imported into a disposable PocketBase on 127.0.0.1; the libraries
//! are written under target/.

use catalog::PartRevision;
use catalog::pocketbase::{
    ArchiveCommand, CatalogReadback, EditCommand, ImportCommand, read_back, read_import_state,
    read_team, submit_archive, submit_edit, submit_import, submit_team_code,
};
use catalog_import::services;
use collaboration::PocketBaseClient;
use kicad_integration::library_file::{
    SymbolLibrary, SymbolLookup, escape_text, model_references, rename_symbol_entry,
};
use kicad_integration::{
    CliSettingsFolder, Discovery, DiscoveryOptions, KicadInstall, SearchLocations, discover,
    run_kicad_cli,
};
use kicad_libraries::{Library, LibraryKind, Naming, Team, setup, update};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(180);

/// Clears every read-only mark under `folder`, so it can be deleted.
fn clear_read_only(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            clear_read_only(&path);
        } else if let Ok(metadata) = fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(&path, permissions);
        }
    }
}

fn target_folder(name: &str) -> PathBuf {
    let folder = services::repository_root()
        .join("target")
        .join("kicad-libraries-tests")
        .join(name);
    clear_read_only(&folder);
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

/// The row of `entry` in the database, with the KiCad field each column
/// becomes for its table, as the `.kicad_dbl` maps them.
struct Row {
    values: BTreeMap<String, String>,
    /// (column, KiCad field name) for every mapped field.
    fields: Vec<(String, String)>,
}

fn row_of(folder: &Path, entry: &str) -> Row {
    let dbl: serde_json::Value =
        serde_json::from_slice(&fs::read(folder.join("MA-Parts.kicad_dbl")).unwrap()).unwrap();
    let connection = rusqlite::Connection::open(folder.join("MA-Parts.sqlite")).unwrap();
    for library in dbl["libraries"].as_array().unwrap() {
        let table = library["table"].as_str().unwrap();
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{table}\" WHERE \"Entry\" = ?1"))
            .unwrap();
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let found: Option<BTreeMap<String, String>> = statement
            .query_map([entry], |row| {
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
            .next()
            .map(Result::unwrap);
        if let Some(values) = found {
            let fields = library["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| {
                    (
                        field["column"].as_str().unwrap().to_owned(),
                        field["name"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            return Row { values, fields };
        }
    }
    panic!("{entry} is in no table");
}

/// `entry` with its `(property "NAME" ...)` value set to `value`, or a new
/// hidden property added before the first unit if it has none.
fn set_property(entry: &str, name: &str, value: &str) -> String {
    let head = format!("(property \"{}\" \"", escape_text(name));
    if let Some(start) = entry.find(&head) {
        let value_start = start + head.len();
        let mut end = value_start;
        let bytes = entry.as_bytes();
        while bytes[end] != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        return format!(
            "{}{}{}",
            &entry[..value_start],
            escape_text(value),
            &entry[end..]
        );
    }
    let at = entry.find("\n\t\t(symbol \"").unwrap_or(entry.len() - 1);
    format!(
        "{}\n\t\t(property \"{}\" \"{}\"\n\t\t\t(at 0 0 0)\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t)\n\t\t\t\t(hide yes)\n\t\t\t)\n\t\t){}",
        &entry[..at],
        escape_text(name),
        escape_text(value),
        &entry[at..]
    )
}

/// `entry` without its `(property "NAME" ...)`, if it has one.
fn remove_property(entry: &str, name: &str) -> String {
    let head = format!("(property \"{name}\"");
    let Some(start) = entry.find(&head) else {
        return entry.to_owned();
    };
    let bytes = entry.as_bytes();
    let (mut depth, mut end, mut quoted) = (0, start, false);
    loop {
        match bytes[end] {
            b'\\' if quoted => end += 1,
            b'"' => quoted = !quoted,
            b'(' if !quoted => depth += 1,
            b')' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        end += 1;
    }
    format!(
        "{}{}",
        entry[..start].trim_end_matches(['\t', '\n']),
        &entry[end + 1..]
    )
}

/// The symbol KiCad makes for the database library entry `link` (such as
/// `MA-Parts:RES-0000-0470_v1`), as a schematic saves it: the source
/// symbol named after the entry, with the row's fields.
fn saved_copy(folder: &Path, share: &Path, link: &str) -> String {
    let entry_name = link.split_once(':').unwrap().1;
    let row = row_of(folder, entry_name);
    let (library, source) = row.values["Symbol"].split_once(':').unwrap();
    let file = if library == "Device" {
        share.join("symbols").join("Device.kicad_sym")
    } else {
        folder.join("symbols").join(format!("{library}.kicad_sym"))
    };
    let bytes = fs::read(&file).unwrap();
    let parsed = SymbolLibrary::parse(&bytes).unwrap();
    let SymbolLookup::Found(found) = parsed.find(source) else {
        panic!("{source} is not in {}", file.display());
    };
    let mut text = String::from_utf8(bytes[found.span.clone()].to_vec()).unwrap();
    // Before file version 20250318, "~" meant empty text; the schematic is
    // saved in KiCad 10's version, where it no longer does.
    if parsed.version.as_deref().unwrap_or_default() < "20250318" {
        text = text.replace("\"~\"", "\"\"");
    }
    let renamed = rename_symbol_entry(text.as_bytes(), source, entry_name).unwrap();
    let mut text = String::from_utf8(renamed).unwrap().replacen(
        &format!("(symbol \"{entry_name}\""),
        &format!("(symbol \"{link}\""),
        1,
    );
    let footprints: Vec<&str> = row.values["Footprint"]
        .split(';')
        .filter(|footprint| !footprint.is_empty())
        .collect();
    text = set_property(
        &text,
        "Footprint",
        footprints.first().copied().unwrap_or_default(),
    );
    text = remove_property(&text, "ki_fp_filters");
    if !footprints.is_empty() {
        text = set_property(&text, "ki_fp_filters", &footprints.join(" "));
    }
    for (column, field) in &row.fields {
        text = set_property(&text, field, &row.values[column]);
    }
    text
}

/// A schematic placing one symbol from each of `placed`, each saved with
/// its copy.
fn schematic(placed: &[(&str, String)]) -> String {
    let root = "11111111-2222-3333-4444-555555555555";
    let mut lib_symbols = String::new();
    let mut symbols = String::new();
    for (index, (link, copy)) in placed.iter().enumerate() {
        let name = link.split_once(':').unwrap().1;
        lib_symbols.push_str("\t\t");
        lib_symbols.push_str(copy);
        lib_symbols.push('\n');
        let x = 50 + 20 * index;
        symbols.push_str(&format!(
            "\t(symbol\n\t\t(lib_id \"{link}\")\n\t\t(at {x} 50 0)\n\t\t(unit 1)\n\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\t\t(dnp no)\n\t\t(uuid \"00000000-0000-0000-0000-{index:012}\")\n\t\t(property \"Reference\" \"R{index}\"\n\t\t\t(at {x} 45 0)\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t\t(property \"Value\" \"{name}\"\n\t\t\t(at {x} 55 0)\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t\t(instances\n\t\t\t(project \"check\"\n\t\t\t\t(path \"/{root}\"\n\t\t\t\t\t(reference \"R{index}\")\n\t\t\t\t\t(unit 1)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t)\n"
        ));
    }
    format!(
        "(kicad_sch\n\t(version 20260306)\n\t(generator \"eeschema\")\n\t(generator_version \"10.0\")\n\t(uuid \"{root}\")\n\t(paper \"A4\")\n\t(lib_symbols\n{lib_symbols}\t)\n{symbols}\t(sheet_instances\n\t\t(path \"/\"\n\t\t\t(page \"1\")\n\t\t)\n\t)\n)\n"
    )
}

/// A plain box saved as the copy of `link`, which no library symbol is.
fn plain_box(link: &str) -> String {
    let name = link.split_once(':').unwrap().1;
    format!(
        "(symbol \"{link}\"\n\t\t\t(property \"Reference\" \"R\"\n\t\t\t\t(at 0 0 0)\n\t\t\t\t(effects\n\t\t\t\t\t(font\n\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t\t(symbol \"{name}_0_1\"\n\t\t\t\t(rectangle\n\t\t\t\t\t(start -1.27 -1.27)\n\t\t\t\t\t(end 1.27 1.27)\n\t\t\t\t\t(stroke\n\t\t\t\t\t\t(width 0)\n\t\t\t\t\t\t(type default)\n\t\t\t\t\t)\n\t\t\t\t\t(fill\n\t\t\t\t\t\t(type none)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t)"
    )
}

/// The ERC report's violations as (type, description).
fn violations(report: &serde_json::Value) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for sheet in report["sheets"].as_array().into_iter().flatten() {
        for violation in sheet["violations"].as_array().into_iter().flatten() {
            found.push((
                violation["type"].as_str().unwrap_or_default().to_owned(),
                violation["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
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
    let share = kicad
        .share_dir
        .clone()
        .expect("KiCad's own libraries next to kicad-cli");
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    import(&admin, &["g-res", "g-ind"]);
    submit_team_code(&admin, "MA").unwrap();
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
    // kicad-cli draws every generated footprint, and every rewritten 3D
    // model link names a file beside the library.
    let mut footprints = 0;
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
                if let Some(relative) = link.strip_prefix("../") {
                    assert!(
                        pretty.parent().unwrap().join(relative).is_file(),
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
    // The folder moves: KiCad still finds the model by its relative link.
    let moved = folder.join("moved").join("MA");
    copy_folder(&libraries, &moved);
    check_model_found(&kicad, &settings, &folder, &moved, &footprint_link);

    // A project whose symbol library table is the setup text, plus KiCad's
    // Device library for the revisions that use KiCad's own symbols.
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
        "MA-Parts:RES-0000-0000_v2",
        // Current again after its newer revision was archived.
        "MA-Parts:RES-0000-0000_v1",
        // An inductor, in another category.
        "MA-Parts:IND-0000-0221_v1",
    ];
    let mut placed: Vec<(&str, String)> = real
        .iter()
        .map(|link| (*link, saved_copy(&libraries, &share, link)))
        .collect();
    // A wrong copy, which KiCad must tell apart from the library's symbol,
    // and an entry that does not exist.
    placed.push((
        "MA-Parts:IND-0001-0101_v1",
        plain_box("MA-Parts:IND-0001-0101_v1"),
    ));
    placed.push((
        "MA-Parts:RES-0000-9999_v1",
        plain_box("MA-Parts:RES-0000-9999_v1"),
    ));
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
            "Symbol 'IND-0001-0101_v1' doesn't match copy in library 'MA-Parts'"
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

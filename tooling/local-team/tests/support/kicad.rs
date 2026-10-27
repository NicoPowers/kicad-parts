//! Helpers for the tests that check generated KiCad libraries with the
//! installed KiCad 10 (`kicad_libraries.rs`, `full_catalog_kicad.rs`).
//! kicad-cli runs only through run_kicad_cli, with a settings folder of
//! the test's own, never the person's.

use kicad_integration::library_file::{
    SymbolLibrary, SymbolLookup, escape_text, model_references, rename_symbol_entry,
};
use kicad_integration::{
    CliSettingsFolder, Discovery, DiscoveryOptions, KicadInstall, SearchLocations, discover,
    run_kicad_cli,
};
use kicad_libraries::{Library, LibraryKind};
use local_team::services;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(180);

/// Clears every read-only mark under `folder`, so it can be deleted.
pub fn clear_read_only(folder: &Path) {
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

pub fn target_folder(name: &str) -> PathBuf {
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
pub fn installed_kicad(settings: &CliSettingsFolder) -> Option<KicadInstall> {
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

pub fn run(
    kicad: &KicadInstall,
    settings: &CliSettingsFolder,
    args: &[&std::ffi::OsStr],
) -> String {
    let output = run_kicad_cli(&kicad.cli, settings, args, TIMEOUT)
        .unwrap_or_else(|error| panic!("{}", error.message(&kicad.cli)));
    assert!(output.success(), "kicad-cli {args:?}: {}", output.message());
    output.message()
}

/// The row of `entry` in the database, with the KiCad field each column
/// becomes for its table, as the `.kicad_dbl` maps them.
pub struct Row {
    pub values: BTreeMap<String, String>,
    /// (column, KiCad field name) for every mapped field.
    pub fields: Vec<(String, String)>,
}

pub fn row_of(folder: &Path, entry: &str) -> Row {
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
pub fn set_property(entry: &str, name: &str, value: &str) -> String {
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
pub fn remove_property(entry: &str, name: &str) -> String {
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
pub fn saved_copy(folder: &Path, share: &Path, link: &str) -> String {
    let entry_name = link.split_once(':').unwrap().1;
    let row = row_of(folder, entry_name);
    let (library, source) = row.values["Symbol"].split_once(':').unwrap();
    // A generated symbol library, or else one of KiCad's own.
    let generated = folder.join("symbols").join(format!("{library}.kicad_sym"));
    let file = if generated.is_file() {
        generated
    } else {
        share.join("symbols").join(format!("{library}.kicad_sym"))
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
    // KiCad keeps the footprint choices as its footprint filters, one per
    // space, so a space inside a name is written `{space}`.
    text = remove_property(&text, "ki_fp_filters");
    if !footprints.is_empty() {
        let filters: Vec<String> = footprints
            .iter()
            .map(|footprint| footprint.replace(' ', "{space}"))
            .collect();
        text = set_property(&text, "ki_fp_filters", &filters.join(" "));
    }
    for (column, field) in &row.fields {
        text = set_property(&text, field, &row.values[column]);
    }
    text
}

/// A schematic placing one symbol from each of `placed`, each saved with
/// its copy.
pub fn schematic(placed: &[(&str, String)]) -> String {
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
pub fn plain_box(link: &str) -> String {
    let name = link.split_once(':').unwrap().1;
    format!(
        "(symbol \"{link}\"\n\t\t\t(property \"Reference\" \"R\"\n\t\t\t\t(at 0 0 0)\n\t\t\t\t(effects\n\t\t\t\t\t(font\n\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t\t(symbol \"{name}_0_1\"\n\t\t\t\t(rectangle\n\t\t\t\t\t(start -1.27 -1.27)\n\t\t\t\t\t(end 1.27 1.27)\n\t\t\t\t\t(stroke\n\t\t\t\t\t\t(width 0)\n\t\t\t\t\t\t(type default)\n\t\t\t\t\t)\n\t\t\t\t\t(fill\n\t\t\t\t\t\t(type none)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t)"
    )
}

/// The ERC report's violations as (type, description).
pub fn violations(report: &serde_json::Value) -> Vec<(String, String)> {
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

pub fn libraries_of(outcome: &[Library], kind: LibraryKind) -> Vec<&Library> {
    outcome
        .iter()
        .filter(|library| library.kind == kind)
        .collect()
}

/// kicad-cli draws every symbol of every generated symbol library in
/// `libraries`, into `out`. Returns how many it drew; a symbol it did not
/// draw fails the test.
pub fn draw_every_symbol(
    kicad: &KicadInstall,
    settings: &CliSettingsFolder,
    libraries: &[Library],
    out: &Path,
) -> usize {
    let mut symbols = 0;
    for library in libraries_of(libraries, LibraryKind::Symbols) {
        let target = out.join("symbols").join(&library.nickname);
        fs::create_dir_all(&target).unwrap();
        let path = PathBuf::from(&library.path);
        run(
            kicad,
            settings,
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
    symbols
}

/// kicad-cli draws every footprint of every generated footprint library in
/// `libraries`, into `out`, and every rewritten 3D model link names a file
/// beside its library. Returns how many it drew, and the link and file of
/// the first footprint with such a model link.
pub fn draw_every_footprint(
    kicad: &KicadInstall,
    settings: &CliSettingsFolder,
    libraries: &[Library],
    out: &Path,
) -> (usize, Option<(String, PathBuf)>) {
    let mut footprints = 0;
    let mut with_model: Option<(String, PathBuf)> = None;
    for library in libraries_of(libraries, LibraryKind::Footprints) {
        let target = out.join("footprints").join(&library.nickname);
        fs::create_dir_all(&target).unwrap();
        let pretty = PathBuf::from(&library.path);
        run(
            kicad,
            settings,
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
    (footprints, with_model)
}

/// Makes kicad-cli load libraries one at a time, in the order of the
/// symbol library table.
///
/// KiCad 10.0.7 builds every database library entry as the database
/// library loads (KiCad issue 25870). An entry whose symbol library is
/// still loading gets an empty symbol, so ERC says it "doesn't match copy
/// in library". KiCad's own Device library is large, so its entries lost
/// that race on most runs. With one thread, every library listed before
/// the database library has loaded before it. Earlier KiCad 10 versions
/// read each entry when asked, so this changes nothing there.
///
/// Our setup text lists only the team libraries first. KiCad puts pasted
/// rows above its own libraries, so in a real setup the database library
/// loads before Device. These tests check the best case instead: KiCad's
/// libraries first and one thread, which no pasted setup has.
///
/// Remove when KiCad fixes issue 25870 (open; not fixed in 10.0.8-rc1):
/// https://gitlab.com/kicad/code/kicad/-/work_items/25870
fn load_libraries_in_table_order(kicad: &KicadInstall, settings: &CliSettingsFolder) {
    let folder = settings.settings_for(&kicad.version);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("kicad_advanced"), "MaximumThreads=1\n").unwrap();
}

/// Runs KiCad's electrical rules check on `sheet` and returns its
/// violations as (type, description).
pub fn erc(
    kicad: &KicadInstall,
    settings: &CliSettingsFolder,
    sheet: &Path,
) -> Vec<(String, String)> {
    load_libraries_in_table_order(kicad, settings);
    let report_file = sheet.with_extension("erc.json");
    let _ = fs::remove_file(&report_file);
    let output = run_kicad_cli(
        &kicad.cli,
        settings,
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
    violations(&report)
}

//! Every entry of the KiCad libraries made from the whole CSV catalog,
//! checked by the installed KiCad 10 (kicad-parts-357b):
//!
//! - kicad-cli draws every generated symbol and every generated footprint;
//! - a schematic places every database library entry, each saved with an
//!   exact copy of the symbol the library should give it, and KiCad's
//!   electrical rules check finds every entry and its exact symbol, through
//!   the SQLite3 ODBC driver and KiCad's own libraries.
//!
//! KiCad finds all 177 entries. Two kinds of entry get a symbol other
//! than the saved copy, each named by the test: the two whose symbol the
//! CSV catalog itself lacks (the CSV catalog fixture lists them as
//! missing-member symbol references), and the two that use a symbol of
//! KiCad's own that extends another, which this test cannot copy as KiCad
//! does.
//!
//! Every CSV file is imported into a disposable PocketBase on 127.0.0.1;
//! the libraries and kicad-cli's settings are written under target/. The
//! test takes about 30 seconds and prints its times, so it runs with the
//! other tests.

mod support;

use catalog::pocketbase::read_back;
use catalog_import::{imports, libraries, services};
use kicad_integration::CliSettingsFolder;
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup};
use kicad_libraries::{LibraryKind, Naming, setup};
use std::collections::BTreeSet;
use std::fs;
use std::time::Instant;
use support::kicad::{
    draw_every_footprint, draw_every_symbol, erc, installed_kicad, plain_box, saved_copy,
    schematic, target_folder,
};

#[test]
fn kicad_finds_every_entry_of_the_whole_catalog() {
    let started = Instant::now();
    let folder = target_folder("full-catalog");
    let settings = CliSettingsFolder::new(folder.join("kicad-cli-settings"));
    let Some(kicad) = installed_kicad(&settings) else {
        return;
    };
    let share = kicad
        .share_dir
        .clone()
        .expect("KiCad's own libraries next to kicad-cli");
    let root = services::repository_root();
    let oracle =
        csv_catalog_fixture::load_manifest(&root.join("fixtures/import/csv-catalog/manifest.json"))
            .unwrap();

    // Every CSV file, imported as a team would.
    let planned = services::plan_all(&root, &services::csv_files(&root).unwrap()).unwrap();
    let (server, admin, _superuser) = services::start(&root, &folder.join("server")).unwrap();
    let allow = services::Allow {
        uncommitted_changes: true,
        ..services::Allow::default()
    };
    imports::import_in_order(&admin, &root, &planned, allow).unwrap();
    let readback = read_back(&admin).unwrap();
    server.stop();
    let team = libraries::disposable_team(Naming::new("MA").unwrap());
    let outcome = libraries::write(&folder.join("libraries"), &readback, &team).unwrap();
    assert_eq!(outcome.problems, Vec::<String>::new());
    let made = started.elapsed();

    // kicad-cli draws every generated symbol and footprint.
    let out = folder.join("svg");
    let symbols = draw_every_symbol(&kicad, &settings, &outcome.libraries, &out);
    let (footprints, _) = draw_every_footprint(&kicad, &settings, &outcome.libraries, &out);
    let drawn = started.elapsed();

    // Every entry, from every table of the database.
    let connection = rusqlite::Connection::open(outcome.folder.join("MA-Parts.sqlite")).unwrap();
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut entries: Vec<(String, String)> = Vec::new();
    for table in tables {
        let mut statement = connection
            .prepare(&format!("SELECT \"Entry\", \"Symbol\" FROM \"{table}\""))
            .unwrap();
        entries.extend(
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(Result::unwrap),
        );
    }
    drop(connection);
    assert_eq!(entries.len(), 177, "every row with a symbol has an entry");

    // The entries whose symbol the CSV catalog lacks.
    let missing: BTreeSet<String> = oracle
        .assets
        .symbol_references
        .iter()
        .filter(|reference| reference.local && reference.status != "resolved")
        .flat_map(|reference| reference.source_ipns.iter())
        .map(|ipn| format!("{ipn}_v1"))
        .collect();
    assert!(!missing.is_empty());

    // A project whose symbol library table is the setup text, plus each of
    // KiCad's own libraries an entry uses.
    let project = folder.join("project");
    fs::create_dir_all(&project).unwrap();
    let ours: BTreeSet<&str> = outcome
        .libraries
        .iter()
        .filter(|library| library.kind == LibraryKind::Symbols)
        .map(|library| library.nickname.as_str())
        .collect();
    let mut kicads_own = String::new();
    let used: BTreeSet<&str> = entries
        .iter()
        .filter_map(|(_, symbol)| symbol.split_once(':').map(|(library, _)| library))
        .filter(|library| !ours.contains(library))
        .collect();
    for library in used {
        let file = share.join("symbols").join(format!("{library}.kicad_sym"));
        if file.is_file() {
            kicads_own.push_str(&format!(
                "  (lib (name \"{library}\")(type \"KiCad\")(uri \"{}\")(options \"\")(descr \"\"))\n",
                file.to_string_lossy().replace('\\', "/")
            ));
        }
    }
    let table = setup::symbol_table(&outcome.libraries).replacen(
        "  (version 7)\n",
        &format!("  (version 7)\n{kicads_own}"),
        1,
    );
    fs::write(project.join("sym-lib-table"), table).unwrap();
    fs::write(project.join("check.kicad_pro"), "{}\n").unwrap();

    let links: Vec<String> = entries
        .iter()
        .map(|(entry, _)| format!("MA-Parts:{entry}"))
        .collect();
    let placed: Vec<(&str, String)> = entries
        .iter()
        .zip(&links)
        .map(|((entry, _), link)| {
            let copy = if missing.contains(entry) {
                plain_box(link)
            } else {
                saved_copy(&outcome.folder, &share, link)
            };
            (link.as_str(), copy)
        })
        .collect();
    let sheet = project.join("check.kicad_sch");
    fs::write(&sheet, schematic(&placed)).unwrap();
    let found = erc(&kicad, &settings, &sheet);
    // KiCad found every entry: none is "not found".
    let library_issues: Vec<&(String, String)> = found
        .iter()
        .filter(|(kind, _)| kind.starts_with("lib_symbol"))
        .collect();
    assert!(
        library_issues
            .iter()
            .all(|(kind, _)| kind == "lib_symbol_mismatch"),
        "{library_issues:#?}"
    );
    // And each with exactly the symbol it should have, apart from two
    // kinds of entry. One is the entries whose symbol the CSV catalog
    // lacks: KiCad cannot give them their real symbol. The other is the
    // entries using a symbol of KiCad's own that extends another symbol:
    // KiCad gives them the whole symbol, which this test cannot write as
    // the saved copy, so their copies differ.
    let extending: BTreeSet<String> = entries
        .iter()
        .filter(|(_, symbol)| extends_another(&share, symbol))
        .map(|(entry, _)| entry.clone())
        .collect();
    let differing: BTreeSet<String> = library_issues
        .iter()
        .filter_map(|(_, description)| {
            entries
                .iter()
                .map(|(entry, _)| entry)
                .find(|entry| description.contains(&format!("'{entry}'")))
                .cloned()
        })
        .collect();
    assert_eq!(library_issues.len(), differing.len(), "{library_issues:#?}");
    let expected: BTreeSet<String> = missing.union(&extending).cloned().collect();
    assert_eq!(differing, expected, "{library_issues:#?}");
    eprintln!(
        "KiCad checked {} entries, {symbols} symbols and {footprints} footprints: libraries made in {:.1} s, drawn by {:.1} s, ERC done by {:.1} s.",
        entries.len(),
        made.as_secs_f64(),
        drawn.as_secs_f64(),
        started.elapsed().as_secs_f64()
    );
}

/// Whether `symbol`, a link such as `Relay:G2RL-1`, names a symbol of
/// KiCad's own libraries in `share` that extends another symbol.
fn extends_another(share: &std::path::Path, symbol: &str) -> bool {
    let Some((library, name)) = symbol.split_once(':') else {
        return false;
    };
    let Ok(bytes) = fs::read(share.join("symbols").join(format!("{library}.kicad_sym"))) else {
        return false;
    };
    SymbolLibrary::parse(&bytes).is_ok_and(
        |parsed| matches!(parsed.find(name), SymbolLookup::Found(entry) if entry.extends.is_some()),
    )
}

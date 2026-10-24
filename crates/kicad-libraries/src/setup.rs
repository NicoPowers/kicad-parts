//! What a person adds to KiCad once: each library's nickname and path, as
//! text KiCad's Manage Symbol Libraries and Manage Footprint Libraries
//! dialogs take pasted in. The app never writes KiCad's own settings or
//! library tables.

use crate::plan::{Naming, Plan};
use kicad_integration::lib_table::{LibTable, LibTableRow, TableKind};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryKind {
    /// The database library: one entry per part revision.
    Parts,
    Symbols,
    Footprints,
}

/// One library to add to KiCad.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Library {
    pub kind: LibraryKind,
    pub nickname: String,
    /// The full path, with `/`.
    pub path: String,
}

/// `relative` inside `folder`, written with `/` as KiCad accepts on every
/// system, without a `\\?\` prefix.
pub fn absolute(folder: &Path, relative: &str) -> String {
    let folder = folder.to_string_lossy().replace('\\', "/");
    let folder = folder.strip_prefix("//?/").unwrap_or(&folder);
    format!("{}/{relative}", folder.trim_end_matches('/'))
}

/// The libraries in `folder` for `plan`: the database library first, then
/// symbol libraries and footprint libraries by nickname.
pub fn libraries(folder: &Path, plan: &Plan<'_>, naming: &Naming) -> Vec<Library> {
    let mut libraries = vec![Library {
        kind: LibraryKind::Parts,
        nickname: naming.database_nickname(),
        path: absolute(folder, &naming.dbl_file()),
    }];
    libraries.extend(plan.symbol_libraries.iter().map(|library| Library {
        kind: LibraryKind::Symbols,
        nickname: library.nickname.clone(),
        path: absolute(folder, &library.path),
    }));
    libraries.extend(plan.footprint_libraries.iter().map(|library| Library {
        kind: LibraryKind::Footprints,
        nickname: library.nickname.clone(),
        path: absolute(folder, &library.path),
    }));
    libraries
}

/// The symbol library table text: the team symbol libraries, hidden in
/// KiCad's chooser so people pick whole parts, then the database library
/// that draws from them.
///
/// The symbol libraries come first because KiCad 10.0.7 loads libraries in
/// table order and builds every database entry as the database library
/// loads. An entry whose symbol library has not loaded yet gets an empty
/// symbol, which ERC calls "doesn't match copy in library" (KiCad issue
/// 25870). This helps only our team libraries. KiCad puts pasted rows
/// above its own libraries, so parts using KiCad's own symbols stay
/// affected until KiCad fixes the issue.
pub fn symbol_table(libraries: &[Library]) -> String {
    let parts = libraries
        .iter()
        .find(|library| library.kind == LibraryKind::Parts)
        .map(|library| library.nickname.as_str())
        .unwrap_or("the database library");
    let symbols = libraries
        .iter()
        .filter(|library| library.kind == LibraryKind::Symbols)
        .map(|library| LibTableRow {
            name: library.nickname.clone(),
            kind: "KiCad".into(),
            uri: library.path.clone(),
            descr: format!("Team symbols that {parts} uses"),
            hidden: true,
            ..LibTableRow::default()
        });
    let database = libraries
        .iter()
        .filter(|library| library.kind == LibraryKind::Parts)
        .map(|library| LibTableRow {
            name: library.nickname.clone(),
            kind: "Database".into(),
            uri: library.path.clone(),
            descr: "Parts from the team catalog, one entry per revision".into(),
            ..LibTableRow::default()
        });
    let rows = symbols.chain(database).collect();
    LibTable {
        version: Some("7".into()),
        rows,
    }
    .format(TableKind::Symbol)
}

/// The footprint library table text.
pub fn footprint_table(libraries: &[Library]) -> String {
    let rows = libraries
        .iter()
        .filter(|library| library.kind == LibraryKind::Footprints)
        .map(|library| LibTableRow {
            name: library.nickname.clone(),
            kind: "KiCad".into(),
            uri: library.path.clone(),
            descr: "Team footprints".into(),
            ..LibTableRow::default()
        })
        .collect();
    LibTable {
        version: Some("7".into()),
        rows,
    }
    .format(TableKind::Footprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(kind: LibraryKind, nickname: &str) -> Library {
        Library {
            kind,
            nickname: nickname.into(),
            path: format!("/libraries/{nickname}"),
        }
    }

    #[test]
    fn the_symbol_table_lists_the_symbol_libraries_before_the_database_library() {
        let libraries = [
            library(LibraryKind::Parts, "MA-Parts"),
            library(LibraryKind::Symbols, "MA-ind-20241209"),
            library(LibraryKind::Symbols, "MA-res-20241209"),
            library(LibraryKind::Footprints, "MA-Resistor_SMD"),
        ];
        let table = symbol_table(&libraries);
        let names: Vec<&str> = table
            .lines()
            .filter_map(|line| line.split("(name \"").nth(1))
            .filter_map(|rest| rest.split('"').next())
            .collect();
        assert_eq!(names, ["MA-ind-20241209", "MA-res-20241209", "MA-Parts"]);
        assert!(table.contains("(descr \"Team symbols that MA-Parts uses\")(hidden))"));
    }
}

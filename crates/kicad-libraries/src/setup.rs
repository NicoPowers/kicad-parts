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

/// The symbol library table text: the database library, shown in KiCad's
/// chooser, and the team symbol libraries it draws from, hidden there so
/// people pick whole parts.
pub fn symbol_table(libraries: &[Library]) -> String {
    let parts = libraries
        .iter()
        .find(|library| library.kind == LibraryKind::Parts)
        .map(|library| library.nickname.as_str())
        .unwrap_or("the database library");
    let rows = libraries
        .iter()
        .filter_map(|library| match library.kind {
            LibraryKind::Parts => Some(LibTableRow {
                name: library.nickname.clone(),
                kind: "Database".into(),
                uri: library.path.clone(),
                descr: "Parts from the team catalog, one entry per revision".into(),
                ..LibTableRow::default()
            }),
            LibraryKind::Symbols => Some(LibTableRow {
                name: library.nickname.clone(),
                kind: "KiCad".into(),
                uri: library.path.clone(),
                descr: format!("Team symbols that {parts} uses"),
                hidden: true,
                ..LibTableRow::default()
            }),
            LibraryKind::Footprints => None,
        })
        .collect();
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

//! Gathers the library check's inputs: the CSV catalog's own libraries and
//! the symbols and footprints its CSV files name, from a CSV catalog
//! folder, and the footprints KiCad's own libraries have. Only reads.
//!
//! Libraries are found through a library source profile: our own layout,
//! [`SourceProfile::own`], for the CSV catalog folder.

use super::{CheckInputs, CsvReference, KnownCad, Libraries, LibraryCheck, LibraryFootprint};
use super::{LibrarySymbol, Today, check};
use crate::cad_item::CadItemKind;
use crate::csv_import::{is_link, parse_csv};
use crate::download::model_links;
use crate::hash::sha256_hex;
use kicad_integration::footprint_choices;
use kicad_integration::lib_table::TableKind;
use kicad_integration::library_file::{Sexp, SymbolLibrary};
use kicad_integration::library_source::{SourceProfile, join, native_path};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Runs the library check on the CSV catalog under `root` against what
/// the Catalog has (`known`), with KiCad's own footprints from
/// `kicad_share` (KiCad's data folder) when it is known.
pub fn check_folder(
    root: &Path,
    known: &KnownCad,
    kicad_share: Option<&Path>,
) -> Result<LibraryCheck, String> {
    check_folder_with(root, known, kicad_share, super::today())
}

/// [`check_folder`] with `today` in place of today's CSV catalog built into
/// the program.
pub fn check_folder_with(
    root: &Path,
    known: &KnownCad,
    kicad_share: Option<&Path>,
    today: &Today,
) -> Result<LibraryCheck, String> {
    let source = SourceProfile::own();
    let mut libraries = read_source_libraries(root, &source);
    let csv = read_csv_references(root, &mut libraries.unreadable);
    let kicad = kicad_share.map(|share| kicad_footprints(share, &source, &libraries));
    Ok(check(CheckInputs {
        libraries: &libraries,
        known,
        csv: &csv,
        kicad_footprints: kicad.as_ref(),
        today,
    }))
}

/// Why a link is not followed, as an unreadable file says it.
const LINK: &str =
    "it is a link, which the library check does not follow (the import refuses links too)";

/// Reads every symbol of `symbols/g-*.kicad_sym`, every footprint of
/// `footprints/g-*.pretty` and the name of every file under `3d-models/`.
/// A missing folder has nothing in it; a file that cannot be read, and a
/// link (never followed, as the import refuses links), is listed in
/// [`Libraries::unreadable`].
pub fn read_libraries(root: &Path) -> Libraries {
    read_source_libraries(root, &SourceProfile::own())
}

/// Reads the libraries `source` names under `root` whose names the app
/// made for it (such as our `g-` libraries): every symbol of each
/// `.kicad_sym` symbol library, every footprint of each footprint library,
/// and the name of every file in its model folders. A missing folder has
/// nothing in it; a file that cannot be read, and a link (never followed,
/// as the import refuses links), is listed in [`Libraries::unreadable`].
pub fn read_source_libraries(root: &Path, source: &SourceProfile) -> Libraries {
    let mut libraries = Libraries::default();
    let unreadable = &mut libraries.unreadable;
    let found = |kind: TableKind, unreadable: &mut Vec<(String, String)>| {
        let mut found = Vec::new();
        for folder in source.library_folders(kind) {
            for (path, file) in entries(root, &folder, unreadable) {
                let library = source
                    .nickname_of(kind, &file)
                    .filter(|library| source.is_own_name(library));
                if let Some(library) = library {
                    found.push((path, file, library));
                }
            }
        }
        found
    };
    for (path, file, library) in found(TableKind::Symbol, unreadable) {
        // A KiCad 10 `.kicad_symdir` folder is not checked yet.
        if !file.ends_with(".kicad_sym") || !path.is_file() {
            continue;
        }
        let parsed = fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                SymbolLibrary::parse(&bytes)
                    .map(|parsed| (bytes, parsed))
                    .map_err(|error| error.to_string())
            });
        match parsed {
            Ok((bytes, parsed)) => {
                for entry in parsed.entries {
                    let text = &bytes[entry.span.clone()];
                    let footprint = kicad_integration::library_file::symbol_properties(text)
                        .into_iter()
                        .find(|(name, _)| name == "Footprint")
                        .map(|(_, value)| value)
                        .unwrap_or_default();
                    let power =
                        Sexp::parse(text).is_ok_and(|tree| tree.children("power").next().is_some());
                    libraries.symbols.push(LibrarySymbol {
                        library: library.clone(),
                        file: file.clone(),
                        name: entry.name,
                        footprint,
                        power,
                    });
                }
            }
            Err(why) => unreadable.push((file, why)),
        }
    }
    for (folder, folder_file, library) in found(TableKind::Footprint, unreadable) {
        if !folder.is_dir() {
            continue;
        }
        for (path, file) in entries(root, &folder_file, unreadable) {
            let Some(name) = file_name(&path)
                .strip_suffix(".kicad_mod")
                .map(str::to_owned)
            else {
                continue;
            };
            if !path.is_file() {
                continue;
            }
            match fs::read(&path) {
                Ok(bytes) => {
                    // Quoted paths and KiCad 5 paths written without
                    // quotes, whether or not the footprint parses.
                    let models = model_links(&String::from_utf8_lossy(&bytes));
                    libraries.footprints.push(LibraryFootprint {
                        library: library.clone(),
                        name,
                        file: file.clone(),
                        sha256: sha256_hex(&bytes),
                        models,
                    });
                }
                Err(error) => unreadable.push((file, error.to_string())),
            }
        }
    }
    let mut models = Vec::new();
    for folder in &source.model_folders {
        files_under(root, folder, &mut models, unreadable);
    }
    libraries.models = models.into_iter().collect();
    libraries
}

/// Every symbol and footprint the CSV files (`database/g-*.csv`) name in
/// their Symbol and Footprint columns, one footprint choice (or symbol)
/// each. A CSV file that cannot be read, or a link, is added to
/// `unreadable` and the rest is read.
pub fn read_csv_references(
    root: &Path,
    unreadable: &mut Vec<(String, String)>,
) -> Vec<CsvReference> {
    let mut references = BTreeSet::new();
    for (path, file) in entries(root, "database", unreadable) {
        let name = file_name(&path);
        let is_csv_file = name.starts_with("g-") && name.ends_with(".csv");
        if !is_csv_file || !path.is_file() {
            continue;
        }
        let table = match fs::read(&path) {
            Ok(bytes) => parse_csv(&file, &bytes).map_err(|error| error.0),
            Err(error) => Err(error.to_string()),
        };
        let table = match table {
            Ok(table) => table,
            Err(why) => {
                unreadable.push((file, why));
                continue;
            }
        };
        for (kind, column) in [
            (CadItemKind::Symbol, "Symbol"),
            (CadItemKind::Footprint, "Footprint"),
        ] {
            let Some(index) = table.headers.iter().position(|header| header == column) else {
                continue;
            };
            for record in &table.records {
                let value = record.fields.get(index).map(String::as_str).unwrap_or("");
                // Like the import, a value naming several (`;`) names each.
                for reference in footprint_choices(value) {
                    references.insert(CsvReference {
                        file: file.clone(),
                        kind,
                        reference: reference.to_owned(),
                    });
                }
            }
        }
    }
    references.into_iter().collect()
}

/// The footprints outside `source`'s own libraries that the symbols name
/// and that KiCad's data folder `share` has
/// (`footprints/<library>.pretty/<name>.kicad_mod`).
pub fn kicad_footprints(
    share: &Path,
    source: &SourceProfile,
    libraries: &Libraries,
) -> BTreeSet<String> {
    let kicad = SourceProfile::kicad_installed();
    libraries
        .symbols
        .iter()
        .map(|symbol| symbol.footprint.trim())
        .filter_map(|footprint| {
            let (library, name) = footprint.split_once(':')?;
            let plain = |part: &str| {
                !part.is_empty() && part != "." && part != ".." && !part.contains(['/', '\\'])
            };
            (!source.is_own_name(library) && plain(library) && plain(name))
                .then_some((footprint, library, name))
        })
        .filter(|(_, library, name)| {
            kicad
                .library_path(TableKind::Footprint, library)
                .is_some_and(|folder| {
                    native_path(share, &folder)
                        .join(format!("{name}.kicad_mod"))
                        .is_file()
                })
        })
        .map(|(footprint, _, _)| footprint.to_owned())
        .collect()
}

/// The bytes of the repository file `relative` (a `/`-separated path such
/// as `symbols/g-ics.kicad_sym`) under `root`, read with the same safety as
/// the rest of the library check: every part of the path must be a plain
/// name (never `..` or empty), and no folder on the way, nor the file, may
/// be a link, which is never followed. Refuses with a plain reason.
pub fn read_library_file(root: &Path, relative: &str) -> Result<Vec<u8>, String> {
    let parts: Vec<&str> = relative.split('/').collect();
    let plain = |part: &&str| {
        !part.is_empty() && *part != "." && *part != ".." && !part.contains(['\\', ':'])
    };
    if !parts.iter().all(plain) {
        return Err(format!(
            "{relative} is not a path inside the CSV catalog folder."
        ));
    }
    let mut path = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| format!("{relative} is not in the CSV catalog folder."))?;
        if is_link(&metadata) {
            return Err(format!("{relative} could not be read: {LINK}."));
        }
        let last = index + 1 == parts.len();
        if last && !metadata.is_file() {
            return Err(format!("{relative} is not a file."));
        }
    }
    fs::read(&path).map_err(|error| format!("{relative} could not be read: {error}."))
}

/// The entries of the folder `relative` (a `/`-separated repository path)
/// under `root`, in name order, each with its repository path; none if it
/// is missing or cannot be read. Links are never followed: a link, the
/// folder itself or an entry in it, is added to `unreadable` instead.
fn entries(
    root: &Path,
    relative: &str,
    unreadable: &mut Vec<(String, String)>,
) -> Vec<(PathBuf, String)> {
    let folder = root.join(relative);
    match fs::symlink_metadata(&folder) {
        Ok(metadata) if is_link(&metadata) => {
            unreadable.push((relative.to_owned(), LINK.to_owned()));
            return Vec::new();
        }
        Ok(_) => {}
        Err(_) => return Vec::new(),
    }
    let Ok(read) = fs::read_dir(&folder) else {
        return Vec::new();
    };
    let mut paths: Vec<(PathBuf, String)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let file = join(relative, file_name(&path));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_link(&metadata) => unreadable.push((file, LINK.to_owned())),
            Ok(_) => paths.push((path, file)),
            Err(error) => unreadable.push((file, error.to_string())),
        }
    }
    paths.sort();
    paths
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}

/// Every file under the folder `relative` (a repository path), as
/// `relative/...` paths with `/`.
fn files_under(
    root: &Path,
    relative: &str,
    found: &mut Vec<String>,
    unreadable: &mut Vec<(String, String)>,
) {
    for (path, file) in entries(root, relative, unreadable) {
        if path.is_dir() {
            files_under(root, &file, found, unreadable);
        } else if path.is_file() {
            found.push(file);
        }
    }
}

//! Gathers the library check's inputs: the CSV catalog's own libraries and
//! the symbols and footprints its CSV files name, from a CSV catalog
//! folder, and the footprints KiCad's own libraries have. Only reads.

use super::{CheckInputs, CsvReference, KnownCad, Libraries, LibraryCheck, LibraryFootprint};
use super::{LibrarySymbol, check};
use crate::cad_item::CadItemKind;
use crate::csv_import::parse_csv;
use crate::hash::sha256_hex;
use kicad_integration::footprint_choices;
use kicad_integration::library_file::{FootprintFile, SymbolLibrary, model_references};
use std::collections::{BTreeMap, BTreeSet};
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
    check_folder_with(
        root,
        known,
        kicad_share,
        super::baseline::todays_footprints(),
    )
}

/// [`check_folder`] with `todays_footprints` (repository path to SHA-256)
/// in place of today's CSV catalog built into the program.
pub fn check_folder_with(
    root: &Path,
    known: &KnownCad,
    kicad_share: Option<&Path>,
    todays_footprints: &BTreeMap<String, String>,
) -> Result<LibraryCheck, String> {
    let libraries = read_libraries(root);
    let csv = read_csv_references(root)?;
    let kicad = kicad_share.map(|share| kicad_footprints(share, &libraries));
    Ok(check(CheckInputs {
        libraries: &libraries,
        known,
        csv: &csv,
        kicad_footprints: kicad.as_ref(),
        todays_footprints,
    }))
}

/// Reads every symbol of `symbols/g-*.kicad_sym`, every footprint of
/// `footprints/g-*.pretty` and the name of every file under `3d-models/`.
/// A missing folder has nothing in it; a file that cannot be read is listed
/// in [`Libraries::unreadable`].
pub fn read_libraries(root: &Path) -> Libraries {
    let mut libraries = Libraries::default();
    for path in entries(&root.join("symbols")) {
        let Some(library) = file_name(&path)
            .strip_suffix(".kicad_sym")
            .filter(|name| name.starts_with("g-"))
            .map(str::to_owned)
        else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        let file = format!("symbols/{library}.kicad_sym");
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
                    let footprint = kicad_integration::library_file::symbol_properties(
                        &bytes[entry.span.clone()],
                    )
                    .into_iter()
                    .find(|(name, _)| name == "Footprint")
                    .map(|(_, value)| value)
                    .unwrap_or_default();
                    libraries.symbols.push(LibrarySymbol {
                        library: library.clone(),
                        name: entry.name,
                        footprint,
                    });
                }
            }
            Err(why) => libraries.unreadable.push((file, why)),
        }
    }
    for folder in entries(&root.join("footprints")) {
        let Some(library) = file_name(&folder)
            .strip_suffix(".pretty")
            .filter(|name| name.starts_with("g-"))
            .map(str::to_owned)
        else {
            continue;
        };
        if !folder.is_dir() {
            continue;
        }
        for path in entries(&folder) {
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
                    // A footprint that does not parse still names its 3D
                    // models the way the CSV catalog fixture reads them.
                    let models = match FootprintFile::parse(&bytes) {
                        Ok(parsed) => parsed.model_references,
                        Err(_) => model_references(&String::from_utf8_lossy(&bytes)),
                    };
                    libraries.footprints.push(LibraryFootprint {
                        library: library.clone(),
                        name,
                        sha256: sha256_hex(&bytes),
                        models,
                    });
                }
                Err(error) => libraries.unreadable.push((
                    format!("footprints/{library}.pretty/{name}.kicad_mod"),
                    error.to_string(),
                )),
            }
        }
    }
    let mut models = Vec::new();
    files_under(&root.join("3d-models"), "3d-models", &mut models);
    libraries.models = models.into_iter().collect();
    libraries
}

/// Every symbol and footprint the CSV files (`database/g-*.csv`) name in
/// their Symbol and Footprint columns, one footprint choice (or symbol)
/// each.
pub fn read_csv_references(root: &Path) -> Result<Vec<CsvReference>, String> {
    let mut references = BTreeSet::new();
    for path in entries(&root.join("database")) {
        let name = file_name(&path);
        let is_csv_file = name.starts_with("g-") && name.ends_with(".csv");
        if !is_csv_file || !path.is_file() {
            continue;
        }
        let file = format!("database/{name}");
        let bytes = fs::read(&path).map_err(|error| format!("cannot read {file}: {error}"))?;
        let table = parse_csv(&file, &bytes).map_err(|error| error.0)?;
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
    Ok(references.into_iter().collect())
}

/// The footprints outside our libraries that the symbols name and that
/// KiCad's data folder `share` has (`footprints/<library>.pretty/<name>.kicad_mod`).
pub fn kicad_footprints(share: &Path, libraries: &Libraries) -> BTreeSet<String> {
    libraries
        .symbols
        .iter()
        .map(|symbol| symbol.footprint.trim())
        .filter_map(|footprint| {
            let (library, name) = footprint.split_once(':')?;
            let plain = |part: &str| {
                !part.is_empty() && part != "." && part != ".." && !part.contains(['/', '\\'])
            };
            (!library.starts_with("g-") && plain(library) && plain(name))
                .then_some((footprint, library, name))
        })
        .filter(|(_, library, name)| {
            share
                .join("footprints")
                .join(format!("{library}.pretty"))
                .join(format!("{name}.kicad_mod"))
                .is_file()
        })
        .map(|(footprint, _, _)| footprint.to_owned())
        .collect()
}

/// The entries of `folder`, in name order, links left out; none if it
/// cannot be read.
fn entries(folder: &Path) -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .filter(|entry| {
            fs::symlink_metadata(entry.path()).is_ok_and(|metadata| !is_link(&metadata))
        })
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}

/// Every file under `folder`, as `prefix/...` paths with `/`.
fn files_under(folder: &Path, prefix: &str, found: &mut Vec<String>) {
    for path in entries(folder) {
        let name = file_name(&path);
        if name.is_empty() {
            continue;
        }
        let relative = format!("{prefix}/{name}");
        if path.is_dir() {
            files_under(&path, &relative, found);
        } else if path.is_file() {
            found.push(relative);
        }
    }
}

#[cfg(not(windows))]
fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

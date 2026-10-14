//! Which libraries the browser lists, in three groups:
//!
//! - Ours: the CSV catalog's `symbols/g-*.kicad_sym` (or `.kicad_symdir`)
//!   and `footprints/g-*.pretty`, found without following links, as the
//!   library check reads them;
//! - KiCad: KiCad's own `symbols/` and `footprints/` in its installed data
//!   folder, described by KiCad's template tables;
//! - Installed: the other KiCad libraries in the person's library tables.
//!   Rows pointing into KiCad's data folder or into our libraries are left
//!   out (they are already listed), as are hidden and disabled rows.

use desktop_contract::{LibraryGroup, LibraryKey, PreviewKind};
use kicad_integration::lib_table::{
    LibTable, PathVariables, TableKind, VariableSources, is_inside, read_tables,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// How a library is stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Format {
    /// One `.kicad_sym` file.
    SymbolFile,
    /// A `.kicad_symdir` folder (KiCad 10): one `.kicad_sym` per symbol.
    SymbolFolder,
    /// A `.pretty` folder of `.kicad_mod` files.
    Footprints,
}

/// One library to list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Source {
    pub key: LibraryKey,
    pub kind: PreviewKind,
    pub description: String,
    pub path: PathBuf,
    pub format: Format,
    /// Why it cannot be read at all, such as a path variable with no value.
    pub problem: Option<String>,
}

/// Where the libraries are.
#[derive(Clone, Debug, Default)]
pub(crate) struct Places {
    /// The CSV catalog folder.
    pub catalog_root: PathBuf,
    /// KiCad's installed data folder.
    pub share: Option<PathBuf>,
    /// KiCad's settings folder holding the person's tables.
    pub kicad_config: Option<PathBuf>,
    /// The process environment, for path variables.
    pub environment: BTreeMap<String, String>,
}

/// KiCad's version whose variables (`KICAD10_*`) are expanded.
const KICAD_MAJOR: u32 = kicad_integration::SUPPORTED_MAJOR;

/// Every library, and notes for the person, in display order: symbols
/// then footprints, each by group and then nickname.
pub(crate) fn find_sources(places: &Places) -> (Vec<Source>, Vec<String>) {
    let mut sources = Vec::new();
    let mut notes = Vec::new();
    ours(&places.catalog_root, &mut sources);
    if let Some(share) = &places.share {
        kicad(share, &mut sources);
    }
    if let Some(config) = &places.kicad_config {
        installed(places, config, &mut sources, &mut notes);
    }
    sources.sort_by(|a, b| {
        (a.kind as u8, a.key.group, a.key.nickname.to_lowercase()).cmp(&(
            b.kind as u8,
            b.key.group,
            b.key.nickname.to_lowercase(),
        ))
    });
    (sources, notes)
}

fn ours(root: &Path, sources: &mut Vec<Source>) {
    for (kind, folder) in [
        (PreviewKind::Symbol, "symbols"),
        (PreviewKind::Footprint, "footprints"),
    ] {
        let folder = root.join(folder);
        if is_link(&folder) {
            continue;
        }
        for (path, name) in plain_entries(&folder) {
            let found = match kind {
                PreviewKind::Symbol => name
                    .strip_suffix(".kicad_sym")
                    .map(|nick| (nick, Format::SymbolFile))
                    .or_else(|| {
                        name.strip_suffix(".kicad_symdir")
                            .map(|nick| (nick, Format::SymbolFolder))
                    }),
                PreviewKind::Footprint => name
                    .strip_suffix(".pretty")
                    .map(|nick| (nick, Format::Footprints)),
            };
            let Some((nickname, format)) = found.filter(|(nick, _)| nick.starts_with("g-")) else {
                continue;
            };
            sources.push(Source {
                key: LibraryKey {
                    group: LibraryGroup::Ours,
                    nickname: nickname.to_owned(),
                },
                kind,
                description: String::new(),
                path,
                format,
                problem: None,
            });
        }
    }
}

fn kicad(share: &Path, sources: &mut Vec<Source>) {
    for (kind, folder, table) in [
        (PreviewKind::Symbol, "symbols", TableKind::Symbol),
        (PreviewKind::Footprint, "footprints", TableKind::Footprint),
    ] {
        // KiCad's template table describes each library.
        let descriptions: BTreeMap<String, String> =
            fs::read(share.join("template").join(table.file_name()))
                .ok()
                .and_then(|bytes| LibTable::parse(&bytes, table).ok())
                .map(|table| {
                    table
                        .rows
                        .into_iter()
                        .map(|row| (row.name, row.descr))
                        .collect()
                })
                .unwrap_or_default();
        for (path, name) in plain_entries(&share.join(folder)) {
            let found = match kind {
                PreviewKind::Symbol => name
                    .strip_suffix(".kicad_sym")
                    .map(|nick| (nick, Format::SymbolFile))
                    .or_else(|| {
                        name.strip_suffix(".kicad_symdir")
                            .map(|nick| (nick, Format::SymbolFolder))
                    }),
                PreviewKind::Footprint => name
                    .strip_suffix(".pretty")
                    .map(|nick| (nick, Format::Footprints)),
            };
            let Some((nickname, format)) = found else {
                continue;
            };
            sources.push(Source {
                key: LibraryKey {
                    group: LibraryGroup::Kicad,
                    nickname: nickname.to_owned(),
                },
                kind,
                description: descriptions.get(nickname).cloned().unwrap_or_default(),
                path,
                format,
                problem: None,
            });
        }
    }
}

fn installed(places: &Places, config: &Path, sources: &mut Vec<Source>, notes: &mut Vec<String>) {
    let documents = ["USERPROFILE", "HOME"]
        .iter()
        .find_map(|name| places.environment.get(*name))
        .map(|home| PathBuf::from(home).join("Documents"));
    let (vars, problem) = PathVariables::for_kicad(&VariableSources {
        environment: places.environment.clone(),
        config_dir: Some(config.to_path_buf()),
        share_dir: places.share.clone(),
        documents_dir: documents,
        major: KICAD_MAJOR,
    });
    notes.extend(problem);
    // Already listed under Ours or KiCad.
    let mut listed: Vec<PathBuf> = vec![
        places.catalog_root.join("symbols"),
        places.catalog_root.join("footprints"),
    ];
    if let Some(share) = &places.share {
        listed.push(share.join("symbols"));
        listed.push(share.join("footprints"));
    }
    let mut other_kinds = 0usize;
    for (kind, table) in [
        (PreviewKind::Symbol, TableKind::Symbol),
        (PreviewKind::Footprint, TableKind::Footprint),
    ] {
        let reading = read_tables(config, table, &vars);
        notes.extend(reading.problems);
        for library in reading.libraries {
            if library.row.hidden {
                continue;
            }
            if !library.row.is_kicad() {
                other_kinds += 1;
                continue;
            }
            let (path, problem) = match library.path {
                Ok(path) => {
                    if listed.iter().any(|folder| is_inside(&path, folder)) {
                        continue;
                    }
                    let problem = (!path.exists())
                        .then(|| format!("It was not found at {}.", path.display()));
                    (path, problem)
                }
                Err(why) => (
                    PathBuf::new(),
                    Some(format!("Its path cannot be found: {why}.")),
                ),
            };
            let format = match kind {
                PreviewKind::Footprint => Format::Footprints,
                PreviewKind::Symbol
                    if path.is_dir()
                        || path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("kicad_symdir")) =>
                {
                    Format::SymbolFolder
                }
                PreviewKind::Symbol => Format::SymbolFile,
            };
            sources.push(Source {
                key: LibraryKey {
                    group: LibraryGroup::Installed,
                    nickname: library.row.name.clone(),
                },
                kind,
                description: library.row.descr.clone(),
                path,
                format,
                problem,
            });
        }
    }
    if other_kinds > 0 {
        notes.push(format!(
            "{other_kinds} {} in your KiCad library tables {} not KiCad files (such as database libraries), so {} not shown.",
            if other_kinds == 1 { "library" } else { "libraries" },
            if other_kinds == 1 { "is" } else { "are" },
            if other_kinds == 1 { "it is" } else { "they are" },
        ));
    }
}

/// Whether `path` is a link (never followed).
pub(crate) fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_symlink() || is_windows_reparse_point(&metadata)
    })
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_windows_reparse_point(_: &fs::Metadata) -> bool {
    false
}

/// The entries of `folder` that are not links, with their names; none if
/// it cannot be read.
pub(crate) fn plain_entries(folder: &Path) -> Vec<(PathBuf, String)> {
    let Ok(read) = fs::read_dir(folder) else {
        return Vec::new();
    };
    read.flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| !kind.is_symlink())
                && entry
                    .metadata()
                    .is_ok_and(|metadata| !is_windows_reparse_point(&metadata))
        })
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            Some((entry.path(), name))
        })
        .collect()
}

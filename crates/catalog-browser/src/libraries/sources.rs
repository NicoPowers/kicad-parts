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
//!
//! Finding them touches no installed library itself, so an offline network
//! share cannot hold up opening the browser; the reader threads find out
//! whether each one exists.
//!
//! Links: our libraries follow the library check's rule (no link or other
//! reparse point anywhere). Elsewhere only symbolic links and junctions
//! count as links, never followed inside a library, but the folder or
//! file an installed library's row names may be one: the person chose it,
//! and KiCad follows it.

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
    /// Our libraries: refuse any reparse point, as the library check does.
    pub strict_links: bool,
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
    /// The person's Documents folder, for `KICAD10_3RD_PARTY`.
    pub documents: Option<PathBuf>,
}

/// What [`find_sources`] found.
#[derive(Clone, Debug, Default)]
pub(crate) struct Found {
    pub sources: Vec<Source>,
    /// Things the person should know, such as a table that could not be read.
    pub notes: Vec<String>,
    /// Said in the Installed group when it lists nothing, such as KiCad's
    /// settings not being found.
    pub installed_note: Option<String>,
}

/// KiCad's version whose variables (`KICAD10_*`) are expanded.
const KICAD_MAJOR: u32 = kicad_integration::SUPPORTED_MAJOR;

/// Every library, and notes for the person, in display order: symbols
/// then footprints, each by group and then nickname.
pub(crate) fn find_sources(places: &Places) -> Found {
    let mut sources = Vec::new();
    let mut notes = Vec::new();
    ours(&places.catalog_root, &mut sources);
    if let Some(share) = &places.share {
        kicad(share, &mut sources);
    }
    let before = sources.len();
    let installed_note = match &places.kicad_config {
        Some(config) if config.is_dir() => {
            installed(places, config, &mut sources, &mut notes);
            (sources.len() == before)
                .then(|| "Your KiCad library tables add no other libraries.".to_owned())
        }
        Some(config) => Some(format!(
            "KiCad {KICAD_MAJOR}'s settings were not found at {}, so no installed libraries are listed.",
            config.display()
        )),
        None => Some(format!(
            "KiCad {KICAD_MAJOR}'s settings folder was not found, so no installed libraries are listed."
        )),
    };
    sources.sort_by(|a, b| {
        (a.kind as u8, a.key.group, a.key.nickname.to_lowercase()).cmp(&(
            b.kind as u8,
            b.key.group,
            b.key.nickname.to_lowercase(),
        ))
    });
    Found {
        sources,
        notes,
        installed_note,
    }
}

fn ours(root: &Path, sources: &mut Vec<Source>) {
    for (kind, folder) in [
        (PreviewKind::Symbol, "symbols"),
        (PreviewKind::Footprint, "footprints"),
    ] {
        let folder = root.join(folder);
        if is_link(&folder, true) {
            continue;
        }
        for (path, name) in plain_entries(&folder, true).0 {
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
                strict_links: true,
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
        for (path, name) in plain_entries(&share.join(folder), false).0 {
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
                strict_links: false,
            });
        }
    }
}

fn installed(places: &Places, config: &Path, sources: &mut Vec<Source>, notes: &mut Vec<String>) {
    let documents = places.documents.clone();
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
                    (path, None)
                }
                Err(why) => (
                    PathBuf::new(),
                    Some(format!("Its path cannot be found: {why}.")),
                ),
            };
            let format = match kind {
                PreviewKind::Footprint => Format::Footprints,
                // By its name only: the reader threads touch the library.
                PreviewKind::Symbol
                    if path
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
                strict_links: false,
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

/// Whether `metadata` is a link: a symbolic link or junction, or, with
/// `strict` (our libraries, as the library check reads them), any reparse
/// point. OneDrive's online-only files are reparse points but not links.
pub(crate) fn metadata_is_link(metadata: &fs::Metadata, strict: bool) -> bool {
    metadata.file_type().is_symlink() || (strict && is_windows_reparse_point(metadata))
}

/// Whether `path` is a link (see [`metadata_is_link`]).
pub(crate) fn is_link(path: &Path, strict: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata_is_link(&metadata, strict))
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

/// The entries of `folder` that are not links (see [`metadata_is_link`]),
/// with their names, and how many links were left out; none if it cannot
/// be read.
pub(crate) fn plain_entries(folder: &Path, strict: bool) -> (Vec<(PathBuf, String)>, usize) {
    let Ok(read) = fs::read_dir(folder) else {
        return (Vec::new(), 0);
    };
    let mut entries = Vec::new();
    let mut links = 0;
    for entry in read.flatten() {
        // On Windows a directory entry's metadata describes the entry
        // itself, never what a link points at.
        if entry
            .metadata()
            .is_ok_and(|metadata| metadata_is_link(&metadata, strict))
        {
            links += 1;
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            entries.push((entry.path(), name.to_owned()));
        }
    }
    (entries, links)
}

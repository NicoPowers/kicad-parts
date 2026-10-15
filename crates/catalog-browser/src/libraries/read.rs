//! Reads one library into the index, with the library check's safety: no
//! link inside a library is followed, files over a size limit are refused,
//! and so is a file nested too deeply. In a folder library a file that
//! cannot be read is skipped and the rest is kept, as KiCad does; the
//! library then says how many were skipped. What was read is cached on
//! disk, one file per library, with the library's fingerprint (sizes and
//! modification times), so it is read again only when it changed.

use super::sources::{Format, Source, plain_entries};
use kicad_integration::library_summary::{
    Mount, SUMMARY_VERSION, SymbolSummary, inherit_from_parents, read_footprint_file,
    read_symbol_file,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The largest symbol library file read; KiCad's largest is 16 MB.
pub(crate) const MAX_SYMBOL_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// The largest footprint file read; KiCad's are all under 0.5 MB.
pub(crate) const MAX_FOOTPRINT_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// The most files read from one library folder.
pub(crate) const MAX_FILES: usize = 50_000;

/// The cache folder's name: bumped whenever what is cached changes shape,
/// and whenever the readers change what they return ([`SUMMARY_VERSION`]).
fn cache_layout() -> String {
    format!("v2-summary{SUMMARY_VERSION}")
}

/// One symbol, as the browser lists and filters it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SymbolItem {
    pub name: String,
    pub description: String,
    pub keywords: String,
    pub reference: String,
    pub footprint: String,
    pub filters: Vec<String>,
    pub power: bool,
    pub pins: u32,
    /// In a `.kicad_symdir`: the file it is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// One footprint, as the browser lists and filters it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct FootprintItem {
    pub name: String,
    pub description: String,
    pub keywords: String,
    pub pads: u32,
    /// `smd`, `tht` or blank.
    pub mount: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "items", rename_all = "snake_case")]
pub(crate) enum Items {
    Symbols(Vec<SymbolItem>),
    Footprints(Vec<FootprintItem>),
}

impl Items {
    pub fn len(&self) -> usize {
        match self {
            Self::Symbols(items) => items.len(),
            Self::Footprints(items) => items.len(),
        }
    }
}

/// A library's items with what searching them needs.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub fingerprint: String,
    pub items: Items,
    /// Why files of a folder library were skipped, one line each.
    pub skipped: Vec<String>,
    /// Per item, its name, description and keywords in lower case,
    /// separated by a character no search term holds.
    pub search: Vec<String>,
}

/// What reading a library gave.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Read {
    pub items: Option<Items>,
    pub skipped: Vec<String>,
}

impl Loaded {
    fn new(fingerprint: String, items: Items, skipped: Vec<String>) -> Self {
        let text = |parts: [&str; 3]| {
            parts
                .iter()
                .map(|part| part.to_lowercase())
                .collect::<Vec<_>>()
                .join("\u{1f}")
        };
        let search = match &items {
            Items::Symbols(items) => items
                .iter()
                .map(|item| text([&item.name, &item.description, &item.keywords]))
                .collect(),
            Items::Footprints(items) => items
                .iter()
                .map(|item| text([&item.name, &item.description, &item.keywords]))
                .collect(),
        };
        Self {
            fingerprint,
            items,
            skipped,
            search,
        }
    }
}

/// A cheap stand-in for a library's content: its file's size and
/// modification time, or those of every library file in its folder.
pub(crate) fn fingerprint(source: &Source) -> Result<String, String> {
    let stamp = |metadata: &fs::Metadata| {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos());
        format!("{}:{modified}", metadata.len())
    };
    // The reader threads, not the browser's opening, find out whether an
    // installed library is there (it may be on a network share).
    if !source.path.exists() {
        return Err(format!("It was not found at {}.", source.path.display()));
    }
    match source.format {
        Format::SymbolFile => {
            let metadata = fs::metadata(&source.path).map_err(|error| {
                format!("{} could not be read: {error}.", source.path.display())
            })?;
            Ok(stamp(&metadata))
        }
        Format::SymbolFolder | Format::Footprints => {
            if !source.path.is_dir() {
                return Err(format!("{} is not a folder.", source.path.display()));
            }
            let mut lines: Vec<String> = plain_entries(&source.path, source.strict_links)
                .0
                .into_iter()
                .filter(|(_, name)| wanted_file(source.format, name))
                .filter_map(|(path, name)| {
                    let metadata = fs::metadata(path).ok()?;
                    Some(format!("{name}\0{}", stamp(&metadata)))
                })
                .collect();
            lines.sort();
            let mut hasher = Sha256::new();
            for line in &lines {
                hasher.update(line.as_bytes());
                hasher.update(b"\n");
            }
            Ok(format!("{}:{:x}", lines.len(), hasher.finalize()))
        }
    }
}

fn wanted_file(format: Format, name: &str) -> bool {
    match format {
        Format::SymbolFile => false,
        Format::SymbolFolder => name.ends_with(".kicad_sym"),
        Format::Footprints => name.ends_with(".kicad_mod"),
    }
}

/// The library's items: from memory when `known` has the same
/// fingerprint, else from the disk cache, else read and cached.
pub(crate) fn load(
    source: &Source,
    known: Option<&std::sync::Arc<Loaded>>,
    cache_dir: &Path,
) -> Result<std::sync::Arc<Loaded>, String> {
    let fingerprint = fingerprint(source)?;
    if let Some(known) = known.filter(|known| known.fingerprint == fingerprint) {
        return Ok(known.clone());
    }
    let file = cache_file(cache_dir, source);
    if let Some(read) = read_cache(&file, source, &fingerprint) {
        if let Some(items) = read.items {
            return Ok(std::sync::Arc::new(Loaded::new(
                fingerprint,
                items,
                read.skipped,
            )));
        }
    }
    let read = read_library(source)?;
    write_cache(&file, source, &fingerprint, &read);
    let items = read.items.unwrap_or(Items::Symbols(Vec::new()));
    Ok(std::sync::Arc::new(Loaded::new(
        fingerprint,
        items,
        read.skipped,
    )))
}

/// How [`read_limited`] treats a link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Links {
    /// The library's own file, which an installed library's row may name
    /// through a link.
    Follow,
    /// A file inside a library; `strict` for our libraries.
    Refuse { strict: bool },
}

/// Reads every item of a library. A folder library keeps what it can read
/// and lists the files it skipped.
pub(crate) fn read_library(source: &Source) -> Result<Read, String> {
    match source.format {
        Format::SymbolFile => {
            let links = if source.strict_links {
                Links::Refuse { strict: true }
            } else {
                Links::Follow
            };
            let bytes = read_limited(&source.path, MAX_SYMBOL_FILE_BYTES, links)?;
            let summary = read_symbol_file(&bytes).map_err(|error| {
                format!("{} could not be read: {error}.", source.path.display())
            })?;
            Ok(Read {
                items: Some(Items::Symbols(
                    summary
                        .symbols
                        .into_iter()
                        .map(|symbol| symbol_item(symbol, None))
                        .collect(),
                )),
                skipped: Vec::new(),
            })
        }
        Format::SymbolFolder => {
            let (files, mut skipped) = folder_files(source)?;
            let links = Links::Refuse {
                strict: source.strict_links,
            };
            let mut symbols: Vec<(SymbolSummary, String)> = Vec::new();
            for (path, name) in files {
                let summary = read_limited(&path, MAX_SYMBOL_FILE_BYTES, links).and_then(|bytes| {
                    read_symbol_file(&bytes)
                        .map_err(|error| format!("{name} could not be read: {error}."))
                });
                match summary {
                    Ok(summary) => symbols.extend(
                        summary
                            .symbols
                            .into_iter()
                            .map(|symbol| (symbol, name.clone())),
                    ),
                    Err(why) => skipped.push(why),
                }
            }
            // A derived symbol's parent is in another file of the folder.
            let (mut summaries, files): (Vec<SymbolSummary>, Vec<String>) =
                symbols.into_iter().unzip();
            inherit_from_parents(&mut summaries);
            Ok(Read {
                items: Some(Items::Symbols(
                    summaries
                        .into_iter()
                        .zip(files)
                        .map(|(symbol, file)| symbol_item(symbol, Some(file)))
                        .collect(),
                )),
                skipped,
            })
        }
        Format::Footprints => {
            let (files, mut skipped) = folder_files(source)?;
            let links = Links::Refuse {
                strict: source.strict_links,
            };
            let mut footprints = Vec::new();
            for (path, name) in files {
                let summary =
                    read_limited(&path, MAX_FOOTPRINT_FILE_BYTES, links).and_then(|bytes| {
                        read_footprint_file(&bytes)
                            .map_err(|error| format!("{name} could not be read: {error}."))
                    });
                let summary = match summary {
                    Ok(summary) => summary,
                    Err(why) => {
                        skipped.push(why);
                        continue;
                    }
                };
                footprints.push(FootprintItem {
                    // KiCad names a footprint by its file.
                    name: name.trim_end_matches(".kicad_mod").to_owned(),
                    description: summary.description,
                    keywords: summary.keywords,
                    pads: summary.pad_count,
                    mount: match summary.mount {
                        Mount::Smd => "smd",
                        Mount::ThroughHole => "tht",
                        Mount::Other => "",
                    }
                    .to_owned(),
                    model: summary.model,
                });
            }
            Ok(Read {
                items: Some(Items::Footprints(footprints)),
                skipped,
            })
        }
    }
}

fn symbol_item(symbol: SymbolSummary, file: Option<String>) -> SymbolItem {
    SymbolItem {
        name: symbol.name,
        description: symbol.description,
        keywords: symbol.keywords,
        reference: symbol.reference,
        footprint: symbol.footprint,
        filters: symbol.footprint_filters,
        power: symbol.power,
        pins: symbol.pin_count,
        file,
    }
}

/// The library files of a folder library, in name order, and a line for
/// the links left out (never followed).
/// A folder library's files (path and name) and why others were skipped.
type FolderFiles = (Vec<(PathBuf, String)>, Vec<String>);

fn folder_files(source: &Source) -> Result<FolderFiles, String> {
    if !source.path.is_dir() {
        return Err(format!("{} is not a folder.", source.path.display()));
    }
    let (entries, links) = plain_entries(&source.path, source.strict_links);
    let mut files: Vec<(PathBuf, String)> = entries
        .into_iter()
        .filter(|(_, name)| wanted_file(source.format, name))
        .collect();
    if files.len() > MAX_FILES {
        return Err(format!(
            "{} has more than {MAX_FILES} files, so it was not read.",
            source.path.display()
        ));
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let skipped = match links {
        0 => Vec::new(),
        1 => vec!["1 link in it was not followed.".to_owned()],
        n => vec![format!("{n} links in it were not followed.")],
    };
    Ok((files, skipped))
}

/// A file's bytes, refusing one larger than `limit` and, unless `links`
/// says to follow, a link.
pub(crate) fn read_limited(path: &Path, limit: u64, links: Links) -> Result<Vec<u8>, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let metadata = match links {
        Links::Follow => fs::metadata(path),
        Links::Refuse { .. } => fs::symlink_metadata(path),
    }
    .map_err(|error| format!("{name} could not be read: {error}."))?;
    if let Links::Refuse { strict } = links {
        if super::sources::metadata_is_link(&metadata, strict) {
            return Err(format!("{name} is a link, which is not followed."));
        }
    }
    if !metadata.is_file() {
        return Err(format!("{name} is not a file."));
    }
    if metadata.len() > limit {
        return Err(format!(
            "{name} is larger than {} MB, so it was not read.",
            limit / (1024 * 1024)
        ));
    }
    fs::read(path).map_err(|error| format!("{name} could not be read: {error}."))
}

/// Removes cache folders of other layouts and temporary files a crash left
/// behind. Run when the index starts.
pub(crate) fn clean_cache(cache_dir: &Path) {
    let root = cache_dir.join("library-index");
    let current = cache_layout();
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy() != current {
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            }
            continue;
        }
        for file in fs::read_dir(&path).into_iter().flatten().flatten() {
            if file.file_name().to_string_lossy().ends_with(".tmp") {
                let _ = fs::remove_file(file.path());
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Cached {
    path: String,
    fingerprint: String,
    read: Read,
}

/// `<cache>/library-index/<layout>/<sha256 of what the library is>.json`.
pub(crate) fn cache_file(cache_dir: &Path, source: &Source) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "{:?}\n{:?}\n{}\n{}",
        source.kind,
        source.key.group,
        source.key.nickname,
        source.path.display()
    ));
    cache_dir
        .join("library-index")
        .join(cache_layout())
        .join(format!("{:x}.json", hasher.finalize()))
}

fn read_cache(file: &Path, source: &Source, fingerprint: &str) -> Option<Read> {
    let bytes = fs::read(file).ok()?;
    let cached: Cached = serde_json::from_slice(&bytes).ok()?;
    (cached.fingerprint == fingerprint && cached.path == source.path.display().to_string())
        .then_some(cached.read)
}

/// Writes the cache file through a temporary file, so a crash never
/// leaves half a file. A cache that cannot be written is only slower.
fn write_cache(file: &Path, source: &Source, fingerprint: &str, read: &Read) {
    let Some(folder) = file.parent() else {
        return;
    };
    if fs::create_dir_all(folder).is_err() {
        return;
    }
    let cached = Cached {
        path: source.path.display().to_string(),
        fingerprint: fingerprint.to_owned(),
        read: read.clone(),
    };
    let Ok(bytes) = serde_json::to_vec(&cached) else {
        return;
    };
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    if fs::write(&temporary, bytes).is_ok() && fs::rename(&temporary, file).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

//! Reads one library into the index, with the library check's safety: no
//! link inside a library is followed, files over a size limit are refused,
//! and so is a file nested too deeply. What was read is cached on disk,
//! one file per library, with the library's fingerprint (sizes and
//! modification times), so it is read again only when it changed.

use super::sources::{Format, Source, plain_entries};
use kicad_integration::library_summary::{
    Mount, SymbolSummary, inherit_from_parents, read_footprint_file, read_symbol_file,
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

/// Bumped whenever what is cached changes shape or meaning.
const CACHE_LAYOUT: &str = "v1";

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
    /// Per item, its name, description and keywords in lower case,
    /// separated by a character no search term holds.
    pub search: Vec<String>,
}

impl Loaded {
    fn new(fingerprint: String, items: Items) -> Self {
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
            let mut lines: Vec<String> = plain_entries(&source.path)
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
    if let Some(items) = read_cache(&file, source, &fingerprint) {
        return Ok(std::sync::Arc::new(Loaded::new(fingerprint, items)));
    }
    let items = read_library(source)?;
    write_cache(&file, source, &fingerprint, &items);
    Ok(std::sync::Arc::new(Loaded::new(fingerprint, items)))
}

/// Reads every item of a library.
pub(crate) fn read_library(source: &Source) -> Result<Items, String> {
    match source.format {
        Format::SymbolFile => {
            let bytes = read_limited(&source.path, MAX_SYMBOL_FILE_BYTES)?;
            let summary = read_symbol_file(&bytes).map_err(|error| {
                format!("{} could not be read: {error}.", source.path.display())
            })?;
            Ok(Items::Symbols(
                summary
                    .symbols
                    .into_iter()
                    .map(|symbol| symbol_item(symbol, None))
                    .collect(),
            ))
        }
        Format::SymbolFolder => {
            let mut symbols: Vec<(SymbolSummary, String)> = Vec::new();
            for (path, name) in folder_files(source)? {
                let bytes = read_limited(&path, MAX_SYMBOL_FILE_BYTES)?;
                let summary = read_symbol_file(&bytes)
                    .map_err(|error| format!("{} could not be read: {error}.", path.display()))?;
                symbols.extend(
                    summary
                        .symbols
                        .into_iter()
                        .map(|symbol| (symbol, name.clone())),
                );
            }
            // A derived symbol's parent is in another file of the folder.
            let (mut summaries, files): (Vec<SymbolSummary>, Vec<String>) =
                symbols.into_iter().unzip();
            inherit_from_parents(&mut summaries);
            Ok(Items::Symbols(
                summaries
                    .into_iter()
                    .zip(files)
                    .map(|(symbol, file)| symbol_item(symbol, Some(file)))
                    .collect(),
            ))
        }
        Format::Footprints => {
            let mut footprints = Vec::new();
            for (path, name) in folder_files(source)? {
                let bytes = read_limited(&path, MAX_FOOTPRINT_FILE_BYTES)?;
                let summary = read_footprint_file(&bytes)
                    .map_err(|error| format!("{} could not be read: {error}.", path.display()))?;
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
            Ok(Items::Footprints(footprints))
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

/// The library files of a folder library, in name order; links are left
/// out.
fn folder_files(source: &Source) -> Result<Vec<(PathBuf, String)>, String> {
    if !source.path.is_dir() {
        return Err(format!("{} is not a folder.", source.path.display()));
    }
    let mut files: Vec<(PathBuf, String)> = plain_entries(&source.path)
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
    Ok(files)
}

/// A file's bytes, refusing a link or one larger than `limit`.
pub(crate) fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("{} could not be read: {error}.", path.display()))?;
    if !metadata.is_file() || super::sources::is_link(path) {
        return Err(format!(
            "{} is not a file (links are not followed).",
            path.display()
        ));
    }
    if metadata.len() > limit {
        return Err(format!(
            "{} is larger than {} MB, so it was not read.",
            path.display(),
            limit / (1024 * 1024)
        ));
    }
    fs::read(path).map_err(|error| format!("{} could not be read: {error}.", path.display()))
}

#[derive(Deserialize, Serialize)]
struct Cached {
    path: String,
    fingerprint: String,
    items: Items,
}

/// `<cache>/library-index/v1/<sha256 of what the library is>.json`.
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
        .join(CACHE_LAYOUT)
        .join(format!("{:x}.json", hasher.finalize()))
}

fn read_cache(file: &Path, source: &Source, fingerprint: &str) -> Option<Items> {
    let bytes = fs::read(file).ok()?;
    let cached: Cached = serde_json::from_slice(&bytes).ok()?;
    (cached.fingerprint == fingerprint && cached.path == source.path.display().to_string())
        .then_some(cached.items)
}

/// Writes the cache file through a temporary file, so a crash never
/// leaves half a file. A cache that cannot be written is only slower.
fn write_cache(file: &Path, source: &Source, fingerprint: &str, items: &Items) {
    let Some(folder) = file.parent() else {
        return;
    };
    if fs::create_dir_all(folder).is_err() {
        return;
    }
    let cached = Cached {
        path: source.path.display().to_string(),
        fingerprint: fingerprint.to_owned(),
        items: items.clone(),
    };
    let Ok(bytes) = serde_json::to_vec(&cached) else {
        return;
    };
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    if fs::write(&temporary, bytes).is_ok() && fs::rename(&temporary, file).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

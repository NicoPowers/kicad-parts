//! The KiCad libraries made from the Catalog, so engineers place the
//! team's parts in KiCad and a design never switches to newer CAD by
//! surprise (kicad-parts-f397, ADR 0012).
//!
//! One folder holds, for one computer:
//!
//! - `team-parts.kicad_dbl` and `team-parts.sqlite`: a KiCad database
//!   library with one entry per part revision, named like
//!   `RES-0000-0470_v2`. Its key is the entry name alone
//!   (`globally_unique_keys`), so a placed part's link,
//!   `team-parts:RES-0000-0470_v2`, keeps working when the revision becomes
//!   older or archived and its row moves to another group.
//! - `symbols/<library>.kicad_sym`: every symbol revision, named like
//!   `R_US_v2`, in the library its CAD item names, such as `g-res`.
//! - `footprints/<library>.pretty/<name>_v<n>.kicad_mod`: every footprint
//!   revision, with its 3D model links pointed at
//!   `footprints/<library>.3dshapes/`, where its models are named like it.
//!
//! [`update`] brings the folder to a Catalog state. A revision never
//! changes, so files already written are only checked, never rewritten; an
//! update adds entries and moves rows between groups. CAD that KiCad's own
//! libraries hold stays a link to KiCad's library and is not copied.

mod database;
mod files;
mod names;
pub mod odbc;
mod plan;
pub mod setup;

pub use database::{DATABASE_FILE, REFRESH_SECONDS};
pub use names::{file_stem, revision_name, windows_refuses};
pub use plan::{ARCHIVED_GROUP, OLDER_GROUP};
pub use setup::{DBL_FILE, Library, LibraryKind, NICKNAME};

use catalog::pocketbase::{CatalogReadback, StoredBlob};
use catalog::sha256_hex;
use files::{WriteError, read_if_there, write_whole};
use kicad_integration::library_file::{
    SymbolLibrary, rename_symbol_entry, replace_model_references,
};
use plan::{FileKind, FilePlan, Group, SymbolEntryPlan, SymbolLibraryPlan};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The file version of a generated symbol library whose symbols name none.
const DEFAULT_SYMBOL_VERSION: &str = "20241209";

/// What one update did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Outcome {
    /// Entries in the database library, by where KiCad's chooser shows
    /// them: under their category, or as older or archived revisions.
    pub current: usize,
    pub older: usize,
    pub archived: usize,
    /// Files written or replaced, relative to the folder, with `/`.
    pub written: Vec<String>,
    /// The database changed.
    pub database_changed: bool,
    /// The `.kicad_dbl` was there and changed, such as for a new category
    /// or field. A running KiCad keeps the one it loaded until it
    /// restarts.
    pub layout_changed: bool,
    /// What could not be made, in plain sentences.
    pub problems: Vec<String>,
    /// The libraries to add to KiCad.
    pub libraries: Vec<Library>,
}

/// Why an update stopped. Files written before it stay: each is a whole,
/// correct file. The database and `.kicad_dbl` change only after every
/// file they name is in place.
#[derive(Debug)]
pub enum UpdateError {
    /// KiCad or another program holds the file, so it was not replaced.
    InUse(PathBuf),
    /// A revision's file differs from what the Catalog stores, so
    /// something outside this app changed it. It is left as it is.
    Changed(PathBuf),
    /// A stored file could not be downloaded or did not match its hash.
    Blob(String),
    Io(PathBuf, std::io::Error),
    Database(String),
}

impl UpdateError {
    /// What went wrong, in plain sentences, naming files inside `folder`
    /// by their path from it, such as `symbols/g-res.kicad_sym`.
    pub fn message(&self, folder: &Path) -> String {
        self.describe(Some(folder))
    }

    fn describe(&self, folder: Option<&Path>) -> String {
        let name = |file: &Path| match folder.and_then(|folder| file.strip_prefix(folder).ok()) {
            Some(inside) => inside.to_string_lossy().replace('\\', "/"),
            None => file.display().to_string(),
        };
        match self {
            Self::InUse(file) => format!(
                "KiCad or another program has {} open, so the old file was kept. The next update tries again.",
                name(file)
            ),
            Self::Changed(file) => format!(
                "{} was changed outside this app, so it was left as it is. Delete that file, then update again to write it from the team catalog.",
                name(file)
            ),
            Self::Blob(message) => {
                format!("A file could not be read from the team server: {message}")
            }
            Self::Io(file, error) => format!("{} could not be written: {error}", name(file)),
            Self::Database(message) => format!("The database could not be updated: {message}"),
        }
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.describe(None))
    }
}

impl std::error::Error for UpdateError {}

impl From<WriteError> for UpdateError {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::InUse(file) => Self::InUse(file),
            WriteError::Io(file, error) => Self::Io(file, error),
        }
    }
}

/// Gives the bytes of a stored blob, such as by downloading them.
pub type BlobSource<'a> = dyn FnMut(&StoredBlob) -> Result<Vec<u8>, String> + 'a;

/// The libraries [`update`] would make in `folder`, without writing.
pub fn libraries(folder: &Path, catalog: &CatalogReadback) -> Vec<Library> {
    setup::libraries(folder, &plan::plan(catalog, folder))
}

/// Brings the KiCad libraries in `folder` to `catalog`. Blob bytes come
/// from `blobs`, and only for files not written yet. Only one update may
/// run on a folder at a time.
pub fn update(
    folder: &Path,
    catalog: &CatalogReadback,
    blobs: &mut BlobSource<'_>,
) -> Result<Outcome, UpdateError> {
    let plan = plan::plan(catalog, folder);
    let mut outcome = Outcome {
        problems: plan.problems.clone(),
        libraries: setup::libraries(folder, &plan),
        ..Outcome::default()
    };
    let mut fetch = |blob: &StoredBlob| -> Result<Vec<u8>, UpdateError> {
        let bytes = blobs(blob).map_err(UpdateError::Blob)?;
        if sha256_hex(&bytes) != blob.sha256 {
            return Err(UpdateError::Blob(format!(
                "the file {} does not match its SHA-256",
                blob.sha256
            )));
        }
        Ok(bytes)
    };
    // Models before the footprints that link to them (the paths sort that
    // way), and every file before the rows that name it.
    for file in &plan.files {
        write_revision_file(folder, file, &mut fetch, &mut outcome)?;
    }
    for library in &plan.symbol_libraries {
        write_symbol_library(folder, library, &mut fetch, &mut outcome)?;
    }
    outcome.database_changed =
        database::sync(&folder.join(DATABASE_FILE), &plan.tables).map_err(UpdateError::Database)?;
    let dbl = database::dbl_json(&plan.tables);
    let dbl_path = folder.join(DBL_FILE);
    let existing_dbl = read_if_there(&dbl_path)?;
    if existing_dbl.as_deref() != Some(dbl.as_bytes()) {
        write_whole(&dbl_path, dbl.as_bytes())?;
        outcome.layout_changed = existing_dbl.is_some();
        outcome.written.push(DBL_FILE.to_owned());
    }
    for table in &plan.tables {
        let count = table.rows.len();
        match table.group {
            Group::Category => outcome.current += count,
            Group::Older => outcome.older += count,
            Group::Archived => outcome.archived += count,
        }
    }
    Ok(outcome)
}

/// Writes a footprint or 3D model revision's file if it is not there yet;
/// otherwise checks it is still exactly what the Catalog stores.
fn write_revision_file(
    folder: &Path,
    file: &FilePlan<'_>,
    fetch: &mut dyn FnMut(&StoredBlob) -> Result<Vec<u8>, UpdateError>,
    outcome: &mut Outcome,
) -> Result<(), UpdateError> {
    let path = folder.join(&file.path);
    if let Some(on_disk) = read_if_there(&path)? {
        return if is_written_from(&on_disk, file) {
            Ok(())
        } else {
            Err(UpdateError::Changed(path))
        };
    }
    let stored = fetch(file.blob)?;
    let bytes = match &file.kind {
        FileKind::Model => stored,
        FileKind::Footprint { links, references } => match relinked(&stored, links, references) {
            Some(bytes) => bytes,
            None => {
                outcome.problems.push(format!(
                    "The 3D model links of the {} could not be changed, so they were kept.",
                    file.label
                ));
                stored
            }
        },
    };
    write_whole(&path, &bytes)?;
    outcome.written.push(file.path.clone());
    Ok(())
}

/// A footprint's bytes with its model links replaced, when the links in
/// the bytes are the ones the Catalog recorded.
fn relinked(stored: &[u8], links: &[Option<String>], references: &[String]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(stored).ok()?;
    let (replaced, found) = replace_model_references(text, links);
    (found == references).then(|| replaced.into_bytes())
}

/// Whether `on_disk` is the file `file` writes: its stored bytes, with the
/// planned model links when it is a footprint. Checked without the stored
/// bytes: putting the recorded links back must give bytes with the stored
/// hash.
fn is_written_from(on_disk: &[u8], file: &FilePlan<'_>) -> bool {
    if sha256_hex(on_disk) == file.blob.sha256 {
        return true;
    }
    let FileKind::Footprint { links, references } = &file.kind else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(on_disk) else {
        return false;
    };
    let (same, found) = replace_model_references(text, links);
    if same != text || found.len() != references.len() {
        return false;
    }
    let originals: Vec<Option<String>> = references.iter().cloned().map(Some).collect();
    let (original, _) = replace_model_references(text, &originals);
    sha256_hex(original.as_bytes()) == file.blob.sha256
}

/// Writes a symbol library when its entries changed. Entries already in
/// the file are kept byte for byte, after checking each is still its
/// stored symbol renamed.
fn write_symbol_library(
    folder: &Path,
    library: &SymbolLibraryPlan<'_>,
    fetch: &mut dyn FnMut(&StoredBlob) -> Result<Vec<u8>, UpdateError>,
    outcome: &mut Outcome,
) -> Result<(), UpdateError> {
    let path = folder.join(&library.path);
    let existing = read_if_there(&path)?;
    let mut on_disk: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if let Some(bytes) = &existing {
        let parsed = SymbolLibrary::parse(bytes).map_err(|_| UpdateError::Changed(path.clone()))?;
        for entry in parsed.entries {
            on_disk.insert(entry.name, bytes[entry.span].to_vec());
        }
    }
    let mut entries: Vec<Vec<u8>> = Vec::new();
    let mut version: Option<&str> = None;
    for entry in &library.entries {
        let bytes = match on_disk.get(&entry.name) {
            Some(kept) => {
                let stored = rename_symbol_entry(kept, &entry.name, &entry.original)
                    .map_err(|_| UpdateError::Changed(path.clone()))?;
                if sha256_hex(&stored) != entry.blob.sha256 {
                    return Err(UpdateError::Changed(path));
                }
                kept.clone()
            }
            None => match renamed_symbol(&fetch(entry.blob)?, entry) {
                Ok(bytes) => bytes,
                Err(problem) => {
                    outcome.problems.push(problem);
                    continue;
                }
            },
        };
        if let Some(entry_version) = entry.version.as_deref() {
            version = Some(version.map_or(entry_version, |v| v.max(entry_version)));
        }
        entries.push(bytes);
    }
    let mut content = format!(
        "(kicad_symbol_lib\n\t(version {})\n\t(generator \"kicad_parts\")\n",
        version.unwrap_or(DEFAULT_SYMBOL_VERSION)
    )
    .into_bytes();
    for entry in entries {
        content.push(b'\t');
        content.extend_from_slice(&entry);
        content.push(b'\n');
    }
    content.extend_from_slice(b")\n");
    if existing.as_deref() != Some(&content[..]) {
        write_whole(&path, &content)?;
        outcome.written.push(library.path.clone());
    }
    Ok(())
}

/// A stored symbol renamed to its revision name, or why it cannot be used.
fn renamed_symbol(stored: &[u8], entry: &SymbolEntryPlan<'_>) -> Result<Vec<u8>, String> {
    let unreadable = || {
        format!(
            "The {} could not be read as one KiCad symbol, so it was left out.",
            entry.label
        )
    };
    let mut wrapped = b"(kicad_symbol_lib ".to_vec();
    wrapped.extend_from_slice(stored);
    wrapped.push(b')');
    let library = SymbolLibrary::parse(&wrapped).map_err(|_| unreadable())?;
    let [only] = &library.entries[..] else {
        return Err(unreadable());
    };
    if only.name != entry.original {
        return Err(unreadable());
    }
    if let Some(parent) = &only.extends {
        return Err(format!(
            "The {} is drawn from another symbol, {parent}, which the team catalog does not store with it, so it was left out.",
            entry.label
        ));
    }
    rename_symbol_entry(stored, &entry.original, &entry.name).map_err(|_| unreadable())
}

#[cfg(test)]
mod tests;

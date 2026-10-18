//! The KiCad libraries made from the Catalog, so engineers place the
//! team's parts in KiCad and a design never switches to newer CAD by
//! surprise (kicad-parts-f397, ADR 0012).
//!
//! Each team has a folder, `kicad-libraries/<CODE>/`, for one computer.
//! Its libraries are named with the team's code (see [`Naming`]); for the
//! team with code `MA`:
//!
//! - `MA-Parts.kicad_dbl` and `MA-Parts.sqlite`: a KiCad database library
//!   with one entry per part revision, named like `RES-0000-0470_v2`. Its
//!   key is the entry name alone (`globally_unique_keys`), so a placed
//!   part's link, `MA-Parts:RES-0000-0470_v2`, keeps working when the
//!   revision becomes older or archived and its row moves to another group.
//! - `symbols/MA-res-20241209.kicad_sym`: every symbol revision of one CAD
//!   library saved in one KiCad file version, named like `R_US_v2`.
//! - `footprints/MA-Resistor_SMD.pretty/<name>_v<n>.kicad_mod`: every
//!   footprint revision, with its 3D model links pointed, relative to the
//!   library, at `footprints/MA-Resistor_SMD.3dshapes/`, where its models
//!   are named like it.
//!
//! [`update`] brings the folder to a Catalog state. A revision never
//! changes, so files already written are only checked, never rewritten;
//! an update adds entries and moves rows between groups, and never takes
//! an entry away. CAD that KiCad's own libraries hold stays a link to
//! KiCad's library and is not copied. Generated files are read-only, and
//! files in the folder that the Catalog does not make are reported.

mod database;
mod files;
pub mod kicad_process;
mod names;
pub mod odbc;
mod plan;
pub mod setup;
pub mod team;

pub use database::REFRESH_SECONDS;
pub use files::LOCK_FILE;
pub use names::{file_stem, revision_name, windows_refuses};
pub use plan::{ARCHIVED_GROUP, NEWEST_SYMBOL_VERSION, Naming, OLDER_GROUP, team_code_problem};
pub use setup::{Library, LibraryKind};
pub use team::{FolderState, STATE_FILE, Team, read_state, same_server, team_folder, team_folders};

use catalog::pocketbase::{CatalogReadback, StoredBlob};
use catalog::sha256_hex;
use files::{FolderLock, WriteError, read_if_there, remove_leftovers, write_whole};
use kicad_integration::library_file::{
    SymbolLibrary, rename_symbol_entry, replace_model_references,
};
use plan::{FileKind, FilePlan, Group, SYMBOL_COLUMN, SymbolEntryPlan, SymbolLibraryPlan};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What one update did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Outcome {
    /// The team's folder the libraries are in.
    pub folder: PathBuf,
    /// Entries in the database library, by where KiCad's chooser shows
    /// them: under their category, or as older or archived revisions.
    pub current: usize,
    pub older: usize,
    pub archived: usize,
    /// Files written or replaced, relative to the folder, with `/`.
    pub written: Vec<String>,
    /// The database changed.
    pub database_changed: bool,
    /// The `.kicad_dbl` was there and changed in this update, such as for
    /// a new category or field.
    pub layout_changed: bool,
    /// When the `.kicad_dbl` last changed (milliseconds since 1970, UTC),
    /// in this update or an earlier one. A KiCad that started before then
    /// still has the old groups and fields.
    pub layout_changed_at_ms: Option<u64>,
    /// What could not be made, or was kept although the Catalog no longer
    /// lists it, in plain sentences.
    pub problems: Vec<String>,
    /// The libraries to add to KiCad.
    pub libraries: Vec<Library>,
    /// Libraries this update made that were not there before, when the
    /// folder already had libraries: the person must add them to KiCad.
    pub new_libraries: Vec<Library>,
}

/// Why an update stopped. Files written before it stay: each is a whole,
/// correct file. The database and `.kicad_dbl` change only after every
/// file they name is in place.
#[derive(Debug)]
pub enum UpdateError {
    /// KiCad or another program holds the file, so it was not replaced.
    InUse(PathBuf),
    /// Windows refused to replace the file, and no program seems to hold
    /// it open.
    Denied(PathBuf),
    /// Another update of the same folder is running.
    Busy,
    /// An update of the folder ran for longer than [`use_new_server`]
    /// waits, so the person's answer was not saved.
    AnswerNotSaved,
    /// The folder for the team's code belongs to another team, so it was
    /// not changed. It stays that way until this team's code changes: KiCad
    /// cannot hold two libraries with one name.
    OtherTeam {
        folder: PathBuf,
        code: String,
        /// The team server of the team whose libraries are in the folder.
        other_server: String,
        /// This team's server, where a superuser can change its code.
        server: String,
    },
    /// The team's folder was made from another team server, so it was not
    /// changed. It stays that way until the person confirms that the team
    /// moved ([`use_new_server`]): another server could claim the team's
    /// ID and change its parts.
    OtherServer {
        folder: PathBuf,
        code: String,
        /// The server the folder records.
        recorded_server: String,
        /// The server the update came from.
        server: String,
    },
    /// The folder's path has a character the SQLite3 ODBC driver's
    /// connection string cannot hold.
    Folder(PathBuf),
    /// A stored file could not be downloaded or did not match its hash.
    Blob(String),
    Io(PathBuf, std::io::Error),
    Database(String),
}

/// `file` named by its path from `folder`, such as `symbols/x.kicad_sym`,
/// when it is inside it.
fn inside(folder: Option<&Path>, file: &Path) -> String {
    match folder.and_then(|folder| file.strip_prefix(folder).ok()) {
        Some(inside) => inside.to_string_lossy().replace('\\', "/"),
        None => file.display().to_string(),
    }
}

impl UpdateError {
    /// What went wrong, in plain sentences, naming files inside `folder`
    /// by their path from it, such as `symbols/team-res-20241209.kicad_sym`.
    pub fn message(&self, folder: &Path) -> String {
        self.describe(Some(folder))
    }

    fn describe(&self, folder: Option<&Path>) -> String {
        let name = |file: &Path| inside(folder, file);
        match self {
            Self::InUse(file) => format!(
                "KiCad or another program has {} open, so the old file was kept. The next update tries again.",
                name(file)
            ),
            Self::Denied(file) => format!(
                "Windows refused to replace {} (permission denied), so the old file was kept. Check that you can change files in that folder; the next update tries again.",
                name(file)
            ),
            Self::Busy => "Another update of these libraries is running, from this app or another one. The next update tries again.".to_owned(),
            Self::AnswerNotSaved => "The libraries are being updated, so your answer was not saved. Try again in a moment.".to_owned(),
            Self::OtherTeam {
                folder,
                code,
                other_server,
                server,
            } => format!(
                "Two teams on this computer use the code {code}, and KiCad can't hold both. The folder {} holds the {code} libraries of the team on {other_server}, so it was not changed. To fix this, a superuser of {server} changes this team's code before any board uses it. Then sign in again.",
                folder.display()
            ),
            Self::OtherServer {
                code,
                recorded_server,
                server,
                ..
            // The KiCad settings panel says the same (kicad_libraries.rs in
            // the desktop UI).
            } => format!(
                "The KiCad libraries for {code} come from {recorded_server}, so the app does not update them from {server}. If the team moved to {server}, choose Use the new address on {code}'s row in the Teams menu."
            ),
            Self::Folder(folder) => format!(
                "The folder {} has a ; or a brace in its path, which the SQLite3 ODBC driver cannot read. Use a data folder without them.",
                folder.display()
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
            WriteError::Denied(file) => Self::Denied(file),
            WriteError::Io(file, error) => Self::Io(file, error),
        }
    }
}

/// Gives the bytes of a stored blob, such as by downloading them.
pub type BlobSource<'a> = dyn FnMut(&StoredBlob) -> Result<Vec<u8>, String> + 'a;

/// Whether the SQLite3 ODBC driver can open a database in `folder`: the
/// `.kicad_dbl`'s connection string ends each value at `;` and wraps
/// values in braces, so the path may hold neither.
pub fn folder_is_usable(folder: &Path) -> bool {
    !folder
        .to_string_lossy()
        .chars()
        .any(|ch| matches!(ch, ';' | '{' | '}'))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// Brings `team`'s KiCad libraries, in its folder under `root` (the
/// `kicad-libraries` folder), to `catalog`. Blob bytes come from `blobs`,
/// and only for files not written yet. One update runs on a folder at a
/// time; another one at the same time stops with [`UpdateError::Busy`]. A
/// folder that belongs to another team is never written.
pub fn update(
    root: &Path,
    catalog: &CatalogReadback,
    team: &Team,
    blobs: &mut BlobSource<'_>,
) -> Result<Outcome, UpdateError> {
    let naming = &team.naming;
    let folder = &team_folder(root, naming.code());
    if !folder_is_usable(folder) {
        return Err(UpdateError::Folder(folder.to_path_buf()));
    }
    let refused = |state: FolderState, refusal: team::Refusal| match refusal {
        team::Refusal::OtherTeam => UpdateError::OtherTeam {
            folder: folder.to_path_buf(),
            code: naming.code().to_owned(),
            other_server: state.server,
            server: team.server.clone(),
        },
        team::Refusal::OtherServer => UpdateError::OtherServer {
            folder: folder.to_path_buf(),
            code: naming.code().to_owned(),
            recorded_server: state.server,
            server: team.server.clone(),
        },
    };
    // Checked before the lock too, so a refused folder does not even get a
    // lock file.
    if let Some(state) = read_state(folder) {
        if let Some(refusal) = state.refusal(team) {
            return Err(refused(state, refusal));
        }
    }
    std::fs::create_dir_all(folder)
        .map_err(|error| UpdateError::Io(folder.to_path_buf(), error))?;
    let _lock = FolderLock::take(folder)
        .map_err(|error| UpdateError::Io(folder.join(LOCK_FILE), error))?
        .ok_or(UpdateError::Busy)?;
    let mut state = match read_state(folder) {
        Some(state) => match state.refusal(team) {
            Some(refusal) => return Err(refused(state, refusal)),
            None => state,
        },
        None => FolderState::default(),
    };
    state.team_id.clone_from(&team.id);
    // Records the server in a folder that had none (see
    // `FolderState::refusal`), and writes an older address the way sign-in
    // writes it now.
    state.server.clone_from(&team.server);
    state.code = naming.code().to_owned();
    team::write_state(folder, &state)?;
    remove_leftovers(folder);
    let plan = plan::plan(catalog, naming);
    let libraries = setup::libraries(folder, &plan, naming);
    let dbl_path = folder.join(naming.dbl_file());
    let existing_dbl = read_if_there(&dbl_path)?;
    let mut outcome = Outcome {
        folder: folder.to_path_buf(),
        problems: plan.problems.clone(),
        new_libraries: if existing_dbl.is_some() {
            new_libraries(folder, &plan, &libraries)
        } else {
            Vec::new()
        },
        libraries,
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
    let mut missing_symbols = BTreeSet::new();
    for library in &plan.symbol_libraries {
        for name in write_symbol_library(folder, library, &mut fetch, &mut outcome)? {
            missing_symbols.insert(format!("{}:{name}", library.nickname));
        }
    }
    for file in files_not_from_the_catalog(folder, &plan) {
        outcome.problems.push(format!(
            "{file} did not come from the team catalog. This app leaves it alone, but no one else on the team gets it. Keep your own symbols and footprints in a library of your own."
        ));
    }
    for table in &plan.tables {
        for row in &table.rows {
            let symbol = &row.values[SYMBOL_COLUMN];
            if missing_symbols.contains(symbol) {
                outcome.problems.push(format!(
                    "{} uses the symbol {symbol}, which is not in the KiCad libraries, so KiCad shows it as missing.",
                    row.entry()
                ));
            }
        }
    }
    let synced = database::sync(&folder.join(naming.database_file()), &plan.tables)
        .map_err(UpdateError::Database)?;
    outcome.database_changed = synced.changed;
    for entry in synced.kept {
        outcome.problems.push(format!(
            "{entry} is not in this reading of the team catalog, so its row was kept as it was."
        ));
    }
    let dbl = database::dbl_json(&plan.tables, naming);
    if existing_dbl.as_deref() != Some(dbl.as_bytes()) {
        write_whole(&dbl_path, dbl.as_bytes())?;
        outcome.written.push(naming.dbl_file());
        if existing_dbl.is_some() {
            outcome.layout_changed = true;
            state.layout_changed_at_ms = Some(now_ms());
        }
    }
    state.updated_at_ms = Some(now_ms());
    team::write_state(folder, &state)?;
    outcome.layout_changed_at_ms = state.layout_changed_at_ms;
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

/// How long [`use_new_server`] waits for an update of the folder to end.
const ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The person confirmed that `team` moved from `made_from`, the server
/// they were shown, to `team.server`: its folder records the new server
/// and takes updates from it again.
///
/// A folder of another team is refused, as in [`update`]. So is a folder
/// that no longer records `made_from`, because the person answered a
/// question that is out of date; [`UpdateError::OtherServer`] names the
/// server it records now. A folder that records nothing yet is left alone:
/// the next update records the server.
pub fn use_new_server(root: &Path, team: &Team, made_from: &str) -> Result<(), UpdateError> {
    let folder = &team_folder(root, team.naming.code());
    if read_state(folder).is_none() {
        return Ok(());
    }
    // An update from the old server may hold the folder for a moment.
    let waited_since = std::time::Instant::now();
    let _lock = loop {
        match FolderLock::take(folder)
            .map_err(|error| UpdateError::Io(folder.join(LOCK_FILE), error))?
        {
            Some(lock) => break lock,
            None if waited_since.elapsed() < ANSWER_WAIT => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            None => return Err(UpdateError::AnswerNotSaved),
        }
    };
    let Some(mut state) = read_state(folder) else {
        return Ok(());
    };
    if state.team_id != team.id {
        return Err(UpdateError::OtherTeam {
            folder: folder.to_path_buf(),
            code: team.naming.code().to_owned(),
            other_server: state.server,
            server: team.server.clone(),
        });
    }
    if !same_server(&state.server, made_from) {
        return Err(UpdateError::OtherServer {
            folder: folder.to_path_buf(),
            code: team.naming.code().to_owned(),
            recorded_server: state.server,
            server: team.server.clone(),
        });
    }
    state.server.clone_from(&team.server);
    team::write_state(folder, &state)
}

/// The files in the `symbols` and `footprints` folders that `plan` does
/// not make, by path from `folder` with `/`, such as a footprint someone
/// saved into a library with KiCad: KiCad lets them, because the folders
/// can be written. Hidden files and the ones File Explorer makes are left
/// out.
fn files_not_from_the_catalog(folder: &Path, plan: &plan::Plan<'_>) -> Vec<String> {
    fn walk(base: &Path, folder: &Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') || name == "desktop.ini" || name == "thumbs.db" {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, found);
            } else if let Ok(inside) = path.strip_prefix(base) {
                found.push(inside.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    // Windows paths ignore case.
    let planned: BTreeSet<String> = plan
        .files
        .iter()
        .map(|file| file.path.as_str())
        .chain(
            plan.symbol_libraries
                .iter()
                .map(|library| library.path.as_str()),
        )
        .map(str::to_lowercase)
        .collect();
    let mut found = Vec::new();
    for kind in ["symbols", "footprints"] {
        walk(folder, &folder.join(kind), &mut found);
    }
    found.retain(|file| !planned.contains(&file.to_lowercase()));
    found.sort();
    found
}

/// The symbol and footprint libraries `plan` has that the folder does not
/// have yet.
fn new_libraries(folder: &Path, plan: &plan::Plan<'_>, libraries: &[Library]) -> Vec<Library> {
    let paths: Vec<&str> = plan
        .symbol_libraries
        .iter()
        .map(|library| library.path.as_str())
        .chain(
            plan.footprint_libraries
                .iter()
                .map(|library| library.path.as_str()),
        )
        .collect();
    paths
        .into_iter()
        .zip(libraries.iter().skip(1))
        .filter(|(path, _)| !folder.join(path).exists())
        .map(|(_, library)| library.clone())
        .collect()
}

/// Writes a footprint or 3D model revision's file if it is not there yet.
/// A file that is there is checked: if something outside this app changed
/// it, it is kept as it is and reported, and the update carries on.
fn write_revision_file(
    folder: &Path,
    file: &FilePlan<'_>,
    fetch: &mut dyn FnMut(&StoredBlob) -> Result<Vec<u8>, UpdateError>,
    outcome: &mut Outcome,
) -> Result<(), UpdateError> {
    let path = folder.join(&file.path);
    if let Some(on_disk) = read_if_there(&path)? {
        if !is_written_from(&on_disk, file) {
            outcome.problems.push(changed_outside(&file.path));
        }
        return Ok(());
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

/// The problem for a generated file something outside this app changed.
fn changed_outside(file: &str) -> String {
    format!(
        "{file} was changed outside this app, so it was kept as it is. Delete that file, then update again to write it from the team catalog."
    )
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

/// Writes a symbol library when its entries changed, and returns the
/// names of planned entries it could not hold. Entries already in the file
/// are kept byte for byte, after checking each is still its stored symbol
/// renamed; entries the plan no longer has are kept too, because a row may
/// still use them. A file changed outside this app is kept as it is and
/// reported, and its new entries wait until it is fixed.
fn write_symbol_library(
    folder: &Path,
    library: &SymbolLibraryPlan<'_>,
    fetch: &mut dyn FnMut(&StoredBlob) -> Result<Vec<u8>, UpdateError>,
    outcome: &mut Outcome,
) -> Result<Vec<String>, UpdateError> {
    let path = folder.join(&library.path);
    let existing = read_if_there(&path)?;
    let mut on_disk: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut disk_order: Vec<String> = Vec::new();
    // The planned entries a kept file lacks.
    let lacking = |on_disk: &BTreeMap<String, Vec<u8>>| -> Vec<String> {
        library
            .entries
            .iter()
            .filter(|entry| !on_disk.contains_key(&entry.name))
            .map(|entry| entry.name.clone())
            .collect()
    };
    if let Some(bytes) = &existing {
        let Ok(parsed) = SymbolLibrary::parse(bytes) else {
            outcome.problems.push(changed_outside(&library.path));
            return Ok(lacking(&BTreeMap::new()));
        };
        let same_version = parsed.version.as_deref() == Some(library.version.as_str());
        for entry in parsed.entries {
            disk_order.push(entry.name.clone());
            on_disk.insert(entry.name, bytes[entry.span].to_vec());
        }
        if !same_version {
            outcome.problems.push(changed_outside(&library.path));
            return Ok(lacking(&on_disk));
        }
    }
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut left_out = Vec::new();
    for entry in &library.entries {
        let bytes = match on_disk.remove(&entry.name) {
            Some(kept) => {
                let stored = rename_symbol_entry(&kept, &entry.name, &entry.original);
                if !stored.is_ok_and(|stored| sha256_hex(&stored) == entry.blob.sha256) {
                    outcome.problems.push(changed_outside(&library.path));
                    on_disk.insert(entry.name.clone(), kept);
                    return Ok(lacking(&on_disk));
                }
                kept
            }
            None => match renamed_symbol(&fetch(entry.blob)?, entry) {
                Ok(bytes) => bytes,
                Err(problem) => {
                    outcome.problems.push(problem);
                    left_out.push(entry.name.clone());
                    continue;
                }
            },
        };
        entries.push((entry.name.clone(), bytes));
    }
    // What the file has that the plan does not: kept in place.
    for name in disk_order {
        if let Some(bytes) = on_disk.remove(&name) {
            outcome.problems.push(format!(
                "The symbol {name} in {} is not in this reading of the team catalog, so it was kept.",
                library.path
            ));
            entries.push((name, bytes));
        }
    }
    let mut content = format!(
        "(kicad_symbol_lib\n\t(version {})\n\t(generator \"kicad_parts\")\n",
        library.version
    )
    .into_bytes();
    for (_, entry) in entries {
        content.push(b'\t');
        content.extend_from_slice(&entry);
        content.push(b'\n');
    }
    content.extend_from_slice(b")\n");
    if existing.as_deref() != Some(&content[..]) {
        write_whole(&path, &content)?;
        outcome.written.push(library.path.clone());
    }
    Ok(left_out)
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

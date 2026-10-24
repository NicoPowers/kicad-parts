//! The scan of a library repo at one commit (kicad-parts-e9a3): where its
//! symbol and footprint libraries, 3D models, path variables, lib tables
//! and `.kicad_dbl` files are. It works on the file list of the app's own
//! copy and the bytes of a few files, never on a person's checkout.
//!
//! [`files_to_read`] says which files the scan needs; [`scan`] is one pure
//! function over the list and those bytes, so tests pass made-up repos.
//!
//! - **Libraries** are found by file type: `.kicad_sym` files,
//!   `.kicad_symdir` folders and `.pretty` folders.
//! - **Nicknames** come from the repo's own lib tables when a row names
//!   the library (`${KIPRJMOD}` is the table's folder), else from the file
//!   or folder name. They are never changed (ADR 0022).
//! - **Path variables** come from footprints' 3D model links, such as
//!   `${ACME_3D}/R_0603.step`, and from lib table rows. The folder each
//!   must point to is the one whose files match the most links.
//! - **Doubts** are what the person should check: a link to a missing 3D
//!   model, a name that differs only in capitals, two libraries with one
//!   nickname, a variable with no matching folder, a file stored with Git
//!   LFS, a lib table that can't be read, a library file whose name
//!   Windows can't keep (such as `CON.kicad_sym`), which is left out, an
//!   old-format `.lib` symbol library, a footprint outside any `.pretty`
//!   folder, and links through KiCad's own folders other than its 3D
//!   models, such as `${KICAD8_3RD_PARTY}`.
//!
//! Paths are repo paths with `/`, compared with exact case, as on Linux:
//! a link that works only on Windows or macOS is a doubt.

use super::super::RepoFile;
use catalog::download::model_links;
use desktop_contract::{LayoutDoubt, LayoutLibrary, LayoutVariable, LibraryLayout};
use kicad_integration::lib_table::{LibTable, TableKind};
use kicad_integration::library_source::{
    is_plain_repo_path, join, parent, plain_path, resolve_path,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The file extensions of 3D models KiCad opens.
const MODEL_EXTENSIONS: [&str; 5] = ["step", "stp", "stpz", "wrl", "wrz"];

/// How every Git LFS pointer file starts.
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/";

/// Git LFS pointer files are about 130 bytes; larger files are never one.
const LFS_POINTER_MAX: u64 = 1024;

/// The files the scan reads: every lib table and footprint, and every small
/// 3D model or library file, which may be a Git LFS pointer.
pub(crate) fn files_to_read(files: &[RepoFile]) -> Vec<&str> {
    files
        .iter()
        .filter(|file| {
            table_kind(&file.path).is_some()
                || has_extension(&file.path, "kicad_mod")
                || (file.size <= LFS_POINTER_MAX
                    && (is_model(&file.path) || has_extension(&file.path, "kicad_sym")))
        })
        .map(|file| file.path.as_str())
        .collect()
}

/// Scans the repo at `commit`, whose files are `files`, with the bytes of
/// [`files_to_read`] in `read` by path.
pub(crate) fn scan(
    commit: &str,
    files: &[RepoFile],
    read: &HashMap<String, Vec<u8>>,
) -> LibraryLayout {
    let mut doubts: BTreeSet<LayoutDoubt> = BTreeSet::new();
    let paths: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let lowercase: HashMap<String, &str> = paths
        .iter()
        .map(|path| (path.to_ascii_lowercase(), *path))
        .collect();
    for (path, bytes) in read {
        if bytes.starts_with(LFS_POINTER) {
            doubts.insert(LayoutDoubt::GitLfs { path: path.clone() });
        }
    }

    // Libraries and models, by file type.
    let mut symbols: BTreeMap<String, Found> = BTreeMap::new();
    let mut footprints: BTreeMap<String, Found> = BTreeMap::new();
    let mut models: Vec<&str> = Vec::new();
    let mut databases = Vec::new();
    for path in &paths {
        let library_file = symbol_library(path).is_some() || footprint_library(path).is_some();
        if library_file && !is_plain_repo_path(path) {
            // The app could not write it out as a file on Windows.
            doubts.insert(LayoutDoubt::PathNotUsable {
                path: (*path).to_owned(),
            });
            continue;
        }
        if let Some(library) = symbol_library(path) {
            symbols
                .entry(library.clone())
                .or_insert_with(|| Found::named(&library));
        } else if let Some(library) = footprint_library(path) {
            footprints
                .entry(library.clone())
                .or_insert_with(|| Found::named(&library));
        } else if is_model(path) {
            models.push(path);
        }
        if has_extension(path, "kicad_dbl") {
            databases.push((*path).to_owned());
        }
        if has_extension(path, "lib") {
            doubts.insert(LayoutDoubt::OldSymbolLibrary {
                path: (*path).to_owned(),
            });
        }
        if has_extension(path, "kicad_mod") && footprint_library(path).is_none() {
            doubts.insert(LayoutDoubt::FootprintOutsideLibrary {
                path: (*path).to_owned(),
            });
        }
    }

    // Lib tables: each row's path, or its variable and the rest.
    let mut tables: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|path| table_kind(path).is_some())
        .collect();
    // Top tables first, so their nicknames win.
    tables.sort_by_key(|path| (path.matches('/').count(), *path));
    let mut rows: Vec<Row> = Vec::new();
    for table in &tables {
        let Some(kind) = table_kind(table) else {
            continue;
        };
        let Some(bytes) = read.get(*table) else {
            continue;
        };
        if bytes.starts_with(LFS_POINTER) {
            continue;
        }
        match LibTable::parse(bytes, kind) {
            Ok(parsed) => {
                for row in parsed.rows {
                    if row.disabled || !row.is_kicad() {
                        continue;
                    }
                    if let Some(target) = row_target(&row.uri, parent(table)) {
                        rows.push(Row {
                            kind,
                            nickname: row.name,
                            target,
                        });
                    }
                }
            }
            Err(error) => {
                doubts.insert(LayoutDoubt::TableUnreadable {
                    table: (*table).to_owned(),
                    why: error.to_string(),
                });
            }
        }
    }

    // Model links through a variable, from every footprint, in a library
    // or not. KiCad's own variables point into each person's KiCad, not
    // this repo; its own 3D models are fine, anything else is a doubt.
    let mut links: Vec<Link> = Vec::new();
    let mut kicads: BTreeMap<String, usize> = BTreeMap::new();
    for path in &paths {
        if !has_extension(path, "kicad_mod") {
            continue;
        }
        let Some(bytes) = read.get(*path) else {
            continue;
        };
        for link in model_links(&String::from_utf8_lossy(bytes)) {
            let Some((variable, rest)) = through_variable(&link) else {
                continue;
            };
            if is_kicads_own(&variable) {
                if !is_kicads_model_folder(&variable) {
                    *kicads.entry(variable).or_default() += 1;
                }
            } else if variable != "KIPRJMOD" {
                // ${KIPRJMOD} is a project's folder, which a library repo
                // can't know.
                links.push(Link {
                    footprint: (*path).to_owned(),
                    text: link.clone(),
                    variable,
                    rest,
                });
            }
        }
    }
    for (name, links) in kicads {
        doubts.insert(LayoutDoubt::KicadVariable { name, links });
    }

    // Each variable's folder: the one whose files match the most uses.
    let library_paths: Vec<String> = symbols.keys().chain(footprints.keys()).cloned().collect();
    let library_refs: Vec<&str> = library_paths.iter().map(String::as_str).collect();
    let mut votes: BTreeMap<String, Votes> = BTreeMap::new();
    for link in &links {
        votes
            .entry(link.variable.clone())
            .or_default()
            .add(&link.rest, &models);
    }
    for row in &rows {
        if let Target::Variable(variable, rest) = &row.target {
            if !is_kicads_own(variable) {
                votes
                    .entry(variable.clone())
                    .or_default()
                    .add(rest, &library_refs);
            }
        }
    }
    let variables: Vec<LayoutVariable> = votes
        .iter()
        .map(|(name, votes)| LayoutVariable {
            name: name.clone(),
            folder: votes.winner(),
            links: votes.uses,
        })
        .collect();
    let folder_of: BTreeMap<&str, &str> = variables
        .iter()
        .filter_map(|variable| Some((variable.name.as_str(), variable.folder.as_deref()?)))
        .collect();
    for variable in &variables {
        if variable.folder.is_none() {
            doubts.insert(LayoutDoubt::VariableNotFound {
                name: variable.name.clone(),
            });
        }
    }

    // Nicknames from the lib tables, the first row naming a library.
    for row in &rows {
        let path = match &row.target {
            Target::InRepo(path) => Some(path.clone()),
            Target::Variable(variable, rest) => folder_of
                .get(variable.as_str())
                .map(|folder| join(folder, rest)),
        };
        let found = match row.kind {
            TableKind::Symbol => &mut symbols,
            TableKind::Footprint => &mut footprints,
        };
        if let Some(library) = path.and_then(|path| found.get_mut(&path)) {
            if !library.from_table {
                library.nickname = row.nickname.clone();
                library.from_table = true;
            }
        }
    }

    // Links to 3D models the repo does not have, or has only in other
    // capitals.
    for link in &links {
        let Some(folder) = folder_of.get(link.variable.as_str()) else {
            continue;
        };
        let target = join(folder, &link.rest);
        if paths.contains(target.as_str()) {
            continue;
        }
        doubts.insert(match lowercase.get(&target.to_ascii_lowercase()) {
            Some(file) => LayoutDoubt::CaseDiffers {
                footprint: link.footprint.clone(),
                link: link.text.clone(),
                file: (*file).to_owned(),
            },
            None => LayoutDoubt::MissingModel {
                footprint: link.footprint.clone(),
                link: link.text.clone(),
            },
        });
    }

    let symbol_libraries = listed(symbols, &mut doubts);
    let footprint_libraries = listed(footprints, &mut doubts);
    let model_folders: BTreeSet<String> = models.iter().map(|path| model_folder(path)).collect();
    let mut folders: BTreeSet<String> = BTreeSet::new();
    for folder in model_folders
        .iter()
        .cloned()
        .chain(library_paths.iter().map(|path| parent(path).to_owned()))
    {
        let mut folder = folder.as_str();
        loop {
            folders.insert(folder.to_owned());
            if folder.is_empty() {
                break;
            }
            folder = parent(folder);
        }
    }
    if folders.is_empty() {
        folders.insert(String::new());
    }
    LibraryLayout {
        commit: commit.to_owned(),
        symbol_libraries,
        footprint_libraries,
        model_folders: model_folders.into_iter().collect(),
        model_files: models.len(),
        path_variables: variables,
        database_libraries: databases,
        lib_tables: tables.iter().map(|path| (*path).to_owned()).collect(),
        doubts: doubts.into_iter().collect(),
        folders: folders.into_iter().collect(),
    }
}

/// A library found by its file type, and its nickname so far.
struct Found {
    nickname: String,
    from_table: bool,
}

impl Found {
    /// Named by its file or folder name.
    fn named(path: &str) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path);
        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        Self {
            nickname: stem.to_owned(),
            from_table: false,
        }
    }
}

/// The libraries by nickname, noting nicknames used twice.
fn listed(
    found: BTreeMap<String, Found>,
    doubts: &mut BTreeSet<LayoutDoubt>,
) -> Vec<LayoutLibrary> {
    let mut libraries: Vec<LayoutLibrary> = found
        .into_iter()
        .map(|(path, found)| LayoutLibrary {
            nickname: found.nickname,
            path,
            from_table: found.from_table,
        })
        .collect();
    libraries.sort_by(|a, b| {
        (a.nickname.to_ascii_lowercase(), &a.nickname, &a.path).cmp(&(
            b.nickname.to_ascii_lowercase(),
            &b.nickname,
            &b.path,
        ))
    });
    let mut by_nickname: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for library in &libraries {
        by_nickname
            .entry(&library.nickname)
            .or_default()
            .push(library.path.clone());
    }
    for (nickname, paths) in by_nickname {
        if paths.len() > 1 {
            doubts.insert(LayoutDoubt::NicknameClash {
                nickname: nickname.to_owned(),
                paths,
            });
        }
    }
    libraries
}

/// One lib table row naming a KiCad library.
struct Row {
    kind: TableKind,
    nickname: String,
    target: Target,
}

/// Where a lib table row's path leads.
enum Target {
    InRepo(String),
    /// Through a path variable, with the rest of the path.
    Variable(String, String),
}

/// One 3D model link through a path variable.
struct Link {
    /// The footprint file's repo path.
    footprint: String,
    /// The link as written.
    text: String,
    variable: String,
    /// The plain path after the variable.
    rest: String,
}

/// How many uses of a variable each folder would satisfy.
#[derive(Default)]
struct Votes {
    uses: usize,
    exact: BTreeMap<String, usize>,
    /// Matches that differ only in capitals, used when no folder matches
    /// exactly.
    any_case: BTreeMap<String, usize>,
}

impl Votes {
    /// One use of the variable with the path `rest` after it: a vote for
    /// each folder that holds a file of `known` at `rest`.
    fn add(&mut self, rest: &str, known: &[&str]) {
        self.uses += 1;
        let mut exact = BTreeSet::new();
        let mut any_case = BTreeSet::new();
        let lower_rest = rest.to_ascii_lowercase();
        for path in known {
            if let Some(folder) = folder_above(path, rest) {
                exact.insert(folder);
            } else if let Some(folder) = folder_above(&path.to_ascii_lowercase(), &lower_rest) {
                // The folder keeps the file's own capitals.
                any_case.insert(path[..folder.len()].to_owned());
            }
        }
        for folder in exact {
            *self.exact.entry(folder).or_default() += 1;
        }
        for folder in any_case {
            *self.any_case.entry(folder).or_default() += 1;
        }
    }

    /// The folder with the most votes; on a tie, the shortest.
    fn winner(&self) -> Option<String> {
        let votes = if self.exact.is_empty() {
            &self.any_case
        } else {
            &self.exact
        };
        votes
            .iter()
            .max_by(|a, b| {
                a.1.cmp(b.1)
                    .then_with(|| b.0.len().cmp(&a.0.len()))
                    .then_with(|| b.0.cmp(a.0))
            })
            .map(|(folder, _)| folder.clone())
    }
}

/// The folder `path` is in when it ends with `rest`: blank when they are
/// the same.
fn folder_above(path: &str, rest: &str) -> Option<String> {
    if path == rest {
        return Some(String::new());
    }
    path.strip_suffix(rest)?
        .strip_suffix('/')
        .map(str::to_owned)
}

/// Where the row path `uri` of a table in the repo folder `folder` leads:
/// a repo path, or a variable and the rest. `None` for a path outside the
/// repo, such as an absolute one.
fn row_target(uri: &str, folder: &str) -> Option<Target> {
    match variable_and_rest(uri) {
        Some((variable, rest)) if variable == "KIPRJMOD" => {
            resolve_path(folder, rest).map(Target::InRepo)
        }
        Some((variable, rest)) => Some(Target::Variable(variable, plain_path(rest)?)),
        None if uri.starts_with('$') => None,
        // An absolute path, or one leaving the repo, is not the repo's.
        None => resolve_path(folder, uri).map(Target::InRepo),
    }
}

/// `${NAME}/rest` or `$(NAME)/rest`: the name and the plain rest, which
/// never climbs out of the variable's folder.
fn through_variable(text: &str) -> Option<(String, String)> {
    let (name, rest) = variable_and_rest(text)?;
    Some((name, plain_path(rest)?))
}

/// `${NAME}/rest` or `$(NAME)/rest`: the name and the rest as written.
fn variable_and_rest(text: &str) -> Option<(String, &str)> {
    let after = text.strip_prefix('$')?;
    let close = match after.chars().next()? {
        '{' => '}',
        '(' => ')',
        _ => return None,
    };
    let end = after.find(close)?;
    let name = &after[1..end];
    let rest = after[end + 1..].strip_prefix(['/', '\\'])?;
    if name.is_empty() {
        return None;
    }
    Some((name.to_owned(), rest))
}

/// KiCad's own variables, which point into KiCad's installed libraries.
fn is_kicads_own(variable: &str) -> bool {
    variable.starts_with("KISYS")
        || variable.strip_prefix("KICAD").is_some_and(|rest| {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            digits > 0 && rest[digits..].starts_with('_')
        })
}

/// KiCad's own 3D model folder (`KICAD<n>_3DMODEL_DIR`, or the older
/// `KISYS3DMOD`): every engineer's KiCad has those models.
fn is_kicads_model_folder(variable: &str) -> bool {
    variable == "KISYS3DMOD" || variable.ends_with("_3DMODEL_DIR")
}

/// The library a symbol file belongs to: a `.kicad_sym` file, or the
/// `.kicad_symdir` folder a file is in.
fn symbol_library(path: &str) -> Option<String> {
    if let Some(folder) = folder_ending(path, ".kicad_symdir") {
        return Some(folder);
    }
    has_extension(path, "kicad_sym").then(|| path.to_owned())
}

/// The `.pretty` folder a `.kicad_mod` file is in.
fn footprint_library(path: &str) -> Option<String> {
    if !has_extension(path, "kicad_mod") {
        return None;
    }
    folder_ending(path, ".pretty").filter(|folder| parent(path) == folder.as_str())
}

/// The first folder of `path` whose name ends with `suffix` (any
/// capitals), up to and including it.
fn folder_ending(path: &str, suffix: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let (_, folders) = parts.split_last()?;
    let at = folders.iter().position(|part| {
        part.len() > suffix.len() && part.to_ascii_lowercase().ends_with(suffix)
    })?;
    Some(parts[..=at].join("/"))
}

/// The folder a 3D model file counts in: its own, or for KiCad's
/// `<library>.3dshapes` folders, the folder above them.
fn model_folder(path: &str) -> String {
    let folder = parent(path);
    let name = folder.rsplit('/').next().unwrap_or(folder);
    if name.to_ascii_lowercase().ends_with(".3dshapes") {
        parent(folder).to_owned()
    } else {
        folder.to_owned()
    }
}

/// A lib table, by its file name: `sym-lib-table` or `fp-lib-table`, or a
/// name ending so, as a table's Table rows may use.
fn table_kind(path: &str) -> Option<TableKind> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.ends_with("sym-lib-table") {
        Some(TableKind::Symbol)
    } else if name.ends_with("fp-lib-table") {
        Some(TableKind::Footprint)
    } else {
        None
    }
}

fn is_model(path: &str) -> bool {
    MODEL_EXTENSIONS
        .iter()
        .any(|extension| has_extension(path, extension))
}

/// Whether the file name ends with `.extension`, in any capitals.
fn has_extension(path: &str, extension: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(stem, found)| !stem.is_empty() && found.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;

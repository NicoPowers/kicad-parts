//! The library files of a reviewed layout, written out of the app's copy
//! so the Libraries browser and kicad-cli can read them as files
//! (kicad-parts-e9a3). The copy has no files checked out, so each commit's
//! symbol and footprint libraries go in `<repo key>/<commit start>/`, once.
//! Both names are short, to stay clear of Windows' path length limit.
//! 3D models stay in the copy: they can be large, and browsing needs none.
//!
//! These are the app's own files, made from one commit; a person never
//! edits them, and an older commit's folder is deleted.

use super::super::{LibraryRepo, RepoFile};
use desktop_contract::RepoProblem;
use kicad_integration::lib_table::TableKind;
use kicad_integration::library_source::{
    LibraryList, SourceProfile, is_plain_repo_path, native_path,
};
use std::fs;
use std::path::{Path, PathBuf};

/// Marks a commit's folder as written whole.
const COMPLETE: &str = ".complete";

/// The folder holding the library files of `profile` at `commit`, written
/// from `repo`'s copy if they are not there yet. `files_folder` is the
/// repo's own; other commits' folders in it are deleted.
pub(crate) fn library_files(
    repo: &LibraryRepo,
    commit: &str,
    profile: &SourceProfile,
    files_folder: &Path,
) -> Result<PathBuf, RepoProblem> {
    let short = &commit[..12.min(commit.len())];
    let folder = files_folder.join(short);
    if folder.join(COMPLETE).is_file() {
        return Ok(folder);
    }
    let listed = repo.list_files(commit)?;
    let wanted = wanted(&listed, profile);
    let paths: Vec<&str> = wanted.iter().map(String::as_str).collect();
    let bytes = repo.read_files(commit, &paths)?;
    let partial = files_folder.join(format!("{short}.partial"));
    let written = (|| -> std::io::Result<()> {
        super::super::repo::remove_folder(&partial)?;
        for (path, bytes) in paths.iter().zip(bytes) {
            let Some(bytes) = bytes else { continue };
            let file = native_path(&partial, path);
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(file, bytes)?;
        }
        fs::create_dir_all(&partial)?;
        fs::write(partial.join(COMPLETE), commit)?;
        super::super::repo::remove_folder(&folder)?;
        fs::rename(&partial, &folder)
    })();
    written.map_err(|error| RepoProblem {
        summary: format!(
            "Can't show the libraries of {}: the app could not write their files on this computer. Check that there is free disk space.",
            repo.address().name()
        ),
        details: error.to_string(),
        offline: false,
    })?;
    // Only the newest commit's files are kept.
    if let Ok(entries) = fs::read_dir(files_folder) {
        for entry in entries.flatten() {
            if entry.file_name() != short {
                let _ = super::super::repo::remove_folder(&entry.path());
            }
        }
    }
    Ok(folder)
}

/// The repo paths of every file of the profile's libraries: a symbol
/// library's file, and every file in a library folder.
fn wanted(listed: &[RepoFile], profile: &SourceProfile) -> Vec<String> {
    let libraries: Vec<&str> = [TableKind::Symbol, TableKind::Footprint]
        .into_iter()
        .flat_map(|kind| match profile.libraries(kind) {
            LibraryList::Listed { libraries } => {
                libraries.iter().map(|l| l.path.as_str()).collect()
            }
            LibraryList::EveryLibraryIn { .. } => Vec::new(),
        })
        .collect();
    listed
        .iter()
        .filter(|file| is_plain_repo_path(&file.path))
        .filter(|file| {
            libraries.iter().any(|library| {
                file.path == *library
                    || file
                        .path
                        .strip_prefix(library)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        })
        .map(|file| file.path.clone())
        .collect()
}

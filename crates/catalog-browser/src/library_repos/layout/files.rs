//! The library files of a reviewed layout, written out of the app's copy
//! so the Libraries browser and kicad-cli can read them as files
//! (kicad-parts-e9a3). The copy has no files checked out, so each commit's
//! symbol and footprint libraries go in `<repo key>/<commit start>/`, once.
//! Both names are short, to stay clear of Windows' path length limit.
//! 3D models stay in the copy: they can be large, and browsing needs none.
//!
//! The files are written in batches, so a large library never sits in
//! memory whole, into a `.partial` folder moved into place when done.
//! These are the app's own files, made from one commit; a person never
//! edits them. [`super::Layouts`] runs one writer per repo at a time and
//! deletes an older commit's folder only once the browser uses the new
//! one.

use super::super::{LibraryRepo, RepoFile};
use desktop_contract::RepoProblem;
use kicad_integration::lib_table::TableKind;
use kicad_integration::library_source::{
    LibraryList, SourceProfile, is_plain_repo_path, native_path,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Marks a commit's folder as written whole.
const COMPLETE: &str = ".complete";

/// How many bytes one batch read asks git for at most.
const BATCH_BYTES: u64 = 16 * 1024 * 1024;

/// The folder for the library files at `commit`, in the repo's own
/// `files_folder`.
pub(crate) fn commit_folder(files_folder: &Path, commit: &str) -> PathBuf {
    files_folder.join(short(commit))
}

/// Whether `folder` holds every library file of `commit`: its mark names
/// that whole commit, not only one that starts the same.
pub(crate) fn is_complete(folder: &Path, commit: &str) -> bool {
    fs::read_to_string(folder.join(COMPLETE)).is_ok_and(|written| written == commit)
}

/// Writes the library files of `profile` at `commit` from `repo`'s copy
/// into [`commit_folder`], unless they are there already.
pub(crate) fn write_library_files(
    repo: &LibraryRepo,
    commit: &str,
    profile: &SourceProfile,
    files_folder: &Path,
) -> Result<(), RepoProblem> {
    let folder = commit_folder(files_folder, commit);
    if is_complete(&folder, commit) {
        return Ok(());
    }
    let listed = repo.list_files(commit)?;
    let wanted = wanted(&listed, profile);
    let partial = files_folder.join(format!("{}.partial", short(commit)));
    let problem = |error: io::Error| RepoProblem {
        summary: format!(
            "Can't show the libraries of {}: the app could not write their files on this computer. Check that there is free disk space.",
            repo.address().name()
        ),
        details: error.to_string(),
        offline: false,
    };
    super::super::repo::remove_folder(&partial).map_err(problem)?;
    fs::create_dir_all(&partial).map_err(problem)?;
    for batch in batches(&wanted) {
        let paths: Vec<&str> = batch.iter().map(|file| file.path.as_str()).collect();
        let bytes = repo.read_files(commit, &paths)?;
        for (path, bytes) in paths.iter().zip(bytes) {
            let Some(bytes) = bytes else { continue };
            let file = native_path(&partial, path);
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent).map_err(problem)?;
            }
            fs::write(file, bytes).map_err(problem)?;
        }
    }
    fs::write(partial.join(COMPLETE), commit).map_err(problem)?;
    super::super::repo::remove_folder(&folder).map_err(problem)?;
    fs::rename(&partial, &folder).map_err(problem)
}

/// Deletes every commit folder in `files_folder` but those in `keep`,
/// such as one the browser may still read.
pub(crate) fn delete_other_commits(files_folder: &Path, keep: &[&Path]) {
    let Ok(entries) = fs::read_dir(files_folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !keep.iter().any(|kept| *kept == path) {
            let _ = super::super::repo::remove_folder(&path);
        }
    }
}

/// The start of `commit`: enough to tell commits apart, short for paths.
fn short(commit: &str) -> &str {
    &commit[..12.min(commit.len())]
}

/// `files` in batches of at most [`BATCH_BYTES`]; a larger file has a
/// batch of its own.
fn batches<'a>(files: &[&'a RepoFile]) -> Vec<Vec<&'a RepoFile>> {
    let mut batches: Vec<Vec<&RepoFile>> = Vec::new();
    let mut bytes = 0;
    for file in files {
        match batches.last_mut() {
            Some(batch) if bytes + file.size <= BATCH_BYTES => batch.push(file),
            _ => {
                batches.push(vec![file]);
                bytes = 0;
            }
        }
        bytes += file.size;
    }
    batches
}

/// Every file of the profile's libraries: a symbol library's file, and
/// every file in a library folder. Only plain repo paths.
fn wanted<'a>(listed: &'a [RepoFile], profile: &SourceProfile) -> Vec<&'a RepoFile> {
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_complete_only_for_the_whole_commit_it_names() {
        let folder = tempfile::tempdir().unwrap();
        let commit = "0123456789ab".to_owned() + &"c".repeat(28);
        let other = "0123456789ab".to_owned() + &"d".repeat(28);
        assert!(!is_complete(folder.path(), &commit));
        fs::write(folder.path().join(COMPLETE), &commit).unwrap();
        assert!(is_complete(folder.path(), &commit));
        // Another commit that starts the same shares the short folder name.
        assert!(!is_complete(folder.path(), &other));
    }

    fn file(path: &str, size: u64) -> RepoFile {
        RepoFile {
            path: path.into(),
            size,
            blob: "a".repeat(40),
        }
    }

    #[test]
    fn files_are_read_in_batches_of_a_few_megabytes() {
        let files = [
            file("a", BATCH_BYTES / 2),
            file("b", BATCH_BYTES / 2),
            file("c", 1),
            file("huge", BATCH_BYTES * 2),
            file("d", 1),
        ];
        let refs: Vec<&RepoFile> = files.iter().collect();
        let names: Vec<Vec<&str>> = batches(&refs)
            .into_iter()
            .map(|batch| batch.into_iter().map(|file| file.path.as_str()).collect())
            .collect();
        assert_eq!(names, [vec!["a", "b"], vec!["c"], vec!["huge"], vec!["d"]]);
    }
}

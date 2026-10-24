//! A library repo's layout: where its libraries, 3D models and path
//! variables are (kicad-parts-e9a3, ADR 0022).
//!
//! The app scans its own copy of the repo ([`scan`]), the person reviews
//! the result on one screen and fixes only what the scan got wrong, and
//! the reviewed layout is kept as a library source profile ([`store`]).
//! Each later fetch follows the newest commit: silently while it scans the
//! same, or marked changed so the person reviews it again. The Libraries
//! browser reads a reviewed layout's libraries from files written out of
//! the copy ([`files`]).

mod files;
mod scan;
mod store;

use super::LibraryRepo;
use desktop_contract::{
    LayoutChoices, LayoutState, LibraryLayout, LibraryLayoutReview, RepoProblem,
};
use kicad_integration::lib_table::TableKind;
use kicad_integration::library_source::{LibraryList, SourceProfile};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use store::{Followed, Reviewed, Store};

/// Every repo's reviewed layout, in `library-layouts/`.
pub(crate) struct Layouts {
    store: Store,
    /// Held while a repo's file is read and written again, so a fetch
    /// that follows a new commit never undoes a review saved meanwhile.
    writing: Mutex<()>,
}

impl Layouts {
    pub(crate) fn new(folder: PathBuf) -> Self {
        Self {
            store: Store::new(folder),
            writing: Mutex::new(()),
        }
    }

    /// Scans `repo` at the latest commit of its main branch in the app's
    /// copy.
    pub(crate) fn scan_latest(repo: &LibraryRepo) -> Result<LibraryLayout, RepoProblem> {
        let Some(commit) = repo.latest_commit()? else {
            return Err(RepoProblem {
                summary: "The repo has no commits yet, so there are no libraries to review."
                    .to_owned(),
                details: String::new(),
                offline: false,
            });
        };
        Self::scan_at(repo, &commit)
    }

    fn scan_at(repo: &LibraryRepo, commit: &str) -> Result<LibraryLayout, RepoProblem> {
        let listed = repo.list_files(commit)?;
        let wanted = scan::files_to_read(&listed);
        let bytes = repo.read_files(commit, &wanted)?;
        let read: HashMap<String, Vec<u8>> = wanted
            .iter()
            .zip(bytes)
            .filter_map(|(path, bytes)| Some(((*path).to_owned(), bytes?)))
            .collect();
        Ok(scan::scan(commit, &listed, &read))
    }

    /// What the team's row says about the layout of the repo whose copy is
    /// `repo`; `None` before the app has a copy.
    pub(crate) fn state(&self, repo: &LibraryRepo) -> Option<LayoutState> {
        if !repo.is_copied() {
            return None;
        }
        Some(match self.store.read(&repo.address().folder_name()) {
            None => LayoutState::NotReviewed,
            Some(reviewed) if reviewed.changed => LayoutState::Changed,
            Some(reviewed) => LayoutState::Reviewed {
                symbol_libraries: count(&reviewed.profile, TableKind::Symbol),
                footprint_libraries: count(&reviewed.profile, TableKind::Footprint),
            },
        })
    }

    /// After a fetch: follows the newest commit of a reviewed layout. A
    /// scan that fails leaves the layout as it was; the next fetch tries
    /// again.
    pub(crate) fn follow_latest(&self, repo: &LibraryRepo) {
        let name = repo.address().folder_name();
        let _writing = self.writing();
        let Some(mut reviewed) = self.store.read(&name) else {
            return;
        };
        let Ok(Some(commit)) = repo.latest_commit() else {
            return;
        };
        if commit == reviewed.commit {
            return;
        }
        let Ok(latest) = Self::scan_at(repo, &commit) else {
            return;
        };
        if store::follow(&mut reviewed, &latest) != Followed::Same {
            self.save(&name, &reviewed);
        }
    }

    /// The newest scan of `repo` to review, with the choices made before,
    /// or the first ones: the team's code `team_code` as the prefix.
    pub(crate) fn review(
        &self,
        repo: &LibraryRepo,
        server_url: &str,
        team_code: &str,
        note: Option<String>,
    ) -> Result<LibraryLayoutReview, RepoProblem> {
        let layout = Self::scan_latest(repo)?;
        let reviewed = self.store.read(&repo.address().folder_name());
        let repo_name = repo.address().name();
        let choices = match &reviewed {
            Some(reviewed) => reviewed.choices.clone(),
            None => store::first_choices(team_code, &repo_name),
        };
        Ok(LibraryLayoutReview {
            server_url: server_url.to_owned(),
            repo_name,
            layout,
            choices,
            reviewed_before: reviewed.is_some(),
            note,
        })
    }

    /// Keeps the layout of `repo` as scanned at `commit`, with `choices`.
    pub(crate) fn use_layout(
        &self,
        repo: &LibraryRepo,
        commit: &str,
        choices: LayoutChoices,
    ) -> Result<(), String> {
        let layout = Self::scan_at(repo, commit).map_err(|problem| problem.summary)?;
        let choices = store::checked(&layout, choices)?;
        let reviewed = Reviewed {
            commit: commit.to_owned(),
            fingerprint: store::fingerprint(&layout),
            profile: store::profile_of(&layout, &choices),
            choices,
            changed: false,
        };
        let name = repo.address().folder_name();
        let _writing = self.writing();
        self.store.write(&name, &reviewed).map_err(|error| {
            format!(
                "Could not keep the library layout in {}: {error}. Check that there is free disk space.",
                self.store.folder().display()
            )
        })
    }

    /// The reviewed profile of `repo` and the folder its library files are
    /// in, written out of the copy if needed; `None` before a review.
    pub(crate) fn browsable(
        &self,
        repo: &LibraryRepo,
    ) -> Option<Result<(SourceProfile, PathBuf), RepoProblem>> {
        let name = repo.address().folder_name();
        let reviewed = self.store.read(&name)?;
        let files_folder = self.store.folder().join(format!("{name}.files"));
        Some(
            files::library_files(repo, &reviewed.commit, &reviewed.profile, &files_folder)
                .map(|folder| (reviewed.profile, folder)),
        )
    }

    /// Deletes the layouts of repos no team links: every entry not named
    /// for a copy in `wanted`.
    pub(crate) fn delete_unlinked(&self, wanted: &BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(self.store.folder()) else {
            return;
        };
        let _writing = self.writing();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let repo = [".json.tmp", ".json", ".files"]
                .iter()
                .find_map(|suffix| name.strip_suffix(suffix))
                .unwrap_or(&name);
            if wanted.contains(repo) {
                continue;
            }
            let path = entry.path();
            let _ = if path.is_dir() {
                super::repo::remove_folder(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }

    fn save(&self, name: &str, reviewed: &Reviewed) {
        if let Err(error) = self.store.write(name, reviewed) {
            eprintln!(
                "kicad-parts-desktop: could not keep the library layout in {} ({error}).",
                self.store.folder().display()
            );
        }
    }

    fn writing(&self) -> MutexGuard<'_, ()> {
        self.writing
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// How many libraries of `kind` a profile has.
fn count(profile: &SourceProfile, kind: TableKind) -> usize {
    match profile.libraries(kind) {
        LibraryList::Listed { libraries } => libraries.len(),
        LibraryList::EveryLibraryIn { .. } => 0,
    }
}

//! Library layouts the person reviewed, kept on this computer: one small
//! JSON file per repo in `library-layouts/`, beside the app's copies in
//! `library-repos/` (kicad-parts-e9a3, ADR 0022).
//!
//! Each file holds the commit the layout was reviewed at, a fingerprint of
//! that commit's scan, the person's choices and the library source profile
//! they make. No secrets: no address with a password ever gets this far.
//!
//! When the main branch moves on, the newest commit is scanned again. The
//! same scan moves the layout to that commit silently; a different one
//! marks it changed, so the team's row asks the person to review it again.
//! The layout reviewed before stays in use meanwhile.

use desktop_contract::{LayoutChoices, LayoutLibrary, LibraryLayout};
use kicad_integration::library_source::{
    LibraryList, SourceLibrary, SourceProfile, SourceVariable,
};
use kicad_integration::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One repo's reviewed layout, as kept in its file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Reviewed {
    /// The commit whose scan the profile follows.
    pub commit: String,
    /// [`fingerprint`] of that commit's scan.
    pub fingerprint: String,
    pub choices: LayoutChoices,
    pub profile: SourceProfile,
    /// A newer commit scanned differently.
    #[serde(default)]
    pub changed: bool,
    /// The newest commit scanned to follow the main branch, so a fetch
    /// that brings nothing new scans nothing.
    #[serde(default)]
    pub scanned: Option<String>,
}

impl Reviewed {
    /// Whether `commit` was already followed: reviewed, or scanned since.
    pub(crate) fn has_followed(&self, commit: &str) -> bool {
        self.commit == commit || self.scanned.as_deref() == Some(commit)
    }
}

/// What the person reviews, and so what must stay the same for a newer
/// commit to keep the review: library nicknames and paths, path variables
/// and their folders, model folders, lib tables and `.kicad_dbl` files.
/// Counts and per-file doubts are left out, so a new footprint with its
/// 3D model asks for no review.
pub(crate) fn fingerprint(layout: &LibraryLayout) -> String {
    let libraries = |libraries: &[desktop_contract::LayoutLibrary]| -> Vec<(String, String)> {
        libraries
            .iter()
            .map(|library| (library.nickname.clone(), library.path.clone()))
            .collect()
    };
    let variables: Vec<(&str, Option<&str>)> = layout
        .path_variables
        .iter()
        .map(|variable| (variable.name.as_str(), variable.folder.as_deref()))
        .collect();
    let parts = serde_json::json!([
        libraries(&layout.symbol_libraries),
        libraries(&layout.footprint_libraries),
        variables,
        layout.model_folders,
        layout.database_libraries,
        layout.lib_tables,
    ]);
    sha256_hex(parts.to_string().as_bytes())
}

/// The choices a first review starts with: the scan's folders, and every
/// library in, except where two libraries of one kind share a nickname.
/// Then the ones no lib table names are left out, since KiCad finds a
/// nickname through the tables; with no table naming one, all but the
/// first are.
pub(crate) fn first_choices(layout: &LibraryLayout) -> LayoutChoices {
    let mut left_out = Vec::new();
    for libraries in [&layout.symbol_libraries, &layout.footprint_libraries] {
        for clash in clashes(libraries) {
            let keep = clash
                .iter()
                .find(|library| library.from_table)
                .or(clash.first())
                .map(|library| library.path.clone());
            left_out.extend(
                clash
                    .iter()
                    .map(|library| library.path.clone())
                    .filter(|path| Some(path) != keep.as_ref()),
            );
        }
    }
    LayoutChoices {
        left_out,
        ..LayoutChoices::default()
    }
}

/// The groups of libraries that share one nickname, in list order.
fn clashes(libraries: &[LayoutLibrary]) -> Vec<Vec<&LayoutLibrary>> {
    let mut by_nickname: BTreeMap<&str, Vec<&LayoutLibrary>> = BTreeMap::new();
    for library in libraries {
        by_nickname
            .entry(&library.nickname)
            .or_default()
            .push(library);
    }
    by_nickname
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

/// `choices` checked against the scan they were made for, with what is
/// wrong in plain words.
pub(crate) fn checked(
    layout: &LibraryLayout,
    mut choices: LayoutChoices,
) -> Result<LayoutChoices, String> {
    // A choice made for an older commit whose variable or folder is gone
    // is dropped, as a library no longer there is: the person sees the
    // scan's own folder instead.
    choices.variable_folders.retain(|name, folder| {
        layout
            .path_variables
            .iter()
            .any(|variable| &variable.name == name)
            && layout.folders.contains(folder)
    });
    let libraries = || {
        layout
            .symbol_libraries
            .iter()
            .chain(&layout.footprint_libraries)
    };
    choices
        .left_out
        .retain(|path| libraries().any(|library| &library.path == path));
    choices.left_out.sort();
    choices.left_out.dedup();
    // KiCad would use only one library of a nickname, so one must go.
    for libraries in [&layout.symbol_libraries, &layout.footprint_libraries] {
        for clash in clashes(libraries) {
            let kept: Vec<&str> = clash
                .iter()
                .map(|library| library.path.as_str())
                .filter(|path| !choices.left_out.iter().any(|out| out == path))
                .collect();
            if kept.len() > 1 {
                return Err(format!(
                    "{} libraries are named {}: {}. KiCad would use only one, so leave out the others.",
                    kept.len(),
                    clash[0].nickname,
                    kept.join(", ")
                ));
            }
        }
    }
    Ok(choices)
}

/// The library source profile `choices` make of `layout`.
pub(crate) fn profile_of(layout: &LibraryLayout, choices: &LayoutChoices) -> SourceProfile {
    let listed = |libraries: &[desktop_contract::LayoutLibrary]| LibraryList::Listed {
        libraries: libraries
            .iter()
            .filter(|library| !choices.left_out.contains(&library.path))
            .map(|library| SourceLibrary {
                nickname: library.nickname.clone(),
                path: library.path.clone(),
            })
            .collect(),
    };
    SourceProfile {
        symbols: listed(&layout.symbol_libraries),
        footprints: listed(&layout.footprint_libraries),
        model_folders: layout.model_folders.clone(),
        path_variables: layout
            .path_variables
            .iter()
            .map(|variable| SourceVariable {
                name: variable.name.clone(),
                folder: choices
                    .variable_folders
                    .get(&variable.name)
                    .cloned()
                    .or_else(|| variable.folder.clone()),
            })
            .collect(),
        database_libraries: layout.database_libraries.clone(),
        models_by_library: None,
        own_library_start: None,
    }
}

/// How a reviewed layout follows the newest commit's scan.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Followed {
    /// Already at that commit.
    Same,
    /// The scan is the same: moved to the new commit.
    Moved,
    /// The scan differs: marked changed, still at the old commit.
    Changed,
}

/// Follows the newest commit's scan, `latest`.
pub(crate) fn follow(reviewed: &mut Reviewed, latest: &LibraryLayout) -> Followed {
    if reviewed.commit == latest.commit {
        return Followed::Same;
    }
    reviewed.scanned = Some(latest.commit.clone());
    if fingerprint(latest) == reviewed.fingerprint {
        reviewed.commit = latest.commit.clone();
        reviewed.changed = false;
        Followed::Moved
    } else {
        reviewed.changed = true;
        Followed::Changed
    }
}

/// The files of reviewed layouts, one per repo by the copy's folder name.
#[derive(Clone, Debug)]
pub(crate) struct Store {
    folder: PathBuf,
}

impl Store {
    pub(crate) fn new(folder: PathBuf) -> Self {
        Self { folder }
    }

    pub(crate) fn folder(&self) -> &Path {
        &self.folder
    }

    /// A short name for the repo whose copy is `name`: the start of its
    /// SHA-256. Copy folder names can be long, and library paths inside
    /// the files folder add more, near Windows' path length limit.
    pub(crate) fn key(name: &str) -> String {
        sha256_hex(name.as_bytes())[..12].to_owned()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.folder.join(format!("{}.json", Self::key(name)))
    }

    /// The folder for the library files of the repo whose copy is `name`.
    pub(crate) fn files_folder(&self, name: &str) -> PathBuf {
        self.folder.join(Self::key(name))
    }

    /// The reviewed layout of the repo whose copy is `name`; `None` if
    /// there is none, or its file can't be read, which means review again.
    pub(crate) fn read(&self, name: &str) -> Option<Reviewed> {
        let bytes = fs::read(self.file(name)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Keeps `reviewed` for the repo whose copy is `name`, through a
    /// temporary file, so a crash never leaves half a file.
    pub(crate) fn write(&self, name: &str, reviewed: &Reviewed) -> io::Result<()> {
        fs::create_dir_all(&self.folder)?;
        let json = serde_json::to_vec_pretty(reviewed).map_err(io::Error::other)?;
        let file = self.file(name);
        let temp = file.with_extension("json.tmp");
        fs::write(&temp, json)?;
        fs::rename(&temp, &file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_contract::{LayoutDoubt, LayoutLibrary, LayoutVariable};

    fn layout(commit: &str) -> LibraryLayout {
        LibraryLayout {
            commit: commit.into(),
            symbol_libraries: vec![LayoutLibrary {
                nickname: "Acme".into(),
                path: "lib/Acme.kicad_sym".into(),
                from_table: true,
            }],
            footprint_libraries: vec![
                LayoutLibrary {
                    nickname: "Acme_FP".into(),
                    path: "lib/Acme.pretty".into(),
                    from_table: true,
                },
                LayoutLibrary {
                    nickname: "Old".into(),
                    path: "old/Old.pretty".into(),
                    from_table: false,
                },
            ],
            model_folders: vec!["3d".into()],
            model_files: 10,
            path_variables: vec![
                LayoutVariable {
                    name: "ACME_3D".into(),
                    folder: Some("3d".into()),
                    links: 4,
                },
                LayoutVariable {
                    name: "LOST".into(),
                    folder: None,
                    links: 1,
                },
            ],
            database_libraries: vec![],
            lib_tables: vec!["fp-lib-table".into()],
            doubts: vec![LayoutDoubt::VariableNotFound {
                name: "LOST".into(),
            }],
            folders: vec!["".into(), "3d".into(), "lib".into(), "old".into()],
        }
    }

    #[test]
    fn choices_make_the_profile_and_never_rename_a_library() {
        let layout = layout("a");
        let choices = checked(
            &layout,
            LayoutChoices {
                variable_folders: [("LOST".to_owned(), "lib".to_owned())].into(),
                left_out: vec!["old/Old.pretty".into(), "not/a/library".into()],
            },
        )
        .unwrap();
        assert_eq!(choices.left_out, ["old/Old.pretty"]);
        let profile = profile_of(&layout, &choices);
        assert_eq!(
            profile.footprints,
            LibraryList::Listed {
                libraries: vec![SourceLibrary {
                    nickname: "Acme_FP".into(),
                    path: "lib/Acme.pretty".into()
                }]
            }
        );
        assert_eq!(
            profile.path_variables,
            [
                SourceVariable {
                    name: "ACME_3D".into(),
                    folder: Some("3d".into())
                },
                SourceVariable {
                    name: "LOST".into(),
                    folder: Some("lib".into())
                },
            ]
        );
        assert_eq!(profile.model_folders, ["3d"]);
    }

    #[test]
    fn a_folder_choice_whose_variable_or_folder_is_gone_is_dropped() {
        let layout = layout("a");
        let with = |variable: &str, folder: &str| {
            checked(
                &layout,
                LayoutChoices {
                    variable_folders: [(variable.to_owned(), folder.to_owned())].into(),
                    left_out: vec![],
                },
            )
            .unwrap()
            .variable_folders
        };
        assert!(with("NOPE", "3d").is_empty());
        assert!(with("ACME_3D", "elsewhere").is_empty());
        assert_eq!(with("ACME_3D", "").len(), 1);
    }

    #[test]
    fn libraries_sharing_a_nickname_keep_only_the_one_a_table_names() {
        let mut layout = layout("a");
        layout.footprint_libraries.push(LayoutLibrary {
            nickname: "Acme_FP".into(),
            path: "archive/Acme.pretty".into(),
            from_table: false,
        });
        let first = first_choices(&layout);
        assert_eq!(first.left_out, ["archive/Acme.pretty"]);
        assert!(checked(&layout, first).is_ok());
        // Keeping both is refused, saying which.
        let both = checked(&layout, LayoutChoices::default()).unwrap_err();
        assert_eq!(
            both,
            "2 libraries are named Acme_FP: lib/Acme.pretty, archive/Acme.pretty. KiCad would use only one, so leave out the others."
        );
        // With no table naming either, the first is kept.
        layout.footprint_libraries[0].from_table = false;
        assert_eq!(first_choices(&layout).left_out, ["archive/Acme.pretty"]);
    }

    #[test]
    fn a_new_commit_moves_silently_only_while_its_scan_is_the_same() {
        let first = layout("a");
        let choices = first_choices(&first);
        let mut reviewed = Reviewed {
            commit: "a".into(),
            fingerprint: fingerprint(&first),
            profile: profile_of(&first, &choices),
            choices,
            changed: false,
            scanned: None,
        };
        assert_eq!(follow(&mut reviewed, &first), Followed::Same);
        // More 3D models and links, and a doubt about one of them: the
        // same layout as reviewed.
        let mut more_models = LibraryLayout {
            model_files: 99,
            ..layout("b")
        };
        more_models.path_variables[0].links = 40;
        more_models.doubts.push(LayoutDoubt::MissingModel {
            footprint: "lib/Acme.pretty/New.kicad_mod".into(),
            link: "${ACME_3D}/New.step".into(),
        });
        assert_eq!(follow(&mut reviewed, &more_models), Followed::Moved);
        assert_eq!(reviewed.commit, "b");
        // A new library: review again, still at b meanwhile.
        let mut new_library = layout("c");
        new_library.symbol_libraries.push(LayoutLibrary {
            nickname: "New".into(),
            path: "lib/New.kicad_sym".into(),
            from_table: false,
        });
        assert_eq!(follow(&mut reviewed, &new_library), Followed::Changed);
        assert!(reviewed.changed);
        assert_eq!(reviewed.commit, "b");
        // A fetch that brings nothing newer scans nothing.
        assert!(reviewed.has_followed("c"));
        assert!(!reviewed.has_followed("d"));
        // Taken back out: the same as reviewed again.
        assert_eq!(follow(&mut reviewed, &layout("d")), Followed::Moved);
        assert!(!reviewed.changed);
    }

    #[test]
    fn a_reviewed_layout_reads_back_the_same() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::new(folder.path().join("library-layouts"));
        assert_eq!(store.read("repo"), None);
        let first = layout("a");
        let choices = first_choices(&first);
        let reviewed = Reviewed {
            commit: "a".into(),
            fingerprint: fingerprint(&first),
            profile: profile_of(&first, &choices),
            choices,
            changed: true,
            scanned: None,
        };
        store.write("repo", &reviewed).unwrap();
        assert_eq!(store.read("repo"), Some(reviewed));
        // A broken file means: review again.
        fs::write(store.file("repo"), "{not json").unwrap();
        assert_eq!(store.read("repo"), None);
    }
}

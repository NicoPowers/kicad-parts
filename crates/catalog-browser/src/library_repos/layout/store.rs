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

use desktop_contract::{LayoutChoices, LibraryLayout};
use kicad_integration::library_source::{
    LibraryList, SourceLibrary, SourceProfile, SourceVariable,
};
use kicad_integration::sha256_hex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The longest prefix: it is a label, such as a team code.
const MAX_PREFIX: usize = 12;

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

/// The choices a first review starts with: the team's code as the prefix,
/// the scan's folders, and every library in.
pub(crate) fn first_choices(team_code: &str, repo_name: &str) -> LayoutChoices {
    LayoutChoices {
        prefix: default_prefix(team_code, repo_name),
        ..LayoutChoices::default()
    }
}

/// The team's code, or for a team with none yet, the repo name's first
/// two letters or digits in capitals.
fn default_prefix(team_code: &str, repo_name: &str) -> String {
    if !team_code.trim().is_empty() {
        return team_code.trim().to_owned();
    }
    let name = repo_name.rsplit('/').next().unwrap_or(repo_name);
    let letters: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(2)
        .collect::<String>()
        .to_ascii_uppercase();
    if letters.is_empty() {
        "LIB".to_owned()
    } else {
        letters
    }
}

/// `choices` checked against the scan they were made for, with what is
/// wrong in plain words.
pub(crate) fn checked(
    layout: &LibraryLayout,
    mut choices: LayoutChoices,
) -> Result<LayoutChoices, String> {
    choices.prefix = choices.prefix.trim().to_owned();
    if choices.prefix.is_empty() {
        return Err("Give the library a short name, such as the team's code.".to_owned());
    }
    let plain = choices
        .prefix
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if choices.prefix.chars().count() > MAX_PREFIX || !plain {
        return Err(format!(
            "The short name can have up to {MAX_PREFIX} letters, digits, - or _."
        ));
    }
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
        prefix: choices.prefix.clone(),
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

    fn file(&self, name: &str) -> PathBuf {
        self.folder.join(format!("{name}.json"))
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
                prefix: " AC ".into(),
                variable_folders: [("LOST".to_owned(), "lib".to_owned())].into(),
                left_out: vec!["old/Old.pretty".into(), "not/a/library".into()],
            },
        )
        .unwrap();
        assert_eq!(choices.prefix, "AC");
        assert_eq!(choices.left_out, ["old/Old.pretty"]);
        let profile = profile_of(&layout, &choices);
        assert_eq!(profile.prefix, "AC");
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
    fn choices_that_do_not_fit_the_scan_are_refused_plainly() {
        let layout = layout("a");
        let with = |prefix: &str, variable: &str, folder: &str| {
            checked(
                &layout,
                LayoutChoices {
                    prefix: prefix.into(),
                    variable_folders: [(variable.to_owned(), folder.to_owned())].into(),
                    left_out: vec![],
                },
            )
        };
        assert!(
            with("", "ACME_3D", "3d")
                .unwrap_err()
                .contains("short name")
        );
        assert!(with("A C", "ACME_3D", "3d").is_err());
        assert!(with("ABCDEFGHIJKLM", "ACME_3D", "3d").is_err());
        // A folder choice whose variable or folder is gone is dropped.
        assert!(
            with("AC", "NOPE", "3d")
                .unwrap()
                .variable_folders
                .is_empty()
        );
        assert!(
            with("AC", "ACME_3D", "elsewhere")
                .unwrap()
                .variable_folders
                .is_empty()
        );
        assert!(with("AC-2_x", "ACME_3D", "").is_ok());
    }

    #[test]
    fn the_prefix_starts_as_the_team_code() {
        assert_eq!(first_choices("AC", "acme/kicad-lib").prefix, "AC");
        assert_eq!(first_choices("", "acme/kicad-lib").prefix, "KI");
        assert_eq!(first_choices("", "--").prefix, "LIB");
    }

    #[test]
    fn a_new_commit_moves_silently_only_while_its_scan_is_the_same() {
        let first = layout("a");
        let choices = first_choices("AC", "acme/lib");
        let mut reviewed = Reviewed {
            commit: "a".into(),
            fingerprint: fingerprint(&first),
            profile: profile_of(&first, &choices),
            choices,
            changed: false,
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
        let choices = first_choices("AC", "acme/lib");
        let reviewed = Reviewed {
            commit: "a".into(),
            fingerprint: fingerprint(&first),
            profile: profile_of(&first, &choices),
            choices,
            changed: true,
        };
        store.write("repo", &reviewed).unwrap();
        assert_eq!(store.read("repo"), Some(reviewed));
        // A broken file means: review again.
        fs::write(store.folder().join("repo.json"), "{not json").unwrap();
        assert_eq!(store.read("repo"), None);
    }
}

//! The library check in the desktop app (kicad-parts-da65): the local
//! library folder the app reads, compared with the team catalog, for people
//! who can edit. Worked out again only when the team catalog or the folder
//! changed, never per keystroke.

use crate::team::TeamCatalog;
use catalog::library_check::{Group, KnownCad, LibraryCheck, read::check_folder};
use desktop_contract::{LibraryCheckGroup, LibraryCheckItem, LibraryCheckView};
use kicad_integration::library_source::own;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

/// Who was signed in, the team catalog's checkpoint and catch-ups,
/// and the folder's fingerprint, when a library check was worked out.
type Position = (String, u64, u64, Fingerprint);

/// What changes when someone changes the local library folder, cheaply:
/// the git commit checked out, and the number, total size and newest
/// change time of the files in `symbols/`, `footprints/` and `3d-models/`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Fingerprint {
    head: String,
    files: u64,
    bytes: u64,
    newest: u128,
}

/// The last library check and the position it was worked out for.
#[derive(Default)]
pub(crate) struct Cached(Mutex<Option<(Position, Arc<LibraryCheck>)>>);

impl Cached {
    /// The library check for what `team` follows now and the folder as it
    /// is, worked out again only if either changed since the last one;
    /// `None` while signed out or for someone who can only view.
    pub(crate) fn get(
        &self,
        team: &TeamCatalog,
        root: &Path,
        kicad_share: Option<&Path>,
    ) -> Result<Option<LibraryCheckView>, String> {
        Ok(self
            .check(team, root, kicad_share)?
            .map(|check| view_of(&check)))
    }

    /// [`Self::get`], as the check itself, for Add part from our libraries
    /// (kicad-parts-37a7), which needs each finding's kind.
    pub(crate) fn check(
        &self,
        team: &TeamCatalog,
        root: &Path,
        kicad_share: Option<&Path>,
    ) -> Result<Option<Arc<LibraryCheck>>, String> {
        let Some((email, checkpoint, catch_ups)) = team.editor_position() else {
            return Ok(None);
        };
        let position = (email, checkpoint, catch_ups, fingerprint(root));
        if let Some((at, check)) = self.lock().as_ref() {
            if *at == position {
                return Ok(Some(check.clone()));
            }
        }
        let known = KnownCad {
            items: team.current_cad_bytes()?,
        };
        let check = Arc::new(check_folder(root, &known, kicad_share));
        *self.lock() = Some((position, check.clone()));
        Ok(Some(check))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(Position, Arc<LibraryCheck>)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The folder's [`Fingerprint`]: no file is read but `.git/HEAD` and the
/// branch it names; the rest is file sizes and times.
fn fingerprint(root: &Path) -> Fingerprint {
    let mut found = Fingerprint::default();
    let git = root.join(".git");
    if let Ok(head) = fs::read_to_string(git.join("HEAD")) {
        let branch = head
            .trim()
            .strip_prefix("ref: ")
            .and_then(|reference| fs::read_to_string(git.join(reference)).ok())
            .unwrap_or_default();
        found.head = format!("{}\n{}", head.trim(), branch.trim());
    }
    for folder in [own::SYMBOL_FOLDER, own::FOOTPRINT_FOLDER, own::MODEL_FOLDER] {
        add_files(&root.join(folder), &mut found);
    }
    found
}

fn add_files(folder: &Path, found: &mut Fingerprint) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            add_files(&entry.path(), found);
            continue;
        }
        found.files += 1;
        found.bytes += metadata.len();
        let changed = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos());
        found.newest = found.newest.max(changed);
    }
}

/// The check as the app shows it: the counts, and each group that has
/// items, in order, with a sentence saying what it means.
pub(crate) fn view_of(check: &LibraryCheck) -> LibraryCheckView {
    let groups = Group::ALL
        .into_iter()
        .filter(|group| check.count(*group) > 0)
        .map(|group| LibraryCheckGroup {
            title: group.title().to_owned(),
            hint: hint(group).to_owned(),
            items: check
                .in_group(group)
                .map(|finding| {
                    // A symbol or footprint by its library and name; a 3D
                    // model or a file that could not be read by its path.
                    let (library, name) = if finding.item.contains(':') {
                        (finding.library().to_owned(), finding.name().to_owned())
                    } else {
                        (String::new(), finding.item.clone())
                    };
                    LibraryCheckItem {
                        library,
                        name,
                        file: finding.file.clone(),
                        message: finding.message.clone(),
                    }
                })
                .collect(),
        })
        .collect();
    LibraryCheckView {
        // The desktop names the team it asked for.
        team: String::new(),
        to_fix: check.count(Group::ToFix),
        not_in_catalog: check.count(Group::NotInCatalog),
        worth_a_look: check.count(Group::WorthALook),
        clean_up: check.count(Group::CleanUp),
        kicad_footprints_checked: check.kicad_footprints_checked,
        groups,
    }
}

fn hint(group: Group) -> &'static str {
    match group {
        Group::ToFix => {
            "These stop the next import that uses them, and Add part or Edit part with them, until they are fixed in KiCad."
        }
        Group::NotInCatalog => {
            "In our libraries, but no part uses them and the Catalog does not have them. They were probably made in KiCad."
        }
        Group::WorthALook => "Check these; some need fixing before a part uses them.",
        Group::CleanUp => {
            "Old problems of footprints that have not changed since the last import. They stop nothing; fix them when you can."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::library_check::{Finding, FindingKind, Severity};

    #[test]
    fn the_view_has_the_counts_and_only_the_groups_with_items() {
        let check = LibraryCheck {
            findings: vec![
                Finding {
                    severity: Severity::Error,
                    kind: FindingKind::ModelOutside,
                    item: "g-Crystal:Abracon_ABS07L".into(),
                    file: "footprints/g-Crystal.pretty/Abracon_ABS07L.kicad_mod".into(),
                    message: "Outside.".into(),
                },
                Finding {
                    severity: Severity::Warning,
                    kind: FindingKind::NotInCatalog,
                    item: "3d-models/g-Crystal.3dshapes/X.step".into(),
                    file: "3d-models/g-Crystal.3dshapes/X.step".into(),
                    message: "New.".into(),
                },
                Finding {
                    severity: Severity::Warning,
                    kind: FindingKind::Unreadable,
                    item: "symbols/g-ics.kicad_sym".into(),
                    file: "symbols/g-ics.kicad_sym".into(),
                    message: "Could not be read: bad.".into(),
                },
            ],
            kicad_footprints_checked: false,
        };
        let view = view_of(&check);
        assert_eq!(
            (
                view.to_fix,
                view.not_in_catalog,
                view.worth_a_look,
                view.clean_up
            ),
            (1, 1, 1, 0)
        );
        assert!(!view.kicad_footprints_checked);
        let titles: Vec<&str> = view
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect();
        assert_eq!(titles, ["To fix", "Not in the Catalog yet", "Worth a look"]);
        assert_eq!(
            view.groups[0].items[0],
            LibraryCheckItem {
                library: "g-Crystal".into(),
                name: "Abracon_ABS07L".into(),
                file: "footprints/g-Crystal.pretty/Abracon_ABS07L.kicad_mod".into(),
                message: "Outside.".into(),
            }
        );
        // Paths are shown plainly, not as LIBRARY:NAME.
        let path = |group: usize| {
            let item = &view.groups[group].items[0];
            (item.library.clone(), item.name.clone())
        };
        assert_eq!(
            path(1),
            (String::new(), "3d-models/g-Crystal.3dshapes/X.step".into())
        );
        assert_eq!(path(2), (String::new(), "symbols/g-ics.kicad_sym".into()));
        assert!(view_of(&LibraryCheck::default()).is_empty());
    }

    #[test]
    fn the_fingerprint_changes_with_any_file_of_the_folder() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("footprints/g-a.pretty")).unwrap();
        let file = root.path().join("footprints/g-a.pretty/F.kicad_mod");
        fs::write(&file, "(footprint \"F\")").unwrap();
        let before = fingerprint(root.path());
        assert_eq!(fingerprint(root.path()), before);
        fs::write(&file, "(footprint \"F\" (model \"x\"))").unwrap();
        assert_ne!(fingerprint(root.path()), before);
    }
}

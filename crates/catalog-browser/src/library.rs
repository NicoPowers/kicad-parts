//! The library check in the desktop app (kicad-parts-da65): the CSV catalog
//! folder the app reads, compared with the team catalog, for people who can
//! edit. Worked out once per change of the team catalog, never per
//! keystroke.

use crate::team::TeamCatalog;
use catalog::library_check::{Group, KnownCad, LibraryCheck, read::check_folder};
use desktop_contract::{LibraryCheckGroup, LibraryCheckItem, LibraryCheckView};
use std::path::Path;
use std::sync::Mutex;

/// Who was signed in, and the team catalog's checkpoint and finished
/// imports, when a library check was worked out.
type Position = (String, u64, u64);

/// The last library check and the position it was worked out for.
#[derive(Default)]
pub(crate) struct Cached(Mutex<Option<(Position, LibraryCheckView)>>);

impl Cached {
    /// The library check for what `team` follows now, worked out again only
    /// if the team catalog changed since the last one; `None` while signed
    /// out or for someone who can only view.
    pub(crate) fn get(
        &self,
        team: &TeamCatalog,
        root: &Path,
        kicad_share: Option<&Path>,
    ) -> Result<Option<LibraryCheckView>, String> {
        let Some(position) = team.editor_position() else {
            return Ok(None);
        };
        if let Some((at, view)) = self.lock().as_ref() {
            if *at == position {
                return Ok(Some(view.clone()));
            }
        }
        let state = team.import_state()?;
        let check = check_folder(root, &KnownCad::from_state(&state), kicad_share)?;
        let view = view_of(&check);
        *self.lock() = Some((position, view.clone()));
        Ok(Some(view))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(Position, LibraryCheckView)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
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
                .map(|finding| LibraryCheckItem {
                    library: finding.library().to_owned(),
                    name: finding.name().to_owned(),
                    file: finding.file.clone(),
                    message: finding.message.clone(),
                })
                .collect(),
        })
        .collect();
    LibraryCheckView {
        to_fix: check.count(Group::ToFix),
        not_in_catalog: check.count(Group::NotInCatalog),
        worth_a_look: check.count(Group::WorthALook),
        clean_up: check.count(Group::CleanUp),
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
        Group::WorthALook => "Probably fine, but check them.",
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
            ],
        };
        let view = view_of(&check);
        assert_eq!(
            (
                view.to_fix,
                view.not_in_catalog,
                view.worth_a_look,
                view.clean_up
            ),
            (1, 1, 0, 0)
        );
        let titles: Vec<&str> = view
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect();
        assert_eq!(titles, ["To fix", "Not in the Catalog yet"]);
        assert_eq!(
            view.groups[0].items[0],
            LibraryCheckItem {
                library: "g-Crystal".into(),
                name: "Abracon_ABS07L".into(),
                file: "footprints/g-Crystal.pretty/Abracon_ABS07L.kicad_mod".into(),
                message: "Outside.".into(),
            }
        );
        assert_eq!(
            (
                view.groups[1].items[0].library.as_str(),
                view.groups[1].items[0].name.as_str()
            ),
            ("g-Crystal.3dshapes", "X.step")
        );
        assert!(view_of(&LibraryCheck::default()).is_empty());
    }
}

//! What kind of action published a Change Set.

/// The closed set of `catalog_change_sets.kind` values. The migration
/// declares them as a select field and the import route's
/// `CHANGE_SET_KINDS` names them; the test below keeps all three in step.
///
/// Only [`ChangeSetKind::LegacyImport`] is published today. The import route
/// gives a changed row a successor revision only when its head came from a
/// legacy import, so an edit can never be mistaken for one.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ChangeSetKind {
    LegacyImport,
    /// Reserved for kicad-parts-2254.
    Edit,
    /// Reserved for kicad-parts-2254.
    Restore,
}

impl ChangeSetKind {
    pub const ALL: [Self; 3] = [Self::LegacyImport, Self::Edit, Self::Restore];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LegacyImport => "legacy-import",
            Self::Edit => "edit",
            Self::Restore => "restore",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_import::LEGACY_SOURCE;
    use std::path::PathBuf;

    fn infra(path: &str) -> String {
        std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../infra/pocketbase")
                .join(path),
        )
        .unwrap()
    }

    #[test]
    fn kinds_match_the_migration_and_the_route() {
        let values: Vec<String> = ChangeSetKind::ALL
            .iter()
            .map(|kind| format!("\"{}\"", kind.as_str()))
            .collect();
        let migration = infra("pb_migrations/1791500000_catalog_init.js");
        assert!(
            migration.contains(&format!(
                "{{ name: \"kind\", type: \"select\", required: true, maxSelect: 1, values: [{}] }}",
                values.join(", ")
            )),
            "the migration's kind values differ from ChangeSetKind"
        );
        let library = infra("pb_hooks/catalog_lib.js");
        for (name, kind) in [
            ("legacyImport", ChangeSetKind::LegacyImport),
            ("edit", ChangeSetKind::Edit),
            ("restore", ChangeSetKind::Restore),
        ] {
            assert!(
                library.contains(&format!("  {name}: \"{}\",", kind.as_str())),
                "catalog_lib.js lacks {name}"
            );
        }
        assert!(library.contains(&format!("const LEGACY_SOURCE = \"{LEGACY_SOURCE}\";")));
        assert_eq!(ChangeSetKind::parse("edit"), Some(ChangeSetKind::Edit));
        assert_eq!(ChangeSetKind::parse("Edit"), None);
    }
}

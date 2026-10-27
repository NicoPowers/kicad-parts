//! What kind of action made a Change.

/// The closed set of `catalog_changes.kind` values. The migration
/// declares them as a select field and the team server's `CHANGE_KINDS`
/// names them; the test below keeps all three in step.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ChangeKind {
    /// A change to one part's details, symbol or footprint, published by
    /// `POST /api/catalog/edits`.
    Edit,
    /// One part revision archived, published by `POST /api/catalog/archives`.
    Archive,
    /// One part revision unarchived, published by `POST /api/catalog/archives`.
    Unarchive,
    /// A new part added in the app, published by `POST /api/catalog/parts`.
    Create,
    /// New categories: the starting categories a new team server sets up,
    /// or one a superuser set up with `POST /api/catalog/categories`.
    Category,
}

impl ChangeKind {
    pub const ALL: [Self; 5] = [
        Self::Edit,
        Self::Archive,
        Self::Unarchive,
        Self::Create,
        Self::Category,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Archive => "archive",
            Self::Unarchive => "unarchive",
            Self::Create => "create",
            Self::Category => "category",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let values: Vec<String> = ChangeKind::ALL
            .iter()
            .map(|kind| format!("\"{}\"", kind.as_str()))
            .collect();
        // The latest migration that sets the kinds.
        let migration = infra("pb_migrations/1792600000_category_changes.js");
        assert!(
            migration.contains(&format!("const KINDS = [{}];", values.join(", "))),
            "the migration's kind values differ from ChangeKind"
        );
        let library = infra("pb_hooks/catalog_lib.js");
        for (name, kind) in [
            ("edit", ChangeKind::Edit),
            ("archive", ChangeKind::Archive),
            ("unarchive", ChangeKind::Unarchive),
            ("create", ChangeKind::Create),
            ("category", ChangeKind::Category),
        ] {
            assert!(
                library.contains(&format!("  {name}: \"{}\",", kind.as_str())),
                "catalog_lib.js lacks {name}"
            );
        }
        // The edit route writes part details, revisions and sources with
        // these tags.
        assert!(library.contains(&format!(
            "const PART_DETAILS_SCHEMA = \"{}\";",
            crate::PART_DETAILS_SCHEMA
        )));
        assert!(library.contains(&format!(
            "const PART_REVISION_SCHEMA = \"{}\";",
            crate::PART_REVISION_SCHEMA
        )));
        assert!(library.contains(&format!(
            "const CATEGORY_SCHEMA = \"{}\";",
            crate::CATEGORY_SCHEMA
        )));
        assert!(library.contains(&format!(
            "const CAD_ITEM_DETAILS_SCHEMA = \"{}\";",
            crate::cad_item::CAD_ITEM_DETAILS_SCHEMA
        )));
        // A remembered match is made only by an edit or a new part
        // (catalog_write_guards).
        assert!(library.contains("\"catalog_category_matches\""));
        assert!(!library.contains("restore"));
        assert!(library.contains(&format!(
            "const SOURCE_SCHEMA = \"{}\";",
            crate::SOURCE_SCHEMA
        )));
        assert_eq!(ChangeKind::parse("edit"), Some(ChangeKind::Edit));
        assert_eq!(ChangeKind::parse("Edit"), None);
        assert_eq!(ChangeKind::parse("restore"), None);
    }
}

//! What kind of action made a Change.

/// The closed set of `catalog_changes.kind` values. The migration
/// declares them as a select field and the import route's
/// `CHANGE_KINDS` names them; the test below keeps all three in step.
///
/// The import route saves a changed row's part details only when the
/// part's current details came from a CSV import, and its changed CAD only
/// when its current revision did, so an edit is never overwritten by one.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ChangeKind {
    /// Published by `POST /api/catalog/csv-imports`.
    CsvImport,
    /// A change to one part's details, symbol or footprint, published by
    /// `POST /api/catalog/edits`.
    Edit,
    /// One part revision archived, published by `POST /api/catalog/archives`.
    Archive,
    /// One part revision unarchived, published by `POST /api/catalog/archives`.
    Unarchive,
}

impl ChangeKind {
    pub const ALL: [Self; 4] = [Self::CsvImport, Self::Edit, Self::Archive, Self::Unarchive];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CsvImport => "csv-import",
            Self::Edit => "edit",
            Self::Archive => "archive",
            Self::Unarchive => "unarchive",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv_import::CSV_CATALOG_SOURCE;
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
        let migration = infra("pb_migrations/1791900000_archived_revisions.js");
        assert!(
            migration.contains(&format!("const KINDS = [{}];", values.join(", "))),
            "the migration's kind values differ from ChangeKind"
        );
        let library = infra("pb_hooks/catalog_lib.js");
        for (name, kind) in [
            ("csvImport", ChangeKind::CsvImport),
            ("edit", ChangeKind::Edit),
            ("archive", ChangeKind::Archive),
            ("unarchive", ChangeKind::Unarchive),
        ] {
            assert!(
                library.contains(&format!("  {name}: \"{}\",", kind.as_str())),
                "catalog_lib.js lacks {name}"
            );
        }
        assert!(library.contains(&format!(
            "const CSV_CATALOG_SOURCE = \"{CSV_CATALOG_SOURCE}\";"
        )));
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
        assert!(!library.contains("restore"));
        assert!(library.contains(&format!(
            "const SOURCE_SCHEMA = \"{}\";",
            crate::csv_import::SOURCE_SCHEMA
        )));
        assert_eq!(ChangeKind::parse("edit"), Some(ChangeKind::Edit));
        assert_eq!(ChangeKind::parse("Edit"), None);
        assert_eq!(ChangeKind::parse("restore"), None);
    }
}

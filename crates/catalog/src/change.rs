//! What kind of action made a Change.

/// The closed set of `catalog_changes.kind` values. The migration
/// declares them as a select field and the import route's
/// `CHANGE_KINDS` names them; the test below keeps all three in step.
///
/// The import route gives a changed row a new revision only when the
/// part's current revision came from a CSV import, so an edit is never
/// overwritten by one.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ChangeKind {
    /// Published by `POST /api/catalog/csv-imports`.
    CsvImport,
    /// A change to one part's details, published by
    /// `POST /api/catalog/edits`.
    Edit,
    /// A new revision that copied an earlier one's details. Nothing
    /// publishes these any more (the restore route was removed in
    /// kicad-parts-9296); the kind stays so that Changes saved before then
    /// can still be read, until kicad-parts-7556 replaces the kinds.
    Restore,
}

impl ChangeKind {
    pub const ALL: [Self; 3] = [Self::CsvImport, Self::Edit, Self::Restore];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CsvImport => "csv-import",
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
        let migration = infra("pb_migrations/1791500000_catalog_init.js");
        assert!(
            migration.contains(&format!(
                "{{ name: \"kind\", type: \"select\", required: true, maxSelect: 1, values: [{}] }}",
                values.join(", ")
            )),
            "the migration's kind values differ from ChangeKind"
        );
        let library = infra("pb_hooks/catalog_lib.js");
        for (name, kind) in [
            ("csvImport", ChangeKind::CsvImport),
            ("edit", ChangeKind::Edit),
            ("restore", ChangeKind::Restore),
        ] {
            assert!(
                library.contains(&format!("  {name}: \"{}\",", kind.as_str())),
                "catalog_lib.js lacks {name}"
            );
        }
        assert!(library.contains(&format!(
            "const CSV_CATALOG_SOURCE = \"{CSV_CATALOG_SOURCE}\";"
        )));
        // The edit route writes part details and sources with these tags.
        assert!(library.contains(&format!(
            "const PART_DETAILS_SCHEMA = \"{}\";",
            crate::PART_DETAILS_SCHEMA
        )));
        assert!(library.contains(&format!(
            "const SOURCE_SCHEMA = \"{}\";",
            crate::csv_import::SOURCE_SCHEMA
        )));
        assert_eq!(ChangeKind::parse("edit"), Some(ChangeKind::Edit));
        assert_eq!(ChangeKind::parse("Edit"), None);
    }
}

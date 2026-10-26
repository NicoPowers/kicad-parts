//! CSV imports after edits that used files (seed kicad-parts-3b8b): an
//! import never replaces a CAD item revision made in the app, and a CSV
//! that caught up with the app is no change. Split from
//! `pocketbase_uploads.rs` (kicad-parts-fbc2): these test the CSV import,
//! which kicad-parts-e257 removes with the CSV catalog.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 and imports a
//! temp copy of the resistor CSV file; the repository is only read.

use catalog::cad_file::{footprint_upload, symbol_upload};
use catalog::pocketbase::{
    CatalogReadback, EditCommand, ImportCommand, ImportOutcome, PublishOutcome, PublishReply,
    read_back, read_import_state, submit_edit, submit_import,
};
use catalog::{CadReferenceTarget, CsvCatalog, PartRevision};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use std::fs;

const IPN: &str = "RES-0000-0470";
const OTHER: &str = "RES-0000-1000";
const FOOTPRINT: &str = "g-Resistor_SMD:R_0603_1608Metric";
const FOOTPRINT_FILE: &str = "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod";

/// Copies the CSV catalog trees the import and the zero-loss oracle read
/// into `target`.
fn copy_catalog(target: &std::path::Path) {
    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let destination = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        copy_tree(&services::repository_root().join(tree), &target.join(tree));
    }
}

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    /// A copy of the CSV catalog, which the tests change.
    copy: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Team {
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = tempfile::tempdir().unwrap();
        copy_catalog(copy.path());
        let team = Self {
            server,
            admin,
            copy,
            _state: state,
        };
        assert_eq!(team.import(), "applied");
        team
    }

    fn member(&self, name: &str, can_edit: bool) -> PocketBaseClient {
        let email = format!("{}@example.test", name.to_lowercase().replace(' ', "."));
        let member = services::add_member(&self.admin, name, &email, can_edit).unwrap();
        PocketBaseClient::new(self.server.url())
            .authenticate("users", &member.email, &member.password)
            .unwrap()
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }

    /// Imports the copy as an importer does (reading the team catalog
    /// first) and returns the outcome; the zero-loss report must hold.
    fn import(&self) -> &'static str {
        let import = CsvCatalog::new(self.copy.path())
            .import("g-res", None)
            .unwrap();
        let state = read_import_state(&self.admin).unwrap();
        let command = ImportCommand::new(&import, "Import the resistors").expecting(&state);
        let reply = submit_import(&self.admin, &command).unwrap();
        let outcome = match reply.outcome {
            ImportOutcome::Applied { .. } => "applied",
            ImportOutcome::NothingChanged { .. } => "nothing_changed",
            ImportOutcome::Noop { .. } => "noop",
            _ => panic!("the import did not go through: {}", reply.body),
        };
        let oracle = csv_catalog_fixture::build_manifest(self.copy.path()).unwrap();
        let report = catalog_import::report(&oracle, "g-res", &self.read(), self.copy.path());
        assert!(
            report.is_lossless(),
            "{}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
        outcome
    }

    fn file(&self, path: &str) -> Vec<u8> {
        fs::read(self.copy.path().join(path)).unwrap()
    }
}

fn current_revision(stored: &CatalogReadback, ipn: &str) -> (u32, PartRevision) {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

/// A CAD edit of `ipn` as it is now, setting `values` and sending `uploads`.
fn cad_edit(
    stored: &CatalogReadback,
    ipn: &str,
    values: &[(&str, &str)],
    uploads: Vec<catalog::pocketbase::CadUpload>,
) -> EditCommand {
    let (number, revision) = current_revision(stored, ipn);
    let mut command =
        EditCommand::new(ipn, "Use the corrected file").editing_cad(&revision, number);
    for (field, value) in values {
        assert!(command.set_field(field, value), "{field}");
    }
    command.cad.as_mut().unwrap().uploads = uploads;
    command
}

/// A new CAD item revision an edit reported: kind, key, number, and how
/// many other parts use an older revision.
type NewRevision = (String, String, u32, Option<u32>);

/// The revision number an applied edit added and the new CAD item revisions
/// it reported.
fn applied(reply: &PublishReply) -> (Option<u32>, Vec<NewRevision>) {
    match &reply.outcome {
        PublishOutcome::Applied {
            revision,
            new_cad_item_revisions,
            ..
        } => (
            revision.as_ref().map(|revision| revision.number),
            new_cad_item_revisions
                .iter()
                .map(|new| {
                    (
                        new.kind.clone(),
                        new.key.clone(),
                        new.number,
                        new.other_parts,
                    )
                })
                .collect(),
        ),
        other => panic!("expected applied, got {other:?}: {}", reply.body),
    }
}

/// The CAD item revision a part's current revision binds for `reference`.
fn bound(stored: &CatalogReadback, ipn: &str, reference: &str) -> CadReferenceTarget {
    let (_, revision) = current_revision(stored, ipn);
    revision
        .cad_references
        .iter()
        .find(|item| item.reference == reference)
        .map(|item| item.target.clone())
        .unwrap_or_else(|| panic!("{ipn} does not use {reference}"))
}

fn number_of(target: &CadReferenceTarget) -> u32 {
    match target {
        CadReferenceTarget::CadItemRevision { number, .. } => *number,
        other => panic!("not bound to a CAD item revision: {other:?}"),
    }
}

/// The parts whose current revision uses `reference`.
fn users(stored: &CatalogReadback, reference: &str) -> Vec<String> {
    stored
        .parts
        .iter()
        .filter(|part| {
            current_revision(stored, &part.ipn)
                .1
                .cad_references
                .iter()
                .any(|item| item.reference == reference)
        })
        .map(|part| part.ipn.clone())
        .collect()
}

/// The footprint file with a circle added to its silkscreen.
fn redrawn(footprint: &[u8]) -> Vec<u8> {
    let text = String::from_utf8(footprint.to_vec()).unwrap();
    let changed = text.replacen(
        "\t(fp_line",
        "\t(fp_circle (center 0 0) (end 2 0) (stroke (width 0.3) (type solid)) (fill no) (layer \"F.SilkS\"))\n\t(fp_line",
        1,
    );
    assert_ne!(changed, text);
    changed.into_bytes()
}

/// The import problems with `code` the latest import run found, by subject.
fn found(stored: &CatalogReadback, code: &str) -> Vec<catalog::pocketbase::StoredImportProblem> {
    let run = stored
        .audit_entries
        .iter()
        .filter(|entry| entry.csv_import_number > 0)
        .max_by_key(|entry| entry.csv_import_number)
        .and_then(|entry| entry.import_run())
        .unwrap();
    stored
        .problems
        .iter()
        .filter(|problem| problem.code == code && run.problems.contains(&problem.id))
        .cloned()
        .collect()
}

/// The newest revision number of `reference`'s CAD item.
fn newest(stored: &CatalogReadback, kind: catalog::CadItemKind, reference: &str) -> u32 {
    let cad_item = catalog::CadItemId::new(kind, reference);
    stored
        .cad_item_revisions
        .iter()
        .filter(|revision| revision.cad_item == cad_item.as_str())
        .map(|revision| revision.number)
        .max()
        .unwrap_or(0)
}

#[test]
fn imports_never_replace_a_cad_item_revision_made_in_the_app() {
    use catalog::pocketbase::{
        CSV_CAD_DIFFERS, CSV_CAD_ITEM_KEPT, CsvReviewCommand, CsvReviewOutcome,
        recorded_csv_document, submit_csv_review,
    };
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let footprint_users = users(&before, FOOTPRINT);
    let original = team.file(FOOTPRINT_FILE);
    let upload = footprint_upload(&redrawn(&original), FOOTPRINT, "R_0603.kicad_mod").unwrap();
    applied(&submit_edit(&editor, &cad_edit(&before, IPN, &[], vec![upload])).unwrap());
    let edited = team.read();
    let footprint = catalog::CadItemKind::Footprint;
    assert_eq!(newest(&edited, footprint, FOOTPRINT), 2);

    // The CSV's file has not changed since the import last applied it, so
    // nothing happens: no new footprint revision, every part as it was.
    assert_eq!(team.import(), "nothing_changed");
    let after = team.read();
    assert_eq!(newest(&after, footprint, FOOTPRINT), 2);
    assert_eq!(
        after.cad_item_revisions.len(),
        edited.cad_item_revisions.len()
    );
    assert_eq!(after.part_revisions.len(), edited.part_revisions.len());
    assert_eq!(number_of(&bound(&after, IPN, FOOTPRINT)), 2);
    for ipn in footprint_users.iter().filter(|ipn| *ipn != IPN) {
        assert_eq!(number_of(&bound(&after, ipn, FOOTPRINT)), 1, "{ipn}");
    }
    assert!(found(&after, CSV_CAD_ITEM_KEPT).is_empty());
    // The edited part's row is kept, as for any edited CAD; what it records
    // is exactly what the last import saved, so the app asks nothing.
    let kept = found(&after, CSV_CAD_DIFFERS);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].subject, IPN);
    let first = after
        .part_revisions
        .iter()
        .find(|revision| revision.part == after.part(IPN).unwrap().id && revision.number == 1)
        .unwrap();
    assert_eq!(recorded_csv_document(&kept[0]).unwrap(), first.snapshot);
    assert_eq!(team.import(), "noop");

    // An importer that does not know the footprint was changed in the app
    // would send a new revision of it: the server refuses.
    let import = CsvCatalog::new(team.copy.path())
        .import("g-res", None)
        .unwrap();
    let mut state = read_import_state(&team.admin).unwrap();
    assert!(!state.current_cad.edited_in_app.is_empty());
    state.current_cad.edited_in_app.clear();
    let command = ImportCommand::new(&import, "Import the resistors").expecting(&state);
    match submit_import(&team.admin, &command) {
        Err(CollabError::Http {
            status: 400,
            message,
        }) => {
            assert!(message.contains("never replaces"), "{message}")
        }
        other => panic!("expected 400, got {other:?}"),
    }
    assert_eq!(team.read().counts(), after.counts());

    // The CSV's file changes: still no new footprint revision, but every
    // part whose row uses it keeps the row with a problem the app shows.
    let changed = String::from_utf8(original).unwrap().replacen(
        "(tags \"resistor\")",
        "(tags \"resistor 0603\")",
        1,
    );
    fs::write(team.copy.path().join(FOOTPRINT_FILE), changed).unwrap();
    assert_eq!(team.import(), "nothing_changed");
    let changed = team.read();
    assert_eq!(newest(&changed, footprint, FOOTPRINT), 2);
    assert_eq!(changed.part_revisions.len(), edited.part_revisions.len());
    let problems = found(&changed, CSV_CAD_ITEM_KEPT);
    let mut subjects: Vec<String> = problems.iter().map(|p| p.subject.clone()).collect();
    subjects.sort();
    let mut expected = footprint_users.clone();
    expected.sort();
    assert_eq!(subjects, expected);
    let problem = problems.iter().find(|p| p.subject == OTHER).unwrap();
    assert_eq!(problem.location, format!("rows[IPN={OTHER}].Footprint"));

    // The CSV's file is not stored, so it can only be kept.
    let (number, _) = current_revision(&changed, OTHER);
    let take = CsvReviewCommand::new(
        OTHER,
        &problem.id,
        vec!["Footprint".into()],
        number,
        "Take it",
    );
    match submit_csv_review(&editor, &take) {
        Err(CollabError::Http {
            status: 400,
            message,
        }) => {
            assert!(
                message.contains("choose the file in Edit part"),
                "{message}"
            )
        }
        other => panic!("expected 400, got {other:?}"),
    }
    let keep = CsvReviewCommand::new(OTHER, &problem.id, vec![], number, "Keep ours");
    let reply = submit_csv_review(&editor, &keep).unwrap();
    assert!(
        matches!(
            reply.outcome,
            CsvReviewOutcome::Applied { revision: None, .. }
        ),
        "{}",
        reply.body
    );
    // Importing the same CSV again finds the same problem, which the review
    // covers.
    assert_eq!(team.import(), "noop");
}

#[test]
fn a_csv_that_caught_up_with_the_app_is_not_a_change() {
    use catalog::pocketbase::CSV_CAD_ITEM_KEPT;
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    // The CSV catalog's symbol library with a symbol the app adds first.
    let library_path = team.copy.path().join("symbols/g-res.kicad_sym");
    let library = fs::read_to_string(&library_path).unwrap();
    let entry = "\t(symbol \"R_Added\"\n\t\t(in_bom yes)\n\t\t(property \"Reference\" \"R\" (at 0 0 0) (effects (font (size 1.27 1.27))))\n\t)\n";
    let end = library.trim_end().len() - 1;
    let caught_up = format!("{}{entry}{}", &library[..end], &library[end..]);
    let symbol = symbol_upload(
        caught_up.as_bytes(),
        "R_Added",
        "g-res:R_Added",
        "added.kicad_sym",
    )
    .unwrap();
    let before = team.read();
    applied(
        &submit_edit(
            &editor,
            &cad_edit(&before, IPN, &[("Symbol", "g-res:R_Added")], vec![symbol]),
        )
        .unwrap(),
    );
    // And a corrected footprint for the same part.
    let corrected = redrawn(&team.file(FOOTPRINT_FILE));
    let edited = team.read();
    let footprint = footprint_upload(&corrected, FOOTPRINT, "R.kicad_mod").unwrap();
    applied(&submit_edit(&editor, &cad_edit(&edited, IPN, &[], vec![footprint])).unwrap());

    // The CSV catalog catches up with both, byte for byte, and another row
    // uses the new symbol.
    fs::write(&library_path, caught_up).unwrap();
    fs::write(team.copy.path().join(FOOTPRINT_FILE), &corrected).unwrap();
    let csv = team.copy.path().join("database/g-res.csv");
    let text = fs::read_to_string(&csv).unwrap();
    let row = text
        .split_inclusive('\n')
        .find(|line| line.starts_with(&format!("\"{OTHER}\"")))
        .unwrap()
        .to_owned();
    fs::write(
        &csv,
        text.replacen(
            &row,
            &row.replacen("\"g-res:R_US\"", "\"g-res:R_Added\"", 1),
            1,
        ),
    )
    .unwrap();
    let made = team.read();
    team.import();
    let after = team.read();
    // Nothing to show, and no CAD item gets a revision.
    assert!(
        found(&after, CSV_CAD_ITEM_KEPT).is_empty(),
        "{:?}",
        found(&after, CSV_CAD_ITEM_KEPT)
    );
    assert_eq!(
        after.cad_item_revisions.len(),
        made.cad_item_revisions.len()
    );
    // The other row's part follows the CSV to the revisions the app made.
    assert_eq!(number_of(&bound(&after, OTHER, "g-res:R_Added")), 1);
    assert_eq!(number_of(&bound(&after, OTHER, FOOTPRINT)), 2);
}

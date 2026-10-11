//! Excluded from BOM follows the CAD of each new part revision
//! (kicad-parts-ccc0): the server works it out from the stored bytes of the
//! CAD item revisions a new revision binds, for imports, edits and CSV
//! reviews alike, and an existing revision never changes. Each test starts
//! its own disposable PocketBase on 127.0.0.1 and imports temp copies of the
//! CSV catalog; the repository is only read.

use catalog::pocketbase::{
    CSV_CAD_DIFFERS, CatalogReadback, CsvReviewCommand, EditCommand, ImportCommand, ImportOutcome,
    StoredImportProblem, read_back, read_import_state, submit_csv_review, submit_edit,
    submit_import,
};
use catalog::{CsvCatalog, CsvImport, PartRevision, sha256_hex};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use std::fs;
use std::path::Path;

/// A resistor the tests edit.
const EDITED: &str = "RES-0000-0470";
/// A resistor only the CSV changes.
const IMPORTED: &str = "RES-0000-1000";
/// The test point, whose symbol g-art:TestPoint says (in_bom no).
const TEST_POINT: &str = "ART-0000-0001";
/// A footprint whose (attr ...) has exclude_from_bom.
const NO_BOM_FOOTPRINT: &str = "g-no_bom:TestPoint_Pad_D1.0mm";

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    copy: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Team {
    /// A team server with the test points and the resistors imported.
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = tempfile::tempdir().unwrap();
        for tree in ["database", "symbols", "footprints", "3d-models"] {
            copy_tree(&root.join(tree), &copy.path().join(tree));
        }
        let team = Self {
            server,
            admin,
            copy,
            _state: state,
        };
        assert_eq!(team.import("g-art"), "applied");
        assert_eq!(team.import("g-res"), "applied");
        team
    }

    fn plan(&self, csv_file: &str) -> CsvImport {
        CsvCatalog::new(self.copy.path())
            .import(csv_file, None)
            .unwrap()
    }

    /// Imports `csv_file` from the copy as an importer does; the zero-loss
    /// report must hold.
    fn import(&self, csv_file: &str) -> &'static str {
        let import = self.plan(csv_file);
        let state = read_import_state(&self.admin).unwrap();
        let command = ImportCommand::new(&import, "Import").expecting(&state);
        let reply = submit_import(&self.admin, &command).unwrap();
        let outcome = match reply.outcome {
            ImportOutcome::Applied { .. } => "applied",
            ImportOutcome::NothingChanged { .. } => "nothing_changed",
            ImportOutcome::Noop { .. } => "noop",
            _ => panic!("the import did not go through: {}", reply.body),
        };
        let oracle = csv_catalog_fixture::build_manifest(self.copy.path()).unwrap();
        let report = catalog_import::report(&oracle, csv_file, &self.read(), self.copy.path());
        assert!(
            report.is_lossless(),
            "{}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
        outcome
    }

    /// Replaces exactly one `from` in the row of `ipn` in the copy's
    /// g-res.csv.
    fn change_row(&self, ipn: &str, from: &str, to: &str) {
        let file = self.copy.path().join("database/g-res.csv");
        let text = fs::read_to_string(&file).unwrap();
        let row = text
            .split_inclusive('\n')
            .find(|line| line.starts_with(&format!("\"{ipn}\"")))
            .unwrap()
            .to_owned();
        assert_eq!(row.matches(from).count(), 1, "{from:?} in {row}");
        fs::write(&file, text.replacen(&row, &row.replacen(from, to, 1), 1)).unwrap();
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }
}

fn copy_tree(from: &Path, to: &Path) {
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

/// The part's current revision: its number and document.
fn current(stored: &CatalogReadback, ipn: &str) -> (u32, PartRevision) {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

/// The stored snapshot of revision `number` of `ipn`.
fn snapshot(stored: &CatalogReadback, ipn: &str, number: u32) -> String {
    let part = stored.part(ipn).unwrap();
    stored
        .part_revisions
        .iter()
        .find(|revision| revision.part == part.id && revision.number == number)
        .unwrap()
        .snapshot
        .clone()
}

/// Sets the Symbol or Footprint of `ipn` with a CAD edit and returns the
/// new current revision.
fn edit_cad(team: &Team, ipn: &str, field: &str, value: &str) -> (u32, PartRevision) {
    let stored = team.read();
    let (number, revision) = current(&stored, ipn);
    let mut command = EditCommand::new(ipn, &format!("Use {value}")).editing_cad(&revision, number);
    assert!(command.set_field(field, value));
    let reply = submit_edit(&team.admin, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    current(&team.read(), ipn)
}

#[test]
fn an_edit_recalculates_excluded_from_bom_from_the_new_cad() {
    let team = Team::start();
    let stored = team.read();
    // The import set it for the test point, from its symbol's (in_bom no).
    assert!(current(&stored, TEST_POINT).1.excluded_from_bom);
    assert!(!current(&stored, EDITED).1.excluded_from_bom);
    let first = snapshot(&stored, EDITED, 1);

    // Switching to the test point symbol sets it; revision 1 is unchanged.
    let (number, revision) = edit_cad(&team, EDITED, "Symbol", "g-art:TestPoint");
    assert_eq!(number, 2);
    assert!(revision.excluded_from_bom);
    assert!(snapshot(&team.read(), EDITED, 2).ends_with(r#","excluded_from_bom":true}"#));
    assert_eq!(
        snapshot(&team.read(), EDITED, 1),
        first,
        "revision 1 never changes"
    );

    // Switching back clears it; revision 2 keeps it.
    let (number, revision) = edit_cad(&team, EDITED, "Symbol", "g-pas:R_US");
    assert_eq!(number, 3);
    assert!(!revision.excluded_from_bom);
    let stored = team.read();
    assert!(!snapshot(&stored, EDITED, 3).contains("excluded_from_bom"));
    let second: PartRevision = serde_json::from_str(&snapshot(&stored, EDITED, 2)).unwrap();
    assert!(second.excluded_from_bom, "revision 2 never changes");

    // A symbol from KiCad's own library is not read, so it gives no
    // evidence either way.
    let (_, revision) = edit_cad(&team, EDITED, "Symbol", "Device:R");
    assert!(!revision.excluded_from_bom);

    // Footprint choices: the import keeps disagreeing ones in an import
    // problem and leaves the setting unset...
    team.change_row(
        IMPORTED,
        "\"g-res:R_0603_1608Metric\"",
        &format!("\"g-res:R_0603_1608Metric;{NO_BOM_FOOTPRINT}\""),
    );
    assert_eq!(team.import("g-res"), "applied");
    let stored = team.read();
    assert!(!current(&stored, IMPORTED).1.excluded_from_bom);
    assert!(stored.problems.iter().any(|problem| {
        problem.code == "footprint-choices-disagree-on-bom" && problem.subject == IMPORTED
    }));
    // ...an edit to only the marked footprint sets it, even with KiCad's
    // own symbol...
    let (_, revision) = edit_cad(&team, EDITED, "Footprint", NO_BOM_FOOTPRINT);
    assert!(revision.excluded_from_bom);
    // ...and an edit to disagreeing choices leaves it unset.
    let (_, revision) = edit_cad(
        &team,
        EDITED,
        "Footprint",
        &format!("g-res:R_0603_1608Metric;{NO_BOM_FOOTPRINT}"),
    );
    assert!(!revision.excluded_from_bom);

    // The zero-loss report checks every revision, including those edits
    // added: a revision 2 that lost the setting is a loss.
    let mut tampered = team.read();
    let part = tampered.part(EDITED).unwrap().id.clone();
    let second = tampered
        .part_revisions
        .iter_mut()
        .find(|revision| revision.part == part && revision.number == 2)
        .unwrap();
    second.snapshot = second.snapshot.replace(r#","excluded_from_bom":true"#, "");
    let oracle = csv_catalog_fixture::build_manifest(team.copy.path()).unwrap();
    let report = catalog_import::report(&oracle, "g-res", &tampered, team.copy.path());
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem
                .contains(&format!("{EDITED} revision 2: Excluded from BOM is false"))),
        "{:?}",
        report.problems
    );
    team.server.stop();
}

/// The CSV CAD problem about `ipn` that the latest import run found.
fn cad_problem(stored: &CatalogReadback, ipn: &str) -> StoredImportProblem {
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
        .find(|problem| {
            problem.code == CSV_CAD_DIFFERS
                && problem.subject == ipn
                && run.problems.contains(&problem.id)
        })
        .cloned()
        .unwrap()
}

#[test]
fn imports_and_csv_reviews_recalculate_it_the_same_way() {
    let team = Team::start();
    // The edited resistor's CAD is now an app edit (revision 2).
    let (number, _) = edit_cad(&team, EDITED, "Symbol", "Device:R");
    assert_eq!(number, 2);
    // The CSV gives both resistors the test point symbol.
    team.change_row(EDITED, "\"g-pas:R_US\"", "\"g-art:TestPoint\"");
    team.change_row(IMPORTED, "\"g-pas:R_US\"", "\"g-art:TestPoint\"");
    assert_eq!(team.import("g-res"), "applied");
    let stored = team.read();
    // The import applies it to the resistor nobody edited, and sets it.
    let (number, revision) = current(&stored, IMPORTED);
    assert_eq!(number, 2);
    assert!(revision.excluded_from_bom);
    // Accepting the CSV symbol on the edited resistor sets it the same way.
    let problem = cad_problem(&stored, EDITED);
    let review = CsvReviewCommand::new(
        EDITED,
        &problem.id,
        vec!["Symbol".into()],
        2,
        "Accept CSV values from g-res.csv",
    );
    let reply = submit_csv_review(&team.admin, &review).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let stored = team.read();
    let (number, accepted) = current(&stored, EDITED);
    assert_eq!(number, 3);
    assert!(accepted.excluded_from_bom);
    assert_eq!(
        accepted.cad_references, revision.cad_references,
        "the same CAD as the imported resistor"
    );
    team.server.stop();
}

#[test]
fn an_import_cannot_claim_a_different_setting_than_its_cad_says() {
    let team = Team::start();
    let before = team.read();
    for (ipn, claim, says) in [
        (EDITED, true, "cannot be Excluded from BOM"),
        (TEST_POINT, false, "must be Excluded from BOM"),
    ] {
        let csv_file = if ipn == TEST_POINT { "g-art" } else { "g-res" };
        let mut import = team.plan(csv_file);
        let part = import
            .parts
            .iter_mut()
            .find(|part| part.ipn.as_str() == ipn)
            .unwrap();
        part.revision.excluded_from_bom = claim;
        part.revision_json = part.revision.canonical_json();
        part.revision_sha256 = sha256_hex(&part.revision_json);
        let state = read_import_state(&team.admin).unwrap();
        let command = ImportCommand::new(&import, "Import").expecting(&state);
        match submit_import(&team.admin, &command) {
            Err(CollabError::Http {
                status: 400,
                message,
            }) => assert!(message.contains(says), "{ipn}: {message}"),
            other => panic!("{ipn}: expected 400, got {other:?}"),
        }
    }
    assert_eq!(team.read(), before, "nothing was written");
    team.server.stop();
}

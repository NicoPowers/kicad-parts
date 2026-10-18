//! Reviewing CSV changes that were not applied to an edited part, through
//! the protected Catalog route (seed kicad-parts-75a5): taking CSV details
//! saves the part's next details and adds no revision, taking a symbol or
//! footprint adds a revision, CAD that matches an archived revision is
//! refused, keeping the part's values is recorded, and a review covers what
//! the CSV said until the CSV changes again. The route builds what it saves
//! from the import problem's recorded CSV document, never from the request.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 and imports a
//! temp copy of the resistor CSV file; the repository is only read.

use catalog::pocketbase::{
    ArchiveCommand, CSV_CAD_DIFFERS, CSV_DETAILS_DIFFER, CSV_REVIEW_ROUTE, CatalogReadback,
    CsvReviewCommand, CsvReviewOutcome, CsvReviewReply, EditCommand, ImportCommand, ImportOutcome,
    StoredImportProblem, read_back, read_import_state, recorded_csv_document, submit_archive,
    submit_csv_review, submit_edit, submit_import,
};
use catalog::{CsvCatalog, PartDetails, PartRevision, sha256_hex};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use std::fs;

const CSV: &str = "database/g-res.csv";
const IPN: &str = "RES-0000-0470";
const OTHER: &str = "RES-0000-1000";

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
        Self::start_with(None)
    }

    /// With `extra_hook`, also loads that hook file (for tests that save
    /// records directly, as buggy server code might).
    fn start_with(extra_hook: Option<&str>) -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let mut options = services::options(&root, state.path());
        if let Some(hook) = extra_hook {
            let hooks = state.path().join("hooks");
            fs::create_dir_all(&hooks).unwrap();
            for entry in fs::read_dir(root.join("infra/pocketbase/pb_hooks")).unwrap() {
                let entry = entry.unwrap();
                fs::copy(entry.path(), hooks.join(entry.file_name())).unwrap();
            }
            fs::write(hooks.join("test_csv_review.pb.js"), hook).unwrap();
            options.hooks_dir = hooks;
        }
        let superuser = native_services::Superuser::generate();
        let binary = native_services::pocketbase_binary(&root).unwrap();
        let server = PocketBaseServer::start(&binary, &options, &superuser).unwrap();
        let admin = PocketBaseClient::new(server.url())
            .authenticate("_superusers", &superuser.email, &superuser.password)
            .unwrap();
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

    /// Replaces exactly one occurrence of `from` in the row of `ipn` in the
    /// copied CSV file.
    fn change_row(&self, ipn: &str, from: &str, to: &str) {
        let file = self.copy.path().join(CSV);
        let text = fs::read_to_string(&file).unwrap();
        let row = text
            .split_inclusive('\n')
            .find(|line| line.starts_with(&format!("\"{ipn}\"")))
            .unwrap()
            .to_owned();
        assert_eq!(row.matches(from).count(), 1, "{from:?} in {row}");
        fs::write(&file, text.replacen(&row, &row.replacen(from, to, 1), 1)).unwrap();
    }
}

fn current_details(stored: &CatalogReadback, ipn: &str) -> (u32, PartDetails) {
    let part = stored.part(ipn).unwrap();
    let details = stored.part_details(&part.current_details).unwrap();
    (
        details.number,
        serde_json::from_str(&details.snapshot).unwrap(),
    )
}

fn current_revision(stored: &CatalogReadback, ipn: &str) -> (u32, PartRevision) {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

/// Sets fields of `ipn` with one edit by `client`: a CAD edit for Symbol
/// and Footprint, otherwise a details edit.
fn edit(client: &PocketBaseClient, stored: &CatalogReadback, ipn: &str, values: &[(&str, &str)]) {
    let command = EditCommand::new(ipn, "Correct it");
    let mut command = if catalog::CAD_COLUMNS.contains(&values[0].0) {
        let (number, revision) = current_revision(stored, ipn);
        command.editing_cad(&revision, number)
    } else {
        let (number, details) = current_details(stored, ipn);
        command.editing_details(&details, number)
    };
    for (field, value) in values {
        assert!(command.set_field(field, value), "{field}");
    }
    let reply = submit_edit(client, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
}

/// The import problem with `code` about `ipn` that the latest import run
/// found.
fn found_problem(stored: &CatalogReadback, ipn: &str, code: &str) -> Option<StoredImportProblem> {
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
            problem.code == code && problem.subject == ipn && run.problems.contains(&problem.id)
        })
        .cloned()
}

/// Whether a review of `ipn` covers what `problem` says the CSV has.
fn covered(stored: &CatalogReadback, ipn: &str, problem: &StoredImportProblem) -> bool {
    let part = stored.part(ipn).unwrap();
    let sha = sha256_hex(recorded_csv_document(problem).unwrap());
    stored.csv_reviews.iter().any(|review| {
        review.part == part.id && review.code == problem.code && review.csv_sha256 == sha
    })
}

/// A review of `problem` that takes the CSV's values for `accept`.
fn review(
    ipn: &str,
    problem: &StoredImportProblem,
    accept: &[&str],
    expected: u32,
) -> CsvReviewCommand {
    CsvReviewCommand::new(
        ipn,
        &problem.id,
        accept.iter().map(|name| (*name).to_owned()).collect(),
        expected,
        "Accept CSV changes from g-res.csv",
    )
}

/// The applied answer's new details and revision numbers.
fn applied(reply: &CsvReviewReply) -> (Option<u32>, Option<u32>) {
    match &reply.outcome {
        CsvReviewOutcome::Applied {
            details, revision, ..
        } => (
            details.as_ref().map(|details| details.number),
            revision.as_ref().map(|revision| revision.number),
        ),
        other => panic!("expected applied, got {other:?}: {}", reply.body),
    }
}

fn refused(result: Result<CsvReviewReply, CollabError>, status: u16, says: &str) {
    match result {
        Err(CollabError::Http {
            status: got,
            message,
        }) if got == status => assert!(message.contains(says), "{says}: {message}"),
        Err(CollabError::Forbidden(message)) if status == 403 => {
            assert!(message.contains(says), "{says}: {message}")
        }
        other => panic!("expected {status} saying {says:?}, got {other:?}"),
    }
}

#[test]
fn taking_csv_details_saves_them_without_a_revision_and_the_rest_stays() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let viewer = team.member("Vic Viewer", false);
    // Someone edits the description and the value in the app (details 2).
    edit(
        &editor,
        &team.read(),
        IPN,
        &[("Description", "Edited description"), ("Value", "47 ohm")],
    );
    // The CSV changes the value and the tolerance; the import leaves the
    // edited details alone and records what differs.
    team.change_row(IPN, "\"47\"", "\"47R\"");
    team.change_row(IPN, "\"1%\"", "\"0.5%\"");
    assert_eq!(team.import(), "nothing_changed");
    let stored = team.read();
    let problem = found_problem(&stored, IPN, CSV_DETAILS_DIFFER).unwrap();
    assert!(!covered(&stored, IPN, &problem));
    let revisions = stored.part_revisions.len();

    // Viewers cannot review.
    refused(
        submit_csv_review(&viewer, &review(IPN, &problem, &["Value"], 2)),
        403,
        "",
    );
    // Only fields the CSV changed, of the right kind, can be taken.
    for (accept, says) in [
        ("Symbol", "not a field of both"),
        ("Datasheet", "same as the part's"),
        ("Manufacturer parts", "same as the part's"),
        ("Colour", "not a field of both"),
    ] {
        refused(
            submit_csv_review(&editor, &review(IPN, &problem, &[accept], 2)),
            400,
            says,
        );
    }
    // A stale number is a conflict.
    let stale = submit_csv_review(&editor, &review(IPN, &problem, &["Value"], 1)).unwrap();
    assert!(
        matches!(stale.outcome, CsvReviewOutcome::Conflict { current_details: Some(ref details), .. } if details.number == 2),
        "{}",
        stale.body
    );
    // A problem that is not this part's is not found.
    refused(
        submit_csv_review(&editor, &review(OTHER, &problem, &["Value"], 1)),
        404,
        "no such CSV change",
    );
    let mut forged = review(IPN, &problem, &["Value"], 2);
    forged.problem = "aaaaaaaaaaaaaaa".into();
    refused(
        submit_csv_review(&editor, &forged),
        404,
        "no such CSV change",
    );
    assert_eq!(team.read(), stored, "nothing was written");

    // Take the value only, sending a made-up value alongside: the server
    // takes the CSV's value from the problem, never from the request.
    let command = review(IPN, &problem, &["Value"], 2);
    let mut body = command.body();
    body["fields"] = serde_json::json!([{ "name": "Value", "value": "HACKED" }]);
    body["details"] = serde_json::json!({ "fields": [{ "name": "Value", "value": "HACKED" }] });
    let sent = editor.post_json(CSV_REVIEW_ROUTE, &body).unwrap();
    assert_eq!(sent.status, 200, "{}", sent.body);
    let stored = team.read();
    let (number, details) = current_details(&stored, IPN);
    assert_eq!(number, 3);
    assert_eq!(details.field("Value"), Some("47R"));
    assert_eq!(details.field("Description"), Some("Edited description"));
    assert_eq!(
        details.field("Tolerance"),
        Some("1%"),
        "not taken, so it stays"
    );
    assert_eq!(stored.part_revisions.len(), revisions, "no revision added");
    assert_eq!(current_revision(&stored, IPN).0, 1);
    let saved = stored
        .part_details(&stored.part(IPN).unwrap().current_details)
        .unwrap();
    let change = stored
        .changes
        .iter()
        .find(|change| change.id == saved.change)
        .unwrap();
    assert_eq!(change.kind, "csv-review");
    assert_eq!(stored.csv_reviews.len(), 1);
    assert_eq!(stored.csv_reviews[0].accepted, r#"["Value"]"#);
    assert_eq!(stored.csv_reviews[0].change, change.id);
    let audit = stored
        .audit_entries
        .iter()
        .filter(|entry| entry.change == change.id)
        .collect::<Vec<_>>();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].action, "catalog.csv-review");
    assert!(covered(&stored, IPN, &problem));

    // The same request again replays the answer; the same ID for another
    // request is refused; another review of the same CSV values changes
    // nothing.
    let replay = editor.post_json(CSV_REVIEW_ROUTE, &body).unwrap();
    assert_eq!((replay.status, replay.replayed), (200, true));
    assert_eq!(replay.body, sent.body);
    let mut reused = command.clone();
    reused.accept = vec!["Tolerance".into()];
    let reused = submit_csv_review(&editor, &reused).unwrap();
    assert!(matches!(
        reused.outcome,
        CsvReviewOutcome::CorrelationReused { .. }
    ));
    let again = submit_csv_review(&editor, &review(IPN, &problem, &["Tolerance"], 3)).unwrap();
    assert_eq!(again.status, 422, "{}", again.body);
    assert_eq!(team.read(), stored);

    // Importing the same CSV again stays lossless and saves nothing: the
    // tolerance still differs, but the review covers what the CSV says.
    let outcome = team.import();
    assert!(
        outcome == "nothing_changed" || outcome == "noop",
        "{outcome}"
    );
    let after = team.read();
    assert_eq!(current_details(&after, IPN).0, 3);
    assert_eq!(after.part_revisions.len(), revisions);
    let found = found_problem(&after, IPN, CSV_DETAILS_DIFFER).unwrap();
    assert!(
        covered(&after, IPN, &found),
        "the taken value never shows again"
    );
    assert_eq!(team.import(), "noop");

    // When the CSV changes again, its changes show again; keeping the
    // current values is recorded and saves no details.
    team.change_row(IPN, "\"0.5%\"", "\"0.1%\"");
    assert_eq!(team.import(), "nothing_changed");
    let stored = team.read();
    let problem = found_problem(&stored, IPN, CSV_DETAILS_DIFFER).unwrap();
    assert!(!covered(&stored, IPN, &problem));
    let kept = submit_csv_review(&editor, &review(IPN, &problem, &[], 3)).unwrap();
    assert_eq!(applied(&kept), (None, None));
    let stored = team.read();
    assert_eq!(current_details(&stored, IPN).0, 3);
    assert!(covered(&stored, IPN, &problem));
    let review_record = stored.csv_reviews.last().unwrap();
    assert_eq!(review_record.accepted, "[]");
    let change = stored
        .changes
        .iter()
        .find(|change| change.id == review_record.change)
        .unwrap();
    assert_eq!(change.kind, "csv-review");
    let outcome = team.import();
    assert!(
        outcome == "nothing_changed" || outcome == "noop",
        "{outcome}"
    );

    // A difference a later import run no longer finds is out of date.
    team.change_row(IPN, "\"0.1%\"", "\"2%\"");
    assert_eq!(team.import(), "nothing_changed");
    let late = submit_csv_review(&editor, &review(IPN, &problem, &[], 3)).unwrap();
    assert!(
        matches!(late.outcome, CsvReviewOutcome::OutOfDate { .. }),
        "{}",
        late.body
    );
    team.server.stop();
}

#[test]
fn taking_a_csv_symbol_adds_a_revision_unless_it_is_archived() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    // Revisions 2 and 3 by edits.
    edit(&editor, &team.read(), IPN, &[("Symbol", "Device:R")]);
    edit(&editor, &team.read(), IPN, &[("Symbol", "Device:R_Small")]);
    // The CSV now names revision 2's symbol; the import keeps the edited
    // revision 3 and records what differs.
    team.change_row(IPN, "\"g-res:R_US\"", "\"Device:R\"");
    assert_eq!(team.import(), "nothing_changed");
    let stored = team.read();
    let problem = found_problem(&stored, IPN, CSV_CAD_DIFFERS).unwrap();
    assert!(found_problem(&stored, IPN, CSV_DETAILS_DIFFER).is_none());

    refused(
        submit_csv_review(&editor, &review(IPN, &problem, &["Description"], 3)),
        400,
        "not the Symbol or the Footprint",
    );
    refused(
        submit_csv_review(&editor, &review(IPN, &problem, &["Footprint"], 3)),
        400,
        "same as the part's",
    );
    // Revision 2 is archived since: taking its symbol back is refused with
    // the message edits give.
    let archived = submit_archive(&editor, &ArchiveCommand::new(IPN, 2, true, "Wrong")).unwrap();
    assert_eq!(archived.status, 200, "{}", archived.body);
    refused(
        submit_csv_review(&editor, &review(IPN, &problem, &["Symbol"], 3)),
        400,
        "unarchive revision 2 instead",
    );
    // Unarchived, taking it adds revision 4 and leaves the details alone.
    let unarchived =
        submit_archive(&editor, &ArchiveCommand::new(IPN, 2, false, "Needed")).unwrap();
    assert_eq!(unarchived.status, 200, "{}", unarchived.body);
    let details_before = current_details(&team.read(), IPN);
    let took = submit_csv_review(&editor, &review(IPN, &problem, &["Symbol"], 3)).unwrap();
    assert_eq!(applied(&took), (None, Some(4)));
    let stored = team.read();
    let (number, revision) = current_revision(&stored, IPN);
    assert_eq!(number, 4);
    assert_eq!(revision.field("Symbol"), Some("Device:R"));
    assert_eq!(current_details(&stored, IPN), details_before);
    // Now the part has what the CSV says, so a re-import finds nothing.
    let outcome = team.import();
    assert!(
        outcome == "nothing_changed" || outcome == "noop",
        "{outcome}"
    );
    assert!(found_problem(&team.read(), IPN, CSV_CAD_DIFFERS).is_none());

    // Taking back a footprint the CSV binds to a stored CAD item revision
    // keeps that exact revision.
    edit(
        &editor,
        &team.read(),
        OTHER,
        &[("Footprint", "Resistor_SMD:R_0603_1608Metric")],
    );
    let outcome = team.import();
    assert!(outcome == "nothing_changed", "{outcome}");
    let stored = team.read();
    let problem = found_problem(&stored, OTHER, CSV_CAD_DIFFERS).unwrap();
    let took = submit_csv_review(&editor, &review(OTHER, &problem, &["Footprint"], 2)).unwrap();
    assert_eq!(applied(&took), (None, Some(3)));
    let stored = team.read();
    let (_, now) = current_revision(&stored, OTHER);
    let part = stored.part(OTHER).unwrap();
    let first: PartRevision = serde_json::from_str(
        &stored
            .part_revisions
            .iter()
            .find(|revision| revision.part == part.id && revision.number == 1)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    assert_eq!(now, first, "the same symbol and footprint as revision 1");
    team.server.stop();
}

/// A route that saves a CSV review directly, in a new Change, as buggy
/// server code might: a valid review unless the body overrides a field.
/// With `change`, it saves no Change and names that earlier one instead.
const DIRECT_SAVE_HOOK: &str = r#"
routerAdd("POST", "/api/test/csv-review", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  try {
    $app.runInTransaction((txApp) => {
      const part = txApp.findFirstRecordByData("catalog_parts", "ipn", body.ipn);
      const problem = txApp.findRecordById("catalog_import_problems", body.problem);
      const detail = JSON.parse(problem.getString("detail"));
      const recorded = detail.details || detail.revision;
      let change = body.change ? txApp.findRecordById("catalog_changes", body.change) : null;
      if (!change) {
        const last = txApp.findRecordsByFilter("catalog_changes", "seq > 0", "-seq", 1, 0)[0];
        const correlation = "test-" + (last.getInt("seq") + 1);
        change = new Record(txApp.findCollectionByNameOrId("catalog_changes"));
        change.set("id", lib.deriveId("change", correlation));
        change.set("seq", last.getInt("seq") + 1);
        change.set("kind", body.kind || "csv-review");
        change.set("reason", "test");
        change.set("correlation_id", correlation);
        change.set("actor", "test");
        change.set("sources", "{}");
        change.set("summary", "{}");
        txApp.save(change);
      }
      const seq = change.getInt("seq");
      const correlation = change.getString("correlation_id");
      const review = new Record(txApp.findCollectionByNameOrId("catalog_csv_reviews"));
      review.set("id", body.id || lib.deriveId("csv-review", correlation));
      review.set("part", body.part ? txApp.findFirstRecordByData("catalog_parts", "ipn", body.part).id : part.id);
      review.set("problem", problem.id);
      review.set("code", body.code || problem.getString("code"));
      review.set("csv_sha256", body.sha || $security.sha256(recorded));
      review.set("accepted", body.accepted === undefined ? "[]" : body.accepted);
      review.set("change", change.id);
      review.set("seq", body.seq || seq);
      txApp.save(review);
    });
  } catch (error) {
    return e.json(409, { refused: String(error) });
  }
  return e.json(200, { saved: true });
}, $apis.requireSuperuserAuth());
"#;

#[test]
fn server_code_can_only_save_the_reviews_the_route_saves() {
    let team = Team::start_with(Some(DIRECT_SAVE_HOOK));
    let editor = team.member("Ada Editor", true);
    edit(&editor, &team.read(), IPN, &[("Value", "47 ohm")]);
    team.change_row(IPN, "\"47\"", "\"47R\"");
    assert_eq!(team.import(), "nothing_changed");
    let stored = team.read();
    let problem = found_problem(&stored, IPN, CSV_DETAILS_DIFFER).unwrap();

    let save = |overrides: serde_json::Value| {
        let mut body = serde_json::json!({ "ipn": IPN, "problem": problem.id });
        for (key, value) in overrides.as_object().unwrap() {
            body[key] = value.clone();
        }
        team.admin.post_json("/api/test/csv-review", &body).unwrap()
    };
    for (overrides, says) in [
        (
            serde_json::json!({ "part": OTHER }),
            "a CSV difference of its own part",
        ),
        (
            serde_json::json!({ "code": CSV_CAD_DIFFERS }),
            "copy its problem's code",
        ),
        (
            serde_json::json!({ "sha": "0".repeat(64) }),
            "SHA-256 of the CSV document",
        ),
        (
            serde_json::json!({ "kind": "edit" }),
            "a Change of kind csv-review",
        ),
        (serde_json::json!({ "seq": 999 }), "copy its seq"),
        (
            serde_json::json!({ "id": "aaaaaaaaaaaaaaa" }),
            "ID derived from its Change",
        ),
        (serde_json::json!({ "accepted": "Value" }), "as JSON"),
    ] {
        let reply = save(overrides.clone());
        assert_eq!(reply.status, 409, "{overrides}: {}", reply.body);
        assert!(reply.body.contains(says), "{says}: {}", reply.body);
    }
    assert_eq!(team.read(), stored, "nothing was written");
    // Nobody writes reviews through the record API, superusers included.
    let direct = team
        .admin
        .post_json(
            "/api/collections/catalog_csv_reviews/records",
            &serde_json::json!({ "part": stored.part(IPN).unwrap().id }),
        )
        .unwrap();
    assert_eq!(direct.status, 403, "{}", direct.body);
    // A valid review is accepted, and never changes again.
    let valid = save(serde_json::json!({}));
    assert_eq!(valid.status, 200, "{}", valid.body);
    let saved = team.read().csv_reviews;
    assert_eq!(saved.len(), 1);
    // The same review again, in that review's Change, passes every check
    // above but is refused: its Change was saved by an earlier transaction,
    // so desktops that applied it would never fetch this one
    // (kicad-parts-2707).
    let late_change = team.read().changes.last().unwrap().id.clone();
    let late = save(serde_json::json!({ "change": late_change }));
    assert_eq!(late.status, 409, "{}", late.body);
    assert!(
        late.body.contains("this transaction saved no Change"),
        "{}",
        late.body
    );
    assert_eq!(team.read().csv_reviews, saved, "nothing was written");
    let path = format!(
        "/api/collections/catalog_csv_reviews/records/{}",
        saved[0].id
    );
    let changed = team
        .admin
        .patch_json(&path, &serde_json::json!({ "accepted": "[\"Value\"]" }))
        .unwrap();
    assert_eq!(changed.status, 403, "{}", changed.body);
    team.server.stop();
}

/// `text` (a CSV file with every field quoted) as a spreadsheet saves it:
/// a UTF-8 byte order mark, CRLF line ends, and quotes only around fields
/// that need them.
fn as_a_spreadsheet_saves_it(text: &str) -> String {
    let mut saved = String::from("\u{feff}");
    for line in text.lines() {
        let inner = line
            .strip_prefix('"')
            .and_then(|line| line.strip_suffix('"'))
            .unwrap_or_else(|| panic!("not fully quoted: {line}"));
        let fields: Vec<String> = inner
            .split("\",\"")
            .map(|field| {
                assert!(!field.contains('"'), "{field}");
                if field.contains(',') {
                    format!("\"{field}\"")
                } else {
                    field.to_owned()
                }
            })
            .collect();
        saved.push_str(&fields.join(","));
        saved.push_str("\r\n");
    }
    saved
}

#[test]
fn a_csv_file_saved_by_a_spreadsheet_changes_no_part() {
    // Opening g-res.csv in a spreadsheet and saving it as "CSV UTF-8"
    // adds a byte order mark, CRLF line ends and fewer quotes. The parts
    // say the same, so the import changes none of them.
    let team = Team::start();
    let before = team.read();
    let file = team.copy.path().join(CSV);
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, as_a_spreadsheet_saves_it(&text)).unwrap();
    assert_eq!(team.import(), "nothing_changed");
    let after = team.read();
    assert_eq!(after.part_details, before.part_details);
    assert_eq!(after.part_revisions, before.part_revisions);
    assert_eq!(after.changes, before.changes);
    team.server.stop();
}

#[test]
fn a_csv_value_changed_back_asks_again_unless_already_reviewed() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    // Edited in the app: the value is 47R (details 2).
    edit(&editor, &team.read(), IPN, &[("Value", "47R")]);
    let notice = |team: &Team| {
        let stored = team.read();
        found_problem(&stored, IPN, CSV_DETAILS_DIFFER)
            .filter(|problem| !covered(&stored, IPN, problem))
    };
    let keep = |problem: &StoredImportProblem| {
        let reply = submit_csv_review(&editor, &review(IPN, problem, &[], 2)).unwrap();
        assert_eq!(applied(&reply), (None, None));
    };
    // The CSV goes from 47 to 47.0: asked, and the edited value is kept.
    team.change_row(IPN, "\"47\"", "\"47.0\"");
    assert_eq!(team.import(), "nothing_changed");
    let first = notice(&team).expect("the CSV changed, so the app asks");
    keep(&first);
    assert!(notice(&team).is_none());
    // Back to 47, what the CSV said before the edit: that is a change of
    // the CSV, never reviewed, so the app asks again.
    team.change_row(IPN, "\"47.0\"", "\"47\"");
    assert_eq!(team.import(), "nothing_changed");
    let second = notice(&team).expect("the CSV changed back, so the app asks again");
    assert_ne!(second.id, first.id);
    keep(&second);
    // And to 47.0 again: those exact values were already reviewed, so the
    // app does not ask about them a second time.
    team.change_row(IPN, "\"47\"", "\"47.0\"");
    assert_eq!(team.import(), "nothing_changed");
    assert!(notice(&team).is_none(), "already reviewed");
    // The edited value stayed throughout, with no new details.
    let stored = team.read();
    assert_eq!(current_details(&stored, IPN).0, 2);
    assert_eq!(current_details(&stored, IPN).1.field("Value"), Some("47R"));
    team.server.stop();
}

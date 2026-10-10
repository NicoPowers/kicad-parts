//! Re-imports of the resistor CSV file (g-res) into a real, disposable
//! PocketBase: the zero-loss report holds after every kind of change to the
//! CSV catalog, and an import never undoes newer changes.
//!
//! Each test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; every changed input is a temp copy.

use catalog::pocketbase::{
    CatalogReadback, EditCommand, ImportCommand, ImportOutcome, ImportReply, read_back,
    read_import_state, submit_edit, submit_import,
};
use catalog::{CsvCatalog, CsvImport, PartDetails, sha256_hex};
use catalog_import::{ZeroLossReport, services};
use collaboration::PocketBaseClient;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REASON: &str = "Import the resistors from database/g-res.csv";
const CSV: &str = "database/g-res.csv";
const SUBSTITUTES_CSV: &str = "database/substitutes.csv";

/// Every file the resistor import reads.
const INPUTS: [&str; 4] = [
    "database/g-res.csv",
    "database/substitutes.csv",
    "symbols/g-pas.kicad_sym",
    "footprints/g-res.pretty/R_0603_1608Metric.kicad_mod",
];

fn repository() -> PathBuf {
    services::repository_root()
}

/// The hashes of every CSV catalog tree file, to show the tests never
/// change them.
fn catalog_hashes() -> Vec<(PathBuf, String)> {
    fn walk(path: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), out);
            } else {
                out.push((entry.path(), sha256_hex(fs::read(entry.path()).unwrap())));
            }
        }
    }
    let mut out = Vec::new();
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        walk(&repository().join(tree), &mut out);
    }
    out.sort();
    out
}

/// Copies the CSV catalog trees the oracle reads into `target`.
fn copy_catalog(target: &Path) {
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
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        copy_tree(&repository().join(tree), &target.join(tree));
    }
}

/// Replaces exactly one occurrence of `from` in `path` under `root`.
fn replace_once(root: &Path, path: &str, from: &str, to: &str) {
    let file = root.join(path);
    let text = fs::read_to_string(&file).unwrap();
    assert_eq!(text.matches(from).count(), 1, "{from:?} in {path}");
    fs::write(&file, text.replacen(from, to, 1)).unwrap();
}

/// The line of `path` that starts with `"<ipn>"`, terminator included.
fn row_of(root: &Path, path: &str, ipn: &str) -> String {
    let text = fs::read_to_string(root.join(path)).unwrap();
    text.split_inclusive('\n')
        .find(|line| line.starts_with(&format!("\"{ipn}\"")))
        .unwrap()
        .to_owned()
}

fn plan(root: &Path) -> CsvImport {
    CsvCatalog::new(root).import("g-res", None).unwrap()
}

/// Imports as an importer does: reads the team catalog first and expects
/// each part's current revision as read. (Temp copies are not git
/// checkouts, so the checkout check is left out here.)
fn import_reading_first(admin: &PocketBaseClient, import: &CsvImport, suffix: &str) -> ImportReply {
    let state = read_import_state(admin).unwrap();
    let mut command = ImportCommand::new(import, REASON).expecting(&state);
    command.correlation_id.push_str(suffix);
    submit_import(admin, &command).unwrap()
}

fn outcome(reply: &ImportReply) -> &'static str {
    match reply.outcome {
        ImportOutcome::Applied { .. } => "applied",
        ImportOutcome::NothingChanged { .. } => "nothing_changed",
        ImportOutcome::Noop { .. } => "noop",
        ImportOutcome::Conflict { .. } => "conflict",
        ImportOutcome::CorrelationReused { .. } => "correlation_reused",
        ImportOutcome::IdCollision { .. } => "id_collision",
    }
}

/// The zero-loss report of `root`'s CSV catalog against what is stored. It
/// must be lossless.
fn lossless_report(root: &Path, stored: &CatalogReadback, step: &str) -> ZeroLossReport {
    let oracle = legacy_import_fixture::build_manifest(root).unwrap();
    let report = catalog_import::report(&oracle, "g-res", stored, root);
    assert!(
        report.is_lossless(),
        "{step}: {}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    assert_eq!(
        report.recorded_byte_sha256, report.oracle_byte_sha256,
        "{step}"
    );
    report
}

/// The field account of `column` in the row of `ipn`.
fn field<'r>(
    report: &'r ZeroLossReport,
    ipn: &str,
    column: &str,
) -> &'r catalog_import::zero_loss::FieldAccount {
    report
        .field_accounts
        .iter()
        .find(|account| account.ipn == ipn && account.column == column)
        .unwrap()
}

fn current_revision(stored: &CatalogReadback, ipn: &str) -> (u32, PartDetails) {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

#[test]
fn re_imports_stay_lossless() {
    let before = catalog_hashes();
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    copy_catalog(root);
    // RES-0000-1500 starts with a blank MPN, so an import problem keeps its
    // half manufacturer part until the MPN is filled in.
    replace_once(
        root,
        CSV,
        "\"RES-0000-1500\",\"RC0603FR-07150RL\"",
        "\"RES-0000-1500\",\"\"",
    );
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();

    let first = import_reading_first(&admin, &plan(root), "-first");
    assert_eq!(outcome(&first), "applied", "{}", first.body);
    let report = lossless_report(root, &read_back(&admin).unwrap(), "first import");
    assert_eq!(
        field(&report, "RES-0000-1500", "MPN").destination,
        "import-problem"
    );

    // An LCSC-only change makes no revision, but the run is recorded with
    // the new file's hash and rows, and the report shows the new code.
    replace_once(root, CSV, "\"C22775\"", "\"C99999\"");
    let reply = import_reading_first(&admin, &plan(root), "-lcsc");
    assert_eq!(outcome(&reply), "nothing_changed", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    assert_eq!(current_revision(&stored, "RES-0000-1000").0, 1);
    let report = lossless_report(root, &stored, "LCSC-only change");
    let lcsc = field(&report, "RES-0000-1000", "LCSC");
    assert_eq!(
        (lcsc.destination.as_str(), lcsc.stored.as_deref()),
        ("supplier-data", Some("C99999"))
    );
    let run = report.import_run.as_ref().unwrap();
    assert_eq!(run.change, "", "a supplier-only run makes no Change");

    // A run that only reads a changed substitutes.csv (here a new row for
    // another part) is recorded too, even with nothing else to record.
    let substitutes = root.join(SUBSTITUTES_CSV);
    let header = fs::read_to_string(&substitutes).unwrap();
    fs::write(
        &substitutes,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-10K-ND\n"),
    )
    .unwrap();
    let reply = import_reading_first(&admin, &plan(root), "-substitute");
    assert_eq!(outcome(&reply), "applied", "{}", reply.body);
    lossless_report(
        root,
        &read_back(&admin).unwrap(),
        "substitutes.csv row added",
    );
    // A supplier-only change in substitutes.csv: no revision, and the report
    // reads the new SupplierPN from the run.
    fs::write(
        &substitutes,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-10K-CHANGED\n"),
    )
    .unwrap();
    let reply = import_reading_first(&admin, &plan(root), "-supplier-pn");
    assert_eq!(outcome(&reply), "nothing_changed", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    assert_eq!(current_revision(&stored, "RES-0000-1002").0, 2);
    let report = lossless_report(root, &stored, "substitutes.csv supplier change");
    assert_eq!(report.substitutes_csv_rows.destinations["supplier-data"], 2);

    // Filling in the blank MPN makes a new revision with a main
    // manufacturer part; the old import problem no longer applies.
    replace_once(
        root,
        CSV,
        "\"RES-0000-1500\",\"\"",
        "\"RES-0000-1500\",\"RC0603FR-07150RL\"",
    );
    let reply = import_reading_first(&admin, &plan(root), "-mpn");
    assert_eq!(outcome(&reply), "applied", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    let report = lossless_report(root, &stored, "blank MPN filled in");
    let mpn = field(&report, "RES-0000-1500", "MPN");
    assert_eq!(
        (mpn.destination.as_str(), mpn.stored.as_deref()),
        ("part-details.manufacturer-parts", Some("RC0603FR-07150RL"))
    );
    // Unchanged parts keep revision 1, whose source names the first file.
    assert_eq!(current_revision(&stored, "RES-0000-0000").0, 1);

    // An edit after the latest import does not matter: the report checks
    // the revision that import left the part at.
    let edit = {
        let (number, details) = current_revision(&stored, "RES-0000-0470");
        let mut command = EditCommand::starting_from(&details, number, "Note the exact value");
        command.set_field("Value", "47 Ohm");
        command
    };
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    lossless_report(root, &read_back(&admin).unwrap(), "edit after the import");
    // A changed row of a part maintained by edits is kept whole by an
    // import problem.
    replace_once(
        root,
        CSV,
        "\"RES 47 OHM 1% 1/10W 0603\",\"47\"",
        "\"RES 47 OHM 1% 1/10W 0603\",\"47.0\"",
    );
    let reply = import_reading_first(&admin, &plan(root), "-edited");
    assert_eq!(outcome(&reply), "nothing_changed", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    assert_eq!(
        current_revision(&stored, "RES-0000-0470").1.field("Value"),
        Some("47 Ohm")
    );
    let report = lossless_report(root, &stored, "part maintained by edits");
    assert_eq!(
        field(&report, "RES-0000-0470", "Value").destination,
        "import-problem"
    );
    assert_eq!(report.destinations["import-problem"], 16);

    // An inserted row becomes a new part; every row after it moves down,
    // and rows are still matched by IPN.
    let source_row = row_of(root, CSV, "RES-0000-1000");
    let inserted = source_row
        .replace("RES-0000-1000", "RES-0000-1050")
        .replace("\"C99999\"", "\"C11111\"");
    replace_once(root, CSV, &source_row, &format!("{inserted}{source_row}"));
    let reply = import_reading_first(&admin, &plan(root), "-inserted");
    assert_eq!(outcome(&reply), "applied", "{}", reply.body);
    let report = lossless_report(root, &read_back(&admin).unwrap(), "inserted row");
    assert_eq!((report.rows, report.parts), (30, 30));

    // Reordered rows change no part details; the run is recorded.
    let first_row = row_of(root, CSV, "RES-0000-0000");
    let second_row = row_of(root, CSV, "RES-0000-0470");
    replace_once(
        root,
        CSV,
        &format!("{first_row}{second_row}"),
        &format!("{second_row}{first_row}"),
    );
    let reply = import_reading_first(&admin, &plan(root), "-reordered");
    assert_eq!(outcome(&reply), "nothing_changed", "{}", reply.body);
    lossless_report(root, &read_back(&admin).unwrap(), "reordered rows");

    // A removed row deletes nothing: an import problem says the part is no
    // longer in the file, and the part keeps its revisions.
    let removed = row_of(root, CSV, "RES-0000-1800");
    replace_once(root, CSV, &removed, "");
    let reply = import_reading_first(&admin, &plan(root), "-removed");
    assert_eq!(outcome(&reply), "nothing_changed", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    assert!(stored.part("RES-0000-1800").is_some());
    let report = lossless_report(root, &stored, "removed row");
    assert_eq!(report.removed_parts, ["RES-0000-1800"]);
    assert!(stored.problems.iter().any(
        |problem| problem.code == "part-not-in-csv-file" && problem.subject == "RES-0000-1800"
    ));
    // Importing the same files again is a no-op.
    let reply = import_reading_first(&admin, &plan(root), "-again");
    assert_eq!(outcome(&reply), "noop", "{}", reply.body);

    // The report still detects a lost removal problem.
    let mut copy = stored.clone();
    copy.problems
        .retain(|problem| problem.code != "part-not-in-csv-file");
    let oracle = legacy_import_fixture::build_manifest(root).unwrap();
    let report = catalog_import::report(&oracle, "g-res", &copy, root);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("RES-0000-1800 was a part of database/g-res.csv")),
        "{:?}",
        report.problems
    );

    server.stop();
    assert_eq!(
        catalog_hashes(),
        before,
        "the tests changed the CSV catalog"
    );
}

/// Copies the resistor inputs under `target`.
fn copy_inputs(target: &Path) {
    for path in INPUTS {
        let destination = target.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(repository().join(path), destination).unwrap();
    }
}

/// Runs git in a throwaway fixture repository: windowless, without line
/// ending conversion, and with no signing or hooks from the user's
/// configuration.
fn git(repository: &Path, hooks: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(repository).args([
        "-c",
        "user.name=fixture",
        "-c",
        "user.email=fixture@example.test",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.autocrlf=false",
    ]);
    command
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(args);
    let output = native_services::run_hidden("git", &mut command).unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn latest_audit_entry(stored: &CatalogReadback) -> &catalog::pocketbase::StoredAuditEntry {
    stored
        .audit_entries
        .iter()
        .max_by_key(|entry| entry.csv_import_number)
        .unwrap()
}

#[test]
fn an_older_checkout_is_refused_unless_allowed_and_the_override_is_recorded() {
    let checkout = tempfile::tempdir().unwrap();
    let hooks = tempfile::tempdir().unwrap();
    let root = checkout.path();
    let git = |args: &[&str]| git(root, hooks.path(), args);
    copy_inputs(root);
    git(&["init", "-q"]);
    let mut add = vec!["add", "--"];
    add.extend_from_slice(&INPUTS);
    git(&add);
    git(&["commit", "-q", "-m", "older"]);
    let older = git(&["rev-parse", "HEAD"]);
    replace_once(
        root,
        CSV,
        "\"RES 47 OHM 1% 1/10W 0603\",\"47\"",
        "\"RES 47 OHM 1% 1/10W 0603\",\"47R\"",
    );
    git(&["commit", "-q", "-m", "newer", "--", CSV]);
    let newer = git(&["rev-parse", "HEAD"]);

    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    let import = services::plan(root, "g-res").unwrap();
    assert_eq!(import.source_commit.as_deref(), Some(newer.as_str()));
    let command = services::prepare_import(&admin, root, &import, REASON, false).unwrap();
    assert_eq!(
        outcome(&submit_import(&admin, &command).unwrap()),
        "applied"
    );
    let imported = read_back(&admin).unwrap();
    assert_eq!(
        current_revision(&imported, "RES-0000-0470")
            .1
            .field("Value"),
        Some("47R")
    );

    // The older checkout would undo the newer import: refused, and nothing
    // is written.
    git(&["checkout", "-q", &older]);
    let old_import = services::plan(root, "g-res").unwrap();
    assert_eq!(old_import.source_commit.as_deref(), Some(older.as_str()));
    let refused = services::prepare_import(&admin, root, &old_import, REASON, false).unwrap_err();
    assert!(
        refused.contains("is older than the team catalog")
            && refused.contains(&newer[..12])
            && refused.contains("database/g-res.csv"),
        "{refused}"
    );
    assert_eq!(read_back(&admin).unwrap(), imported);

    // Allowed explicitly, it imports, and the audit entry records that.
    let command = services::prepare_import(&admin, root, &old_import, REASON, true).unwrap();
    assert!(command.allow_older_checkout);
    assert_eq!(
        outcome(&submit_import(&admin, &command).unwrap()),
        "applied"
    );
    let stored = read_back(&admin).unwrap();
    let entry = latest_audit_entry(&stored);
    assert!(entry.allowed_older_checkout);
    assert_eq!(entry.source_commit, older);
    let (number, details) = current_revision(&stored, "RES-0000-0470");
    assert_eq!((number, details.field("Value")), (2, Some("47")));
    // No earlier run was allowed anything.
    assert_eq!(
        stored
            .audit_entries
            .iter()
            .filter(|entry| entry.allowed_older_checkout)
            .count(),
        1
    );

    // A changed input makes the source commit `<commit>+dirty`, which counts
    // as its base commit: the newer checkout with a local change imports.
    git(&["checkout", "-q", &newer]);
    let substitutes = root.join(SUBSTITUTES_CSV);
    let header = fs::read_to_string(&substitutes).unwrap();
    fs::write(
        &substitutes,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-10K-ND\n"),
    )
    .unwrap();
    let dirty = services::plan(root, "g-res").unwrap();
    assert_eq!(dirty.source_commit, Some(format!("{newer}+dirty")));
    let command = services::prepare_import(&admin, root, &dirty, REASON, false).unwrap();
    assert_eq!(
        outcome(&submit_import(&admin, &command).unwrap()),
        "applied"
    );
    assert!(!latest_audit_entry(&read_back(&admin).unwrap()).allowed_older_checkout);

    // Now the latest run is `<newer>+dirty`, so the older checkout is
    // refused again.
    git(&["checkout", "-q", "--", SUBSTITUTES_CSV]);
    git(&["checkout", "-q", &older]);
    let old_again = services::plan(root, "g-res").unwrap();
    let refused = services::prepare_import(&admin, root, &old_again, REASON, false).unwrap_err();
    assert!(
        refused.contains("is older than the team catalog"),
        "{refused}"
    );
    server.stop();
}

#[test]
fn a_part_changed_between_reading_and_importing_is_reported_not_overwritten() {
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    copy_inputs(root);
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    assert_eq!(
        outcome(&import_reading_first(&admin, &plan(root), "")),
        "applied"
    );

    // The importer reads the team catalog...
    replace_once(
        root,
        CSV,
        "\"RES 47 OHM 1% 1/10W 0603\",\"47\"",
        "\"RES 47 OHM 1% 1/10W 0603\",\"47R\"",
    );
    let changed = plan(root);
    let read = read_import_state(&admin).unwrap();
    let command = ImportCommand::new(&changed, REASON).expecting(&read);
    assert_eq!(command.expected_revisions["RES-0000-0470"], 1);

    // ...someone edits the part before the import arrives...
    let edit = {
        let stored = read_back(&admin).unwrap();
        let (number, details) = current_revision(&stored, "RES-0000-0470");
        let mut command = EditCommand::starting_from(&details, number, "Note the exact value");
        command.set_field("Value", "47 Ohm");
        command
    };
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    let edited = read_back(&admin).unwrap();

    // ...so the import is refused with the part's new current revision, and
    // nothing is written.
    let reply = submit_import(&admin, &command).unwrap();
    assert_eq!(reply.status, 409, "{}", reply.body);
    let ImportOutcome::Conflict {
        conflicts, message, ..
    } = &reply.outcome
    else {
        panic!("expected a conflict, got {}", reply.body);
    };
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    let conflict = &conflicts[0];
    assert_eq!(
        (conflict.ipn.as_str(), conflict.expected_revision),
        ("RES-0000-0470", 1)
    );
    let current = conflict.current_revision.as_ref().unwrap();
    assert_eq!((current.number, current.change_kind.as_str()), (2, "edit"));
    assert!(
        message.contains("someone else changed RES-0000-0470"),
        "{message}"
    );
    assert_eq!(read_back(&admin).unwrap(), edited);
    // A conflict is not stored, so a retry is checked again.
    let retry = submit_import(&admin, &command).unwrap();
    assert_eq!((retry.status, retry.replayed), (409, false));

    // Read again and import: the edit stays, and the changed row is kept by
    // an import problem.
    let reply = import_reading_first(&admin, &changed, "-reread");
    assert_eq!(outcome(&reply), "nothing_changed", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    assert_eq!(
        current_revision(&stored, "RES-0000-0470").1.field("Value"),
        Some("47 Ohm")
    );
    assert!(stored.problems.iter().any(|problem| problem.code
        == "csv-row-conflicts-with-current-revision"
        && problem.subject == "RES-0000-0470"));

    // A command that expects every part to be new, sent to a catalog that
    // has them, conflicts on every part.
    let mut stale = ImportCommand::new(&changed, REASON);
    stale.correlation_id.push_str("-stale");
    let reply = submit_import(&admin, &stale).unwrap();
    let ImportOutcome::Conflict { conflicts, .. } = &reply.outcome else {
        panic!("expected a conflict, got {}", reply.body);
    };
    assert_eq!(conflicts.len(), 29);
    assert!(
        conflicts
            .iter()
            .all(|conflict| conflict.expected_revision == 0)
    );
    server.stop();
}

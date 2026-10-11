//! Re-imports of the resistor CSV file (g-res) into a real, disposable
//! PocketBase: the zero-loss report holds after every kind of change to the
//! CSV catalog, and an import never undoes newer changes.
//!
//! Each test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; every changed input is a temp copy.

use catalog::pocketbase::{
    CatalogReadback, EditCommand, ImportCommand, ImportOutcome, ImportReply, RestoreCommand,
    StoredCadItemRevision, read_back, read_import_state, submit_edit, submit_import,
    submit_restore,
};
use catalog::{
    CadItemDetails, CadItemId, CadItemKind, CadItemRevisionId, CadReferenceRole,
    CadReferenceTarget, CsvCatalog, CsvImport, ModelTarget, PartDetails, sha256_hex,
};
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
        ImportOutcome::NewerImportRun { .. } => "newer_import_run",
    }
}

/// The zero-loss report of `root`'s CSV catalog against what is stored. It
/// must be lossless.
fn lossless_report(root: &Path, stored: &CatalogReadback, step: &str) -> ZeroLossReport {
    let oracle = csv_catalog_fixture::build_manifest(root).unwrap();
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
    // Importing the same files again, with the correlation ID the command
    // derives itself, is a no-op; doing it once more replays that answer.
    let reply = import_reading_first(&admin, &plan(root), "");
    assert_eq!(outcome(&reply), "noop", "{}", reply.body);
    let rerun = import_reading_first(&admin, &plan(root), "");
    assert_eq!(
        (rerun.status, rerun.replayed),
        (422, true),
        "{}",
        rerun.body
    );

    // A neighbouring symbol in the same library changes. The resistor
    // symbol keeps its revision (whose source names the old file hash and
    // byte range), and the report checks it against the file as it is now.
    replace_once(
        root,
        "symbols/g-pas.kicad_sym",
        "\t(symbol \"C\"\n",
        "\t(symbol \"C\"\n\t\t(property \"Note\" \"a neighbouring change\"\n\t\t\t(at 0 0 0)\n\t\t)\n",
    );
    let reply = import_reading_first(&admin, &plan(root), "");
    assert_eq!(outcome(&reply), "nothing_changed", "{}", reply.body);
    let report = lossless_report(root, &read_back(&admin).unwrap(), "neighbouring symbol");
    // The removed part is still reported by this later run.
    assert_eq!(report.removed_parts, ["RES-0000-1800"]);
    assert!(
        report
            .cad_items
            .iter()
            .any(|item| item.reference == "g-pas:R_US")
    );

    // The report still detects a lost removal problem.
    let mut copy = stored.clone();
    copy.problems
        .retain(|problem| problem.code != "part-not-in-csv-file");
    let oracle = csv_catalog_fixture::build_manifest(root).unwrap();
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
        refused.contains(&format!("does not include commit {}", &newer[..12]))
            && refused.contains("database/g-res.csv"),
        "{refused}"
    );
    assert_eq!(read_back(&admin).unwrap(), imported);

    // Allowed explicitly, it imports, and the audit entry records that.
    let command = services::prepare_import(&admin, root, &old_import, REASON, true).unwrap();
    assert!(command.allow_older_checkout());
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
    let after_dirty = read_back(&admin).unwrap();
    assert!(!latest_audit_entry(&after_dirty).allowed_older_checkout);

    // A clean checkout of that same commit would undo the uncommitted
    // change: refused unless allowed. So are other uncommitted files.
    git(&["checkout", "-q", "--", SUBSTITUTES_CSV]);
    let clean = services::plan(root, "g-res").unwrap();
    assert_eq!(clean.source_commit.as_deref(), Some(newer.as_str()));
    let refused = services::prepare_import(&admin, root, &clean, REASON, false).unwrap_err();
    assert!(
        refused.contains("came from uncommitted changes on commit")
            && refused.contains(&newer[..12]),
        "{refused}"
    );
    fs::write(
        &substitutes,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-10K-OTHER\n"),
    )
    .unwrap();
    let other = services::plan(root, "g-res").unwrap();
    assert_eq!(other.source_commit, Some(format!("{newer}+dirty")));
    let refused = services::prepare_import(&admin, root, &other, REASON, false).unwrap_err();
    assert!(refused.contains("uncommitted changes"), "{refused}");
    assert_eq!(read_back(&admin).unwrap(), after_dirty);
    // The same uncommitted files again are accepted (and change nothing).
    fs::write(
        &substitutes,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-10K-ND\n"),
    )
    .unwrap();
    let same = services::plan(root, "g-res").unwrap();
    let command = services::prepare_import(&admin, root, &same, REASON, false).unwrap();
    assert_eq!(outcome(&submit_import(&admin, &command).unwrap()), "noop");
    // An unrelated later commit on top of the clean commit would undo the
    // uncommitted change just the same: refused.
    git(&["checkout", "-q", "--", SUBSTITUTES_CSV]);
    fs::write(root.join("notes.txt"), "unrelated\n").unwrap();
    git(&["add", "--", "notes.txt"]);
    git(&["commit", "-q", "-m", "unrelated"]);
    let unrelated = services::plan(root, "g-res").unwrap();
    assert_eq!(
        unrelated.source_commit.as_deref(),
        Some(git(&["rev-parse", "HEAD"]).as_str())
    );
    let refused = services::prepare_import(&admin, root, &unrelated, REASON, false).unwrap_err();
    assert!(
        refused.contains("other copies of database/substitutes.csv"),
        "{refused}"
    );
    // Committing the change itself imports (nothing new to record).
    fs::write(
        &substitutes,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-10K-ND\n"),
    )
    .unwrap();
    git(&[
        "commit",
        "-q",
        "-m",
        "commit the change",
        "--",
        SUBSTITUTES_CSV,
    ]);
    let committed = services::plan(root, "g-res").unwrap();
    assert!(
        !committed
            .source_commit
            .as_deref()
            .unwrap()
            .ends_with("+dirty")
    );
    let command = services::prepare_import(&admin, root, &committed, REASON, false).unwrap();
    assert_eq!(outcome(&submit_import(&admin, &command).unwrap()), "noop");
    assert_eq!(read_back(&admin).unwrap(), after_dirty);

    // Allowed explicitly, the clean checkout imports.
    git(&["checkout", "-q", &newer]);
    let command = services::prepare_import(&admin, root, &clean, REASON, true).unwrap();
    assert_eq!(
        outcome(&submit_import(&admin, &command).unwrap()),
        "applied"
    );

    // The older checkout is refused again.
    git(&["checkout", "-q", &older]);
    let old_again = services::plan(root, "g-res").unwrap();
    let refused = services::prepare_import(&admin, root, &old_again, REASON, false).unwrap_err();
    assert!(refused.contains("does not include commit"), "{refused}");
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
    assert_eq!(command.expected_revisions()["RES-0000-0470"], 1);

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
    let mut latest_only = read_import_state(&admin).unwrap();
    latest_only.current_revisions.clear();
    let stale = ImportCommand::new(&changed, REASON).expecting(&latest_only);
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
    let ImportOutcome::Conflict { message, .. } = &reply.outcome else {
        unreachable!()
    };
    assert!(message.contains("and 19 more"), "{message}");

    // Another import of the file lands after the importer read the team
    // catalog: its checkout check is out of date, so it is refused.
    let read = read_import_state(&admin).unwrap();
    replace_once(root, CSV, "\"C22775\"", "\"C99999\"");
    let late = plan(root);
    let command = ImportCommand::new(&late, REASON).expecting(&read);
    replace_once(root, CSV, "\"C99999\"", "\"C88888\"");
    let first = import_reading_first(&admin, &plan(root), "");
    assert_eq!(outcome(&first), "nothing_changed", "{}", first.body);
    let before = read_back(&admin).unwrap();
    let reply = submit_import(&admin, &command).unwrap();
    assert_eq!(reply.status, 409, "{}", reply.body);
    let ImportOutcome::NewerImportRun {
        expected_csv_import,
        latest_csv_import,
        message,
        ..
    } = &reply.outcome
    else {
        panic!("expected a newer import run, got {}", reply.body);
    };
    assert_eq!(*latest_csv_import, expected_csv_import + 1);
    assert!(
        message.contains("another import of database/g-res.csv"),
        "{message}"
    );
    assert_eq!(read_back(&admin).unwrap(), before);
    server.stop();
}

#[test]
fn import_problems_of_earlier_runs_explain_no_rows() {
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    copy_catalog(root);
    // RES-0000-0470 appears again as row 4, so that row is kept by a
    // duplicate-ipn import problem.
    let original = row_of(root, CSV, "RES-0000-0470");
    let third = row_of(root, CSV, "RES-0000-1000");
    replace_once(root, CSV, &third, &format!("{third}{original}"));
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    let reply = import_reading_first(&admin, &plan(root), "");
    assert_eq!(outcome(&reply), "applied", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    assert!(
        stored
            .problems
            .iter()
            .any(|problem| problem.code == "duplicate-ipn")
    );

    // The first RES-0000-0470 row goes and a new row comes first, so the
    // copy is row 4 again with the same bytes, but it is now the part's
    // only row. The old duplicate-ipn problem must not explain it.
    let first_row = row_of(root, CSV, "RES-0000-0000");
    let new_row = first_row
        .replace("RES-0000-0000", "RES-0000-0001")
        .replace("\"C21189\"", "\"C11111\"");
    let text = fs::read_to_string(root.join(CSV)).unwrap();
    let text =
        text.replacen(&original, "", 1)
            .replacen(&first_row, &format!("{new_row}{first_row}"), 1);
    fs::write(root.join(CSV), text).unwrap();
    assert_eq!(row_of(root, CSV, "RES-0000-0470"), original);
    let reply = import_reading_first(&admin, &plan(root), "-moved");
    assert_eq!(outcome(&reply), "applied", "{}", reply.body);
    let stored = read_back(&admin).unwrap();
    let report = lossless_report(root, &stored, "duplicate became the only row");
    assert_eq!(
        field(&report, "RES-0000-0470", "Value").destination,
        "part-details"
    );
    assert!(
        report
            .other_problems
            .iter()
            .any(|line| line.starts_with("duplicate-ipn")),
        "{:?}",
        report.other_problems
    );
    server.stop();
}

const FOOTPRINT: &str = "footprints/g-res.pretty/R_0603_1608Metric.kicad_mod";
const SYMBOLS: &str = "symbols/g-pas.kicad_sym";
const MODEL: &str = "3d-models/D_SOD-923.step";

/// Every revision of the CAD item `kind` `key`, by number.
fn revisions_of<'a>(
    stored: &'a CatalogReadback,
    kind: CadItemKind,
    key: &str,
) -> Vec<&'a StoredCadItemRevision> {
    let id = CadItemId::new(kind, key);
    let mut revisions: Vec<_> = stored
        .cad_item_revisions
        .iter()
        .filter(|revision| revision.cad_item == id.as_str())
        .collect();
    revisions.sort_by_key(|revision| revision.number);
    revisions
}

/// The revision number `details` binds for its first `role` reference.
fn bound_number(details: &PartDetails, role: CadReferenceRole) -> u32 {
    details
        .cad_references
        .iter()
        .find(|reference| reference.role == role)
        .map(|reference| match &reference.target {
            CadReferenceTarget::CadItemRevision { number, .. } => *number,
            other => panic!("{} is bound to {other:?}", reference.reference),
        })
        .unwrap()
}

/// Checks that `kind` `key` has revisions 1..=`count`, each the successor
/// of the one before, with the ID derived from its number, and that every
/// part whose current revision uses it binds the newest one, except the
/// parts in `kept_older` (which still bind an older revision). Returns the
/// revisions.
fn numbered_and_current<'a>(
    stored: &'a CatalogReadback,
    kind: CadItemKind,
    key: &str,
    count: u32,
    kept_older: &[&str],
) -> Vec<&'a StoredCadItemRevision> {
    let revisions = revisions_of(stored, kind, key);
    let cad_item = CadItemId::new(kind, key);
    let numbers: Vec<u32> = revisions.iter().map(|revision| revision.number).collect();
    assert_eq!(numbers, (1..=count).collect::<Vec<_>>(), "{key}");
    for pair in revisions.windows(2) {
        assert_eq!(pair[1].predecessor, pair[0].id, "{key}");
    }
    for revision in &revisions {
        assert_eq!(
            revision.id,
            CadItemRevisionId::new(&cad_item, revision.number).as_str()
        );
    }
    let newest = revisions.last().unwrap();
    let role = match kind {
        CadItemKind::Symbol => Some(CadReferenceRole::Symbol),
        CadItemKind::Footprint => Some(CadReferenceRole::Footprint),
        CadItemKind::Model3d => None,
    };
    if let Some(role) = role {
        let mut users = 0;
        for part in &stored.parts {
            let revision = stored.part_revision(&part.current_revision).unwrap();
            let details: PartDetails = serde_json::from_str(&revision.snapshot).unwrap();
            let uses = details.cad_references.iter().any(|reference| {
                reference.role == role
                    && matches!(&reference.target, CadReferenceTarget::CadItemRevision { cad_item: id, .. } if id == cad_item.as_str())
            });
            if uses && !kept_older.contains(&part.ipn.as_str()) {
                users += 1;
                assert_eq!(bound_number(&details, role), count, "{}", part.ipn);
                assert!(
                    revision.cad_item_revisions.contains(&newest.id),
                    "{}",
                    part.ipn
                );
            }
        }
        assert_eq!(users, 29 - kept_older.len(), "every resistor uses {key}");
    }
    revisions
}

/// Sends the importer's own request for `root`, except that `ipn`'s
/// footprint binding names `(now, number, snapshot_sha256)`: it binds
/// revision `now` and is changed to `number` with that hash. The route must
/// refuse it with a message containing `says`.
fn refuse_footprint_binding(
    admin: &PocketBaseClient,
    root: &Path,
    ipn: &str,
    (now, number, snapshot_sha256): (u32, u32, &str),
    says: &str,
) {
    let mut body = ImportCommand::new(&plan(root), REASON)
        .expecting(&read_import_state(admin).unwrap())
        .body();
    body["correlationId"] = format!("rebind-{ipn}-to-{number}").into();
    let index = body["parts"]
        .as_array()
        .unwrap()
        .iter()
        .position(|part| part["ipn"] == ipn)
        .unwrap();
    let mut snapshot: serde_json::Value =
        serde_json::from_str(body["parts"][index]["snapshot"].as_str().unwrap()).unwrap();
    for reference in snapshot["cad_references"].as_array_mut().unwrap() {
        if reference["role"] == "footprint" {
            assert_eq!(reference["target"]["number"], now, "{ipn}");
            reference["target"]["number"] = number.into();
            reference["target"]["snapshot_sha256"] = snapshot_sha256.into();
        }
    }
    let snapshot = snapshot.to_string();
    body["parts"][index]["snapshotSha256"] = sha256_hex(&snapshot).into();
    body["parts"][index]["snapshot"] = snapshot.into();
    let reply = admin
        .post_json(catalog::pocketbase::CSV_IMPORT_ROUTE, &body)
        .unwrap();
    assert_eq!(reply.status, 400, "{ipn}: {}", reply.body);
    assert!(reply.body.contains(says), "{ipn}: {}", reply.body);
}

/// A resistor kept by an edit in the test below.
const EDITED: &str = "RES-0000-1000";

/// Sets `field` of `ipn` to `value` with an edit, as a person in the app does.
fn edit_field(admin: &PocketBaseClient, ipn: &str, field: &str, value: &str) {
    let stored = read_back(admin).unwrap();
    let (number, details) = current_revision(&stored, ipn);
    let mut command = EditCommand::starting_from(&details, number, &format!("Set {field}"));
    assert!(command.set_field(field, value));
    let reply = submit_edit(admin, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
}

/// The current revision of `ipn`: its number, and its footprint and symbol
/// numbers.
fn bound_state(stored: &CatalogReadback, ipn: &str) -> (u32, u32, u32) {
    let (number, details) = current_revision(stored, ipn);
    (
        number,
        bound_number(&details, CadReferenceRole::Footprint),
        bound_number(&details, CadReferenceRole::Symbol),
    )
}

/// The messages of the `csv-row-conflicts-with-current-revision` problems
/// about `ipn` that the latest import run found.
fn conflicts_found(stored: &CatalogReadback, ipn: &str) -> Vec<String> {
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
        .filter(|problem| {
            problem.code == "csv-row-conflicts-with-current-revision"
                && problem.subject == ipn
                && run.problems.contains(&problem.id)
        })
        .map(|problem| problem.message.clone())
        .collect()
}

/// Imports `root` and expects `outcome`; then the zero-loss report must be
/// lossless.
fn import_step(
    admin: &PocketBaseClient,
    root: &Path,
    step: &str,
    expected: &str,
) -> CatalogReadback {
    let reply = import_reading_first(admin, &plan(root), "");
    assert_eq!(outcome(&reply), expected, "{step}: {}", reply.body);
    let stored = read_back(admin).unwrap();
    lossless_report(root, &stored, step);
    stored
}

#[test]
fn going_back_to_earlier_cad_content_adds_a_revision_that_parts_bind() {
    let before = catalog_hashes();
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    copy_catalog(root);
    // The resistor footprint names KiCad's own 3D model, which is not
    // stored; point it at a model in the CSV catalog instead, so the 3D
    // model case is covered too.
    replace_once(
        root,
        FOOTPRINT,
        "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
        "${GITPLM_PARTS}/3d-models/D_SOD-923.step",
    );
    let footprint_a = fs::read_to_string(root.join(FOOTPRINT)).unwrap();
    let symbols_a = fs::read_to_string(root.join(SYMBOLS)).unwrap();
    let model_a = fs::read(root.join(MODEL)).unwrap();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    let footprint = "g-res:R_0603_1608Metric";
    let symbol = "g-pas:R_US";

    import_step(&admin, root, "content A", "applied");
    // A person edits one resistor and changes it back, so its details equal
    // its row again, but an edit, not the CSV, now keeps it: it misses the
    // CAD changes below.
    edit_field(&admin, EDITED, "Value", "100 Ohm");
    edit_field(&admin, EDITED, "Value", "100");
    let cad_changed =
        format!("the symbols, footprints or 3D models that the row of {EDITED} uses changed");
    // Footprint A, B, A: revisions 1, 2 and 3, with 3 current. Parts the
    // imports keep get real revisions binding 1, 2 and 3.
    replace_once(
        root,
        FOOTPRINT,
        "(tags \"resistor\")",
        "(tags \"resistor B\")",
    );
    let stored = import_step(&admin, root, "footprint B", "applied");
    assert_eq!(bound_state(&stored, "RES-0000-0470"), (2, 2, 1));
    // Edits win: the edited part keeps footprint revision 1, and the import
    // problem says plainly that only the CAD it uses changed.
    assert_eq!(bound_state(&stored, EDITED), (3, 1, 1));
    let found = conflicts_found(&stored, EDITED);
    assert!(
        found.len() == 1 && found[0].starts_with(&cad_changed),
        "{found:?}"
    );
    fs::write(root.join(FOOTPRINT), &footprint_a).unwrap();
    let stored = import_step(&admin, root, "footprint A again", "applied");
    assert_eq!(bound_state(&stored, "RES-0000-0470"), (3, 3, 1));
    // Footprint revision 3 has the content of revision 1, which the edited
    // part binds: the same content under a newer number is not a change, so
    // the part keeps revision 1, gets no revision and no conflict problem.
    assert_eq!(bound_state(&stored, EDITED), (3, 1, 1));
    assert!(conflicts_found(&stored, EDITED).is_empty());
    let revisions = numbered_and_current(&stored, CadItemKind::Footprint, footprint, 3, &[EDITED]);
    assert_eq!(revisions[2].snapshot_sha256, revisions[0].snapshot_sha256);
    assert_ne!(revisions[1].snapshot_sha256, revisions[0].snapshot_sha256);
    // The bytes are stored once, shared by revisions 1 and 3.
    assert_eq!(revisions[2].blob, revisions[0].blob);
    let blob = stored.blob(&revisions[0].blob).unwrap();
    assert_eq!(
        stored
            .blobs
            .iter()
            .filter(|stored| stored.sha256 == blob.sha256)
            .count(),
        1
    );
    assert_eq!(blob.bytes, footprint_a.as_bytes());
    // Importing the same content again adds nothing, and replays.
    let noop = import_reading_first(&admin, &plan(root), "");
    assert_eq!(outcome(&noop), "noop", "{}", noop.body);
    let replay = import_reading_first(&admin, &plan(root), "");
    assert_eq!((replay.status, replay.replayed), (422, true));
    assert_eq!(read_back(&admin).unwrap(), stored);
    // The server holds parts to the same rule as the importer, whatever a
    // hand-made request says. RES-0000-0470 binds footprint revision 3, so
    // naming revision 1 (same content) is refused; the edited part binds
    // revision 1, so naming the current revision 3 (same content) is
    // refused too.
    refuse_footprint_binding(
        &admin,
        root,
        "RES-0000-0470",
        (3, 1, &revisions[0].snapshot_sha256),
        "must keep revision 3 of its CAD item",
    );
    refuse_footprint_binding(
        &admin,
        root,
        EDITED,
        (1, 3, &revisions[2].snapshot_sha256),
        "must keep revision 1 of its CAD item",
    );
    assert_eq!(read_back(&admin).unwrap(), stored);

    // The edited part is still kept by its edit: a change to its row is an
    // import problem, not a revision.
    let value = "\"RES 100 OHM 1% 1/10W 0603\",\"100\"";
    let changed_value = "\"RES 100 OHM 1% 1/10W 0603\",\"100R\"";
    replace_once(root, CSV, value, changed_value);
    let stored = import_step(&admin, root, "edited part's row changed", "nothing_changed");
    assert_eq!(bound_state(&stored, EDITED), (3, 1, 1));
    let found = conflicts_found(&stored, EDITED);
    assert!(
        found.len() == 1 && found[0].starts_with(&format!("the source row of {EDITED} changed")),
        "{found:?}"
    );
    replace_once(root, CSV, changed_value, value);
    import_step(&admin, root, "edited part's row back", "nothing_changed");

    // The same for a symbol.
    replace_once(
        root,
        SYMBOLS,
        "\t(symbol \"R_US\"\n",
        "\t(symbol \"R_US\"\n\t\t(property \"Note\" \"content B\"\n\t\t\t(at 0 0 0)\n\t\t)\n",
    );
    let stored = import_step(&admin, root, "symbol B", "applied");
    assert_eq!(bound_state(&stored, EDITED), (3, 1, 1));
    let found = conflicts_found(&stored, EDITED);
    assert!(
        found.len() == 1 && found[0].starts_with(&cad_changed),
        "{found:?}"
    );
    fs::write(root.join(SYMBOLS), &symbols_a).unwrap();
    let stored = import_step(&admin, root, "symbol A again", "applied");
    assert_eq!(bound_state(&stored, EDITED), (3, 1, 1));
    assert!(conflicts_found(&stored, EDITED).is_empty());
    let symbols = numbered_and_current(&stored, CadItemKind::Symbol, symbol, 3, &[EDITED]);
    assert_eq!(symbols[2].snapshot_sha256, symbols[0].snapshot_sha256);
    assert_eq!(symbols[2].blob, symbols[0].blob);
    let noop = import_reading_first(&admin, &plan(root), "");
    assert_eq!(outcome(&noop), "noop", "{}", noop.body);

    // The same for the footprint's 3D model. A new 3D model revision is
    // new footprint content (its snapshot names the model's number), so
    // the footprint gets revisions 4 and 5 and the parts bind 5.
    let mut model_b = model_a.clone();
    model_b.extend_from_slice(b"/* content B */\n");
    fs::write(root.join(MODEL), &model_b).unwrap();
    let stored = import_step(&admin, root, "3D model B", "applied");
    // Footprint revision 4 names model revision 2, other content.
    assert_eq!(bound_state(&stored, EDITED), (3, 1, 1));
    let found = conflicts_found(&stored, EDITED);
    assert!(
        found.len() == 1 && found[0].starts_with(&cad_changed),
        "{found:?}"
    );
    fs::write(root.join(MODEL), &model_a).unwrap();
    let stored = import_step(&admin, root, "3D model A again", "applied");
    // Footprint revision 5 differs from revision 1 only in naming model
    // revision 3 instead of 1, which have the same content. So the edited
    // part's CAD is unchanged: it keeps footprint revision 1, quietly.
    assert_eq!(bound_state(&stored, EDITED), (3, 1, 1));
    assert!(conflicts_found(&stored, EDITED).is_empty());
    let models = numbered_and_current(&stored, CadItemKind::Model3d, MODEL, 3, &[]);
    assert_eq!(models[2].snapshot_sha256, models[0].snapshot_sha256);
    assert_eq!(models[2].blob, models[0].blob);
    let footprints = numbered_and_current(&stored, CadItemKind::Footprint, footprint, 5, &[EDITED]);
    let newest: CadItemDetails = serde_json::from_str(&footprints[4].snapshot).unwrap();
    let model_numbers: Vec<u32> = newest
        .model_references
        .iter()
        .filter_map(|model| match &model.target {
            ModelTarget::CadItemRevision { number, .. } => Some(*number),
            ModelTarget::Unresolved { .. } => None,
        })
        .collect();
    assert_eq!(model_numbers, [3]);
    // Footprint revisions 3 and 5 have the same bytes, so one blob.
    assert_eq!(footprints[4].blob, footprints[2].blob);
    let noop = import_reading_first(&admin, &plan(root), "");
    assert_eq!(outcome(&noop), "noop", "{}", noop.body);
    let cad_item_revisions = stored.cad_item_revisions.len();

    // A hand-made request cannot add a footprint revision that differs from
    // the current one (revision 5, naming model revision 3) only by naming
    // model revision 1, which has the same content.
    let mut body = ImportCommand::new(&plan(root), REASON)
        .expecting(&read_import_state(&admin).unwrap())
        .body();
    body["correlationId"] = "footprint-differs-only-in-numbers".into();
    let entry = body["cadItemRevisions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["kind"] == "footprint")
        .unwrap();
    assert_eq!(
        (entry["expectedRevision"].as_u64(), entry["number"].as_u64()),
        (Some(5), Some(5))
    );
    let mut snapshot: serde_json::Value =
        serde_json::from_str(entry["snapshot"].as_str().unwrap()).unwrap();
    for model in snapshot["model_references"].as_array_mut().unwrap() {
        if model["target"]["kind"] == "cad-item-revision" {
            assert_eq!(model["target"]["number"], 3);
            model["target"]["number"] = 1.into();
        }
    }
    let snapshot = snapshot.to_string();
    let cad_item = CadItemId::from_stored(entry["cadItemId"].as_str().unwrap());
    entry["snapshotSha256"] = sha256_hex(&snapshot).into();
    entry["snapshot"] = snapshot.into();
    entry["number"] = 6.into();
    entry["id"] = CadItemRevisionId::new(&cad_item, 6).as_str().into();
    let reply = admin
        .post_json(catalog::pocketbase::CSV_IMPORT_ROUTE, &body)
        .unwrap();
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(
        reply
            .body
            .contains("differs only in the revision numbers it names"),
        "{}",
        reply.body
    );
    assert_eq!(read_back(&admin).unwrap(), stored);

    // An edit of the part that kept the older numbers keeps them too: the
    // binding moves only when the CAD content or the reference changes.
    edit_field(&admin, EDITED, "Value", "100 Ohm");
    let stored = read_back(&admin).unwrap();
    assert_eq!(bound_state(&stored, EDITED), (4, 1, 1));
    assert_eq!(stored.cad_item_revisions.len(), cad_item_revisions);

    // An edit after that binds the current revisions, and adds none.
    let ipn = "RES-0000-0470";
    let (number, details) = current_revision(&stored, ipn);
    assert_eq!(number, 7);
    let mut edit = EditCommand::starting_from(&details, number, "Note the exact value");
    edit.set_field("Value", "47 Ohm");
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    let edited = read_back(&admin).unwrap();
    let (number, details) = current_revision(&edited, ipn);
    assert_eq!(number, 8);
    assert_eq!(bound_number(&details, CadReferenceRole::Footprint), 5);
    assert_eq!(bound_number(&details, CadReferenceRole::Symbol), 3);
    assert_eq!(edited.cad_item_revisions.len(), cad_item_revisions);
    lossless_report(root, &edited, "edit after the imports");

    // Restoring revision 1 copies it exactly, CAD bindings included: symbol
    // revision 1 and footprint revision 1. A restore never moves a binding
    // and adds no CAD item revision.
    let restore = RestoreCommand::new(ipn, 8, 1, "Go back to the first import");
    let reply = submit_restore(&admin, &restore).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let restored = read_back(&admin).unwrap();
    let (number, details) = current_revision(&restored, ipn);
    assert_eq!(number, 9);
    assert_eq!(bound_number(&details, CadReferenceRole::Symbol), 1);
    assert_eq!(bound_number(&details, CadReferenceRole::Footprint), 1);
    assert_eq!(
        restored
            .part_revision(&restored.part(ipn).unwrap().current_revision)
            .unwrap()
            .snapshot,
        restored
            .part_revisions
            .iter()
            .find(|revision| revision.part == restored.part(ipn).unwrap().id && revision.number == 1)
            .unwrap()
            .snapshot
    );
    let first = restored
        .part_revisions
        .iter()
        .find(|revision| revision.part == restored.part(ipn).unwrap().id && revision.number == 1)
        .unwrap();
    let first_details: PartDetails = serde_json::from_str(&first.snapshot).unwrap();
    assert_eq!(details.fields, first_details.fields);
    assert_eq!(details.manufacturer_parts, first_details.manufacturer_parts);
    let current = restored
        .part_revision(&restored.part(ipn).unwrap().current_revision)
        .unwrap();
    assert_eq!(details.canonical_json(), current.snapshot);
    let mut bound = current.cad_item_revisions.clone();
    bound.sort();
    let mut expected = vec![
        CadItemRevisionId::new(&CadItemId::new(CadItemKind::Symbol, symbol), 1).to_string(),
        CadItemRevisionId::new(&CadItemId::new(CadItemKind::Footprint, footprint), 1).to_string(),
    ];
    expected.sort();
    assert_eq!(bound, expected);
    assert_eq!(restored.cad_item_revisions, edited.cad_item_revisions);
    lossless_report(root, &restored, "restore after the imports");

    server.stop();
    assert_eq!(
        catalog_hashes(),
        before,
        "the tests changed the CSV catalog"
    );
}

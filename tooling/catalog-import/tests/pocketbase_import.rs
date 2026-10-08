//! The resistor grouping imported into a real, disposable PocketBase and
//! proved lossless against the legacy import oracle.
//!
//! Each test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; changed inputs are made in temp copies.

use catalog::pocketbase::{
    CatalogReadback, ImportCommand, ImportOutcome, LEGACY_IMPORT_ROUTE, read_back, submit_import,
};
use catalog::{LegacyCatalog, PartRevisionSnapshot, sha256_hex};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use std::fs;
use std::path::{Path, PathBuf};

const REASON: &str = "Import the Legacy Catalog resistor grouping (database/g-res.csv) into canonical Part Revisions";

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

fn input_hashes(root: &Path) -> Vec<String> {
    INPUTS
        .iter()
        .map(|path| sha256_hex(fs::read(root.join(path)).unwrap()))
        .collect()
}

fn oracle() -> legacy_import_fixture::Manifest {
    legacy_import_fixture::load_manifest(
        &repository().join("fixtures/import/legacy-catalog/manifest.json"),
    )
    .unwrap()
}

/// A copy of the resistor inputs under a temp root, for changed-input runs.
fn copy_inputs(target: &Path) {
    for path in INPUTS {
        let destination = target.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(repository().join(path), destination).unwrap();
    }
}

fn applied(reply: &catalog::pocketbase::ImportReply) -> catalog::pocketbase::Counts {
    match &reply.outcome {
        ImportOutcome::Applied { created, .. } => created.clone(),
        other => panic!("expected an applied import, got {other:?}: {}", reply.body),
    }
}

fn head(readback: &CatalogReadback, ipn: &str) -> (u32, PartRevisionSnapshot) {
    let part = readback.part(ipn).unwrap();
    let revision = readback.part_revision(&part.head).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

#[test]
fn the_resistor_grouping_imports_idempotently_and_losslessly() {
    let root = repository();
    let before = input_hashes(&root);
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let url = server.url().to_owned();

    let import = LegacyCatalog::new(&root)
        .import("g-res", services::source_commit(&root))
        .unwrap();
    let command = ImportCommand::new(&import, REASON);

    // Only a superuser may import: anonymous and member calls are refused,
    // and members cannot write Catalog collections directly.
    let anonymous = PocketBaseClient::new(&url);
    assert!(matches!(
        submit_import(&anonymous, &command),
        Err(CollabError::Unauthorized(_))
    ));
    let password = native_services::random_token(24);
    admin
        .post_json(
            "/api/collections/users/records",
            &serde_json::json!({
                "email": "engineer@example.test",
                "password": password,
                "passwordConfirm": password,
                "verified": true,
            }),
        )
        .unwrap()
        .success()
        .unwrap();
    let member = PocketBaseClient::new(&url)
        .authenticate("users", "engineer@example.test", &password)
        .unwrap();
    assert!(matches!(
        submit_import(&member, &command),
        Err(CollabError::Forbidden(_))
    ));
    let direct = member
        .post_json(
            "/api/collections/catalog_managed_parts/records",
            &serde_json::json!({ "ipn": "RES-9999-9999" }),
        )
        .unwrap();
    assert!(direct.success().is_err(), "{direct:?}");
    assert_eq!(read_back(&admin).unwrap(), CatalogReadback::default());

    // The first import publishes exactly one reasoned Change Set.
    let first = submit_import(&admin, &command).unwrap();
    assert_eq!(first.status, 200, "{}", first.body);
    let created = applied(&first);
    assert_eq!(
        (
            created.managed_parts,
            created.part_revisions,
            created.successor_revisions
        ),
        (29, 29, 0)
    );
    assert_eq!(
        (created.assets, created.asset_revisions, created.blobs),
        (2, 2, 2)
    );
    assert_eq!(created.diagnostics, 1);
    let stored = read_back(&admin).unwrap();
    assert_eq!(stored.change_sets.len(), 1);
    let change_set = &stored.change_sets[0];
    assert_eq!(
        (
            change_set.seq,
            change_set.kind.as_str(),
            change_set.reason.as_str()
        ),
        (1, "legacy-import", REASON)
    );
    assert_eq!(
        Some(change_set.source_commit.as_str()),
        import.source_commit.as_deref()
    );
    assert_eq!(stored.audit_events.len(), 1);
    assert_eq!(stored.audit_events[0].change_set, change_set.id);

    // Members can read what was published.
    let visible: Vec<serde_json::Value> = member
        .list_records("catalog_managed_parts", "", "ipn")
        .unwrap();
    assert_eq!(visible.len(), 29);

    // Stable identities and immutable IPNs: every ID is the one the domain
    // derives, and every part has exactly revision 1.
    for part in &import.parts {
        let record = stored.part(part.ipn.as_str()).unwrap();
        assert_eq!(record.id, part.id.as_str());
        assert_eq!(
            (
                record.ipn_family.as_str(),
                record.ipn_base.as_str(),
                record.variation.as_str()
            ),
            (part.ipn.family(), part.ipn.base(), part.ipn.variation())
        );
        assert_eq!(record.head_number, 1);
        let revision = stored.part_revision(&record.head).unwrap();
        assert_eq!(revision.snapshot, part.snapshot_json);
        assert_eq!(revision.predecessor, "");
        assert_eq!(revision.asset_revisions.len(), 2);
    }

    // The zero-loss report, read back from PocketBase, against the oracle.
    let report = catalog_import::report(&oracle(), "g-res", &stored, &root);
    assert!(
        report.is_lossless(),
        "{}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    assert_eq!((report.rows, report.columns, report.fields), (29, 16, 464));
    assert_eq!(report.destinations["part-revision-snapshot"], 29 * 7);
    assert_eq!(report.destinations["approved-manufacturer-part"], 29 * 2);
    assert_eq!(report.destinations["operational-fact"], 29 * 7);
    assert!(!report.destinations.contains_key("import-diagnostic"));
    assert_eq!(report.rebuilt_byte_sha256, report.oracle_byte_sha256);
    assert_eq!(report.assets.len(), 2);
    assert_eq!(report.oracle_diagnostics.len(), 1);
    assert!(report.other_diagnostics.is_empty());
    assert_eq!(
        (report.substitutes.rows, report.substitutes.rows_in_scope),
        (0, 0)
    );

    // The report detects loss: each tampered read-back fails it.
    let fails = |tamper: &dyn Fn(&mut CatalogReadback), expected: &str| {
        let mut copy = stored.clone();
        tamper(&mut copy);
        let report = catalog_import::report(&oracle(), "g-res", &copy, &root);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains(expected)),
            "expected a problem containing {expected:?}, got {:?}",
            report.problems
        );
    };
    let head_of = |copy: &mut CatalogReadback, ipn: &str| -> usize {
        let head = copy.part(ipn).unwrap().head.clone();
        copy.part_revisions
            .iter()
            .position(|revision| revision.id == head)
            .unwrap()
    };
    fails(
        &|copy| {
            let index = head_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            revision.snapshot = revision.snapshot.replace("\"47\"", "\"48\"");
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "field Value",
    );
    fails(
        &|copy| {
            let index = head_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            revision.provenance = revision.provenance.replace("\"C23182\"", "\"\"");
        },
        "field LCSC",
    );
    fails(
        &|copy| {
            let index = head_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            revision.snapshot = revision
                .snapshot
                .replace("{\"name\":\"Tolerance\",\"value\":\"1%\"},", "");
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "field Tolerance",
    );
    fails(
        &|copy| {
            copy.managed_parts
                .retain(|part| part.ipn != "RES-0000-1000")
        },
        "neither a Managed Part nor an Import Diagnostic",
    );
    fails(&|copy| copy.diagnostics.clear(), "no Import Diagnostic");
    fails(
        &|copy| copy.asset_blobs[0].bytes.push(b' '),
        "stored blob is not the declared bytes",
    );
    fails(&|copy| copy.audit_events.clear(), "has 0 Audit Events");

    // Repeating the command replays its stored answer and writes nothing.
    let replay = submit_import(&admin, &command).unwrap();
    assert!(replay.replayed, "{} {}", replay.status, replay.body);
    assert_eq!((replay.status, &replay.body), (200, &first.body));
    assert_eq!(read_back(&admin).unwrap(), stored);

    // A fresh import of the unchanged inputs is a no-op: no Change Set,
    // revision, asset or blob.
    let again = LegacyCatalog::new(&root)
        .import("g-res", services::source_commit(&root))
        .unwrap();
    let mut retry = ImportCommand::new(&again, REASON);
    retry.correlation_id.push_str("-again");
    let noop = submit_import(&admin, &retry).unwrap();
    assert_eq!(noop.status, 422, "{}", noop.body);
    assert!(matches!(noop.outcome, ImportOutcome::Noop { .. }));
    assert_eq!(read_back(&admin).unwrap(), stored);

    // Reusing a correlation ID for a different request is refused.
    let mut reused = ImportCommand::new(&import, "a different reason");
    reused.correlation_id = command.correlation_id.clone();
    let refused = submit_import(&admin, &reused).unwrap();
    assert_eq!(refused.status, 409);
    assert!(matches!(
        refused.outcome,
        ImportOutcome::CorrelationReused { .. }
    ));

    server.stop();
    assert_eq!(input_hashes(&root), before, "the import changed its inputs");
}

#[test]
fn changed_rows_get_successors_or_diagnostics_and_bad_requests_write_nothing() {
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let original = LegacyCatalog::new(&root).import("g-res", None).unwrap();
    applied(&submit_import(&admin, &ImportCommand::new(&original, REASON)).unwrap());
    let initial = read_back(&admin).unwrap();

    // Tampered requests are refused before anything is written.
    let command = ImportCommand::new(&original, REASON);
    let mut body = command.body();
    body["correlationId"] = "tampered-snapshot".into();
    body["parts"][0]["snapshotSha256"] = sha256_hex("not the snapshot").into();
    let reply = admin.post_json(LEGACY_IMPORT_ROUTE, &body).unwrap();
    assert_eq!(reply.status, 400, "{}", reply.body);
    let mut body = command.body();
    body["correlationId"] = "tampered-blob".into();
    body["blobs"][0]["base64"] = catalog::pocketbase::base64(b"other bytes").into();
    assert_eq!(
        admin.post_json(LEGACY_IMPORT_ROUTE, &body).unwrap().status,
        400
    );
    let mut body = command.body();
    body["correlationId"] = "tampered-id".into();
    body["parts"][0]["id"] = "aaaaaaaaaaaaaaa".into();
    assert_eq!(
        admin.post_json(LEGACY_IMPORT_ROUTE, &body).unwrap().status,
        400
    );
    let mut body = command.body();
    body["correlationId"] = "no-reason".into();
    body["reason"] = "   ".into();
    assert_eq!(
        admin.post_json(LEGACY_IMPORT_ROUTE, &body).unwrap().status,
        400
    );
    assert_eq!(read_back(&admin).unwrap(), initial);

    // A changed semantic field becomes a successor revision; a changed
    // Operational Fact creates no revision but keeps the new row as an
    // Import Diagnostic. Nothing else is touched.
    let copy = tempfile::tempdir().unwrap();
    copy_inputs(copy.path());
    let csv_path = copy.path().join("database/g-res.csv");
    let csv = fs::read_to_string(&csv_path).unwrap();
    let csv = csv
        .replace(
            "\"RES 47 OHM 1% 1/10W 0603\",\"47\"",
            "\"RES 47 OHM 1% 1/10W 0603\",\"47R\"",
        )
        .replace("\"C22775\"", "\"C99999\"");
    fs::write(&csv_path, &csv).unwrap();
    let changed = LegacyCatalog::new(copy.path())
        .import("g-res", None)
        .unwrap();
    let reply = submit_import(&admin, &ImportCommand::new(&changed, REASON)).unwrap();
    let created = applied(&reply);
    assert_eq!(
        (
            created.managed_parts,
            created.part_revisions,
            created.successor_revisions
        ),
        (0, 1, 1)
    );
    assert_eq!(
        (created.assets, created.asset_revisions, created.blobs),
        (0, 0, 0)
    );
    assert_eq!(created.diagnostics, 1);
    let after = read_back(&admin).unwrap();
    assert_eq!(after.change_sets.len(), 2);
    assert_eq!(after.change_sets[1].seq, 2);
    let (number, snapshot) = head(&after, "RES-0000-0470");
    assert_eq!((number, snapshot.field("Value")), (2, Some("47R")));
    let successor = after
        .part_revision(&after.part("RES-0000-0470").unwrap().head)
        .unwrap();
    let predecessor = after.part_revision(&successor.predecessor).unwrap();
    // The first revision is immutable.
    assert_eq!(
        Some(predecessor),
        initial.part_revision(&initial.part("RES-0000-0470").unwrap().head)
    );
    assert_eq!(head(&after, "RES-0000-1000").0, 1);
    let kept = after
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "legacy-row-changed-without-semantic-change")
        .unwrap();
    assert_eq!(kept.subject, "RES-0000-1000");
    assert!(kept.detail.contains("C99999"), "{}", kept.detail);

    // A head not published by a legacy import (simulated by relabelling
    // the latest Change Set, as a future Catalog edit would) is never
    // overwritten: the changed row becomes a diagnostic instead.
    let latest = &after.change_sets[1];
    admin
        .patch_json(
            &format!("/api/collections/catalog_change_sets/records/{}", latest.id),
            &serde_json::json!({ "kind": "edit" }),
        )
        .unwrap()
        .success()
        .unwrap();
    let csv = csv.replace("\"47R\"", "\"47.0\"");
    fs::write(&csv_path, &csv).unwrap();
    let conflicting = LegacyCatalog::new(copy.path())
        .import("g-res", None)
        .unwrap();
    let reply = submit_import(&admin, &ImportCommand::new(&conflicting, REASON)).unwrap();
    let created = applied(&reply);
    assert_eq!((created.part_revisions, created.diagnostics), (0, 1));
    let last = read_back(&admin).unwrap();
    assert_eq!(head(&last, "RES-0000-0470"), head(&after, "RES-0000-0470"));
    assert!(last.diagnostics.iter().any(|diagnostic| diagnostic.code
        == "legacy-row-conflicts-with-head"
        && diagnostic.subject == "RES-0000-0470"));
    server.stop();
}

#[test]
fn the_oracle_still_matches_the_checked_in_catalog() {
    // The import must leave every Legacy Catalog input byte-for-byte as
    // the oracle recorded it.
    let root = repository();
    let actual = legacy_import_fixture::build_manifest(&root).unwrap();
    assert!(legacy_import_fixture::compare(&oracle(), &actual).is_empty());
}

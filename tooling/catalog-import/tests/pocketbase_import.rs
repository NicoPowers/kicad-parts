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

    let import = services::plan(&root, "g-res").unwrap();
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
    // ...but cannot create, update or delete any of it directly.
    let part_id = stored.managed_parts[0].id.clone();
    let revision_id = stored.managed_parts[0].head.clone();
    for (collection, id) in [
        ("catalog_managed_parts", &part_id),
        ("catalog_part_revisions", &revision_id),
        ("catalog_change_sets", &change_set.id),
        ("catalog_asset_blobs", &stored.asset_blobs[0].id),
    ] {
        let path = format!("/api/collections/{collection}/records/{id}");
        let update = member
            .patch_json(
                &path,
                &serde_json::json!({ "ipn": "RES-9999-9999", "reason": "x" }),
            )
            .unwrap();
        assert!(update.success().is_err(), "{collection} update: {update:?}");
        let delete = member.delete(&path).unwrap();
        assert!(delete.success().is_err(), "{collection} delete: {delete:?}");
    }
    assert_eq!(read_back(&admin).unwrap(), stored);

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
    // The stored header line and row bytes must rebuild the file exactly.
    fails(
        &|copy| {
            let mut sources: serde_json::Value =
                serde_json::from_str(&copy.change_sets[0].sources).unwrap();
            for file in sources["files"].as_array_mut().unwrap() {
                if file["path"] == "database/g-res.csv" {
                    let header = format!("{} ", file["header_raw"].as_str().unwrap());
                    file["header_raw"] = header.into();
                }
            }
            copy.change_sets[0].sources = sources.to_string();
        },
        "rebuilt from PocketBase",
    );
    fails(
        &|copy| {
            let index = head_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            let mut provenance: serde_json::Value =
                serde_json::from_str(&revision.provenance).unwrap();
            let raw = provenance["raw_record"]
                .as_str()
                .unwrap()
                .replace('\n', "\r\n");
            provenance["raw_record"] = raw.into();
            revision.provenance = provenance.to_string();
        },
        "rebuilt from PocketBase",
    );
    // A symbol blob must be the whole symbol, not a prefix of it.
    fails(
        &|copy| {
            let revision = copy
                .asset_revisions
                .iter_mut()
                .find(|revision| revision.declaration.contains("\"kind\":\"symbol\""))
                .unwrap();
            let mut provenance: serde_json::Value =
                serde_json::from_str(&revision.provenance).unwrap();
            let end = provenance["byte_range"][1].as_u64().unwrap();
            provenance["byte_range"][1] = (end - 1).into();
            revision.provenance = provenance.to_string();
            let blob_id = revision.blob.clone();
            let blob = copy
                .asset_blobs
                .iter_mut()
                .find(|b| b.id == blob_id)
                .unwrap();
            blob.bytes.pop();
        },
        "are not the whole symbol",
    );
    fails(
        &|copy| copy.asset_revisions[0].license_status = "MIT".into(),
        "License Declaration is not explicit",
    );
    // A part bound to a different Asset Revision than its peers.
    fails(
        &|copy| {
            let index = head_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            let mut snapshot: serde_json::Value = serde_json::from_str(&revision.snapshot).unwrap();
            snapshot["asset_bindings"][1]["target"]["asset_revision_sha256"] =
                "0".repeat(64).into();
            revision.snapshot = serde_json::to_string(
                &serde_json::from_value::<PartRevisionSnapshot>(snapshot).unwrap(),
            )
            .unwrap();
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "binds 2 different targets",
    );
    // A field in the snapshot that no source column put there.
    fails(
        &|copy| {
            let index = head_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            revision.snapshot = revision.snapshot.replace(
                "\"fields\":[",
                "\"fields\":[{\"name\":\"Extra\",\"value\":\"x\"},",
            );
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "are not the semantic columns",
    );

    // Repeating the command replays its stored answer and writes nothing.
    let replay = submit_import(&admin, &command).unwrap();
    assert!(replay.replayed, "{} {}", replay.status, replay.body);
    assert_eq!((replay.status, &replay.body), (200, &first.body));
    assert_eq!(read_back(&admin).unwrap(), stored);

    // A fresh import of the unchanged inputs is a no-op: no Change Set,
    // revision, asset or blob.
    let again = services::plan(&root, "g-res").unwrap();
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

    // The kind is a closed set, so a mistyped kind cannot be stored.
    let mistyped = admin
        .patch_json(
            &format!("/api/collections/catalog_change_sets/records/{}", latest.id),
            &serde_json::json!({ "kind": "Edit" }),
        )
        .unwrap();
    assert!(mistyped.success().is_err(), "{mistyped:?}");

    // A head without Legacy Catalog Provenance (as an edit's might be) is
    // answered with a diagnostic that keeps the row, not a server error.
    let head_id = last.part("RES-0000-1001").unwrap().head.clone();
    admin
        .patch_json(
            &format!("/api/collections/catalog_part_revisions/records/{head_id}"),
            &serde_json::json!({ "provenance": "not a Legacy Catalog row" }),
        )
        .unwrap()
        .success()
        .unwrap();
    let mut command = ImportCommand::new(&conflicting, REASON);
    command.correlation_id.push_str("-provenance");
    let reply = submit_import(&admin, &command).unwrap();
    assert_eq!(applied(&reply).diagnostics, 1, "{}", reply.body);
    let kept = read_back(&admin).unwrap();
    let diagnostic = kept
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "head-without-legacy-provenance")
        .unwrap();
    assert_eq!(diagnostic.subject, "RES-0000-1001");
    assert!(
        diagnostic.detail.contains("raw_record"),
        "{}",
        diagnostic.detail
    );
    assert_eq!(head(&kept, "RES-0000-1001").0, 1);

    // A stored record whose derived ID matches but whose content does not
    // is an ID collision: refused, with nothing written.
    let collides = |collection: &str, id: &str, field: &str, value: &str, original: &str| {
        let path = format!("/api/collections/{collection}/records/{id}");
        admin
            .patch_json(&path, &serde_json::json!({ field: value }))
            .unwrap()
            .success()
            .unwrap();
        // (read_back itself refuses a tampered blob, so count Change Sets.)
        let change_sets = || {
            admin
                .list_records::<serde_json::Value>("catalog_change_sets", "", "seq")
                .unwrap()
                .len()
        };
        let before = change_sets();
        let mut command = ImportCommand::new(&conflicting, REASON);
        command
            .correlation_id
            .push_str(&format!("-collision-{collection}"));
        let reply = submit_import(&admin, &command).unwrap();
        assert_eq!(reply.status, 409, "{}", reply.body);
        let ImportOutcome::IdCollision { collisions, .. } = &reply.outcome else {
            panic!("expected an ID collision, got {}", reply.body);
        };
        assert!(
            collisions
                .iter()
                .any(|c| c.contains(collection) && c.contains(id)),
            "{collisions:?}"
        );
        assert_eq!(change_sets(), before);
        admin
            .patch_json(&path, &serde_json::json!({ field: original }))
            .unwrap()
            .success()
            .unwrap();
    };
    let diagnostic = &kept.diagnostics[0];
    collides(
        "catalog_import_diagnostics",
        &diagnostic.id,
        "detail",
        "{}",
        &diagnostic.detail,
    );
    let revision = &kept.asset_revisions[0];
    collides(
        "catalog_asset_revisions",
        &revision.id,
        "revision_sha256",
        &"0".repeat(64),
        &revision.revision_sha256,
    );
    let blob = &kept.asset_blobs[0];
    collides(
        "catalog_asset_blobs",
        &blob.id,
        "sha256",
        &"f".repeat(64),
        &blob.sha256,
    );
    let asset = &kept.assets[0];
    collides("catalog_assets", &asset.id, "key", "g-x:Other", &asset.key);
    assert_eq!(read_back(&admin).unwrap(), kept);
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

/// Copies the Legacy Catalog trees the oracle reads into `target`.
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

#[test]
fn substitutes_become_approved_manufacturer_parts_end_to_end() {
    // The checked-in substitutes list has only a header, so a temp copy of
    // the catalog gets synthetic rows; database/ itself is never edited.
    let copy = tempfile::tempdir().unwrap();
    copy_catalog(copy.path());
    let csv_path = copy.path().join("database/g-res.csv");
    let csv = fs::read_to_string(&csv_path).unwrap().replace(
        "\"RES-0000-1500\",\"RC0603FR-07150RL\"",
        "\"RES-0000-1500\",\"\"",
    );
    fs::write(&csv_path, csv).unwrap();
    let substitutes = copy.path().join("database/substitutes.csv");
    let mut text = fs::read_to_string(&substitutes).unwrap();
    text.push_str(concat!(
        "RES-0000-0470,ALT-47,Vishay,https://example.test/47.pdf,DigiKey,541-47-ND\n",
        "RES-0000-0470,RC0603FR-0747RL,Yageo,,,\n",
        "RES-0000-1500,ALT-150,Panasonic,,Mouser,667-150\n",
        "RES-9999-0001,X,Y,,,\n",
    ));
    fs::write(&substitutes, text).unwrap();
    let oracle = legacy_import_fixture::build_manifest(copy.path()).unwrap();

    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    let import = LegacyCatalog::new(copy.path())
        .import("g-res", None)
        .unwrap();
    applied(&submit_import(&admin, &ImportCommand::new(&import, REASON)).unwrap());
    let stored = read_back(&admin).unwrap();
    server.stop();

    let report = catalog_import::report(&oracle, "g-res", &stored, copy.path());
    assert!(
        report.is_lossless(),
        "{}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    // The blank MPN leaves RES-0000-1500 without a primary: both cells are
    // kept by the diagnostic, and its substitute is the first entry.
    assert_eq!(report.destinations["import-diagnostic"], 2);
    let (_, snapshot) = head(&stored, "RES-0000-1500");
    let first = &snapshot.approved_manufacturer_parts[0];
    assert_eq!(
        (first.manufacturer.as_str(), first.mpn.as_str()),
        ("Panasonic", "ALT-150")
    );
    let (_, snapshot) = head(&stored, "RES-0000-0470");
    assert_eq!(snapshot.approved_manufacturer_parts.len(), 2);
    assert_eq!(
        snapshot.approved_manufacturer_parts[1].datasheet.as_deref(),
        Some("https://example.test/47.pdf")
    );
    let substitutes = &report.substitutes;
    assert_eq!((substitutes.rows, substitutes.rows_in_scope), (4, 3));
    assert_eq!(
        substitutes.destinations["approved-manufacturer-part"],
        2 * 4
    );
    assert_eq!(substitutes.destinations["operational-fact"], 2 * 2);
    // The repeated primary is kept whole by a diagnostic.
    assert_eq!(substitutes.destinations["import-diagnostic"], 6);
    // The orphan RES substitute is diagnosed but outside the report's rows.
    assert!(
        report
            .other_diagnostics
            .iter()
            .any(|line| line.starts_with("substitute-without-managed-part"))
    );
}

/// A blob-only import request for the route.
fn blob_request(correlation: &str, sha: &str, bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "correlationId": correlation,
        "reason": "check blob hashing",
        "legacyGrouping": "g-res",
        "sourceCommit": null,
        "sources": [{ "role": "test", "path": "blob", "byteSha256": sha, "size": bytes.len(), "headerRaw": null }],
        "blobs": [{ "sha256": sha, "size": bytes.len(), "base64": catalog::pocketbase::base64(bytes) }],
        "assetRevisions": [],
        "parts": [],
        "diagnostics": [],
    })
}

#[test]
fn the_route_hashes_blob_bytes_exactly() {
    // The route's own SHA-256 and base64 (catalog_lib.js) against Rust's
    // sha2, across SHA-256 padding boundaries and one large blob.
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let sizes = [0usize, 1, 55, 56, 63, 64, 119, 120, 300_000];
    let mut stored_sizes = Vec::new();
    for size in sizes {
        let bytes: Vec<u8> = (0..size).map(|i| ((i * 131 + 7) % 256) as u8).collect();
        let sha = sha256_hex(&bytes);
        // A hash off by one hex digit is refused...
        let mut wrong = sha.clone().into_bytes();
        wrong[63] = if wrong[63] == b'0' { b'1' } else { b'0' };
        let wrong = String::from_utf8(wrong).unwrap();
        let refused = admin
            .post_json(
                LEGACY_IMPORT_ROUTE,
                &blob_request(&format!("wrong-{size}"), &wrong, &bytes),
            )
            .unwrap();
        assert_eq!(refused.status, 400, "{size} bytes: {}", refused.body);
        // ...and the right one is accepted.
        let accepted = admin
            .post_json(
                LEGACY_IMPORT_ROUTE,
                &blob_request(&format!("right-{size}"), &sha, &bytes),
            )
            .unwrap();
        if size == 0 {
            // PocketBase cannot store an empty file; the route must refuse
            // it cleanly rather than store a blob without bytes.
            assert_eq!(accepted.status, 400, "{}", accepted.body);
        } else {
            assert_eq!(accepted.status, 200, "{size} bytes: {}", accepted.body);
            stored_sizes.push(size);
        }
    }
    // read_back re-hashes every downloaded blob and fails on a mismatch.
    let stored = read_back(&admin).unwrap();
    let mut lengths: Vec<usize> = stored
        .asset_blobs
        .iter()
        .map(|blob| blob.bytes.len())
        .collect();
    lengths.sort();
    assert_eq!(lengths, stored_sizes);
    server.stop();
}

#[test]
fn the_catalog_migration_rolls_back_from_a_populated_instance() {
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let options = services::options(&root, state.path());
    let (server, admin, superuser) = services::start(&root, state.path()).unwrap();
    let import = LegacyCatalog::new(&root).import("g-res", None).unwrap();
    applied(&submit_import(&admin, &ImportCommand::new(&import, REASON)).unwrap());
    server.stop();

    let binary = native_services::pocketbase_binary(&root).unwrap();
    let mut command = std::process::Command::new(&binary);
    command
        .args(["migrate", "down", "1"])
        .arg("--dir")
        .arg(&options.data_dir)
        .arg("--migrationsDir")
        .arg(&options.migrations_dir)
        .arg("--hooksDir")
        .arg(&options.hooks_dir)
        .arg("--automigrate=false");
    // It asks for confirmation; without it, it cancels and still exits 0.
    let output = native_services::run_hidden_with_input(
        "pocketbase migrate down",
        &mut command,
        b"y
",
    )
    .unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && said.contains("Reverted 1791500000_catalog_init.js"),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let database = rusqlite::Connection::open_with_flags(
        options.data_dir.join("data.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let names = |sql: &str| -> Vec<String> {
        database
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let collections = names("SELECT name FROM _collections ORDER BY name");
    assert!(
        !collections.iter().any(|name| name.starts_with("catalog_")),
        "{collections:?}"
    );
    assert!(collections.iter().any(|name| name == "collab_records"));
    let tables = names("SELECT name FROM sqlite_master WHERE type = 'table'");
    assert!(
        !tables.iter().any(|name| name.starts_with("catalog_")),
        "{tables:?}"
    );
    let migrations = names("SELECT file FROM _migrations");
    assert!(!migrations.iter().any(|file| file.contains("catalog_init")));
    drop(database);

    // Starting again re-applies it, and the import works on the new schema.
    let server = native_services::PocketBaseServer::start(&binary, &options, &superuser).unwrap();
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    let mut command = ImportCommand::new(&import, REASON);
    command.correlation_id.push_str("-after-rollback");
    assert_eq!(
        applied(&submit_import(&admin, &command).unwrap()).managed_parts,
        29
    );
    server.stop();
}

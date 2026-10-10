//! The resistor CSV file (g-res) imported into a real, disposable PocketBase and
//! proved lossless against the legacy import oracle.
//!
//! Each test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; changed inputs are made in temp copies.

use catalog::pocketbase::{
    CSV_IMPORT_ROUTE, CatalogReadback, EditCommand, ImportCommand, ImportOutcome, ImportReply,
    RestoreCommand, read_back, read_import_state, submit_edit, submit_import, submit_restore,
};
use catalog::{CsvCatalog, PartDetails, sha256_hex};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use std::fs;
use std::path::{Path, PathBuf};

const REASON: &str = "Import the resistors from database/g-res.csv";

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

/// The audit entry and the number of import problems of a run that changed
/// nothing in the catalog.
fn nothing_changed(reply: &catalog::pocketbase::ImportReply) -> (String, u64) {
    match &reply.outcome {
        ImportOutcome::NothingChanged {
            audit_entry_id,
            problems_recorded,
            ..
        } => (audit_entry_id.clone(), *problems_recorded),
        other => panic!("expected nothing changed, got {other:?}: {}", reply.body),
    }
}

fn current_revision(readback: &CatalogReadback, ipn: &str) -> (u32, PartDetails) {
    let part = readback.part(ipn).unwrap();
    let revision = readback.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

/// Imports `import` as an importer does: it reads the team catalog first and
/// expects each part's current revision as read. `suffix` makes the
/// correlation ID unique. (The checkout check is left out: these tests
/// import temp copies, which are not git checkouts.)
fn import_reading_first(
    admin: &PocketBaseClient,
    import: &catalog::CsvImport,
    suffix: &str,
) -> ImportReply {
    let state = read_import_state(admin).unwrap();
    let mut command = ImportCommand::new(import, REASON).expecting(&state);
    command.correlation_id.push_str(suffix);
    submit_import(admin, &command).unwrap()
}

/// The JSON import run that the latest CSV import audit entry records.
fn latest_run_sources(copy: &mut CatalogReadback) -> &mut String {
    &mut copy
        .audit_entries
        .iter_mut()
        .filter(|entry| entry.csv_import_number > 0)
        .max_by_key(|entry| entry.csv_import_number)
        .unwrap()
        .sources
}

/// Changes the latest import run's recorded file `path` with `change`.
fn tamper_run_file(
    copy: &mut CatalogReadback,
    path: &str,
    change: impl Fn(&mut serde_json::Value),
) {
    let sources = latest_run_sources(copy);
    let mut run: serde_json::Value = serde_json::from_str(sources).unwrap();
    for file in run["files"].as_array_mut().unwrap() {
        if file["path"] == path {
            change(file);
        }
    }
    *sources = run.to_string();
}

/// Sets one column of one stored record in a stopped instance's data.db.
/// PocketBase refuses to change Catalog records, so tests that need a
/// tampered record edit the database file directly.
fn tamper(state: &Path, table: &str, column: &str, id: &str, value: &str) {
    let connection = rusqlite::Connection::open(state.join("pb_data").join("data.db")).unwrap();
    let changed = connection
        .execute(
            &format!("UPDATE {table} SET {column} = ?1 WHERE id = ?2"),
            rusqlite::params![value, id],
        )
        .unwrap();
    assert_eq!(changed, 1, "{table}/{id}");
}

/// Stops `server` and starts it again on the same data, signed in as a new
/// superuser.
fn restart(
    root: &Path,
    state: &Path,
    server: PocketBaseServer,
) -> (PocketBaseServer, PocketBaseClient) {
    server.stop();
    let (server, admin, _superuser) = services::start(root, state).unwrap();
    (server, admin)
}

#[test]
fn the_resistor_csv_file_imports_idempotently_and_losslessly() {
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
            "/api/collections/catalog_parts/records",
            &serde_json::json!({ "ipn": "RES-9999-9999" }),
        )
        .unwrap();
    assert!(direct.success().is_err(), "{direct:?}");
    assert_eq!(read_back(&admin).unwrap(), CatalogReadback::default());

    // The first import publishes exactly one reasoned Change.
    let first = submit_import(&admin, &command).unwrap();
    assert_eq!(first.status, 200, "{}", first.body);
    let created = applied(&first);
    assert_eq!(
        (created.parts, created.part_revisions, created.updated_parts),
        (29, 29, 0)
    );
    assert_eq!(
        (created.cad_items, created.cad_item_revisions, created.blobs),
        (2, 2, 2)
    );
    assert_eq!(created.problems, 1);
    let stored = read_back(&admin).unwrap();
    assert_eq!(stored.changes.len(), 1);
    let change = &stored.changes[0];
    assert_eq!(
        (change.seq, change.kind.as_str(), change.reason.as_str()),
        (1, "csv-import", REASON)
    );
    assert_eq!(
        Some(change.source_commit.as_str()),
        import.source_commit.as_deref()
    );
    assert_eq!(stored.audit_entries.len(), 1);
    assert_eq!(stored.audit_entries[0].change, change.id);

    // Members can read what was published.
    let visible: Vec<serde_json::Value> = member.list_records("catalog_parts", "", "ipn").unwrap();
    assert_eq!(visible.len(), 29);
    // ...but cannot create, update or delete any of it directly.
    let part_id = stored.parts[0].id.clone();
    let revision_id = stored.parts[0].current_revision.clone();
    for (collection, id) in [
        ("catalog_parts", &part_id),
        ("catalog_part_revisions", &revision_id),
        ("catalog_changes", &change.id),
        ("catalog_blobs", &stored.blobs[0].id),
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
            (record.ipn_prefix.as_str(), record.variant.as_str()),
            (part.ipn.prefix(), part.ipn.variant())
        );
        assert_eq!(record.current_revision_number, 1);
        let revision = stored.part_revision(&record.current_revision).unwrap();
        assert_eq!(revision.snapshot, part.snapshot_json);
        assert_eq!(revision.predecessor, "");
        assert_eq!(revision.cad_item_revisions.len(), 2);
    }

    // The zero-loss report, read back from PocketBase, against the oracle.
    let report = catalog_import::report(&oracle(), "g-res", &stored, &root);
    assert!(
        report.is_lossless(),
        "{}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    assert_eq!((report.rows, report.columns, report.fields), (29, 16, 464));
    assert_eq!(report.destinations["part-details"], 29 * 7);
    assert_eq!(
        report.destinations["part-details.manufacturer-parts"],
        29 * 2
    );
    assert_eq!(report.destinations["supplier-data"], 29 * 7);
    assert!(!report.destinations.contains_key("import-problem"));
    assert_eq!(report.rebuilt_byte_sha256, report.oracle_byte_sha256);
    assert_eq!(report.cad_items.len(), 2);
    assert_eq!(report.oracle_diagnostics.len(), 1);
    assert!(report.other_problems.is_empty());
    assert_eq!(
        (
            report.substitutes_csv_rows.rows,
            report.substitutes_csv_rows.rows_in_scope
        ),
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
    let current_revision_of = |copy: &mut CatalogReadback, ipn: &str| -> usize {
        let current_revision = copy.part(ipn).unwrap().current_revision.clone();
        copy.part_revisions
            .iter()
            .position(|revision| revision.id == current_revision)
            .unwrap()
    };
    fails(
        &|copy| {
            let index = current_revision_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            revision.snapshot = revision.snapshot.replace("\"47\"", "\"48\"");
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "field Value",
    );
    // Supplier data is checked in the rows the latest import run recorded.
    fails(
        &|copy| {
            tamper_run_file(copy, "database/g-res.csv", |file| {
                for row in file["rows"].as_array_mut().unwrap() {
                    let text = row.as_str().unwrap().replace("\"C23182\"", "\"\"");
                    *row = text.into();
                }
            })
        },
        "field LCSC",
    );
    fails(
        &|copy| {
            let index = current_revision_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            revision.snapshot = revision
                .snapshot
                .replace("{\"name\":\"Tolerance\",\"value\":\"1%\"},", "");
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "field Tolerance",
    );
    fails(
        &|copy| copy.parts.retain(|part| part.ipn != "RES-0000-1000"),
        "RES-0000-1000 is in the latest import run but not a part",
    );
    fails(&|copy| copy.problems.clear(), "no import problem");
    fails(
        &|copy| copy.blobs[0].bytes.push(b' '),
        "stored blob is not the declared bytes",
    );
    fails(&|copy| copy.audit_entries.clear(), "has 0 audit entries");
    // The header line and row bytes the import run recorded must rebuild
    // the file exactly.
    fails(
        &|copy| {
            tamper_run_file(copy, "database/g-res.csv", |file| {
                let header = format!("{} ", file["header_raw"].as_str().unwrap());
                file["header_raw"] = header.into();
            })
        },
        "rebuilt from PocketBase",
    );
    fails(
        &|copy| {
            tamper_run_file(copy, "database/g-res.csv", |file| {
                let row = file["rows"][3].as_str().unwrap().replace('\n', "\r\n");
                file["rows"][3] = row.into();
            })
        },
        "rebuilt from PocketBase",
    );
    // A run that records no files cannot be checked.
    fails(
        &|copy| *latest_run_sources(copy) = String::new(),
        "does not record the files it read",
    );
    // A symbol blob must be the whole symbol, not a prefix of it.
    fails(
        &|copy| {
            let revision = copy
                .cad_item_revisions
                .iter_mut()
                .find(|revision| revision.snapshot.contains("\"kind\":\"symbol\""))
                .unwrap();
            let mut source: serde_json::Value = serde_json::from_str(&revision.source).unwrap();
            let end = source["byte_range"][1].as_u64().unwrap();
            source["byte_range"][1] = (end - 1).into();
            revision.source = source.to_string();
            let blob_id = revision.blob.clone();
            let blob = copy.blobs.iter_mut().find(|b| b.id == blob_id).unwrap();
            blob.bytes.pop();
        },
        "are not the whole symbol",
    );
    fails(
        &|copy| copy.cad_item_revisions[0].license = "MIT".into(),
        "license is not explicit",
    );
    // A part bound to a different CAD item revision than its peers.
    fails(
        &|copy| {
            let index = current_revision_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            let mut snapshot: serde_json::Value = serde_json::from_str(&revision.snapshot).unwrap();
            snapshot["cad_references"][1]["target"]["snapshot_sha256"] = "0".repeat(64).into();
            revision.snapshot =
                serde_json::to_string(&serde_json::from_value::<PartDetails>(snapshot).unwrap())
                    .unwrap();
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "binds 2 different targets",
    );
    // A field in the snapshot that no source column put there.
    fails(
        &|copy| {
            let index = current_revision_of(copy, "RES-0000-0470");
            let revision = &mut copy.part_revisions[index];
            revision.snapshot = revision.snapshot.replace(
                "\"fields\":[",
                "\"fields\":[{\"name\":\"Extra\",\"value\":\"x\"},",
            );
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "are not the part-details columns",
    );

    // Repeating the command replays its stored answer and writes nothing.
    let replay = submit_import(&admin, &command).unwrap();
    assert!(replay.replayed, "{} {}", replay.status, replay.body);
    assert_eq!((replay.status, &replay.body), (200, &first.body));
    assert_eq!(read_back(&admin).unwrap(), stored);

    // A fresh import of the unchanged inputs is a no-op: no Change,
    // revision, CAD item or blob.
    let again = services::plan(&root, "g-res").unwrap();
    let mut retry = services::prepare_import(&admin, &root, &again, REASON, false).unwrap();
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
fn changed_rows_get_new_revisions_or_import_problems_and_bad_requests_write_nothing() {
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let original = CsvCatalog::new(&root).import("g-res", None).unwrap();
    applied(&submit_import(&admin, &ImportCommand::new(&original, REASON)).unwrap());
    let initial = read_back(&admin).unwrap();

    // Tampered requests are refused before anything is written.
    let command = ImportCommand::new(&original, REASON);
    let mut body = command.body();
    body["correlationId"] = "tampered-snapshot".into();
    body["parts"][0]["snapshotSha256"] = sha256_hex("not the snapshot").into();
    let reply = admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap();
    assert_eq!(reply.status, 400, "{}", reply.body);
    let mut body = command.body();
    body["correlationId"] = "tampered-blob".into();
    body["blobs"][0]["base64"] = catalog::pocketbase::base64(b"other bytes").into();
    assert_eq!(
        admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap().status,
        400
    );
    let mut body = command.body();
    body["correlationId"] = "tampered-id".into();
    body["parts"][0]["id"] = "aaaaaaaaaaaaaaa".into();
    assert_eq!(
        admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap().status,
        400
    );
    let mut body = command.body();
    body["correlationId"] = "no-reason".into();
    body["reason"] = "   ".into();
    assert_eq!(
        admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap().status,
        400
    );
    // The rows a run records must be exactly the file with its byte hash,
    // and each part needs the current revision it expects.
    let refused = |name: &str, change: &dyn Fn(&mut serde_json::Value), says: &str| {
        let mut body = command.body();
        body["correlationId"] = name.into();
        change(&mut body);
        let reply = admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap();
        assert_eq!(reply.status, 400, "{name}: {}", reply.body);
        assert!(reply.body.contains(says), "{name}: {}", reply.body);
    };
    let csv_source = |body: &mut serde_json::Value| -> usize {
        body["sources"]
            .as_array()
            .unwrap()
            .iter()
            .position(|source| source["role"] == "catalog-csv")
            .unwrap()
    };
    refused(
        "tampered-row",
        &|body| {
            let index = csv_source(body);
            body["sources"][index]["rows"][0] = "\"RES-0000-0000\"\n".into();
        },
        "are not the file with its byte hash",
    );
    refused(
        "dropped-rows",
        &|body| {
            let index = csv_source(body);
            body["sources"][index]
                .as_object_mut()
                .unwrap()
                .remove("rows");
        },
        "must be a list of text",
    );
    refused(
        "rows-of-a-footprint",
        &|body| {
            let index = body["sources"]
                .as_array()
                .unwrap()
                .iter()
                .position(|source| source["role"] == "footprint")
                .unwrap();
            body["sources"][index]["rows"] = serde_json::json!([]);
        },
        "kept only for CSV files",
    );
    refused(
        "no-expected-revision",
        &|body| {
            body["parts"][0]
                .as_object_mut()
                .unwrap()
                .remove("expectedRevision");
        },
        "expected revision of RES-0000-0000",
    );
    refused(
        "allow-as-text",
        &|body| body["allowOlderCheckout"] = "yes".into(),
        "allowOlderCheckout setting must be true or false",
    );
    assert_eq!(read_back(&admin).unwrap(), initial);

    // A changed part detail makes a new revision in a new Change; nothing
    // else is touched.
    let copy = tempfile::tempdir().unwrap();
    copy_inputs(copy.path());
    let csv_path = copy.path().join("database/g-res.csv");
    let csv = fs::read_to_string(&csv_path).unwrap().replace(
        "\"RES 47 OHM 1% 1/10W 0603\",\"47\"",
        "\"RES 47 OHM 1% 1/10W 0603\",\"47R\"",
    );
    fs::write(&csv_path, &csv).unwrap();
    let changed = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
    let reply = import_reading_first(&admin, &changed, "");
    let created = applied(&reply);
    assert_eq!(
        (created.parts, created.part_revisions, created.updated_parts),
        (0, 1, 1)
    );
    assert_eq!(
        (created.cad_items, created.cad_item_revisions, created.blobs),
        (0, 0, 0)
    );
    assert_eq!(created.problems, 0);
    let after = read_back(&admin).unwrap();
    assert_eq!(after.changes.len(), 2);
    assert_eq!(after.changes[1].seq, 2);
    let (number, details) = current_revision(&after, "RES-0000-0470");
    assert_eq!((number, details.field("Value")), (2, Some("47R")));
    let newest = after
        .part_revision(&after.part("RES-0000-0470").unwrap().current_revision)
        .unwrap();
    let previous = after.part_revision(&newest.predecessor).unwrap();
    // The first revision never changes.
    assert_eq!(
        Some(previous),
        initial.part_revision(&initial.part("RES-0000-0470").unwrap().current_revision)
    );

    // A re-import that only changes supplier data (here an LCSC code) makes
    // no Change. The new row is kept as an import problem, recorded against
    // the audit entry of that run.
    let csv = csv.replace("\"C22775\"", "\"C99999\"");
    fs::write(&csv_path, &csv).unwrap();
    let supplier_only = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
    let reply = import_reading_first(&admin, &supplier_only, "");
    let (audit_entry_id, recorded) = nothing_changed(&reply);
    assert_eq!(recorded, 1);
    let supplier = read_back(&admin).unwrap();
    assert_eq!(supplier.changes, after.changes);
    assert_eq!(supplier.part_revisions, after.part_revisions);
    assert_eq!(supplier.audit_entries.len(), after.audit_entries.len() + 1);
    let entry = supplier
        .audit_entries
        .iter()
        .find(|entry| entry.id == audit_entry_id)
        .unwrap();
    assert_eq!(entry.change, "");
    let kept = supplier
        .problems
        .iter()
        .find(|problem| problem.code == "csv-row-changed-without-part-details-change")
        .unwrap();
    assert_eq!(
        (kept.subject.as_str(), kept.audit_entry.as_str()),
        ("RES-0000-1000", audit_entry_id.as_str())
    );
    assert!(kept.detail.contains("C99999"), "{}", kept.detail);
    assert_eq!(current_revision(&supplier, "RES-0000-1000").0, 1);

    // The same holds for a substitutes.csv row: changing only its SupplierPN
    // makes no Change, and the new row is kept in an import problem.
    let substitutes_csv = copy.path().join("database/substitutes.csv");
    let header = fs::read_to_string(&substitutes_csv).unwrap();
    fs::write(
        &substitutes_csv,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-47-ND\n"),
    )
    .unwrap();
    let with_row = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
    let reply = import_reading_first(&admin, &with_row, "");
    assert_eq!(applied(&reply).updated_parts, 1);
    fs::write(
        &substitutes_csv,
        format!("{header}RES-0000-1002,ALT-10K,Vishay,,DigiKey,541-47-CHANGED\n"),
    )
    .unwrap();
    let supplier_pn_only = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
    let reply = import_reading_first(&admin, &supplier_pn_only, "");
    let (audit_entry_id, recorded) = nothing_changed(&reply);
    assert_eq!(recorded, 1);
    let supplier = read_back(&admin).unwrap();
    let kept = supplier
        .problems
        .iter()
        .find(|problem| {
            problem.code == "csv-row-changed-without-part-details-change"
                && problem.subject == "RES-0000-1002"
        })
        .unwrap();
    assert_eq!(kept.audit_entry, audit_entry_id);
    assert!(
        kept.detail.contains("541-47-CHANGED") && kept.detail.contains("541-47-ND"),
        "{}",
        kept.detail
    );
    assert_eq!(current_revision(&supplier, "RES-0000-1002").0, 2);
    let changes_before_edit = supplier.changes.len();

    // A current revision published by an edit is never overwritten by a
    // CSV import: the changed row becomes an import problem instead.
    let edit = {
        let (number, details) = current_revision(&supplier, "RES-0000-0470");
        let mut command = EditCommand::starting_from(&details, number, "Note the exact value");
        command.set_field("Value", "47 Ohm");
        command
    };
    let edited = submit_edit(&admin, &edit).unwrap();
    assert_eq!(edited.status, 200, "{}", edited.body);
    let after_edit = read_back(&admin).unwrap();
    assert_eq!(after_edit.changes.len(), changes_before_edit + 1);
    assert_eq!(after_edit.changes.last().unwrap().kind, "edit");
    let csv = csv.replace("\"47R\"", "\"47.0\"");
    fs::write(&csv_path, &csv).unwrap();
    let conflicting = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
    let reply = import_reading_first(&admin, &conflicting, "");
    assert_eq!(nothing_changed(&reply).1, 1, "{}", reply.body);
    let last = read_back(&admin).unwrap();
    assert_eq!(last.changes, after_edit.changes);
    assert_eq!(
        current_revision(&last, "RES-0000-0470"),
        current_revision(&after_edit, "RES-0000-0470")
    );
    assert_eq!(
        current_revision(&last, "RES-0000-0470").1.field("Value"),
        Some("47 Ohm")
    );
    assert!(last.problems.iter().any(|problem| problem.code
        == "csv-row-conflicts-with-current-revision"
        && problem.subject == "RES-0000-0470"));

    // Nobody can relabel a Change, not even a superuser.
    let relabel = admin
        .patch_json(
            &format!(
                "/api/collections/catalog_changes/records/{}",
                after.changes[1].id
            ),
            &serde_json::json!({ "kind": "edit" }),
        )
        .unwrap();
    assert_eq!(relabel.status, 403, "{}", relabel.body);

    // A current revision whose details match the row but whose source is
    // not a CSV row (here a restore of revision 1 after an edit) is answered
    // with an import problem that keeps the row, not a server error.
    let edit = {
        let (number, details) = current_revision(&last, "RES-0000-1001");
        let mut command = EditCommand::starting_from(&details, number, "Try another value");
        command.set_field("Value", "1.0k");
        command
    };
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    let restore = RestoreCommand::new("RES-0000-1001", 2, 1, "Back to the imported details");
    let restored = submit_restore(&admin, &restore).unwrap();
    assert_eq!(restored.status, 200, "{}", restored.body);
    let reply = import_reading_first(&admin, &conflicting, "-source");
    assert_eq!(nothing_changed(&reply).1, 1, "{}", reply.body);
    let kept = read_back(&admin).unwrap();
    let problem = kept
        .problems
        .iter()
        .find(|problem| problem.code == "current-revision-without-csv-source")
        .unwrap();
    assert_eq!(problem.subject, "RES-0000-1001");
    assert!(problem.detail.contains("raw_record"), "{}", problem.detail);
    assert_eq!(current_revision(&kept, "RES-0000-1001").0, 3);

    // A stored record whose derived ID matches but whose content does not
    // is an ID collision: refused, with nothing written. Stored records
    // cannot be changed through PocketBase, so each case edits the stopped
    // instance's data.db directly, then puts the value back.
    let mut running = Some((server, admin));
    let mut collides = |table: &str, id: &str, column: &str, value: &str, original: &str| {
        tamper(state.path(), table, column, id, value);
        let (server, _) = running.take().unwrap();
        let (server, admin) = restart(&root, state.path(), server);
        // (read_back itself refuses a tampered blob, so count Changes.)
        let changes = || {
            admin
                .list_records::<serde_json::Value>("catalog_changes", "", "seq")
                .unwrap()
                .len()
        };
        let before = changes();
        let reply = import_reading_first(&admin, &conflicting, &format!("-collision-{table}"));
        assert_eq!(reply.status, 409, "{}", reply.body);
        let ImportOutcome::IdCollision { collisions, .. } = &reply.outcome else {
            panic!("expected an ID collision, got {}", reply.body);
        };
        assert!(
            collisions
                .iter()
                .any(|c| c.contains(table) && c.contains(id)),
            "{collisions:?}"
        );
        assert_eq!(changes(), before);
        tamper(state.path(), table, column, id, original);
        running = Some(restart(&root, state.path(), server));
    };
    let problem = &kept.problems[0];
    collides(
        "catalog_import_problems",
        &problem.id,
        "detail",
        "{}",
        &problem.detail,
    );
    let revision = &kept.cad_item_revisions[0];
    collides(
        "catalog_cad_item_revisions",
        &revision.id,
        "snapshot_sha256",
        &"0".repeat(64),
        &revision.snapshot_sha256,
    );
    let blob = &kept.blobs[0];
    collides(
        "catalog_blobs",
        &blob.id,
        "sha256",
        &"f".repeat(64),
        &blob.sha256,
    );
    let cad_item = &kept.cad_items[0];
    collides(
        "catalog_cad_items",
        &cad_item.id,
        "key",
        "g-x:Other",
        &cad_item.key,
    );
    let (server, admin) = running.take().unwrap();
    assert_eq!(read_back(&admin).unwrap(), kept);
    server.stop();
}

#[test]
fn the_oracle_still_matches_the_checked_in_catalog() {
    // The import must leave every CSV catalog input byte-for-byte as
    // the oracle recorded it.
    let root = repository();
    let actual = legacy_import_fixture::build_manifest(&root).unwrap();
    assert!(legacy_import_fixture::compare(&oracle(), &actual).is_empty());
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

#[test]
fn substitutes_csv_rows_become_manufacturer_parts_end_to_end() {
    // The checked-in substitutes.csv has only a header, so a temp copy of
    // the catalog gets synthetic rows; database/ itself is never edited.
    let copy = tempfile::tempdir().unwrap();
    copy_catalog(copy.path());
    let csv_path = copy.path().join("database/g-res.csv");
    let csv = fs::read_to_string(&csv_path).unwrap().replace(
        "\"RES-0000-1500\",\"RC0603FR-07150RL\"",
        "\"RES-0000-1500\",\"\"",
    );
    fs::write(&csv_path, csv).unwrap();
    let substitutes_csv_rows = copy.path().join("database/substitutes.csv");
    let mut text = fs::read_to_string(&substitutes_csv_rows).unwrap();
    text.push_str(concat!(
        "RES-0000-0470,ALT-47,Vishay,https://example.test/47.pdf,DigiKey,541-47-ND\n",
        "RES-0000-0470,RC0603FR-0747RL,Yageo,,,\n",
        "RES-0000-1500,ALT-150,Panasonic,,Mouser,667-150\n",
        "RES-0000-0470,ALT2-47,Bourns,https://www.yageogroup.com/content/datasheet/asset/file/PYU-RC_GROUP_51_ROHS_L,,\n",
        "RES-9999-0001,X,Y,,,\n",
    ));
    fs::write(&substitutes_csv_rows, text).unwrap();
    let oracle = legacy_import_fixture::build_manifest(copy.path()).unwrap();

    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    let import = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
    applied(&submit_import(&admin, &ImportCommand::new(&import, REASON)).unwrap());
    let stored = read_back(&admin).unwrap();
    server.stop();

    let report = catalog_import::report(&oracle, "g-res", &stored, copy.path());
    assert!(
        report.is_lossless(),
        "{}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    // The blank MPN leaves RES-0000-1500 without a main: both cells are
    // kept by the problem, and its substitutes.csv row is the first entry.
    assert_eq!(report.destinations["import-problem"], 2);
    let (_, snapshot) = current_revision(&stored, "RES-0000-1500");
    let first = &snapshot.manufacturer_parts[0];
    assert_eq!(
        (first.manufacturer.as_str(), first.mpn.as_str()),
        ("Panasonic", "ALT-150")
    );
    let (_, details) = current_revision(&stored, "RES-0000-0470");
    let mpns: Vec<&str> = details
        .manufacturer_parts
        .iter()
        .map(|part| part.mpn.as_str())
        .collect();
    assert_eq!(mpns, ["RC0603FR-0747RL", "ALT-47", "ALT2-47"]);
    // A part has one datasheet: Vishay's different one is an import problem.
    assert!(stored.problems.iter().any(|problem| problem.code
        == "substitutes-csv-datasheet-differs"
        && problem.detail.contains("https://example.test/47.pdf")));
    let substitutes_csv_rows = &report.substitutes_csv_rows;
    assert_eq!(
        (
            substitutes_csv_rows.rows,
            substitutes_csv_rows.rows_in_scope
        ),
        (5, 4)
    );
    // IPN, MPN and Manufacturer of the three accepted rows.
    assert_eq!(
        substitutes_csv_rows.destinations["part-details.manufacturer-parts"],
        3 * 3
    );
    assert_eq!(substitutes_csv_rows.destinations["supplier-data"], 3 * 2);
    // The blank datasheet and the part's own add nothing to the details.
    assert_eq!(substitutes_csv_rows.destinations["part-details"], 2);
    // The repeated main part (6 fields) and the different datasheet.
    assert_eq!(substitutes_csv_rows.destinations["import-problem"], 6 + 1);
    // The orphan RES row of substitutes.csv is an import problem outside the
    // report's rows.
    assert!(
        report
            .other_problems
            .iter()
            .any(|line| line.starts_with("substitutes-csv-row-without-part"))
    );
}

#[test]
fn adding_or_removing_a_manufacturer_part_makes_a_new_revision() {
    // Manufacturer parts are part details, so adding or removing a
    // substitutes.csv row on re-import makes a new revision.
    let copy = tempfile::tempdir().unwrap();
    copy_inputs(copy.path());
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    let submit = |correlation: &str| {
        let import = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
        import_reading_first(&admin, &import, correlation)
    };
    applied(&submit("-first"));

    let substitutes_csv_rows = copy.path().join("database/substitutes.csv");
    let header = fs::read_to_string(&substitutes_csv_rows).unwrap();
    fs::write(
        &substitutes_csv_rows,
        format!("{header}RES-0000-0470,ALT-47,Vishay,,DigiKey,541-47-ND\n"),
    )
    .unwrap();
    let created = applied(&submit("-added"));
    assert_eq!((created.part_revisions, created.updated_parts), (1, 1));
    let stored = read_back(&admin).unwrap();
    let (number, details) = current_revision(&stored, "RES-0000-0470");
    assert_eq!(number, 2);
    assert_eq!(details.manufacturer_parts[1].mpn, "ALT-47");
    assert_eq!(stored.changes.len(), 2);

    fs::write(&substitutes_csv_rows, &header).unwrap();
    applied(&submit("-removed"));
    let (number, details) = current_revision(&read_back(&admin).unwrap(), "RES-0000-0470");
    assert_eq!((number, details.manufacturer_parts.len()), (3, 1));
    server.stop();
}

/// An import request carrying one 3D model CAD item revision whose blob is
/// `bytes`, hashed as `sha`.
fn blob_request(correlation: &str, sha: &str, bytes: &[u8]) -> serde_json::Value {
    let cad_item = catalog::CadItem::new(
        catalog::CadItemKind::Model3d,
        &format!("3d-models/{correlation}.step"),
    );
    let details = catalog::CadItemDetails {
        schema: "kicad-parts/cad-item-details/1".into(),
        kind: cad_item.kind,
        key: cad_item.key.clone(),
        format: "3d-model".into(),
        format_version: None,
        blob_sha256: sha.into(),
        blob_size: bytes.len() as u64,
        model_references: vec![],
        license: catalog::License::UnknownNeedsReview,
    };
    let snapshot = details.canonical_json();
    let snapshot_sha256 = sha256_hex(&snapshot);
    let id = catalog::CadItemRevisionId::new(&cad_item.id, &snapshot_sha256);
    serde_json::json!({
        "correlationId": correlation,
        "reason": "check blob hashing",
        "csvFile": "g-res",
        "sourceCommit": null,
        "sources": [{ "role": "test", "path": "blob", "byteSha256": sha, "size": bytes.len(), "headerRaw": null }],
        "blobs": [{ "sha256": sha, "size": bytes.len(), "base64": catalog::pocketbase::base64(bytes) }],
        "cadItemRevisions": [{
            "id": id,
            "cadItemId": cad_item.id,
            "kind": "model-3d",
            "key": cad_item.key,
            "snapshotSha256": snapshot_sha256,
            "snapshot": snapshot,
            "source": "{\"schema\":\"kicad-parts/source/1\",\"source\":\"test\"}",
        }],
        "parts": [],
        "problems": [],
    })
}

/// The old blob-only request shape, kept to show it is refused.
fn unused_blob_request(correlation: &str, sha: &str, bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "correlationId": correlation,
        "reason": "check blob hashing",
        "csvFile": "g-res",
        "sourceCommit": null,
        "sources": [{ "role": "test", "path": "blob", "byteSha256": sha, "size": bytes.len(), "headerRaw": null }],
        "blobs": [{ "sha256": sha, "size": bytes.len(), "base64": catalog::pocketbase::base64(bytes) }],
        "cadItemRevisions": [],
        "parts": [],
        "problems": [],
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
                CSV_IMPORT_ROUTE,
                &blob_request(&format!("wrong-{size}"), &wrong, &bytes),
            )
            .unwrap();
        assert_eq!(refused.status, 400, "{size} bytes: {}", refused.body);
        // ...and the right one is accepted.
        let accepted = admin
            .post_json(
                CSV_IMPORT_ROUTE,
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
    // A blob that no CAD item revision uses is refused: it could belong to
    // no Change.
    let bytes = b"unused".to_vec();
    let unused = admin
        .post_json(
            CSV_IMPORT_ROUTE,
            &unused_blob_request("unused", &sha256_hex(&bytes), &bytes),
        )
        .unwrap();
    assert_eq!(unused.status, 400, "{}", unused.body);
    // read_back re-hashes every downloaded blob and fails on a mismatch.
    let stored = read_back(&admin).unwrap();
    let mut lengths: Vec<usize> = stored.blobs.iter().map(|blob| blob.bytes.len()).collect();
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
    let import = CsvCatalog::new(&root).import("g-res", None).unwrap();
    applied(&submit_import(&admin, &ImportCommand::new(&import, REASON)).unwrap());
    // Edits and a restore populate the edit migration's fields too.
    let stored = read_back(&admin).unwrap();
    let (number, details) = current_revision(&stored, "RES-0000-0470");
    let mut edit = EditCommand::starting_from(&details, number, "Before the rollback");
    edit.set_field("Value", "47R");
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    let restore = RestoreCommand::new("RES-0000-0470", 2, 1, "Before the rollback");
    assert_eq!(submit_restore(&admin, &restore).unwrap().status, 200);
    let users_rule = |data_dir: &Path| -> (String, String) {
        let database = rusqlite::Connection::open_with_flags(
            data_dir.join("data.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        database
            .query_row(
                "SELECT updateRule, fields FROM _collections WHERE name = 'users'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    };
    server.stop();
    let (rule, fields) = users_rule(&options.data_dir);
    assert_eq!(
        rule,
        "(id = @request.auth.id) && @request.body.can_edit_catalog:isset = false && @request.body.name:isset = false"
    );
    assert!(fields.contains("can_edit_catalog"));

    let binary = native_services::pocketbase_binary(&root).unwrap();
    let mut command = std::process::Command::new(&binary);
    command
        .args(["migrate", "down", "2"])
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
        output.status.success()
            && said.contains("Reverted 1791600000_catalog_edits.js")
            && said.contains("Reverted 1791500000_catalog_init.js"),
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
    assert!(
        !migrations
            .iter()
            .any(|file| file.contains("catalog_init") || file.contains("catalog_edits"))
    );
    drop(database);
    // The users collection is back as it was: no edit mark, the old rule.
    let (rule, fields) = users_rule(&options.data_dir);
    assert_eq!(rule, "id = @request.auth.id");
    assert!(!fields.contains("can_edit_catalog"), "{fields}");

    // Starting again re-applies both, and the import works on the new schema.
    let server = native_services::PocketBaseServer::start(&binary, &options, &superuser).unwrap();
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    let mut command = ImportCommand::new(&import, REASON);
    command.correlation_id.push_str("-after-rollback");
    assert_eq!(applied(&submit_import(&admin, &command).unwrap()).parts, 29);
    server.stop();
}

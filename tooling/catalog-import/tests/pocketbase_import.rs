//! The resistor CSV file (g-res) imported into a real, disposable PocketBase and
//! proved lossless against the CSV catalog fixture.
//!
//! Each test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; changed inputs are made in temp copies.

use catalog::pocketbase::{
    CSV_IMPORT_ROUTE, CatalogReadback, EditCommand, ImportCommand, ImportOutcome, ImportReply,
    read_back, read_import_state, submit_edit, submit_import,
};
use catalog::{CsvCatalog, PartDetails, PartRevision, sha256_hex};
use catalog_import::services::{self, Allow};
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use std::fs;
use std::path::{Path, PathBuf};

const REASON: &str = "Import the resistors from database/g-res.csv";

/// Every file the resistor import reads.
const INPUTS: [&str; 4] = [
    "database/g-res.csv",
    "database/substitutes.csv",
    "symbols/g-res.kicad_sym",
    "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod",
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

fn oracle() -> csv_catalog_fixture::Manifest {
    csv_catalog_fixture::load_manifest(
        &repository().join("fixtures/import/csv-catalog/manifest.json"),
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

fn current_revision(readback: &CatalogReadback, ipn: &str) -> (u32, PartRevision) {
    let part = readback.part(ipn).unwrap();
    let revision = readback.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

fn current_details(readback: &CatalogReadback, ipn: &str) -> (u32, PartDetails) {
    let part = readback.part(ipn).unwrap();
    let details = readback.part_details(&part.current_details).unwrap();
    (
        details.number,
        serde_json::from_str(&details.snapshot).unwrap(),
    )
}

/// A details edit of `ipn` as it is now that sets `field` to `value`.
fn details_edit(
    readback: &CatalogReadback,
    ipn: &str,
    field: &str,
    value: &str,
    reason: &str,
) -> EditCommand {
    let (number, details) = current_details(readback, ipn);
    let mut command = EditCommand::new(ipn, reason).editing_details(&details, number);
    assert!(command.set_field(field, value));
    command
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
    let command = ImportCommand::new(&import, REASON)
        .allowing_uncommitted_changes(services::has_uncommitted_changes(&import));

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
    assert_eq!((created.part_details, created.updated_details), (29, 0));
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
    let details_id = stored.parts[0].current_details.clone();
    for (collection, id) in [
        ("catalog_parts", &part_id),
        ("catalog_part_revisions", &revision_id),
        ("catalog_part_details", &details_id),
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
    // derives, and every part has exactly details 1 and revision 1.
    for part in &import.parts {
        let record = stored.part(part.ipn.as_str()).unwrap();
        assert_eq!(record.id, part.id.as_str());
        assert_eq!(
            (record.ipn_prefix.as_str(), record.variant.as_str()),
            (part.ipn.prefix(), part.ipn.variant())
        );
        assert_eq!(
            (
                record.current_revision_number,
                record.current_details_number
            ),
            (1, 1)
        );
        let revision = stored.part_revision(&record.current_revision).unwrap();
        assert_eq!(revision.snapshot, part.revision_json);
        assert_eq!(revision.predecessor, "");
        assert_eq!(revision.cad_item_revisions.len(), 2);
        let details = stored.part_details(&record.current_details).unwrap();
        assert_eq!(details.snapshot, part.details_json);
        assert_eq!(
            (details.predecessor.as_str(), details.source.as_str()),
            ("", revision.source.as_str())
        );
    }

    // The zero-loss report, read back from PocketBase, against the oracle.
    let report = catalog_import::report(&oracle(), "g-res", &stored, &root);
    assert!(
        report.is_lossless(),
        "{}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    assert_eq!((report.rows, report.columns, report.fields), (29, 16, 464));
    // Symbol and Footprint go to the revision; the other details columns
    // (IPN, Description, Value, Tolerance, Datasheet) to the part details.
    assert_eq!(report.destinations["part-details"], 29 * 5);
    assert_eq!(report.destinations["part-revision"], 29 * 2);
    assert_eq!(
        report.destinations["part-details.manufacturer-parts"],
        29 * 2
    );
    assert_eq!(report.destinations["supplier-data"], 29 * 7);
    assert!(!report.destinations.contains_key("import-problem"));
    assert_eq!(report.rebuilt_byte_sha256, report.oracle_byte_sha256);
    assert_eq!(report.cad_items.len(), 2);
    assert_eq!(report.oracle_import_problems.len(), 1);
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
    let current_details_of = |copy: &mut CatalogReadback, ipn: &str| -> usize {
        let current_details = copy.part(ipn).unwrap().current_details.clone();
        copy.part_details
            .iter()
            .position(|details| details.id == current_details)
            .unwrap()
    };
    fails(
        &|copy| {
            let index = current_details_of(copy, "RES-0000-0470");
            let details = &mut copy.part_details[index];
            details.snapshot = details.snapshot.replace("\"47\"", "\"48\"");
            details.snapshot_sha256 = sha256_hex(&details.snapshot);
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
            let index = current_details_of(copy, "RES-0000-0470");
            let details = &mut copy.part_details[index];
            details.snapshot = details
                .snapshot
                .replace("{\"name\":\"Tolerance\",\"value\":\"1%\"},", "");
            details.snapshot_sha256 = sha256_hex(&details.snapshot);
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
        "the stored file is not the bytes its revision names",
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
        "as the team catalog recorded it",
    );
    fails(
        &|copy| {
            tamper_run_file(copy, "database/g-res.csv", |file| {
                let row = file["rows"][3].as_str().unwrap().replace('\n', "\r\n");
                file["rows"][3] = row.into();
            })
        },
        "as the team catalog recorded it",
    );
    // Rows split differently from the file are caught even though they
    // rebuild the same bytes.
    fails(
        &|copy| {
            tamper_run_file(copy, "database/g-res.csv", |file| {
                let rows = file["rows"].as_array_mut().unwrap();
                rows.push("".into());
                rows.push("".into());
            })
        },
        "recorded 31 rows of database/g-res.csv, but the file it rebuilds has 29",
    );
    // The short list of file hashes must match the files the run records.
    fails(
        &|copy| {
            copy.audit_entries
                .iter_mut()
                .find(|entry| entry.csv_import_number > 0)
                .unwrap()
                .file_hashes = "[]".into();
        },
        "do not match the files it records",
    );
    // A symbol source naming today's file must name the symbol's exact
    // range in it.
    fails(
        &|copy| {
            let revision = copy
                .cad_item_revisions
                .iter_mut()
                .find(|revision| revision.snapshot.contains("\"kind\":\"symbol\""))
                .unwrap();
            let mut source: serde_json::Value = serde_json::from_str(&revision.source).unwrap();
            let start = source["byte_range"][0].as_u64().unwrap();
            source["byte_range"][0] = (start + 1).into();
            revision.source = source.to_string();
        },
        "are not the whole symbol",
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
        "is not the whole symbol",
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
                serde_json::to_string(&serde_json::from_value::<PartRevision>(snapshot).unwrap())
                    .unwrap();
            revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
        },
        "binds 2 different targets",
    );
    // A field in the snapshot that no source column put there.
    fails(
        &|copy| {
            let index = current_details_of(copy, "RES-0000-0470");
            let details = &mut copy.part_details[index];
            details.snapshot = details.snapshot.replace(
                "\"fields\":[",
                "\"fields\":[{\"name\":\"Extra\",\"value\":\"x\"},",
            );
            details.snapshot_sha256 = sha256_hex(&details.snapshot);
        },
        "are not the part-details columns",
    );

    // Repeating the command replays its stored answer and writes nothing.
    let replay = submit_import(&admin, &command).unwrap();
    assert!(replay.replayed, "{} {}", replay.status, replay.body);
    assert_eq!((replay.status, &replay.body), (200, &first.body));
    assert_eq!(read_back(&admin).unwrap(), stored);

    // A fresh import of the unchanged inputs, prepared as an importer does
    // and with its own correlation ID, is a no-op: no Change, revision, CAD
    // item or blob. Its ID differs from the first import's because it
    // expects the parts that import made.
    // Uncommitted changes in this checkout are allowed: the commit rule has
    // its own tests in temporary repositories.
    let allow = Allow {
        uncommitted_changes: true,
        ..Allow::default()
    };
    let again = services::plan(&root, "g-res").unwrap();
    let retry = services::prepare_import(&admin, &root, &again, REASON, allow).unwrap();
    assert_ne!(retry.correlation_id, command.correlation_id);
    let noop = submit_import(&admin, &retry).unwrap();
    assert_eq!(noop.status, 422, "{}", noop.body);
    assert!(matches!(noop.outcome, ImportOutcome::Noop { .. }));
    assert_eq!(read_back(&admin).unwrap(), stored);
    // Preparing and sending it once more replays that answer.
    let rerun = services::prepare_import(&admin, &root, &again, REASON, allow).unwrap();
    assert_eq!(rerun.correlation_id, retry.correlation_id);
    let replayed = submit_import(&admin, &rerun).unwrap();
    assert_eq!(
        (replayed.status, replayed.replayed, &replayed.body),
        (422, true, &noop.body)
    );

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
    body["correlationId"] = "tampered-details".into();
    body["parts"][0]["details"]["snapshotSha256"] = sha256_hex("not the snapshot").into();
    let reply = admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap();
    assert_eq!(reply.status, 400, "{}", reply.body);
    let mut body = command.body();
    body["correlationId"] = "tampered-revision".into();
    body["parts"][0]["revision"]["snapshotSha256"] = sha256_hex("not the snapshot").into();
    let reply = admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap();
    assert_eq!(reply.status, 400, "{}", reply.body);
    // Each document must be what it says: details are never a revision.
    let mut body = command.body();
    body["correlationId"] = "swapped-documents".into();
    let details = body["parts"][0]["details"].clone();
    body["parts"][0]["revision"] = details;
    let reply = admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap();
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(
        reply.body.contains("must be kicad-parts/part-revision/1"),
        "{}",
        reply.body
    );
    let mut body = command.body();
    body["correlationId"] = "tampered-blob".into();
    body["blobs"][0]["text"] = "other bytes".into();
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
        "empty-rows",
        &|body| {
            let index = csv_source(body);
            let rows = body["sources"][index]["rows"].as_array_mut().unwrap();
            rows.push("".into());
            rows.push("".into());
        },
        "include an empty row",
    );
    refused(
        "not-a-row",
        &|body| {
            let mut source: serde_json::Value =
                serde_json::from_str(body["parts"][0]["source"].as_str().unwrap()).unwrap();
            source["raw_record"] = "\"RES-0000-0000\"\n".into();
            body["parts"][0]["source"] = source.to_string().into();
        },
        "is not a row of the catalog-csv file",
    );
    refused(
        "two-catalog-csvs",
        &|body| {
            let index = csv_source(body);
            let copy = body["sources"][index].clone();
            body["sources"].as_array_mut().unwrap().push(copy);
        },
        "exactly one catalog-csv file, not 2",
    );
    refused(
        "other-catalog-path",
        &|body| {
            let index = csv_source(body);
            body["sources"][index]["path"] = "database/g-other.csv".into();
        },
        "catalog-csv source of g-res must be database/g-res.csv",
    );
    refused(
        "no-substitutes",
        &|body| {
            body["sources"]
                .as_array_mut()
                .unwrap()
                .retain(|source| source["role"] != "substitutes-csv");
        },
        "exactly one substitutes-csv file, not 0",
    );
    refused(
        "no-expected-import",
        &|body| {
            body.as_object_mut().unwrap().remove("expectedCsvImport");
        },
        "expected CSV import number",
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
        "no-expected-details",
        &|body| {
            body["parts"][0]
                .as_object_mut()
                .unwrap()
                .remove("expectedDetails");
        },
        "expected details of RES-0000-0000",
    );
    // What a document holds is checked too, so a field can never move
    // between the part details and the revision.
    let rewrite =
        |body: &mut serde_json::Value, name: &str, change: &dyn Fn(&mut serde_json::Value)| {
            let entry = &mut body["parts"][0][name];
            let mut snapshot: serde_json::Value =
                serde_json::from_str(entry["snapshot"].as_str().unwrap()).unwrap();
            change(&mut snapshot);
            let snapshot = snapshot.to_string();
            entry["snapshotSha256"] = sha256_hex(&snapshot).into();
            entry["snapshot"] = snapshot.into();
        };
    refused(
        "symbol-in-details",
        &|body| {
            rewrite(body, "details", &|details| {
                details["fields"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({ "name": "Symbol", "value": "g-res:R_US" }))
            })
        },
        "it lists Symbol, which belongs to the part's revision",
    );
    refused(
        "description-in-revision",
        &|body| {
            rewrite(body, "revision", &|revision| {
                revision["cad_fields"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({ "name": "Description", "value": "x" }))
            })
        },
        "it lists Description, which is part details",
    );
    refused(
        "footprint-before-symbol",
        &|body| {
            rewrite(body, "revision", &|revision| {
                revision["cad_fields"].as_array_mut().unwrap().reverse()
            })
        },
        "Symbol, then Footprint, each at most once",
    );
    refused(
        "symbol-twice",
        &|body| {
            rewrite(body, "revision", &|revision| {
                let first = revision["cad_fields"][0].clone();
                revision["cad_fields"]
                    .as_array_mut()
                    .unwrap()
                    .insert(0, first)
            })
        },
        "Symbol, then Footprint, each at most once",
    );
    refused(
        "field-value-not-text",
        &|body| {
            rewrite(body, "details", &|details| {
                details["fields"][0]["value"] = 47.into()
            })
        },
        "a name and a value, both text",
    );
    refused(
        "manufacturer-part-without-mpn",
        &|body| {
            rewrite(body, "details", &|details| {
                details["manufacturer_parts"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("mpn");
            })
        },
        "a manufacturer and an MPN, both text",
    );
    refused(
        "references-not-the-fields",
        &|body| {
            rewrite(body, "revision", &|revision| {
                revision["cad_references"][0]["reference"] = "g-res:OTHER".into()
            })
        },
        "not the ones its Symbol and Footprint fields name",
    );
    refused(
        "reference-dropped",
        &|body| {
            rewrite(body, "revision", &|revision| {
                revision["cad_references"].as_array_mut().unwrap().pop();
            })
        },
        "not the ones its Symbol and Footprint fields name",
    );
    assert_eq!(read_back(&admin).unwrap(), initial);
    refused(
        "allow-as-text",
        &|body| body["allowOlderCheckout"] = "yes".into(),
        "allowOlderCheckout setting must be true or false",
    );
    refused(
        "allow-uncommitted-as-text",
        &|body| body["allowUncommittedChanges"] = "yes".into(),
        "allowUncommittedChanges setting must be true or false",
    );
    // Files with uncommitted changes come only with the setting that allows
    // them; the server cannot run git, but it can check that.
    for setting in [serde_json::Value::Null, false.into()] {
        refused(
            "uncommitted-without-setting",
            &|body| {
                body["sourceCommit"] = format!("{}+dirty", "a".repeat(40)).into();
                body["allowUncommittedChanges"] = setting.clone();
            },
            "changes that are not committed (its source commit ends in +dirty). Commit them",
        );
    }
    // A source commit is empty or a commit ID, with +dirty exactly as the
    // importer writes it, so the mark cannot be hidden or misspelt.
    let commit = "a".repeat(40);
    for (name, source_commit) in [
        ("upper-case-mark", format!("{commit}+DIRTY")),
        ("trailing-space", format!("{commit}+dirty ")),
        ("other-mark", format!("{commit}-dirty")),
        ("upper-case-commit", format!("{}+dirty", "A".repeat(40))),
        ("short-commit", "abc123".to_owned()),
    ] {
        refused(
            name,
            &|body| {
                body["sourceCommit"] = source_commit.clone().into();
                body["allowUncommittedChanges"] = true.into();
            },
            "source commit must be empty or a commit ID of 40 lowercase hex digits",
        );
    }
    // A CSV file name is plain, so importers can put it in a filter as it
    // is (read_import_state).
    for (name, csv_file) in [
        ("backslash", "g-res\\x".to_owned()),
        ("trailing-backslash", "g-res\\".to_owned()),
        ("tab", "g\tres".to_owned()),
        ("newline", "g\nres".to_owned()),
        ("quote", "g\"res".to_owned()),
        ("space", "g res".to_owned()),
        ("slash", "sub/g-res".to_owned()),
        ("leading-dot", ".g-res".to_owned()),
        ("too-long", "g".repeat(65)),
    ] {
        refused(
            name,
            &|body| body["csvFile"] = csv_file.clone().into(),
            "CSV file name may only contain letters, digits and . _ -",
        );
    }
    // Allowing uncommitted changes when there are none would leave a wrong,
    // permanent record.
    for source_commit in [commit.clone(), String::new()] {
        refused(
            "nothing-to-allow",
            &|body| {
                body["sourceCommit"] = source_commit.clone().into();
                body["allowUncommittedChanges"] = true.into();
            },
            "are no uncommitted changes to allow",
        );
    }
    // The server numbers CAD item revisions itself and refuses a forged ID
    // or number, whether or not it is consistent with the rest.
    refused(
        "forged-cad-id",
        &|body| body["cadItemRevisions"][0]["id"] = "aaaaaaaaaaaaaaa".into(),
        "CAD item revision ID for",
    );
    refused(
        "forged-cad-number",
        &|body| body["cadItemRevisions"][0]["number"] = 7.into(),
        "must be 0 or 1",
    );
    // From here the commands expect what is stored, so they pass the
    // conflict check and reach the numbering checks.
    let current =
        ImportCommand::new(&original, REASON).expecting(&read_import_state(&admin).unwrap());
    let refused_current = |name: &str, change: &dyn Fn(&mut serde_json::Value), says: &str| {
        let mut body = current.body();
        body["correlationId"] = name.into();
        change(&mut body);
        let reply = admin.post_json(CSV_IMPORT_ROUTE, &body).unwrap();
        assert_eq!(reply.status, 400, "{name}: {}", reply.body);
        assert!(reply.body.contains(says), "{name}: {}", reply.body);
    };
    // Content equal to the current revision cannot claim the next number,
    // even with the ID derived from it.
    refused_current(
        "number-after-current",
        &|body| {
            let revision = &mut body["cadItemRevisions"][0];
            assert_eq!(revision["expectedRevision"], 1);
            let cad_item = catalog::CadItemId::from_stored(revision["cadItemId"].as_str().unwrap());
            revision["number"] = 2.into();
            revision["id"] = catalog::CadItemRevisionId::new(&cad_item, 2)
                .as_str()
                .into();
        },
        "has the content of its current revision 1, so its revision number must be 1",
    );
    // A part revision must bind the current revision: not another number...
    let rebind = |body: &mut serde_json::Value, change: &dyn Fn(&mut serde_json::Value)| {
        let mut snapshot: serde_json::Value =
            serde_json::from_str(body["parts"][0]["revision"]["snapshot"].as_str().unwrap())
                .unwrap();
        let target = snapshot["cad_references"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .map(|reference| &mut reference["target"])
            .find(|target| target["kind"] == "cad-item-revision")
            .unwrap();
        change(target);
        let snapshot = snapshot.to_string();
        body["parts"][0]["revision"]["snapshotSha256"] = sha256_hex(&snapshot).into();
        body["parts"][0]["revision"]["snapshot"] = snapshot.into();
    };
    refused_current(
        "older-binding",
        &|body| rebind(body, &|target| target["number"] = 7.into()),
        "must keep revision 1 of its CAD item",
    );
    // ...nor a record ID or anything else besides the three names.
    refused_current(
        "binding-with-id",
        &|body| rebind(body, &|target| target["id"] = "aaaaaaaaaaaaaaa".into()),
        "by cad_item, number and snapshot_sha256 only",
    );
    refused_current(
        "same-cad-item-twice",
        &|body| {
            let copy = body["cadItemRevisions"][0].clone();
            body["cadItemRevisions"].as_array_mut().unwrap().push(copy);
        },
        "appears twice",
    );
    // A CAD item that changed after the importer read it is a conflict,
    // like a part: 409, nothing written, not stored.
    let mut stale = current.body();
    stale["correlationId"] = "stale-cad-item".into();
    let revision = &mut stale["cadItemRevisions"][0];
    let key = revision["key"].as_str().unwrap().to_owned();
    let cad_item = catalog::CadItemId::from_stored(revision["cadItemId"].as_str().unwrap());
    revision["expectedRevision"] = 0.into();
    revision["number"] = 1.into();
    revision["id"] = catalog::CadItemRevisionId::new(&cad_item, 1)
        .as_str()
        .into();
    let reply = admin.post_json(CSV_IMPORT_ROUTE, &stale).unwrap();
    assert_eq!(reply.status, 409, "{}", reply.body);
    let outcome: ImportOutcome = serde_json::from_str(&reply.body).unwrap();
    let ImportOutcome::Conflict {
        conflicts,
        cad_item_conflicts,
        ..
    } = outcome
    else {
        panic!("expected a conflict, got {}", reply.body);
    };
    assert!(conflicts.is_empty());
    assert_eq!(
        cad_item_conflicts
            .iter()
            .map(|conflict| (
                conflict.key.as_str(),
                conflict.expected_revision,
                conflict.current_revision
            ))
            .collect::<Vec<_>>(),
        [(key.as_str(), 0, 1)]
    );
    assert_eq!(read_back(&admin).unwrap(), initial);

    // A changed part detail saves the part's next details in a new Change,
    // and adds no revision; nothing else is touched.
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
        (0, 0, 0)
    );
    assert_eq!((created.part_details, created.updated_details), (1, 1));
    assert_eq!(
        (created.cad_items, created.cad_item_revisions, created.blobs),
        (0, 0, 0)
    );
    assert_eq!(created.problems, 0);
    let after = read_back(&admin).unwrap();
    assert_eq!(after.changes.len(), 2);
    assert_eq!(after.changes[1].seq, 2);
    let (number, details) = current_details(&after, "RES-0000-0470");
    assert_eq!((number, details.field("Value")), (2, Some("47R")));
    assert_eq!(after.part_revisions, initial.part_revisions);
    let newest = after
        .part_details(&after.part("RES-0000-0470").unwrap().current_details)
        .unwrap();
    let previous = after.part_details(&newest.predecessor).unwrap();
    // The first details never change.
    assert_eq!(
        Some(previous),
        initial.part_details(&initial.part("RES-0000-0470").unwrap().current_details)
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
    assert_eq!(supplier.part_details, after.part_details);
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
    // A new manufacturer part changes the details, not the revision.
    let created = applied(&reply);
    assert_eq!((created.updated_details, created.updated_parts), (1, 0));
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
    assert_eq!(current_details(&supplier, "RES-0000-1002").0, 2);
    assert_eq!(current_revision(&supplier, "RES-0000-1002").0, 1);
    let changes_before_edit = supplier.changes.len();

    // Part details saved by an edit are never overwritten by a CSV import:
    // the changed row becomes an import problem instead.
    let edit = details_edit(
        &supplier,
        "RES-0000-0470",
        "Value",
        "47 Ohm",
        "Note the exact value",
    );
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
        current_details(&last, "RES-0000-0470"),
        current_details(&after_edit, "RES-0000-0470")
    );
    assert_eq!(
        current_details(&last, "RES-0000-0470").1.field("Value"),
        Some("47 Ohm")
    );
    assert!(last.problems.iter().any(|problem| problem.code
        == "csv-details-differ-from-edited-details"
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

    // A part whose details match the row but were last saved by an edit
    // (here one that puts back the imported value after another edit) has
    // no CSV row to compare with: an import problem keeps the row, not a
    // server error.
    let (_, details) = current_details(&last, "RES-0000-1001");
    let imported_value = details.field("Value").unwrap().to_owned();
    let edit = details_edit(&last, "RES-0000-1001", "Value", "1.0k", "Try another value");
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    let back = details_edit(
        &read_back(&admin).unwrap(),
        "RES-0000-1001",
        "Value",
        &imported_value,
        "Back to the imported value",
    );
    let put_back = submit_edit(&admin, &back).unwrap();
    assert_eq!(put_back.status, 200, "{}", put_back.body);
    let reply = import_reading_first(&admin, &conflicting, "-source");
    assert_eq!(nothing_changed(&reply).1, 1, "{}", reply.body);
    let kept = read_back(&admin).unwrap();
    let problem = kept
        .problems
        .iter()
        .find(|problem| problem.code == "part-last-saved-without-csv-source")
        .unwrap();
    assert_eq!(problem.subject, "RES-0000-1001");
    assert!(problem.detail.contains("raw_record"), "{}", problem.detail);
    assert_eq!(current_details(&kept, "RES-0000-1001").0, 3);
    assert_eq!(current_revision(&kept, "RES-0000-1001").0, 1);

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
    let actual = csv_catalog_fixture::build_manifest(&root).unwrap();
    assert!(csv_catalog_fixture::compare(&oracle(), &actual).is_empty());
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
    let oracle = csv_catalog_fixture::build_manifest(copy.path()).unwrap();

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
    let (_, snapshot) = current_details(&stored, "RES-0000-1500");
    let first = &snapshot.manufacturer_parts[0];
    assert_eq!(
        (first.manufacturer.as_str(), first.mpn.as_str()),
        ("Panasonic", "ALT-150")
    );
    let (_, details) = current_details(&stored, "RES-0000-0470");
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
fn adding_or_removing_a_manufacturer_part_saves_new_details_but_no_revision() {
    // Manufacturer parts are part details, so adding or removing a
    // substitutes.csv row on re-import saves the part's next details and
    // adds no revision.
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
    assert_eq!((created.part_revisions, created.updated_parts), (0, 0));
    assert_eq!((created.part_details, created.updated_details), (1, 1));
    let stored = read_back(&admin).unwrap();
    let (number, details) = current_details(&stored, "RES-0000-0470");
    assert_eq!(number, 2);
    assert_eq!(current_revision(&stored, "RES-0000-0470").0, 1);
    assert_eq!(details.manufacturer_parts[1].mpn, "ALT-47");
    assert_eq!(stored.changes.len(), 2);

    fs::write(&substitutes_csv_rows, &header).unwrap();
    applied(&submit("-removed"));
    let (number, details) = current_details(&read_back(&admin).unwrap(), "RES-0000-0470");
    assert_eq!((number, details.manufacturer_parts.len()), (3, 1));
    server.stop();
}

/// The two CSV sources every run reads, for a CSV file of its own
/// (`g-<correlation>`) with only a header line, so each blob request is the
/// first run of its file.
fn header_only_csv_sources(correlation: &str) -> Vec<serde_json::Value> {
    let header = "IPN\n";
    let substitutes = "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n";
    vec![
        serde_json::json!({ "role": "catalog-csv", "path": format!("database/g-{correlation}.csv"), "byteSha256": sha256_hex(header), "size": header.len(), "headerRaw": header, "rows": [] }),
        serde_json::json!({ "role": "substitutes-csv", "path": "database/substitutes.csv", "byteSha256": sha256_hex(substitutes), "size": substitutes.len(), "headerRaw": substitutes, "rows": [] }),
    ]
}

/// The category of the CSV file `g-<correlation>`, which has no columns
/// but IPN.
fn category_entry(correlation: &str) -> serde_json::Value {
    let category = catalog::CategoryDetails::starting(correlation, Vec::new());
    let snapshot = category.canonical_json();
    serde_json::json!({
        "id": catalog::category::category_id(correlation),
        "code": correlation,
        "snapshotSha256": sha256_hex(&snapshot),
        "snapshot": snapshot,
    })
}

/// How a test request sends a blob's bytes.
#[derive(Clone, Copy)]
enum BlobForm {
    Base64,
    /// Only for bytes that are UTF-8.
    Text,
}

/// The `blobs` entry for `bytes`, hashed as `sha`.
fn blob_entry(sha: &str, bytes: &[u8], form: BlobForm) -> serde_json::Value {
    match form {
        BlobForm::Base64 => {
            serde_json::json!({ "sha256": sha, "size": bytes.len(), "base64": catalog::pocketbase::base64(bytes) })
        }
        BlobForm::Text => {
            serde_json::json!({ "sha256": sha, "size": bytes.len(), "text": std::str::from_utf8(bytes).unwrap() })
        }
    }
}

/// An import request carrying one 3D model CAD item revision whose blob is
/// `bytes`, hashed as `sha`.
fn blob_request(correlation: &str, sha: &str, bytes: &[u8], form: BlobForm) -> serde_json::Value {
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
    // A new 3D model, so its first revision.
    let id = catalog::CadItemRevisionId::new(&cad_item.id, 1);
    serde_json::json!({
        "correlationId": correlation,
        "reason": "check blob hashing",
        "csvFile": format!("g-{correlation}"),
        "sourceCommit": null,
        "expectedCsvImport": 0,
        "category": category_entry(correlation),
        "sources": header_only_csv_sources(correlation)
            .into_iter()
            .chain([serde_json::json!({ "role": "test", "path": "blob", "byteSha256": sha, "size": bytes.len(), "headerRaw": null })])
            .collect::<Vec<_>>(),
        "blobs": [blob_entry(sha, bytes, form)],
        "cadItemRevisions": [{
            "id": id,
            "cadItemId": cad_item.id,
            "kind": "model-3d",
            "key": cad_item.key,
            "expectedRevision": 0,
            "number": 1,
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
        "csvFile": format!("g-{correlation}"),
        "sourceCommit": null,
        "expectedCsvImport": 0,
        "category": category_entry(correlation),
        "sources": header_only_csv_sources(correlation)
            .into_iter()
            .chain([serde_json::json!({ "role": "test", "path": "blob", "byteSha256": sha, "size": bytes.len(), "headerRaw": null })])
            .collect::<Vec<_>>(),
        "blobs": [blob_entry(sha, bytes, BlobForm::Base64)],
        "cadItemRevisions": [],
        "parts": [],
        "problems": [],
    })
}

/// `size` bytes of UTF-8 text with one-, two-, three- and four-byte
/// characters, padded with `x` so that the length is exact.
fn utf8_text(size: usize) -> Vec<u8> {
    let mut text = String::new();
    for ch in ['a', 'é', '€', '😀', '\n', '"', '\\'].into_iter().cycle() {
        if text.len() + ch.len_utf8() > size {
            break;
        }
        text.push(ch);
    }
    while text.len() < size {
        text.push('x');
    }
    text.into_bytes()
}

#[test]
fn the_route_hashes_blob_bytes_exactly() {
    // The route's two ways of hashing a blob against Rust's sha2, across
    // SHA-256 padding boundaries and one large blob: bytes that are not
    // UTF-8 come as base64 and are hashed by the route's own SHA-256
    // (catalog_lib.js); UTF-8 text is hashed and stored by PocketBase.
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let sizes = [0usize, 1, 55, 56, 63, 64, 119, 120, 300_000];
    let mut stored_sizes = Vec::new();
    for (form, name) in [(BlobForm::Base64, "base64"), (BlobForm::Text, "text")] {
        for size in sizes {
            let bytes: Vec<u8> = match form {
                BlobForm::Base64 => (0..size).map(|i| ((i * 131 + 7) % 256) as u8).collect(),
                BlobForm::Text => utf8_text(size),
            };
            assert_eq!(bytes.len(), size);
            let sha = sha256_hex(&bytes);
            // A hash off by one hex digit is refused...
            let mut wrong = sha.clone().into_bytes();
            wrong[63] = if wrong[63] == b'0' { b'1' } else { b'0' };
            let wrong = String::from_utf8(wrong).unwrap();
            let refused = admin
                .post_json(
                    CSV_IMPORT_ROUTE,
                    &blob_request(&format!("wrong-{name}-{size}"), &wrong, &bytes, form),
                )
                .unwrap();
            assert_eq!(refused.status, 400, "{name} {size} bytes: {}", refused.body);
            // ...and the right one is accepted.
            let accepted = admin
                .post_json(
                    CSV_IMPORT_ROUTE,
                    &blob_request(&format!("right-{name}-{size}"), &sha, &bytes, form),
                )
                .unwrap();
            if size == 0 {
                // PocketBase cannot store an empty file; the route must
                // refuse it cleanly rather than store a blob without bytes.
                assert_eq!(accepted.status, 400, "{}", accepted.body);
            } else {
                assert_eq!(
                    accepted.status, 200,
                    "{name} {size} bytes: {}",
                    accepted.body
                );
                stored_sizes.push(size);
            }
        }
    }
    // A blob gives its bytes one way only.
    let bytes = b"both".to_vec();
    let mut both = blob_request("both", &sha256_hex(&bytes), &bytes, BlobForm::Text);
    both["blobs"][0]["base64"] = catalog::pocketbase::base64(&bytes).into();
    let reply = admin.post_json(CSV_IMPORT_ROUTE, &both).unwrap();
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(
        reply.body.contains("either text or base64"),
        "{}",
        reply.body
    );
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
    // PocketBase refuses a request over the route's size limit as soon as
    // it reads the length, so the client sees either the 413 or the
    // connection closing while it still sends; nothing is written either
    // way. (submit_import refuses to send such a request at all.)
    let mut huge = blob_request("huge", &sha256_hex(b"x"), b"x", BlobForm::Text);
    huge["padding"] = "x".repeat(catalog::pocketbase::IMPORT_BODY_LIMIT).into();
    match admin.post_json(CSV_IMPORT_ROUTE, &huge) {
        Ok(reply) => assert_eq!(reply.status, 413, "{}", reply.body),
        Err(CollabError::Transport(_)) => {}
        Err(other) => panic!("{other}"),
    }
    // read_back re-hashes every downloaded blob and fails on a mismatch.
    let stored = read_back(&admin).unwrap();
    let mut lengths: Vec<usize> = stored.blobs.iter().map(|blob| blob.bytes.len()).collect();
    lengths.sort();
    stored_sizes.sort();
    assert_eq!(lengths, stored_sizes);
    server.stop();
}

#[test]
fn the_catalog_migrations_refuse_to_undo_csv_reviews() {
    let root = repository();

    // With a CSV review and an archived revision, undoing past the CSV
    // reviews refuses: the older version cannot read CSV reviews (nor,
    // further back, archive entries or part details kept apart from
    // revisions). An undo of several steps is refused as a whole, so
    // nothing is changed, not even the newest steps, which this data allows.
    // (pocketbase_stored_data.rs rolls every migration back; this test
    // keeps the CSV reviews' own refusal, for kicad-parts-e257.)
    let populated = tempfile::tempdir().unwrap();
    let options = services::options(&root, populated.path());
    let (server, admin, _superuser) = services::start(&root, populated.path()).unwrap();
    let csv_only = tempfile::tempdir().unwrap();
    for path in ["database/g-res.csv", "database/substitutes.csv"] {
        let target = csv_only.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(root.join(path), target).unwrap();
    }
    let import = CsvCatalog::new(csv_only.path())
        .import("g-res", None)
        .unwrap();
    applied(&submit_import(&admin, &ImportCommand::new(&import, REASON)).unwrap());
    let edit = details_edit(
        &read_back(&admin).unwrap(),
        "RES-0000-0470",
        "Value",
        "47R",
        "Before the rollback",
    );
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    // A second revision, archived, so the newest undo has archive entries
    // to refuse too.
    let stored = read_back(&admin).unwrap();
    let part = stored.part("RES-0000-0470").unwrap();
    let revision: PartRevision = serde_json::from_str(
        &stored
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut cad = EditCommand::new("RES-0000-0470", "Use KiCad's symbol").editing_cad(&revision, 1);
    assert!(cad.set_field("Symbol", "Device:R"));
    assert_eq!(submit_edit(&admin, &cad).unwrap().status, 200);
    let archive =
        catalog::pocketbase::ArchiveCommand::new("RES-0000-0470", 2, true, "Wrong symbol");
    assert_eq!(
        catalog::pocketbase::submit_archive(&admin, &archive)
            .unwrap()
            .status,
        200
    );
    // Importing again finds the edited value differs from the CSV; keeping
    // the edited value saves a CSV review.
    let state = catalog::pocketbase::read_import_state(&admin).unwrap();
    let again = submit_import(
        &admin,
        &ImportCommand::new(&import, REASON).expecting(&state),
    )
    .unwrap();
    assert_eq!(again.status, 200, "{}", again.body);
    let problem = read_back(&admin)
        .unwrap()
        .problems
        .into_iter()
        .find(|problem| problem.code == catalog::pocketbase::CSV_DETAILS_DIFFER)
        .unwrap();
    let keep = catalog::pocketbase::CsvReviewCommand::new(
        "RES-0000-0470",
        &problem.id,
        Vec::new(),
        2,
        "Keep the edited value",
    );
    assert_eq!(
        catalog::pocketbase::submit_csv_review(&admin, &keep)
            .unwrap()
            .status,
        200
    );
    // A part added in the app, so the newest undo has one to refuse.
    let mut added = catalog::pocketbase::CreateCommand::new("RES-0001-0001", "res", "A new part");
    added.fields = ["Description", "Value", "Tolerance", "Datasheet"]
        .iter()
        .map(|name| catalog::DetailField {
            name: (*name).to_owned(),
            value: String::new(),
        })
        .collect();
    added.cad_fields[0].value = "Device:R".into();
    added.cad_fields[1].value = "Resistor_SMD:R_0603_1608Metric".into();
    assert_eq!(
        catalog::pocketbase::submit_create(&admin, &added)
            .unwrap()
            .status,
        200
    );
    // A team code, so the newest undo has one to refuse.
    catalog::pocketbase::submit_team_code(&admin, "MA").unwrap();
    server.stop();
    // An import sets up no category without a CSV file, so the newest
    // undo would pass; it is set aside, keeping the steps below as they
    // were.
    set_aside_migration(&options, "1792600000_category_changes.js");
    let said = migrate(&root, &options, &["down", "6"]);
    assert!(
        said.contains("team has a code")
            && said.contains("Nothing was changed")
            && !said.contains("Reverted"),
        "{said}"
    );
    set_aside_migration(&options, "1792500000_team_settings.js");
    // The next undo refuses the part added in the app, as a whole.
    let said = migrate(&root, &options, &["down", "5"]);
    assert!(
        said.contains("parts added in the app")
            && said.contains("Nothing was changed")
            && !said.contains("Reverted"),
        "{said}"
    );
    set_aside_migration(&options, "1792400000_new_parts.js");
    // Any data has categories, so the next undo refuses first, as a whole.
    let said = migrate(&root, &options, &["down", "4"]);
    assert!(
        said.contains("has categories")
            && said.contains("Nothing was changed")
            && !said.contains("Reverted"),
        "{said}"
    );
    // With that step set aside, the older undos still refuse their own
    // data.
    set_aside_migration(&options, "1792300000_categories.js");
    let said = migrate(&root, &options, &["down", "3"]);
    assert!(
        said.contains("has CSV reviews")
            && said.contains("Nothing was changed")
            && !said.contains("Reverted"),
        "{said}"
    );
    // Undoing further is refused at the first step too, so neither the
    // archive nor the details migration is undone.
    let said = migrate(&root, &options, &["down", "5"]);
    assert!(
        said.contains("has CSV reviews") && !said.contains("Reverted"),
        "{said}"
    );
    let database = rusqlite::Connection::open_with_flags(
        options.data_dir.join("data.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let details: i64 = database
        .query_row("SELECT COUNT(*) FROM catalog_part_details", [], |row| {
            row.get(0)
        })
        .unwrap();
    // The 29 imported parts, the edit of RES-0000-0470 and the part added
    // in the app.
    assert_eq!(details, 31);
    drop(database);
}

/// Marks `file` as not applied on the stopped instance `options`, keeping
/// what it made, so that undoing reaches the older migrations: the newest
/// one refuses any data with categories, which every import makes.
fn set_aside_migration(options: &native_services::PocketBaseOptions, file: &str) {
    let database = rusqlite::Connection::open(options.data_dir.join("data.db")).unwrap();
    assert_eq!(
        database
            .execute("DELETE FROM _migrations WHERE file = ?1", [file])
            .unwrap(),
        1
    );
}

/// Runs `pocketbase migrate <args>` hidden on `options`, confirming the
/// question `migrate down` asks.
fn migrate(root: &Path, options: &native_services::PocketBaseOptions, args: &[&str]) -> String {
    let binary = native_services::pocketbase_binary(root).unwrap();
    let mut command = std::process::Command::new(&binary);
    command
        .arg("migrate")
        .args(args)
        .arg("--dir")
        .arg(&options.data_dir)
        .arg("--migrationsDir")
        .arg(&options.migrations_dir)
        .arg("--hooksDir")
        .arg(&options.hooks_dir)
        .arg("--automigrate=false");
    let output =
        native_services::run_hidden_with_input("pocketbase migrate", &mut command, b"y\n").unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn an_import_allowed_from_uncommitted_changes_is_recorded_kept_and_not_undone() {
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let options = services::options(&root, state.path());
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let dirty = format!("{}+dirty", "b".repeat(40));
    let import = CsvCatalog::new(&root)
        .import("g-res", Some(dirty.clone()))
        .unwrap();
    applied(
        &submit_import(
            &admin,
            &ImportCommand::new(&import, REASON).allowing_uncommitted_changes(true),
        )
        .unwrap(),
    );
    let entry = read_back(&admin).unwrap().audit_entries[0].clone();
    assert!(entry.allowed_uncommitted_changes && !entry.allowed_older_checkout);
    assert_eq!(entry.source_commit, dirty);
    // Nobody changes the record afterwards, not even a superuser.
    let path = format!(
        "/api/collections/catalog_audit_entries/records/{}",
        entry.id
    );
    let changed = admin
        .patch_json(
            &path,
            &serde_json::json!({ "allowed_uncommitted_changes": false }),
        )
        .unwrap();
    assert_eq!(changed.status, 403, "{}", changed.body);
    assert!(
        read_back(&admin).unwrap().audit_entries[0].allowed_uncommitted_changes,
        "the record changed"
    );
    server.stop();

    // The older version would lose that record, so the undo refuses and
    // changes nothing, not even the newer step, which this data allows.
    // (The categories step, newer still, refuses any data, so it is set
    // aside, with the steps after it.)
    set_aside_migration(&options, "1792600000_category_changes.js");
    set_aside_migration(&options, "1792500000_team_settings.js");
    set_aside_migration(&options, "1792400000_new_parts.js");
    set_aside_migration(&options, "1792300000_categories.js");
    let said = migrate(&root, &options, &["down", "2"]);
    assert!(
        said.contains("allowed from uncommitted changes")
            && said.contains("Nothing was changed")
            && !said.contains("Reverted"),
        "{said}"
    );
}

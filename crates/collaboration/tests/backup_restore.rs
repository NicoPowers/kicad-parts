//! Blank-environment recovery drill: PocketBase backs up to an isolated
//! MinIO bucket, its state is destroyed, and a blank instance restored from
//! the bucket must reproduce the records, the managed files and their
//! hashes.
//!
//! The first run builds MinIO from its pinned source (several minutes,
//! cached under `target/native-services`); later runs take seconds.
//!
//! PocketBase's built-in restore (`POST /api/backups/{key}/restore`)
//! restarts the process in place, which PocketBase 0.38 refuses on Windows
//! ("restore is not supported on Windows"). The drill therefore restores
//! the way an operator would on a new machine: download the backup from
//! the bucket, unpack it into an empty `pb_data`, and start a fresh
//! instance.
//!
//! PocketBase's file storage is local here (S3 file storage is not
//! enabled), so managed files travel inside the backup and the plan's
//! "separately protected objects where enabled" clause does not apply. The
//! seed that enables S3-backed files owns that proof, because a pb_data
//! backup excludes files stored in S3. The drill still keeps independent
//! copies of the canonical export and of each file in a second bucket, as
//! a reference the restore is compared against that PocketBase did not
//! write.

mod support;

use collaboration::{CommandOutcome, EditRecord, PocketBaseClient};
use native_services::{
    MinioServer, Superuser, extract_zip, minio_binary, random_name, repository_root, sha256_hex,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use support::{all_migrations, create_member, sign_in};

/// Collections whose every record must survive the restore exactly.
const EXPORTED: [&str; 5] = [
    "users",
    "collab_records",
    "collab_changes",
    "collab_commands",
    "collab_attachments",
];

/// Everything the API returns for the exported collections, as canonical
/// JSON bytes.
fn canonical_export(admin: &PocketBaseClient) -> Vec<u8> {
    let export: BTreeMap<&str, Vec<Value>> = EXPORTED
        .iter()
        .map(|collection| (*collection, support::records(admin, collection)))
        .collect();
    serde_json::to_vec_pretty(&export).unwrap()
}

/// SHA-256 of each attachment's file, by attachment record ID.
fn attachment_hashes(admin: &PocketBaseClient) -> BTreeMap<String, String> {
    let token = admin.file_token().unwrap();
    support::records(admin, "collab_attachments")
        .iter()
        .map(|attachment| {
            let id = attachment["id"].as_str().unwrap();
            let file = attachment["file"].as_str().unwrap();
            let bytes = admin
                .download_file("collab_attachments", id, file, Some(&token))
                .unwrap();
            (id.to_owned(), sha256_hex(&bytes))
        })
        .collect()
}

#[test]
fn a_backup_in_minio_restores_into_a_blank_instance() {
    let repository = repository_root();
    let temporary = tempfile::tempdir().unwrap();
    let minio = MinioServer::start(
        &minio_binary(&repository).unwrap(),
        &temporary.path().join("minio-data"),
        temporary.path(),
    )
    .unwrap();
    let s3 = minio.s3();
    let run = random_name(10);
    let backups_bucket = format!("pb-backups-{run}");
    let copies_bucket = format!("independent-copies-{run}");
    s3.create_bucket(&backups_bucket).unwrap();
    s3.create_bucket(&copies_bucket).unwrap();

    // A populated instance: members, revisioned records through the
    // protected command, and PocketBase-managed (protected) files.
    let original_root = temporary.path().join("original");
    let migrations = all_migrations();
    let migrations: Vec<&str> = migrations.iter().map(String::as_str).collect();
    let options = support::options(&original_root, &migrations);
    let superuser = Superuser::generate();
    let (server, admin) = support::start(&options, &superuser);
    let member = create_member(&admin, "dana");
    let dana = sign_in(server.url(), &member);
    let mut record_ids = Vec::new();
    for (key, title) in [("res-1", "Resistor"), ("cap-1", "Capacitor")] {
        let reply = dana.execute(&EditRecord::new(key, 0, title, "v1")).unwrap();
        let CommandOutcome::Applied { record_id, .. } = reply.outcome else {
            panic!("expected applied, got {:?}", reply.outcome);
        };
        record_ids.push(record_id);
    }
    let last = dana
        .execute(&EditRecord::new("res-1", 1, "Resistor", "v2"))
        .unwrap();
    let mut binary = vec![0u8; 300 * 1024];
    for (index, byte) in binary.iter_mut().enumerate() {
        *byte = (index.wrapping_mul(2_654_435_761) >> 13) as u8;
    }
    let files: [(&str, &[u8]); 2] = [
        ("datasheet.pdf", &binary),
        ("notes.txt", "line one\r\nline two\n".as_bytes()),
    ];
    for ((name, bytes), record_id) in files.iter().zip(&record_ids) {
        dana.create_with_file(
            "collab_attachments",
            &[("record", record_id), ("label", name)],
            "file",
            name,
            bytes,
        )
        .unwrap();
    }
    let original_export = canonical_export(&admin);
    let original_files = attachment_hashes(&admin);
    assert_eq!(original_files.len(), 2);
    let mut expected_hashes: Vec<String> =
        files.iter().map(|(_, bytes)| sha256_hex(bytes)).collect();
    let mut stored_hashes: Vec<String> = original_files.values().cloned().collect();
    expected_hashes.sort();
    stored_hashes.sort();
    assert_eq!(
        stored_hashes, expected_hashes,
        "PocketBase stores the exact bytes"
    );

    // Independent copies: the canonical export and each file,
    // content-addressed by SHA-256, in their own bucket (see the module
    // docs: not the plan's S3-backed protected objects, which are not
    // enabled here).
    let export_key = format!("exports/{run}.json");
    s3.put_object(&copies_bucket, &export_key, &original_export)
        .unwrap();
    for (_, bytes) in &files {
        s3.put_object(
            &copies_bucket,
            &format!("blobs/{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap();
    }

    // PocketBase writes its backup to the isolated bucket.
    admin
        .patch_json(
            "/api/settings",
            &json!({ "backups": { "s3": {
                "enabled": true,
                "bucket": backups_bucket,
                "region": native_services::s3::REGION,
                "endpoint": minio.endpoint(),
                "accessKey": minio.access_key(),
                "secret": minio.secret_key(),
                "forcePathStyle": true,
            }}}),
        )
        .unwrap()
        .success()
        .unwrap();
    let backup_name = format!("drill_{run}.zip");
    admin
        .post_json("/api/backups", &json!({ "name": backup_name }))
        .unwrap()
        .success()
        .unwrap();
    let listed: Vec<Value> = admin.get("/api/backups").unwrap().json().unwrap();
    assert!(
        listed
            .iter()
            .any(|backup| backup["key"] == backup_name.as_str())
    );
    let objects = s3.list_objects(&backups_bucket, "").unwrap();
    assert_eq!(objects.len(), 1, "{objects:?}");
    assert_eq!(objects[0].key, backup_name);
    let backup = s3.get_object(&backups_bucket, &backup_name).unwrap();
    assert_eq!(backup.len() as u64, objects[0].size);
    let backup_hash = sha256_hex(&backup);

    // What the restore must reproduce beyond records: the schema with its
    // rules, as the API reports it, and (read from data.db once stopped)
    // the collections, applied migrations and superusers.
    let original_collections = collections(&admin);
    let original_superuser_id = admin.auth_id().unwrap().to_owned();

    // Destroy the disposable PocketBase state.
    server.stop();
    let original_tables = system_tables(&options.data_dir.join("data.db"));
    assert!(
        original_tables["_migrations"]
            .iter()
            .any(|row| row[0].contains("collab_record_category"))
    );
    assert_eq!(original_tables["_superusers"].len(), 1);
    fs::remove_dir_all(&original_root).unwrap();
    assert!(!original_root.exists());

    // Restore into a blank instance from what the bucket holds.
    let restored_root = temporary.path().join("restored");
    assert!(!restored_root.exists());
    // No migrations: the schema must come from the backup alone.
    let options = support::options(&restored_root, &[]);
    let downloaded = s3.get_object(&backups_bucket, &backup_name).unwrap();
    assert_eq!(sha256_hex(&downloaded), backup_hash);
    let archive = restored_root.join("backup.zip");
    fs::write(&archive, &downloaded).unwrap();
    extract_zip(&archive, &options.data_dir).unwrap();
    assert!(options.data_dir.join("data.db").is_file());
    let stored_files = walk(&options.data_dir.join("storage"));
    assert_eq!(
        stored_files
            .iter()
            // PocketBase keeps a `.attrs` metadata sidecar per stored file.
            .filter(|path| !path.ends_with(".attrs") && !path.contains("thumbs_"))
            .count(),
        2,
        "the backup holds both managed files: {stored_files:?}"
    );
    // Before PocketBase touches it (its superuser upsert would otherwise
    // hide a missing superuser), the unpacked data.db already holds the
    // same collections, migrations and superusers.
    assert_eq!(
        system_tables(&options.data_dir.join("data.db")),
        original_tables
    );
    let (server, admin) = support::start(&options, &superuser);
    assert_eq!(
        admin.auth_id(),
        Some(original_superuser_id.as_str()),
        "the upsert updated the restored superuser instead of creating one"
    );
    assert_eq!(collections(&admin), original_collections);

    // Records reproduce exactly, and match the separately protected export.
    let restored_export = canonical_export(&admin);
    assert_eq!(sha256_hex(&restored_export), sha256_hex(&original_export));
    assert_eq!(
        String::from_utf8(restored_export).unwrap(),
        String::from_utf8(original_export).unwrap()
    );
    let protected_export = s3.get_object(&copies_bucket, &export_key).unwrap();
    assert_eq!(
        sha256_hex(&protected_export),
        sha256_hex(&canonical_export(&admin))
    );

    // Managed files come back byte for byte, matching the protected blobs.
    let restored_files = attachment_hashes(&admin);
    assert_eq!(restored_files, original_files);
    for hash in restored_files.values() {
        let blob = s3
            .get_object(&copies_bucket, &format!("blobs/{hash}"))
            .unwrap();
        assert_eq!(&sha256_hex(&blob), hash);
    }

    // The team can carry on: members sign in with their passwords, an old
    // correlation ID still replays, and the change log continues.
    let dana = sign_in(server.url(), &member);
    let mut replay = EditRecord::new("res-1", 1, "Resistor", "v2");
    replay.correlation_id = match &last.outcome {
        CommandOutcome::Applied { correlation_id, .. } => correlation_id.clone(),
        other => panic!("expected applied, got {other:?}"),
    };
    let replayed = dana.execute(&replay).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.body, last.body);
    let next = dana
        .execute(&EditRecord::new("res-1", 2, "Resistor", "v3"))
        .unwrap();
    assert!(matches!(
        next.outcome,
        CommandOutcome::Applied {
            revision: 3,
            seq: 4,
            ..
        }
    ));
    server.stop();
    minio.stop();
}

/// Every collection, with its fields, indexes and API rules, as the API
/// reports them, by name.
fn collections(admin: &PocketBaseClient) -> BTreeMap<String, Value> {
    let list: Value = admin
        .get_with_query("/api/collections", &[("perPage", "500".into())])
        .unwrap()
        .json()
        .unwrap();
    let items = list["items"].as_array().unwrap();
    assert!(items.len() < 500);
    items
        .iter()
        .map(|item| (item["name"].as_str().unwrap().to_owned(), item.clone()))
        .collect()
}

/// The rows of PocketBase's schema, migration and superuser tables, read
/// straight from a (stopped) data.db, as text.
fn system_tables(database: &std::path::Path) -> BTreeMap<&'static str, Vec<Vec<String>>> {
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    [
        ("_collections", "SELECT * FROM _collections ORDER BY id"),
        (
            "_migrations",
            "SELECT file, applied FROM _migrations ORDER BY file",
        ),
        (
            "_superusers",
            "SELECT id, email, password FROM _superusers ORDER BY id",
        ),
    ]
    .into_iter()
    .map(|(table, query)| {
        let mut statement = connection.prepare(query).unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| {
                        row.get::<_, rusqlite::types::Value>(index)
                            .map(|value| format!("{value:?}"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (table, rows)
    })
    .collect()
}

/// Every file under `root`, as `/`-separated paths relative to it.
fn walk(root: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

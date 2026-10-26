//! A team server's stored data, old or wrong (kicad-parts-fbc2 moved these
//! from `pocketbase_import.rs`): every catalog migration rolls back, but an
//! undo refuses data the older version cannot read; data saved before a
//! migration that cannot be upgraded is refused, not changed; a CAD item's
//! history never forks; and an edit never saves a field in the wrong
//! document.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 with the
//! sample library's resistors, and stops it before changing its data.db
//! (mx-404964). The repository is only read.

use catalog::pocketbase::{
    ArchiveCommand, EditCommand, read_back, submit_archive, submit_edit, submit_team_code,
};
use catalog::{PartDetails, PartRevision, sha256_hex};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use std::path::{Path, PathBuf};

const IPN: &str = "RES-0000-0470";

/// Every catalog migration, newest first.
const CATALOG_MIGRATIONS: [&str; 12] = [
    "1792600000_category_changes.js",
    "1792500000_team_settings.js",
    "1792400000_new_parts.js",
    "1792300000_categories.js",
    "1792200000_cad_item_revision_successor.js",
    "1792100000_uncommitted_changes.js",
    "1792000000_csv_reviews.js",
    "1791900000_archived_revisions.js",
    "1791800000_shared_part_details.js",
    "1791700000_cad_item_revision_numbers.js",
    "1791600000_catalog_edits.js",
    "1791500000_catalog_init.js",
];

fn repository() -> PathBuf {
    services::repository_root()
}

/// Marks `file` as not applied on the stopped instance `options`, keeping
/// what it made, so that undoing reaches the older migrations.
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

fn current_revision(admin: &PocketBaseClient, ipn: &str) -> (u32, PartRevision) {
    let stored = read_back(admin).unwrap();
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

fn current_details(admin: &PocketBaseClient, ipn: &str) -> (u32, PartDetails) {
    let stored = read_back(admin).unwrap();
    let part = stored.part(ipn).unwrap();
    let details = stored.part_details(&part.current_details).unwrap();
    (
        details.number,
        serde_json::from_str(&details.snapshot).unwrap(),
    )
}

#[test]
fn the_catalog_migrations_roll_back_and_refuse_to_undo_saved_data() {
    let root = repository();
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

    // With sample parts, an edit, an archived revision and a team code,
    // each undo refuses the data the older version cannot read. An undo of
    // several steps is refused as a whole, so nothing is changed, not even
    // the newest steps, which this data allows.
    let populated = tempfile::tempdir().unwrap();
    let options = services::options(&root, populated.path());
    let (server, admin, _superuser) = services::start(&root, populated.path()).unwrap();
    let seeded = sample_library::seed(&admin, &["res"]).unwrap();
    let (number, details) = current_details(&admin, IPN);
    let mut edit = EditCommand::new(IPN, "Before the rollback").editing_details(&details, number);
    assert!(edit.set_field("Value", "47R"));
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    let (number, revision) = current_revision(&admin, IPN);
    let mut cad = EditCommand::new(IPN, "Use KiCad's symbol").editing_cad(&revision, number);
    assert!(cad.set_field("Symbol", "Device:R"));
    assert_eq!(submit_edit(&admin, &cad).unwrap().status, 200);
    let archive = ArchiveCommand::new(IPN, 2, true, "Wrong symbol");
    assert_eq!(submit_archive(&admin, &archive).unwrap().status, 200);
    submit_team_code(&admin, "MA").unwrap();
    server.stop();

    let refuses = |count: &str, says: &str| {
        let said = migrate(&root, &options, &["down", count]);
        assert!(
            said.contains(says)
                && said.contains("Nothing was changed")
                && !said.contains("Reverted"),
            "{said}"
        );
    };
    refuses("7", "categories set up without a CSV file");
    set_aside_migration(&options, "1792600000_category_changes.js");
    refuses("6", "team has a code");
    set_aside_migration(&options, "1792500000_team_settings.js");
    refuses("5", "parts added in the app");
    set_aside_migration(&options, "1792400000_new_parts.js");
    refuses("4", "has categories");
    // With that step set aside, the older undos still refuse their own
    // data: here the archived revision, after three steps this data allows.
    set_aside_migration(&options, "1792300000_categories.js");
    refuses("4", "has archived revisions");
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
    // The sample parts' first details and the edit of RES-0000-0470.
    assert_eq!(details as usize, seeded.parts.len() + 1);
    drop(database);

    // Without parts, every catalog migration rolls back, and the users
    // collection is back as it was.
    let state = tempfile::tempdir().unwrap();
    let options = services::options(&root, state.path());
    let (server, _admin, superuser) = services::start(&root, state.path()).unwrap();
    server.stop();
    let (rule, fields) = users_rule(&options.data_dir);
    assert_eq!(
        rule,
        "(id = @request.auth.id) && @request.body.can_edit_catalog:isset = false && @request.body.name:isset = false"
    );
    assert!(fields.contains("can_edit_catalog"));
    let said = migrate(
        &root,
        &options,
        &["down", &CATALOG_MIGRATIONS.len().to_string()],
    );
    for reverted in CATALOG_MIGRATIONS {
        assert!(said.contains(&format!("Reverted {reverted}")), "{said}");
    }

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
    let (rule, fields) = users_rule(&options.data_dir);
    assert_eq!(rule, "id = @request.auth.id");
    assert!(!fields.contains("can_edit_catalog"), "{fields}");

    // Starting again applies them all again, and a team starts from the
    // sample library on the new schema.
    let binary = native_services::pocketbase_binary(&root).unwrap();
    let server = native_services::PocketBaseServer::start(&binary, &options, &superuser).unwrap();
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    let again = sample_library::seed(&admin, &["res"]).unwrap();
    assert_eq!(again.parts.len(), seeded.parts.len());
    server.stop();
}

#[test]
fn team_server_data_from_before_numbered_cad_revisions_is_refused_not_changed() {
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let options = services::options(&root, state.path());
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    sample_library::seed(&admin, &["res"]).unwrap();
    let revision = read_back(&admin).unwrap().cad_item_revisions[0].clone();
    server.stop();

    // Undoing is refused while there is data: the newest undo refuses the
    // categories set up without a CSV file, and the undo stops there as a
    // whole, so the numbering is never undone either.
    let said = migrate(&root, &options, &["down", "7"]);
    assert!(
        said.contains("which the older version cannot read"),
        "{said}"
    );
    assert!(!said.contains("Reverted"), "{said}");

    // Make the data look like data saved before numbered CAD item
    // revisions: the migration not yet applied, and one CAD item revision
    // whose ID is not derived from its number.
    let old_id = "0123456789abcde";
    let database = rusqlite::Connection::open(options.data_dir.join("data.db")).unwrap();
    assert_eq!(
        database
            .execute(
                "DELETE FROM _migrations WHERE file = '1791700000_cad_item_revision_numbers.js'",
                [],
            )
            .unwrap(),
        1,
        "the undo changed nothing"
    );
    database
        .execute(
            "UPDATE catalog_cad_item_revisions SET id = ?1 WHERE id = ?2",
            rusqlite::params![old_id, revision.id],
        )
        .unwrap();
    drop(database);

    // Starting refuses with a plain message and changes nothing.
    let refused = services::start(&root, state.path()).err().unwrap();
    assert!(
        refused.to_string().contains("cannot be upgraded"),
        "{refused}"
    );
    let database = rusqlite::Connection::open_with_flags(
        options.data_dir.join("data.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let applied_migrations: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM _migrations WHERE file = '1791700000_cad_item_revision_numbers.js'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied_migrations, 0);
    let kept: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM catalog_cad_item_revisions WHERE id = ?1",
            [old_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
}

#[test]
fn a_cad_item_revision_has_at_most_one_successor() {
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let options = services::options(&root, state.path());
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    sample_library::seed(&admin, &["res"]).unwrap();
    let revision = read_back(&admin).unwrap().cad_item_revisions[0].clone();
    server.stop();

    // Adds a copy of `revision` that follows it, with this ID and number,
    // straight into the database, past the routes and their guards.
    let follow = |id: &str, number: u32| {
        let database = rusqlite::Connection::open(options.data_dir.join("data.db")).unwrap();
        database.execute_batch(&format!(
            "DROP TABLE IF EXISTS temp.copy;
             CREATE TEMP TABLE copy AS SELECT * FROM catalog_cad_item_revisions WHERE id = '{0}';
             UPDATE copy SET id = '{id}', number = {number}, predecessor = '{0}';
             INSERT INTO catalog_cad_item_revisions SELECT * FROM copy;",
            revision.id
        ))
    };
    // One successor fits; a second one, which would fork the CAD item's
    // history, is refused by the unique index on predecessor.
    follow("aaaaaaaaaaaaaa1", 2).unwrap();
    let forked = follow("aaaaaaaaaaaaaa2", 3).unwrap_err().to_string();
    assert!(
        forked.contains("UNIQUE constraint failed: catalog_cad_item_revisions.predecessor"),
        "{forked}"
    );

    // Undoing the index loses nothing and lets the fork in; starting again
    // then refuses with a plain message and changes nothing. (The newer
    // steps refuse this data, so they are set aside.)
    for newer in &CATALOG_MIGRATIONS[..4] {
        set_aside_migration(&options, newer);
    }
    let said = migrate(&root, &options, &["down", "1"]);
    assert!(
        said.contains("Reverted 1792200000_cad_item_revision_successor.js"),
        "{said}"
    );
    follow("aaaaaaaaaaaaaa2", 3).unwrap();
    let refused = services::start(&root, state.path()).err().unwrap();
    assert!(
        refused
            .to_string()
            .contains("history forks and it cannot be upgraded"),
        "{refused}"
    );
    let database = rusqlite::Connection::open_with_flags(
        options.data_dir.join("data.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let applied_migrations: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM _migrations WHERE file = '1792200000_cad_item_revision_successor.js'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied_migrations, 0);
    let followers: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM catalog_cad_item_revisions WHERE predecessor = ?1",
            [&revision.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(followers, 2);
}

#[test]
fn team_server_data_from_before_shared_part_details_is_refused_not_changed() {
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let options = services::options(&root, state.path());
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    sample_library::seed(&admin, &["res"]).unwrap();
    let revisions = read_back(&admin).unwrap().part_revisions.len();
    server.stop();

    // Make the data look like data saved before part details were kept
    // apart: the migration not yet applied, with part revisions stored.
    let database = rusqlite::Connection::open(options.data_dir.join("data.db")).unwrap();
    assert_eq!(
        database
            .execute(
                "DELETE FROM _migrations WHERE file = '1791800000_shared_part_details.js'",
                [],
            )
            .unwrap(),
        1
    );
    drop(database);

    // Starting refuses with a plain message and changes nothing.
    let refused = services::start(&root, state.path()).err().unwrap();
    let message = refused.to_string();
    assert!(
        message.contains("before part details were kept apart from revisions")
            && message.contains("cannot be upgraded")
            && message.contains("Start again with an empty data folder"),
        "{message}"
    );
    let database = rusqlite::Connection::open_with_flags(
        options.data_dir.join("data.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let applied_migrations: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM _migrations WHERE file = '1791800000_shared_part_details.js'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied_migrations, 0);
    let kept: i64 = database
        .query_row("SELECT COUNT(*) FROM catalog_part_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(kept, revisions as i64);
}

#[test]
fn an_edit_never_saves_a_field_in_the_wrong_document() {
    // Stored details that wrongly list Symbol (here written straight into
    // data.db) cannot be saved again through an edit: the route checks each
    // new document, and writes nothing.
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    sample_library::seed(&admin, &["res"]).unwrap();
    let stored = read_back(&admin).unwrap();
    let part = stored.part(IPN).unwrap();
    let saved = stored.part_details(&part.current_details).unwrap();
    let mut wrong: PartDetails = serde_json::from_str(&saved.snapshot).unwrap();
    wrong.fields.push(catalog::DetailField {
        name: "Symbol".into(),
        value: "g-res:R_US".into(),
    });
    let snapshot = wrong.canonical_json();
    // Stop the server before touching data.db, then start it again on the
    // same data (mx-404964).
    server.stop();
    tamper(
        state.path(),
        "catalog_part_details",
        "snapshot",
        &saved.id,
        &snapshot,
    );
    tamper(
        state.path(),
        "catalog_part_details",
        "snapshot_sha256",
        &saved.id,
        &sha256_hex(&snapshot),
    );
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let before = read_back(&admin).unwrap();
    let mut edit = EditCommand::new(IPN, "Move the symbol into the details")
        .editing_details(&wrong, saved.number);
    assert!(edit.set_field("Symbol", "Device:R"));
    match submit_edit(&admin, &edit) {
        Err(CollabError::Http {
            status: 400,
            message,
        }) => assert!(
            message.contains("which belongs to the part's revision"),
            "{message}"
        ),
        other => panic!("expected 400, got {other:?}"),
    }
    assert_eq!(read_back(&admin).unwrap(), before);
    server.stop();
}

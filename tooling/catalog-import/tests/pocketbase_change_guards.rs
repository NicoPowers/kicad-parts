//! Every new Catalog record names the Change saved in its own transaction
//! (seed kicad-parts-2707). Desktops fetch a Change's records once, when
//! they apply it, so a record that named an older Change would never reach
//! them. The write guards (`catalog_write_guards.pb.js`) refuse such a save
//! from the record API, superusers included, and from server code. These
//! tests play buggy server code through a test route. Each test starts its
//! own disposable PocketBase on 127.0.0.1 with the sample library's
//! resistors; the repository is only read.
//!
//! The CSV review and remembered-match cases need more setup, so they live
//! with their own guard tests: `pocketbase_csv_reviews.rs` and
//! `pocketbase_part_categories.rs`.

use catalog::PartRevision;
use catalog::pocketbase::{
    ArchiveCommand, CatalogReadback, EditCommand, read_back, submit_archive, submit_edit,
};
use catalog_import::services;
use collaboration::{HttpReply, PocketBaseClient};
use native_services::PocketBaseServer;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const IPN: &str = "RES-0000-0470";
const OTHER: &str = "RES-0000-1000";

/// Every field, in every collection, that names a Change. The guards find
/// these in the schema themselves; this list makes a new one visible in
/// review, because the test below fails until it is added here.
const CHANGE_FIELDS: [(&str, &str); 12] = [
    ("catalog_archive_entries", "change"),
    ("catalog_audit_entries", "change"),
    ("catalog_blobs", "first_change"),
    ("catalog_cad_item_revisions", "change"),
    ("catalog_cad_items", "first_change"),
    ("catalog_categories", "first_change"),
    ("catalog_category_matches", "change"),
    ("catalog_category_revisions", "change"),
    ("catalog_csv_reviews", "change"),
    ("catalog_part_details", "change"),
    ("catalog_part_revisions", "change"),
    ("catalog_parts", "first_change"),
];

/// A route that saves Catalog records the way buggy server code might.
/// One request is one transaction.
///
/// Body:
/// - `changes`: how many new Changes to save first (default 0).
///   `seqGap` skips that many numbers before the first one.
/// - `records`: records to save, in order, each
///   `{ collection, update?, next?, deriveId?, fields }`. `update` names a
///   record to change instead of making one. `next` names an owner: the
///   record starts as a copy of the owner's newest numbered record, as the
///   next number. `deriveId` is `[kind, key]` for the record's ID.
///   `withoutHooks` saves it with no hooks at all, guards included.
///   In `fields`, "$new" stands for the last Change saved here, and
///   "$last" for the record saved just before.
/// - `uncaught`: let a refusal through, so PocketBase answers it.
/// - `noTransaction`: save with `$app` directly, in no transaction.
const SAVE_HOOK: &str = r#"
routerAdd("POST", "/api/test/save", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  const save = (app) => {
    let fresh = "";
    for (let i = 0; i < (body.changes || 0); i++) {
      const last = app.findRecordsByFilter("catalog_changes", "seq > 0", "-seq", 1, 0)[0];
      const correlation = "test-" + $security.randomString(12);
      const change = new Record(app.findCollectionByNameOrId("catalog_changes"));
      change.set("id", lib.deriveId("change", correlation));
      change.set("seq", last.getInt("seq") + 1 + (i === 0 ? body.seqGap || 0 : 0));
      change.set("kind", "edit");
      change.set("reason", "test");
      change.set("correlation_id", correlation);
      change.set("actor", "test");
      change.set("sources", "{}");
      change.set("summary", "{}");
      app.save(change);
      fresh = change.id;
    }
    let last = "";
    for (const item of body.records || []) {
      let made;
      if (item.update) {
        made = app.findRecordById(item.collection, item.update);
      } else {
        made = new Record(app.findCollectionByNameOrId(item.collection));
        if (item.next) {
          const rule = lib.NUMBERED_RECORDS[item.collection];
          const newest = app.findRecordsByFilter(item.collection, rule.ownerField + " = {:owner}", "-number", 1, 0, {
            owner: item.next,
          })[0];
          const fields = made.collection().fields;
          for (let i = 0; i < fields.length; i++) {
            const name = fields[i].name;
            if (name !== "id" && name !== "created" && name !== "updated") {
              made.set(name, newest.get(name));
            }
          }
          const number = newest.getInt("number") + 1;
          made.set("number", number);
          made.set("predecessor", newest.id);
          made.set("id", lib.deriveId(rule.idKind, item.next + "\n" + number));
        }
        if (item.deriveId) {
          made.set("id", lib.deriveId(item.deriveId[0], item.deriveId[1]));
        }
      }
      for (const name of Object.keys(item.fields || {})) {
        const value = item.fields[name];
        made.set(name, value === "$new" ? fresh : value === "$last" ? last : value);
      }
      if (item.withoutHooks) {
        app.unsafeWithoutHooks().save(made);
      } else {
        app.save(made);
      }
      last = made.id;
    }
  };
  try {
    if (body.noTransaction) {
      save($app);
    } else {
      $app.runInTransaction(save);
    }
  } catch (error) {
    if (body.uncaught) {
      throw error;
    }
    return e.json(409, { refused: String(error) });
  }
  return e.json(200, { saved: true });
}, $apis.requireSuperuserAuth());
"#;

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    log_file: PathBuf,
    _state: tempfile::TempDir,
}

impl Team {
    /// A team server with the repository's hooks plus `SAVE_HOOK`, and
    /// the sample resistors.
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let hooks = state.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        for entry in fs::read_dir(root.join("infra/pocketbase/pb_hooks")).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), hooks.join(entry.file_name())).unwrap();
        }
        fs::write(hooks.join("test_save.pb.js"), SAVE_HOOK).unwrap();
        let mut options = services::options(&root, state.path());
        options.hooks_dir = hooks;
        let superuser = native_services::Superuser::generate();
        let binary = native_services::pocketbase_binary(&root).unwrap();
        let server = PocketBaseServer::start(&binary, &options, &superuser).unwrap();
        let admin = PocketBaseClient::new(server.url())
            .authenticate("_superusers", &superuser.email, &superuser.password)
            .unwrap();
        sample_library::seed(&admin, &["res"]).unwrap();
        Self {
            server,
            admin,
            log_file: options.log_file,
            _state: state,
        }
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }

    fn save(&self, body: Value) -> HttpReply {
        self.admin.post_json("/api/test/save", &body).unwrap()
    }

    /// Saves `body` and expects the write guards to refuse it as a
    /// programming error, saying each of `says`.
    fn refused(&self, body: Value, says: &[&str]) {
        let reply = self.save(body.clone());
        assert_eq!(reply.status, 409, "{body}: {}", reply.body);
        let answer: Value = serde_json::from_str(&reply.body).unwrap();
        let message = answer["refused"].as_str().unwrap();
        assert!(
            message.contains("Programming error in the team server"),
            "{message}"
        );
        for words in says {
            assert!(message.contains(words), "{words}: {message}");
        }
    }

    fn saved(&self, body: Value) {
        let reply = self.save(body.clone());
        assert_eq!(reply.status, 200, "{body}: {}", reply.body);
    }
}

/// Changes the Symbol of `ipn` with an edit through the route, which adds a
/// revision in a new Change.
fn change_symbol(team: &Team, ipn: &str, symbol: &str) {
    let readback = team.read();
    let part = readback.part(ipn).unwrap();
    let revision: PartRevision = serde_json::from_str(
        &readback
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut edit = EditCommand::new(ipn, &format!("Use {symbol}"))
        .editing_cad(&revision, part.current_revision_number);
    assert!(edit.set_field("Symbol", symbol));
    let reply = submit_edit(&team.admin, &edit).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
}

/// Every (collection, field) whose relation points at catalog_changes,
/// sorted, read from the server's schema.
fn change_fields(admin: &PocketBaseClient) -> Vec<(String, String)> {
    let reply = admin.get("/api/collections?perPage=500").unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let collections: Value = serde_json::from_str(&reply.body).unwrap();
    let items = collections["items"].as_array().unwrap();
    let changes = items
        .iter()
        .find(|collection| collection["name"] == "catalog_changes")
        .unwrap()["id"]
        .clone();
    let mut found = Vec::new();
    for collection in items {
        for field in collection["fields"].as_array().unwrap() {
            if field["type"] == "relation" && field["collectionId"] == changes {
                found.push((
                    collection["name"].as_str().unwrap().to_string(),
                    field["name"].as_str().unwrap().to_string(),
                ));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_record_api_refuses_every_record_that_names_a_change() {
    let team = Team::start();
    let expected: Vec<(String, String)> = CHANGE_FIELDS
        .iter()
        .map(|(collection, field)| (collection.to_string(), field.to_string()))
        .collect();
    assert_eq!(change_fields(&team.admin), expected);
    let before = team.read();
    let first_change_id = &before.changes[0].id;
    // A superuser, who passes every collection rule, still gets 403.
    for (collection, field) in CHANGE_FIELDS {
        let mut body = serde_json::Map::new();
        body.insert(field.to_string(), json!(first_change_id));
        let reply = team
            .admin
            .post_json(
                &format!("/api/collections/{collection}/records"),
                &Value::Object(body),
            )
            .unwrap();
        assert_eq!(reply.status, 403, "{collection}: {}", reply.body);
    }
    assert_eq!(team.read(), before, "nothing was written");
    team.server.stop();
}

#[test]
fn server_code_must_name_the_change_saved_in_its_own_transaction() {
    let team = Team::start();
    // Revision 2 of IPN, archived and then unarchived through the route:
    // the archive Change is now an old Change of the right kind.
    change_symbol(&team, IPN, "Device:R");
    let reply = submit_archive(&team.admin, &ArchiveCommand::new(IPN, 2, true, "Wrong")).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let reply = submit_archive(
        &team.admin,
        &ArchiveCommand::new(IPN, 2, false, "Right after all"),
    )
    .unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let before = team.read();
    let first_change_id = before.changes[0].id.clone();
    let archive_change = before
        .changes
        .iter()
        .find(|change| change.kind == "archive")
        .unwrap()
        .clone();
    let part = before.part(IPN).unwrap().clone();
    let revision = |number: u32| {
        before
            .part_revisions
            .iter()
            .find(|revision| revision.part == part.id && revision.number == number)
            .unwrap()
            .id
            .clone()
    };
    let cad_item = before.cad_item_revisions[0].cad_item.clone();
    let category = before.categories[0].id.clone();

    // For each collection, a record that passes the collection's own
    // checks but names an old Change. `old` is put in its Change field.
    let records = |old: &str| -> Vec<(&str, Value)> {
        vec![
            (
                "catalog_blobs",
                json!({ "fields": { "sha256": "0".repeat(64), "size": 1, "first_change": old } }),
            ),
            (
                "catalog_cad_items",
                json!({ "fields": { "kind": "symbol", "key": "g-test:T", "first_change": old } }),
            ),
            (
                "catalog_cad_item_revisions",
                json!({ "next": cad_item, "fields": { "change": old } }),
            ),
            (
                "catalog_parts",
                json!({ "fields": {
                    "ipn": "RES-9999-0001", "ipn_prefix": "RES", "variant": "0001",
                    "current_revision_number": 1, "current_details_number": 1,
                    "first_change": old,
                } }),
            ),
            (
                "catalog_part_revisions",
                json!({ "next": part.id, "fields": { "change": old } }),
            ),
            (
                "catalog_part_details",
                json!({ "next": part.id, "fields": { "change": old } }),
            ),
            (
                "catalog_categories",
                json!({ "deriveId": ["category", "zz-test"], "fields": { "code": "zz-test", "first_change": old } }),
            ),
            (
                "catalog_category_revisions",
                json!({ "next": category, "fields": { "change": old } }),
            ),
            (
                "catalog_audit_entries",
                json!({ "fields": {
                    "action": "test", "actor": "test", "reason": "test",
                    "correlation_id": "test-audit", "summary": "{}", "change": old,
                } }),
            ),
            // Archiving revision 2 again, in the old archive Change: right
            // kind, its seq, its ID, a real change of state.
            (
                "catalog_archive_entries",
                json!({
                    "deriveId": ["archive-entry", archive_change.correlation_id],
                    "fields": {
                        "part": part.id, "part_revision": revision(2), "revision_number": 2,
                        "archived": true, "change": old, "seq": archive_change.seq,
                        "current_revision_after": revision(1), "current_revision_number_after": 1,
                    },
                }),
            ),
        ]
    };
    for (collection, record) in records(&first_change_id) {
        let mut record = record.clone();
        record["collection"] = json!(collection);
        // The archive entry must name the archive Change to pass its own
        // checks; every other record names the first Change.
        if collection == "catalog_archive_entries" {
            record["fields"]["change"] = json!(archive_change.id);
        }
        let (field, named) = record["fields"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(name, _)| name.ends_with("change"))
            .unwrap();
        // With no Change saved in the transaction...
        let record_of = format!("the new {collection} record ");
        let names = format!(
            "names Change {} in its \"{field}\" field",
            named.as_str().unwrap()
        );
        team.refused(
            json!({ "records": [record] }),
            &[&record_of, &names, "this transaction saved no Change"],
        );
        // ...and with a new Change that the record does not name.
        team.refused(
            json!({ "changes": 1, "records": [record] }),
            &[&record_of, &names, "It must name Change"],
        );
    }
    assert_eq!(team.read(), before, "nothing was written");

    // An empty Change field passes only where the field is optional: an
    // audit entry of an import run that saved no Change.
    team.saved(json!({ "records": [{
        "collection": "catalog_audit_entries",
        "fields": {
            "action": "test", "actor": "test", "reason": "test",
            "correlation_id": "test-no-change", "summary": "{}", "change": "",
        },
    }] }));
    team.refused(
        json!({ "changes": 1, "records": [{
            "collection": "catalog_parts",
            "fields": {
                "ipn": "RES-9999-0001", "ipn_prefix": "RES", "variant": "0001",
                "current_revision_number": 1, "current_details_number": 1,
                "first_change": "",
            },
        }] }),
        &["names no Change in its \"first_change\" field"],
    );
    team.server.stop();
}

#[test]
fn a_transaction_saves_one_change_numbered_right_after_the_newest() {
    let team = Team::start();
    let before = team.read();
    let newest = before.changes.last().unwrap().seq;
    team.refused(
        json!({ "changes": 2 }),
        &["this transaction already saved Change"],
    );
    team.refused(
        json!({ "changes": 1, "seqGap": 1 }),
        &[&format!("a new Change must be numbered {}", newest + 1)],
    );
    assert_eq!(team.read(), before, "nothing was written");
    team.saved(json!({ "changes": 1 }));
    assert_eq!(team.read().changes.last().unwrap().seq, newest + 1);
    team.server.stop();
}

#[test]
fn a_catalog_write_outside_a_transaction_is_refused() {
    let team = Team::start();
    let before = team.read();
    let part = before.part(IPN).unwrap().clone();
    let says = &[
        "this save is not in a transaction. Save the Change and its records in one $app.runInTransaction.",
    ];
    // A new Change on its own, and then a Change with its details, each
    // saved with $app directly: their records could be seen apart.
    team.refused(json!({ "noTransaction": true, "changes": 1 }), says);
    team.refused(
        json!({ "noTransaction": true, "records": [
            { "collection": "catalog_part_details", "next": part.id, "fields": { "change": before.changes[0].id } },
        ] }),
        says,
    );
    assert_eq!(team.read(), before, "nothing was written");
    team.server.stop();
}

#[test]
fn a_new_part_starts_with_no_current_details_or_revision() {
    let team = Team::start();
    let before = team.read();
    let other = before.part(OTHER).unwrap().clone();
    // A new part, in its own new Change, that already points at another
    // part's records: a desktop signing in would show it, one following
    // Changes never would.
    for (pointer, value) in [
        ("current_details", &other.current_details),
        ("current_revision", &other.current_revision),
    ] {
        let mut fields = json!({
            "ipn": "RES-9999-0001", "ipn_prefix": "RES", "variant": "0001",
            "current_revision_number": 1, "current_details_number": 1,
            "first_change": "$new",
        });
        fields[pointer] = json!(value);
        team.refused(
            json!({ "changes": 1, "records": [{ "collection": "catalog_parts", "fields": fields }] }),
            &["the new part RES-9999-0001 must start with no current details or revision"],
        );
    }
    assert_eq!(team.read(), before, "nothing was written");
    team.server.stop();
}

#[test]
fn a_part_moves_only_to_what_its_own_transactions_change_saved() {
    let team = Team::start();
    let readback = team.read();
    let other = readback.part(OTHER).unwrap().clone();
    // The way the routes do it: new details in a new Change, and the part
    // moved to them, in one transaction. Saving the details already moved
    // the part, so the route's own move changes nothing.
    team.saved(json!({ "changes": 1, "records": [
        { "collection": "catalog_part_details", "next": other.id, "fields": { "change": "$new" } },
        { "collection": "catalog_parts", "update": other.id, "fields": {
            "current_details": "$last",
            "current_details_number": other.current_details_number + 1,
        } },
    ] }));
    assert_eq!(
        team.read().part(OTHER).unwrap().current_details_number,
        other.current_details_number + 1
    );

    // New details and a new revision, saved with their own Change, move
    // the part there by themselves. Desktops following Changes make them
    // current too, so a desktop signing in later agrees.
    let part = readback.part(IPN).unwrap().clone();
    team.saved(json!({ "changes": 1, "records": [
        { "collection": "catalog_part_details", "next": part.id, "fields": { "change": "$new" } },
        { "collection": "catalog_part_revisions", "next": part.id, "fields": { "change": "$new" } },
    ] }));
    let moved = team.read().part(IPN).unwrap().clone();
    assert_eq!(
        (moved.current_details_number, moved.current_revision_number),
        (
            part.current_details_number + 1,
            part.current_revision_number + 1
        )
    );

    // Details and a revision saved with no hooks at all, as raw server
    // code might: the part stays where it was.
    team.saved(json!({ "changes": 1, "records": [
        { "collection": "catalog_part_details", "next": part.id, "withoutHooks": true, "fields": { "change": "$new" } },
        { "collection": "catalog_part_revisions", "next": part.id, "withoutHooks": true, "fields": { "change": "$new" } },
    ] }));
    let stored = team.read();
    assert_eq!(stored.part(IPN).unwrap(), &moved);
    let newest_details = stored
        .part_details
        .iter()
        .filter(|details| details.part == part.id)
        .max_by_key(|details| details.number)
        .unwrap()
        .clone();
    let newest_revision = stored
        .part_revisions
        .iter()
        .filter(|revision| revision.part == part.id)
        .max_by_key(|revision| revision.number)
        .unwrap()
        .clone();
    // A later save cannot move the part there, with no Change or with a
    // new Change that did not save them: desktops would never hear of it.
    for changes in [0, 1] {
        team.refused(
            json!({ "changes": changes, "records": [{
                "collection": "catalog_parts", "update": part.id, "fields": {
                    "current_details": newest_details.id,
                    "current_details_number": newest_details.number,
                },
            }] }),
            &[&format!(
                "the current details of {IPN} moved to details that this transaction's Change did not save"
            )],
        );
        team.refused(
            json!({ "changes": changes, "records": [{
                "collection": "catalog_parts", "update": part.id, "fields": {
                    "current_revision": newest_revision.id,
                    "current_revision_number": newest_revision.number,
                },
            }] }),
            &[&format!(
                "the current revision of {IPN} moved, but this transaction's Change saved neither"
            )],
        );
    }
    assert_eq!(team.read(), stored, "nothing was written");
    team.server.stop();
}

/// Saves `body` with the refusal let through, and checks that PocketBase
/// answers 500 and logs the message as an error in both places. Returns
/// the message.
fn refused_as_programming_error(team: &Team, body: Value) -> String {
    let mut body = body;
    body["uncaught"] = json!(true);
    let reply = team.save(body);
    assert_eq!(reply.status, 500, "{}", reply.body);
    let answer: Value = serde_json::from_str(&reply.body).unwrap();
    let message = answer["message"].as_str().unwrap().to_string();
    assert!(
        message.starts_with("Programming error in the team server, not a user mistake: ")
            && message.ends_with("The write was refused. See catalog_write_guards.pb.js."),
        "{message}"
    );
    // In the server's own output...
    let output = fs::read_to_string(&team.log_file).unwrap();
    assert!(output.contains(&message), "{output}");
    // ...and as an error in the dashboard's Logs, which PocketBase writes
    // in batches.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reply = team
            .admin
            .get("/api/logs?perPage=50&sort=-created&filter=(level%3E%3D8)")
            .unwrap();
        assert_eq!(reply.status, 200, "{}", reply.body);
        let logs: Value = serde_json::from_str(&reply.body).unwrap();
        if logs["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["message"] == message.as_str())
        {
            return message;
        }
        assert!(Instant::now() < deadline, "not in the Logs: {}", reply.body);
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn every_guard_refusal_answers_500_and_is_logged_as_an_error() {
    let team = Team::start();
    let part = team.read().part(IPN).unwrap().clone();
    let first_change_id = team.read().changes[0].id.clone();
    // The rule of this seed: details that name an old Change.
    let message = refused_as_programming_error(
        &team,
        json!({ "records": [
            { "collection": "catalog_part_details", "next": part.id, "fields": { "change": first_change_id } },
        ] }),
    );
    assert!(
        message.contains("the new catalog_part_details record"),
        "{message}"
    );
    // An older rule: details that skip a number.
    let message = refused_as_programming_error(
        &team,
        json!({ "changes": 1, "records": [
            { "collection": "catalog_part_details", "next": part.id, "fields": { "change": "$new", "number": 99 } },
        ] }),
    );
    assert!(message.contains("must be number"), "{message}");
    team.server.stop();
}

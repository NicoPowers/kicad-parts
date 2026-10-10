//! Edits and restores of one part through the protected Catalog routes,
//! records that never change once written, and a second client that
//! converges through realtime notifications and catching up from its
//! checkpoint. Each test starts its own disposable PocketBase on 127.0.0.1
//! with the resistor CSV file imported; the repository is only read.

use catalog::pocketbase::{
    CatalogReadback, EditCommand, ImportCommand, ImportOutcome, PublishOutcome, PublishReply,
    RestoreCommand, StoredChange, read_back, submit_edit, submit_import, submit_restore,
};
use catalog::replica::CatalogReplica;
use catalog::{CadReferenceTarget, DetailField, ManufacturerPart, PartDetails, sha256_hex};
use catalog_import::services;
use collaboration::{ApplyResult, CollabError, PocketBaseClient, RealtimeConnection};
use native_services::PocketBaseServer;
use serde_json::{Value, json};
use std::time::Duration;

const IPN: &str = "RES-0000-0470";

/// A started PocketBase with the resistors imported, its superuser client,
/// and the state directory that must outlive it.
struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    _state: tempfile::TempDir,
}

impl Team {
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let import = services::plan(&root, "g-res").unwrap();
        let reply = submit_import(&admin, &ImportCommand::new(&import, "Import the resistors"))
            .unwrap_or_else(|error| panic!("{error}\n{}", server_errors(&admin)));
        assert!(
            matches!(reply.outcome, ImportOutcome::Applied { .. }),
            "{}",
            reply.body
        );
        Self {
            server,
            admin,
            _state: state,
        }
    }

    fn url(&self) -> &str {
        self.server.url()
    }

    /// Adds a team member and signs them in.
    fn member(&self, name: &str, can_edit: bool) -> PocketBaseClient {
        let email = format!("{}@example.test", name.to_lowercase().replace(' ', "."));
        let password = native_services::random_token(24);
        self.admin
            .post_json(
                "/api/collections/users/records",
                &json!({
                    "email": email,
                    "name": name,
                    "password": password,
                    "passwordConfirm": password,
                    "verified": true,
                    "can_edit_catalog": can_edit,
                }),
            )
            .unwrap()
            .success()
            .unwrap();
        PocketBaseClient::new(self.url())
            .authenticate("users", &email, &password)
            .unwrap()
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }
}

/// The server's latest logged errors, for failure messages.
fn server_errors(admin: &PocketBaseClient) -> String {
    // PocketBase writes its logs in batches.
    std::thread::sleep(Duration::from_secs(3));
    admin
        .get("/api/logs?perPage=5&sort=-created&filter=level%3E0")
        .map(|reply| reply.body)
        .unwrap_or_default()
}

/// The current revision of `ipn`: its number and part details.
fn current(readback: &CatalogReadback, ipn: &str) -> (u32, PartDetails) {
    let part = readback.part(ipn).unwrap();
    let revision = readback.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

fn applied(reply: &PublishReply) -> (u64, u32, String) {
    match &reply.outcome {
        PublishOutcome::Applied {
            seq,
            revision,
            change_id,
            ..
        } => (*seq, revision.number, change_id.clone()),
        other => panic!("expected applied, got {other:?}: {}", reply.body),
    }
}

/// An edit of the current revision of `ipn` that sets `field` to `value`.
fn edit(readback: &CatalogReadback, ipn: &str, field: &str, value: &str) -> EditCommand {
    let (number, details) = current(readback, ipn);
    let mut command = EditCommand::starting_from(&details, number, &format!("Set {field}"));
    assert!(command.set_field(field, value), "{ipn} has no {field}");
    command
}

#[test]
fn an_editor_publishes_edits_and_restores_with_exact_cad_revisions() {
    let team = Team::start();
    let initial = team.read();
    let editor = team.member("Ada Editor", true);
    let other = team.member("Grace Editor", true);
    let viewer = team.member("Vic Viewer", false);

    // Only editors may publish: anonymous calls get 401, members without
    // edit access 403, and nothing is written.
    let command = edit(&initial, IPN, "Value", "47R");
    let anonymous = PocketBaseClient::new(team.url());
    assert!(matches!(
        submit_edit(&anonymous, &command),
        Err(CollabError::Unauthorized(_))
    ));
    assert!(matches!(
        submit_edit(&viewer, &command),
        Err(CollabError::Forbidden(_))
    ));
    assert!(matches!(
        submit_restore(&viewer, &RestoreCommand::new(IPN, 1, 1, "x")),
        Err(CollabError::Forbidden(_))
    ));
    // A member cannot give themselves edit access.
    let own = format!(
        "/api/collections/users/records/{}",
        viewer.auth_id().unwrap()
    );
    let promoted = viewer
        .patch_json(&own, &json!({ "can_edit_catalog": true }))
        .unwrap();
    assert!(promoted.success().is_err(), "{promoted:?}");
    let account: Value = team.admin.get(&own).unwrap().json().unwrap();
    assert_eq!(account["can_edit_catalog"], false);
    // ...but can still change the rest of their account (not their name;
    // see only_an_administrator_names_a_member).
    viewer
        .patch_json(&own, &json!({ "emailVisibility": true }))
        .unwrap()
        .success()
        .unwrap();
    assert_eq!(team.read(), initial);

    // An edit makes exactly one Change, one revision, one move of the
    // current revision and one audit entry. The value is awkward on
    // purpose: the stored bytes must be the canonical JSON Rust writes.
    let awkward = "47 Ω \"exact\" \\ tab\there 😀";
    let mut command = edit(&initial, IPN, "Value", awkward);
    command.manufacturer_parts.push(ManufacturerPart {
        manufacturer: "Vishay".into(),
        mpn: "ALT-47".into(),
    });
    command.reason = "Use the exact value and add a second source".into();
    let first = submit_edit(&editor, &command).unwrap();
    assert_eq!(first.status, 200, "{}", first.body);
    let (seq, number, change_id) = applied(&first);
    assert_eq!((seq, number), (2, 2));
    let after = team.read();
    assert_eq!(after.changes.len(), 2);
    let change = &after.changes[1];
    assert_eq!(
        (
            change.id.as_str(),
            change.kind.as_str(),
            change.reason.as_str()
        ),
        (
            change_id.as_str(),
            "edit",
            "Use the exact value and add a second source"
        )
    );
    assert_eq!(change.actor, format!("users:{}", editor.auth_id().unwrap()));
    assert_eq!(change.actor_name, "Ada Editor");
    let audit = after
        .audit_entries
        .iter()
        .find(|entry| entry.change == change.id)
        .unwrap();
    assert_eq!(
        (audit.action.as_str(), audit.actor_name.as_str()),
        ("catalog.edit", "Ada Editor")
    );
    assert_eq!(after.audit_entries.len(), initial.audit_entries.len() + 1);
    assert_eq!(after.part_revisions.len(), initial.part_revisions.len() + 1);
    let part = after.part(IPN).unwrap();
    assert_eq!(part.current_revision_number, 2);
    let revision = after.part_revision(&part.current_revision).unwrap();
    let first_revision = initial
        .part_revision(&initial.part(IPN).unwrap().current_revision)
        .unwrap();
    assert_eq!(revision.predecessor, first_revision.id);
    let details: PartDetails = serde_json::from_str(&revision.snapshot).unwrap();
    assert_eq!(details.canonical_json(), revision.snapshot);
    assert_eq!(sha256_hex(&revision.snapshot), revision.snapshot_sha256);
    assert_eq!(details.field("Value"), Some(awkward));
    assert_eq!(details.manufacturer_parts.len(), 2);
    // The CAD bindings did not move: the same exact CAD item revisions.
    let (_, first_details) = current(&initial, IPN);
    assert_eq!(details.cad_references, first_details.cad_references);
    assert_eq!(
        revision.cad_item_revisions,
        first_revision.cad_item_revisions
    );
    let source: Value = serde_json::from_str(&revision.source).unwrap();
    assert_eq!(source["source"], "edit");
    assert_eq!(source["previous_revision"]["number"], 1);
    // Revision 1 and every other part are exactly as they were.
    for old in &initial.part_revisions {
        assert_eq!(after.part_revision(&old.id), Some(old));
    }
    for old in initial.parts.iter().filter(|part| part.ipn != IPN) {
        assert_eq!(after.part(&old.ipn), Some(old));
    }

    // Repeating the command replays its answer and writes nothing.
    let replay = submit_edit(&editor, &command).unwrap();
    assert!(replay.replayed);
    assert_eq!((replay.status, &replay.body), (200, &first.body));
    assert_eq!(team.read(), after);
    // Reusing its correlation ID for another edit is refused.
    let mut reused = command.clone();
    reused.reason = "something else".into();
    let refused = submit_edit(&editor, &reused).unwrap();
    assert_eq!(refused.status, 409);
    assert!(matches!(
        refused.outcome,
        PublishOutcome::CorrelationReused { .. }
    ));

    // An edit that changes nothing is rejected, and its answer replayed.
    let unchanged = edit(&after, IPN, "Value", awkward);
    let noop = submit_edit(&editor, &unchanged).unwrap();
    assert_eq!(noop.status, 422, "{}", noop.body);
    assert!(matches!(noop.outcome, PublishOutcome::Noop { .. }));
    let again = submit_edit(&editor, &unchanged).unwrap();
    assert_eq!((again.status, again.replayed), (422, true));
    assert_eq!(team.read(), after);

    // A stale edit is a conflict that names the current revision, so the
    // client can show both. Conflicts are not stored: a retry is checked
    // again.
    let stale = edit(&initial, IPN, "Value", "48");
    for _ in 0..2 {
        let conflict = submit_edit(&other, &stale).unwrap();
        assert_eq!((conflict.status, conflict.replayed), (409, false));
        let PublishOutcome::Conflict {
            expected_revision,
            current_revision,
            ..
        } = &conflict.outcome
        else {
            panic!("expected a conflict, got {}", conflict.body);
        };
        assert_eq!(*expected_revision, 1);
        assert_eq!(current_revision.number, 2);
        assert_eq!(current_revision.snapshot, revision.snapshot);
        assert_eq!(
            (
                current_revision.change_kind.as_str(),
                current_revision.actor_name.as_str()
            ),
            ("edit", "Ada Editor")
        );
        assert_eq!(current_revision.details().unwrap(), details);
    }
    assert_eq!(team.read(), after);

    // Supplier data can never enter the part details: an edit cannot add,
    // drop, rename or reorder fields.
    let bad = |change: &dyn Fn(&mut EditCommand)| {
        let mut command = edit(&after, IPN, "Value", "49");
        change(&mut command);
        match submit_edit(&editor, &command) {
            Err(CollabError::Http { status: 400, .. }) => {}
            other => panic!("expected 400, got {other:?}"),
        }
    };
    bad(&|command| {
        command.fields.push(DetailField {
            name: "LCSC".into(),
            value: "C1".into(),
        })
    });
    bad(&|command| {
        command.fields.pop();
    });
    bad(&|command| command.fields[0].name = "Supplier".into());
    bad(&|command| command.fields.swap(0, 1));
    bad(&|command| {
        command.manufacturer_parts.push(ManufacturerPart {
            manufacturer: "Yageo".into(),
            mpn: String::new(),
        })
    });
    bad(&|command| {
        let first = command.manufacturer_parts[0].clone();
        command.manufacturer_parts.push(first);
    });
    bad(&|command| command.reason = "  ".into());
    // A symbol the Catalog does not have cannot be used.
    bad(&|command| {
        command.set_field("Symbol", "g-pas:NOT_A_SYMBOL");
    });
    assert_eq!(team.read(), after);

    // Pointing the symbol at another library binds no CAD item revision;
    // pointing it back binds the CAD item's exact current revision.
    let symbol = first_details.field("Symbol").unwrap().to_owned();
    let external = submit_edit(&editor, &edit(&after, IPN, "Symbol", "Device:R")).unwrap();
    assert_eq!(applied(&external).1, 3);
    let moved = team.read();
    let (_, details) = current(&moved, IPN);
    let symbol_target = |details: &PartDetails| {
        details
            .cad_references
            .iter()
            .find(|reference| reference.role == catalog::CadReferenceRole::Symbol)
            .map(|reference| (reference.reference.clone(), reference.target.clone()))
            .unwrap()
    };
    assert_eq!(
        symbol_target(&details),
        ("Device:R".into(), CadReferenceTarget::External)
    );
    let bound = &moved
        .part_revision(&moved.part(IPN).unwrap().current_revision)
        .unwrap()
        .cad_item_revisions;
    assert_eq!(bound.len(), first_revision.cad_item_revisions.len() - 1);
    let back = submit_edit(&editor, &edit(&moved, IPN, "Symbol", &symbol)).unwrap();
    let PublishOutcome::Applied {
        cad_item_revisions, ..
    } = &back.outcome
    else {
        panic!("{}", back.body);
    };
    let rebound = team.read();
    let (_, details) = current(&rebound, IPN);
    assert_eq!(symbol_target(&details), symbol_target(&first_details));
    let mut expected = first_revision.cad_item_revisions.clone();
    let mut actual = cad_item_revisions.clone();
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
    for id in cad_item_revisions {
        assert!(rebound.cad_item_revision(id).is_some(), "{id} is stored");
    }
    // No edit created a CAD item, CAD item revision or blob.
    assert_eq!(rebound.cad_items, initial.cad_items);
    assert_eq!(rebound.cad_item_revisions, initial.cad_item_revisions);
    assert_eq!(rebound.blobs.len(), initial.blobs.len());

    // Restoring revision 1 adds revision 5 with exactly its details and
    // CAD item revisions; revision 1 itself is untouched.
    let (number, _) = current(&rebound, IPN);
    assert_eq!(number, 4);
    let restore = RestoreCommand::new(IPN, 4, 1, "Go back to the imported details");
    let restored = submit_restore(&editor, &restore).unwrap();
    let (seq, number, change_id) = applied(&restored);
    assert_eq!((seq, number), (5, 5));
    let last = team.read();
    let revision = last
        .part_revision(&last.part(IPN).unwrap().current_revision)
        .unwrap();
    assert_eq!(revision.snapshot, first_revision.snapshot);
    assert_eq!(revision.snapshot_sha256, first_revision.snapshot_sha256);
    assert_eq!(
        revision.cad_item_revisions,
        first_revision.cad_item_revisions
    );
    let source: Value = serde_json::from_str(&revision.source).unwrap();
    assert_eq!(
        (
            source["source"].as_str(),
            source["restored_revision"]["number"].as_u64()
        ),
        (Some("restore"), Some(1))
    );
    let change = last.changes.iter().find(|c| c.id == change_id).unwrap();
    assert_eq!(change.kind, "restore");
    assert_eq!(last.part_revision(&first_revision.id), Some(first_revision));
    // Restoring it again changes nothing; a missing revision is 404; a
    // stale restore is a conflict.
    let noop = submit_restore(&editor, &RestoreCommand::new(IPN, 5, 1, "again")).unwrap();
    assert_eq!(noop.status, 422, "{}", noop.body);
    assert!(matches!(
        submit_restore(&editor, &RestoreCommand::new(IPN, 5, 99, "missing")),
        Err(CollabError::Http { status: 404, .. })
    ));
    let conflict = submit_restore(&editor, &RestoreCommand::new(IPN, 4, 2, "stale")).unwrap();
    assert!(matches!(conflict.outcome, PublishOutcome::Conflict { .. }));
    assert!(matches!(
        submit_edit(
            &editor,
            &EditCommand {
                ipn: "RES-9999-9999".into(),
                ..edit(&last, IPN, "Value", "50")
            }
        ),
        Err(CollabError::Http { status: 404, .. })
    ));

    // Changes stay numbered without gaps, and the edited part history is
    // a chain of predecessors.
    let last = team.read();
    let sequences: Vec<u64> = last.changes.iter().map(|change| change.seq).collect();
    assert_eq!(sequences, [1, 2, 3, 4, 5]);
    let part = last.part(IPN).unwrap();
    let mut cursor = part.current_revision.clone();
    let mut numbers = Vec::new();
    while let Some(revision) = last.part_revision(&cursor) {
        numbers.push(revision.number);
        cursor = revision.predecessor.clone();
    }
    assert_eq!(numbers, [5, 4, 3, 2, 1]);
    team.server.stop();
}

#[test]
fn catalog_records_never_change_even_for_superusers() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let initial = team.read();
    applied(&submit_edit(&editor, &edit(&initial, IPN, "Value", "47R")).unwrap());
    let stored = team.read();
    let part = stored.part(IPN).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    let first_revision = revision.predecessor.clone();

    let targets: Vec<(&str, &str, Value)> = vec![
        ("catalog_parts", &part.id, json!({ "ipn": "RES-9999-9999" })),
        (
            "catalog_parts",
            &part.id,
            json!({ "current_revision": first_revision, "current_revision_number": 1 }),
        ),
        (
            "catalog_part_revisions",
            &revision.id,
            json!({ "source": "not a CSV catalog row" }),
        ),
        (
            "catalog_changes",
            &stored.changes[0].id,
            json!({ "kind": "edit" }),
        ),
        (
            "catalog_audit_entries",
            &stored.audit_entries[0].id,
            json!({ "reason": "x" }),
        ),
        (
            "catalog_cad_items",
            &stored.cad_items[0].id,
            json!({ "key": "g-x:Other" }),
        ),
        (
            "catalog_cad_item_revisions",
            &stored.cad_item_revisions[0].id,
            json!({ "license": "MIT" }),
        ),
        (
            "catalog_blobs",
            &stored.blobs[0].id,
            json!({ "sha256": "f".repeat(64) }),
        ),
        (
            "catalog_import_problems",
            &stored.problems[0].id,
            json!({ "detail": "{}" }),
        ),
    ];
    // Turn PocketBase's batch API on (it is off by default), and show it
    // works for an ordinary write, so its refusal below comes from the
    // guards.
    team.admin
        .patch_json(
            "/api/settings",
            &json!({ "batch": { "enabled": true, "maxRequests": 50, "timeout": 3 } }),
        )
        .unwrap()
        .success()
        .unwrap();
    let ordinary = team
        .admin
        .post_json(
            "/api/batch",
            &json!({ "requests": [{
                "method": "PATCH",
                "url": format!("/api/collections/users/records/{}", editor.auth_id().unwrap()),
                "body": { "verified": true },
            }]}),
        )
        .unwrap();
    assert_eq!(ordinary.status, 200, "{}", ordinary.body);
    // Superusers bypass collection rules, so only the write guards stop
    // them, always with 403. Members are already stopped by the rules.
    for (client, superuser) in [(&team.admin, true), (&editor, false)] {
        let refused = |status: u16| status == 403 || (!superuser && status == 404);
        for (collection, id, body) in &targets {
            let path = format!("/api/collections/{collection}/records/{id}");
            let update = client.patch_json(&path, body).unwrap();
            assert!(refused(update.status), "{collection} update: {update:?}");
            let delete = client.delete(&path).unwrap();
            assert!(refused(delete.status), "{collection} delete: {delete:?}");
        }
        for collection in catalog::pocketbase::COLLECTIONS {
            let create = client
                .post_json(
                    &format!("/api/collections/{collection}/records"),
                    &json!({ "seq": 99, "ipn": "RES-9999-9999" }),
                )
                .unwrap();
            assert!(refused(create.status), "{collection} create: {create:?}");
        }
        // The batch API is refused too, now that it is on.
        let batch = client
            .post_json(
                "/api/batch",
                &json!({ "requests": [{
                    "method": "PATCH",
                    "url": format!("/api/collections/catalog_parts/records/{}", part.id),
                    "body": { "ipn": "RES-9999-9999" },
                }]}),
            )
            .unwrap();
        assert!(batch.success().is_err(), "{batch:?}");
    }
    assert_eq!(team.read(), stored);
    team.server.stop();
}

/// The Change in a realtime notification.
fn notified_change(event: &collaboration::RealtimeEvent) -> StoredChange {
    assert_eq!(event.data["action"], "create", "{:?}", event.data);
    serde_json::from_value(event.data["record"].clone()).unwrap()
}

#[test]
fn a_second_client_converges_through_realtime_and_its_checkpoint() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let watcher = team.member("Vic Viewer", false);
    let wait = Duration::from_secs(20);

    // The second client starts from its checkpoint: the import.
    let mut replica = CatalogReplica::new();
    assert_eq!(replica.catch_up(&watcher).unwrap(), 1);
    assert_eq!((replica.checkpoint(), replica.parts().len()), (1, 29));
    let mut connection = RealtimeConnection::connect(team.url(), wait).unwrap();
    connection
        .subscribe(&watcher, &["catalog_changes"])
        .unwrap();

    // An edit arrives as a notification and applies once.
    let readback = team.read();
    applied(&submit_edit(&editor, &edit(&readback, IPN, "Value", "47R")).unwrap());
    let event = connection
        .next_on("catalog_changes", wait)
        .unwrap()
        .expect("a notification for the edit");
    let change = notified_change(&event);
    assert_eq!(
        replica.receive(&change, &watcher).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        replica.receive(&change, &watcher).unwrap(),
        ApplyResult::Duplicate
    );
    assert_eq!(replica.parts()[&readback.part(IPN).unwrap().id].number, 2);

    // While it is disconnected, two edits and a restore happen.
    connection.disconnect();
    let other_ipn = "RES-0000-1000";
    applied(&submit_edit(&editor, &edit(&team.read(), IPN, "Value", "47.0")).unwrap());
    applied(&submit_edit(&editor, &edit(&team.read(), other_ipn, "Value", "1k")).unwrap());
    applied(&submit_restore(&editor, &RestoreCommand::new(IPN, 3, 1, "back")).unwrap());

    // It reconnects and catches up from its checkpoint: nothing missed...
    let mut connection = RealtimeConnection::connect(team.url(), wait).unwrap();
    connection
        .subscribe(&watcher, &["catalog_changes"])
        .unwrap();
    assert_eq!(replica.catch_up(&watcher).unwrap(), 3);
    // ...and the next notification follows on directly.
    applied(&submit_edit(&editor, &edit(&team.read(), other_ipn, "Value", "1K")).unwrap());
    let change = notified_change(
        &connection
            .next_on("catalog_changes", wait)
            .unwrap()
            .unwrap(),
    );
    assert_eq!(change.seq, 6);
    assert_eq!(
        replica.receive(&change, &watcher).unwrap(),
        ApplyResult::Applied
    );

    // Two quick edits; handling only the later notification first is a gap,
    // which catches up and covers both. The earlier one is then a duplicate.
    applied(&submit_edit(&editor, &edit(&team.read(), IPN, "Value", "47 R")).unwrap());
    applied(&submit_edit(&editor, &edit(&team.read(), IPN, "Value", "47 Ohm")).unwrap());
    let seventh = notified_change(
        &connection
            .next_on("catalog_changes", wait)
            .unwrap()
            .unwrap(),
    );
    let eighth = notified_change(
        &connection
            .next_on("catalog_changes", wait)
            .unwrap()
            .unwrap(),
    );
    assert_eq!((seventh.seq, eighth.seq), (7, 8));
    assert_eq!(
        replica.receive(&eighth, &watcher).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        replica.receive(&seventh, &watcher).unwrap(),
        ApplyResult::Duplicate
    );

    // Every Change applied exactly once, in order, and the replica agrees
    // with the server about every part's current revision.
    assert_eq!(replica.applied_sequences(), [1, 2, 3, 4, 5, 6, 7, 8]);
    let server = team.read();
    for part in &server.parts {
        let known = &replica.parts()[&part.id];
        assert_eq!(
            (known.revision_id.as_str(), known.number),
            (part.current_revision.as_str(), part.current_revision_number),
            "{}",
            part.ipn
        );
    }
    connection.disconnect();
    team.server.stop();
}

/// The collection's JSON as the API returns it.
fn collection(admin: &PocketBaseClient, name: &str) -> Value {
    admin
        .get(&format!("/api/collections/{name}"))
        .unwrap()
        .json()
        .unwrap()
}

/// PATCHes collection `name` with `change` applied to its JSON; returns the
/// status.
fn change_collection(admin: &PocketBaseClient, name: &str, change: impl Fn(&mut Value)) -> u16 {
    let mut body = collection(admin, name);
    change(&mut body);
    admin
        .patch_json(&format!("/api/collections/{name}"), &body)
        .unwrap()
        .status
}

#[test]
fn catalog_collections_cannot_lose_data_through_the_collection_api() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let initial = team.read();
    applied(&submit_edit(&editor, &edit(&initial, IPN, "Value", "47R")).unwrap());
    let stored = team.read();
    let admin = &team.admin;
    let remove_field = |field: &'static str| {
        move |body: &mut Value| {
            body["fields"]
                .as_array_mut()
                .unwrap()
                .retain(|item| item["name"] != field)
        }
    };

    // Dropping a field would wipe it on every record.
    assert_eq!(
        change_collection(admin, "catalog_part_revisions", remove_field("source")),
        403
    );
    // Renaming or retyping a field, renaming the collection, loosening a
    // rule and dropping the unique seq index are refused too.
    assert_eq!(
        change_collection(admin, "catalog_changes", |body| {
            for field in body["fields"].as_array_mut().unwrap() {
                if field["name"] == "reason" {
                    field["name"] = "why".into();
                }
            }
        }),
        403
    );
    assert_eq!(
        change_collection(admin, "catalog_audit_entries", |body| body["name"] =
            "catalog_audit_entries_old".into()),
        403
    );
    assert_eq!(
        change_collection(admin, "catalog_parts", |body| body["listRule"] = "".into()),
        403
    );
    assert_eq!(
        change_collection(admin, "catalog_parts", |body| body["updateRule"] =
            "@request.auth.id != ''".into()),
        403
    );
    assert_eq!(
        change_collection(admin, "catalog_changes", |body| {
            body["indexes"]
                .as_array_mut()
                .unwrap()
                .retain(|index| !index.as_str().unwrap().contains("seq"))
        }),
        403
    );
    // Deleting a collection, the replay ledger included, is refused.
    for name in ["catalog_commands", "catalog_part_revisions", "users"] {
        let deleted = admin.delete(&format!("/api/collections/{name}")).unwrap();
        assert_eq!(deleted.status, 403, "{name}: {}", deleted.body);
    }
    // So is an import that deletes a collection or drops a field.
    let all: Value = admin
        .get("/api/collections?perPage=500")
        .unwrap()
        .json()
        .unwrap();
    let every: Vec<Value> = all["items"].as_array().unwrap().clone();
    let without_ledger: Vec<Value> = every
        .iter()
        .filter(|item| item["name"] != "catalog_commands")
        .cloned()
        .collect();
    let import = admin
        .put_json(
            "/api/collections/import",
            &json!({ "collections": without_ledger, "deleteMissing": true }),
        )
        .unwrap();
    assert_eq!(import.status, 403, "{}", import.body);
    let mut dropped = every.clone();
    for item in &mut dropped {
        if item["name"] == "catalog_part_revisions" {
            remove_field("source")(item);
        }
    }
    let import = admin
        .put_json(
            "/api/collections/import",
            &json!({ "collections": dropped, "deleteMissing": false }),
        )
        .unwrap();
    assert_eq!(import.status, 403, "{}", import.body);
    // The users fields and rules the edit routes rely on are guarded.
    assert_eq!(
        change_collection(admin, "users", remove_field("can_edit_catalog")),
        403
    );
    assert_eq!(
        change_collection(admin, "users", |body| body["updateRule"] =
            "id = @request.auth.id".into()),
        403
    );

    // Nothing above changed a record.
    assert_eq!(team.read(), stored);
    // Harmless changes still work: a new field on a Catalog collection, a
    // new field on users, and an unchanged import.
    assert_eq!(
        change_collection(admin, "catalog_changes", |body| {
            body["fields"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "name": "note", "type": "text" }))
        }),
        200
    );
    assert_eq!(
        change_collection(admin, "users", |body| {
            body["fields"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "name": "nickname", "type": "text" }))
        }),
        200
    );
    let all: Value = admin
        .get("/api/collections?perPage=500")
        .unwrap()
        .json()
        .unwrap();
    let import = admin
        .put_json(
            "/api/collections/import",
            &json!({ "collections": all["items"], "deleteMissing": false }),
        )
        .unwrap();
    assert!(
        matches!(import.status, 200 | 204),
        "{} {}",
        import.status,
        import.body
    );
    team.server.stop();
}

#[test]
fn only_an_administrator_names_a_member() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let own = format!(
        "/api/collections/users/records/{}",
        editor.auth_id().unwrap()
    );
    // A member cannot rename themselves, so cannot pose as someone else...
    for name in ["Grace Editor", "an administrator"] {
        let renamed = editor.patch_json(&own, &json!({ "name": name })).unwrap();
        assert!(renamed.success().is_err(), "{renamed:?}");
    }
    let account: Value = team.admin.get(&own).unwrap().json().unwrap();
    assert_eq!(account["name"], "Ada Editor");
    // ...and an administrator cannot give anyone a name the desktop uses
    // for authors who are not members.
    for name in ["an administrator", " Someone "] {
        let refused = team
            .admin
            .patch_json(&own, &json!({ "name": name }))
            .unwrap();
        assert_eq!(refused.status, 400, "{name}: {}", refused.body);
    }
    let password = native_services::random_token(24);
    let created = team
        .admin
        .post_json(
            "/api/collections/users/records",
            &json!({
                "email": "x@example.test",
                "name": "An Administrator",
                "password": password,
                "passwordConfirm": password,
            }),
        )
        .unwrap();
    assert_eq!(created.status, 400, "{}", created.body);
    // The name a Change shows is the one the administrator gave.
    applied(&submit_edit(&editor, &edit(&team.read(), IPN, "Value", "47R")).unwrap());
    let change = team.read().changes.pop().unwrap();
    assert_eq!(change.actor_name, "Ada Editor");
    team.server.stop();
}

#[test]
fn simultaneous_edits_apply_once_and_the_rest_conflict() {
    let team = Team::start();
    let editors: Vec<PocketBaseClient> = (0..10)
        .map(|n| team.member(&format!("Editor {n}"), true))
        .collect();
    let readback = team.read();
    let barrier = std::sync::Barrier::new(editors.len());
    let replies: Vec<PublishReply> = std::thread::scope(|scope| {
        let handles: Vec<_> = editors
            .iter()
            .enumerate()
            .map(|(n, client)| {
                let command = edit(&readback, IPN, "Value", &format!("{}R", 40 + n));
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    submit_edit(client, &command).unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let applied_count = replies
        .iter()
        .filter(|reply| matches!(reply.outcome, PublishOutcome::Applied { .. }))
        .count();
    let conflicts = replies
        .iter()
        .filter(|reply| matches!(reply.outcome, PublishOutcome::Conflict { .. }))
        .count();
    assert_eq!((applied_count, conflicts), (1, 9));
    let after = team.read();
    assert_eq!(after.changes.len(), 2);
    assert_eq!(current(&after, IPN).0, 2);
    team.server.stop();
}

#[test]
fn references_are_trimmed_the_way_rust_trims_them() {
    // Rust's str::trim (used by the CSV import) removes U+0085 but not
    // U+FEFF; JavaScript's trim() does the opposite. The route follows Rust.
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let initial = team.read();
    let (_, first) = current(&initial, IPN);
    let symbol = first.field("Symbol").unwrap().to_owned();
    let bound = |details: &PartDetails| {
        details
            .cad_references
            .iter()
            .find(|reference| reference.role == catalog::CadReferenceRole::Symbol)
            .map(|reference| (reference.reference.clone(), reference.target.clone()))
            .unwrap()
    };
    let spaced = format!("\u{85}{symbol}\u{85}");
    applied(&submit_edit(&editor, &edit(&initial, IPN, "Symbol", &spaced)).unwrap());
    let (_, details) = current(&team.read(), IPN);
    assert_eq!(details.field("Symbol"), Some(spaced.as_str()));
    assert_eq!(bound(&details), bound(&first));
    let marked = format!("\u{feff}{symbol}");
    applied(&submit_edit(&editor, &edit(&team.read(), IPN, "Symbol", &marked)).unwrap());
    let (_, details) = current(&team.read(), IPN);
    assert_eq!(
        bound(&details),
        (marked.clone(), CadReferenceTarget::External),
        "U+FEFF is not trimmed, so the library is not a g- library"
    );
    team.server.stop();
}

/// A hook that saves a part directly, as buggy server code might, for the
/// test below. It runs in a copy of the repository's hooks.
const DIRECT_SAVE_HOOK: &str = r#"
routerAdd("POST", "/api/test/save-part", (e) => {
  const body = e.requestInfo().body;
  const part = $app.findFirstRecordByData("catalog_parts", "ipn", body.ipn);
  let revisionId = body.revision;
  if (body.createNumber) {
    const made = new Record($app.findCollectionByNameOrId("catalog_part_revisions"));
    const current = $app.findRecordById("catalog_part_revisions", part.getString("current_revision"));
    made.set("part", part.id);
    made.set("number", body.createNumber);
    made.set("predecessor", current.id);
    made.set("change", current.getString("change"));
    made.set("snapshot", current.getString("snapshot"));
    made.set("snapshot_sha256", current.getString("snapshot_sha256"));
    made.set("source", "{}");
    $app.save(made);
    revisionId = made.id;
  }
  if (body.ipnChange) {
    part.set("ipn", body.ipnChange);
  } else {
    const target = $app.findRecordById("catalog_part_revisions", revisionId);
    part.set("current_revision", target.id);
    part.set("current_revision_number", target.getInt("number"));
  }
  try {
    $app.save(part);
  } catch (error) {
    return e.json(409, { refused: String(error) });
  }
  return e.json(200, { saved: true });
}, $apis.requireSuperuserAuth());
"#;

#[test]
fn server_code_cannot_move_a_part_backwards_or_skip_a_revision() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let hooks = state.path().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for entry in std::fs::read_dir(root.join("infra/pocketbase/pb_hooks")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), hooks.join(entry.file_name())).unwrap();
    }
    std::fs::write(hooks.join("test_direct_save.pb.js"), DIRECT_SAVE_HOOK).unwrap();
    let mut options = services::options(&root, state.path());
    options.hooks_dir = hooks;
    let superuser = native_services::Superuser::generate();
    let binary = native_services::pocketbase_binary(&root).unwrap();
    let server = PocketBaseServer::start(&binary, &options, &superuser).unwrap();
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    let import = services::plan(&root, "g-res").unwrap();
    submit_import(&admin, &ImportCommand::new(&import, "Import the resistors")).unwrap();
    let readback = read_back(&admin).unwrap();
    applied(&submit_edit(&admin, &edit(&readback, IPN, "Value", "47R")).unwrap());
    let after_edit = read_back(&admin).unwrap();
    let part = after_edit.part(IPN).unwrap();
    let second = part.current_revision.clone();
    let first = after_edit
        .part_revision(&second)
        .unwrap()
        .predecessor
        .clone();

    let save = |body: Value| admin.post_json("/api/test/save-part", &body).unwrap();
    // Backwards to revision 1.
    let back = save(json!({ "ipn": IPN, "revision": first }));
    assert_eq!(back.status, 409, "{}", back.body);
    assert!(back.body.contains("right after it"), "{}", back.body);
    // Skipping to a revision 4 (made directly, after revision 2).
    let skip = save(json!({ "ipn": IPN, "createNumber": 4 }));
    assert_eq!(skip.status, 409, "{}", skip.body);
    // Changing the IPN.
    let ipn = save(json!({ "ipn": IPN, "ipnChange": "RES-9999-9999" }));
    assert_eq!(ipn.status, 409, "{}", ipn.body);
    assert!(ipn.body.contains("never changes"), "{}", ipn.body);
    let part = read_back(&admin).unwrap().part(IPN).unwrap().clone();
    assert_eq!(
        (part.current_revision.as_str(), part.current_revision_number),
        (second.as_str(), 2)
    );
    server.stop();
}

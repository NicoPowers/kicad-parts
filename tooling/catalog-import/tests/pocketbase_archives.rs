//! Archiving and unarchiving part revisions through the protected Catalog
//! route (seed kicad-parts-5bea): the current revision is always the newest
//! revision that is not archived, archived revisions stay readable and are
//! never deleted, and a second client follows archive-only Changes live and
//! through its checkpoint. Each test starts its own disposable PocketBase on
//! 127.0.0.1 with the sample library's resistors; the repository is only
//! read.

use catalog::pocketbase::{
    ArchiveCommand, ArchiveOutcome, ArchiveReply, CatalogReadback, EditCommand, StoredChange,
    read_back, submit_archive, submit_edit,
};
use catalog::replica::CatalogReplica;
use catalog::{PartDetails, PartRevision};
use catalog_import::services;
use collaboration::{ApplyResult, CollabError, PocketBaseClient, RealtimeConnection};
use native_services::PocketBaseServer;
use std::time::Duration;

const IPN: &str = "RES-0000-0470";

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    /// The Changes the sample resistors made: one category, then each part.
    seeded: u64,
    _state: tempfile::TempDir,
}

impl Team {
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let seeded = sample_library::seed(&admin, &["res"]).unwrap().last_seq;
        Self {
            server,
            admin,
            seeded,
            _state: state,
        }
    }

    fn member(&self, name: &str, can_edit: bool) -> PocketBaseClient {
        let email = format!("{}@example.test", name.to_lowercase().replace(' ', "."));
        let member = services::add_member(&self.admin, name, &email, can_edit).unwrap();
        PocketBaseClient::new(self.server.url())
            .authenticate("users", &member.email, &member.password)
            .unwrap()
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }
}

/// The part's current revision number and its archived revision numbers.
fn revision_state(readback: &CatalogReadback, ipn: &str) -> (u32, Vec<u32>) {
    let part = readback.part(ipn).unwrap();
    (
        part.current_revision_number,
        readback.archived(&part.id).into_iter().collect(),
    )
}

/// Changes the symbol of `ipn` with an edit, which adds a revision.
fn change_symbol(client: &PocketBaseClient, readback: &CatalogReadback, ipn: &str, symbol: &str) {
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
    let reply = submit_edit(client, &edit).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
}

fn applied(reply: &ArchiveReply) -> (u64, u32) {
    match &reply.outcome {
        ArchiveOutcome::Applied {
            seq,
            current_revision,
            ..
        } => (*seq, current_revision.number),
        other => panic!("expected applied, got {other:?}: {}", reply.body),
    }
}

fn refused(result: Result<ArchiveReply, CollabError>, status: u16, says: &str) {
    match result {
        Err(CollabError::Http {
            status: got,
            message,
        }) if got == status => assert!(message.contains(says), "{message}"),
        Err(CollabError::Forbidden(message)) if status == 403 => {
            assert!(message.contains(says), "{message}")
        }
        other => panic!("expected {status}, got {other:?}"),
    }
}

#[test]
fn archiving_moves_the_current_revision_and_never_deletes() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let viewer = team.member("Vic Viewer", false);
    let initial = team.read();
    let symbol = {
        let part = initial.part(IPN).unwrap();
        let revision: PartRevision = serde_json::from_str(
            &initial
                .part_revision(&part.current_revision)
                .unwrap()
                .snapshot,
        )
        .unwrap();
        revision.field("Symbol").unwrap().to_owned()
    };

    // The only revision cannot be archived, and the refusal says why in
    // plain words; nothing is written.
    refused(
        submit_archive(&editor, &ArchiveCommand::new(IPN, 1, true, "Bad symbol")),
        400,
        "Revision 1 is the only revision that is not archived, so it cannot be archived. Add a corrected revision first",
    );
    assert_eq!(team.read(), initial);

    // A new revision 2; archiving it makes revision 1 current again.
    change_symbol(&editor, &initial, IPN, "Device:R");
    let with_two = team.read();
    assert_eq!(revision_state(&with_two, IPN), (2, vec![]));
    // Only editors may archive, a reason is required, the revision must
    // exist, and nothing is written for any of these.
    let anonymous = PocketBaseClient::new(team.server.url());
    assert!(matches!(
        submit_archive(&anonymous, &ArchiveCommand::new(IPN, 2, true, "x")),
        Err(CollabError::Unauthorized(_))
    ));
    refused(
        submit_archive(&viewer, &ArchiveCommand::new(IPN, 2, true, "x")),
        403,
        "not edit",
    );
    refused(
        submit_archive(&editor, &ArchiveCommand::new(IPN, 2, true, "  ")),
        400,
        "eason is required",
    );
    refused(
        submit_archive(&editor, &ArchiveCommand::new(IPN, 9, true, "x")),
        404,
        "has no revision 9",
    );
    assert_eq!(team.read(), with_two);

    let command = ArchiveCommand::new(IPN, 2, true, "The KiCad symbol confuses assembly");
    let reply = submit_archive(&editor, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let (seq, current) = applied(&reply);
    assert_eq!(current, 1);
    let archived = team.read();
    assert_eq!(revision_state(&archived, IPN), (1, vec![2]));
    // One Change of kind archive, one archive entry, one audit entry; no
    // revision is deleted or changed.
    let change = archived
        .changes
        .iter()
        .find(|change| change.seq == seq)
        .unwrap();
    assert_eq!(
        (change.kind.as_str(), change.reason.as_str()),
        ("archive", "The KiCad symbol confuses assembly")
    );
    assert_eq!(archived.part_revisions, with_two.part_revisions);
    assert_eq!(archived.part_details, with_two.part_details);
    let entry = archived
        .archive_entries
        .iter()
        .find(|entry| entry.change == change.id)
        .unwrap();
    assert_eq!(
        (
            entry.revision_number,
            entry.archived,
            entry.current_revision_number_after,
            entry.seq
        ),
        (2, true, 1, seq)
    );
    assert_eq!(
        archived
            .audit_entries
            .iter()
            .filter(|audit| audit.change == change.id)
            .map(|audit| audit.action.as_str())
            .collect::<Vec<_>>(),
        ["catalog.archive"]
    );
    // The answer replays; archiving again is a no-op; with revision 2
    // archived, revision 1 is the only one left and cannot be archived.
    let replay = submit_archive(&editor, &command).unwrap();
    assert_eq!((replay.status, replay.replayed), (200, true));
    let again = submit_archive(&editor, &ArchiveCommand::new(IPN, 2, true, "again")).unwrap();
    assert_eq!(again.status, 422, "{}", again.body);
    assert!(matches!(again.outcome, ArchiveOutcome::Noop { .. }));
    refused(
        submit_archive(&editor, &ArchiveCommand::new(IPN, 1, true, "both")),
        400,
        "Revision 1 is the only revision that is not archived",
    );
    assert_eq!(team.read(), archived);

    // A CAD edit cannot bring the archived design back...
    let part = archived.part(IPN).unwrap();
    let current: PartRevision = serde_json::from_str(
        &archived
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut back = EditCommand::new(IPN, "Back to KiCad's symbol").editing_cad(&current, 1);
    assert!(back.set_field("Symbol", "Device:R"));
    match submit_edit(&editor, &back) {
        Err(CollabError::Http {
            status: 400,
            message,
        }) => assert!(
            message.contains("those of archived revision 2")
                && message.contains("unarchive revision 2"),
            "{message}"
        ),
        other => panic!("expected 400, got {other:?}"),
    }
    // ...but other new CAD adds revision 3, after the archived revision 2.
    change_symbol(&editor, &archived, IPN, " Device:R_Small");
    let three = team.read();
    assert_eq!(revision_state(&three, IPN), (3, vec![2]));
    let part = three.part(IPN).unwrap();
    let newest = three.part_revision(&part.current_revision).unwrap();
    let archived_two = three
        .part_revisions
        .iter()
        .find(|revision| revision.part == part.id && revision.number == 2)
        .unwrap();
    assert_eq!(newest.predecessor, archived_two.id);

    // Unarchiving revision 2 leaves revision 3 current; archiving 3 then
    // makes 2 current again, and unarchiving 3 moves it forward to 3.
    let reply = submit_archive(
        &editor,
        &ArchiveCommand::new(IPN, 2, false, "Assembly is fine with it"),
    )
    .unwrap();
    assert_eq!(applied(&reply).1, 3);
    assert_eq!(revision_state(&team.read(), IPN), (3, vec![]));
    assert_eq!(
        applied(
            &submit_archive(
                &editor,
                &ArchiveCommand::new(IPN, 3, true, "Wrong pin names")
            )
            .unwrap()
        )
        .1,
        2
    );
    assert_eq!(revision_state(&team.read(), IPN), (2, vec![3]));
    assert_eq!(
        applied(
            &submit_archive(
                &editor,
                &ArchiveCommand::new(IPN, 3, false, "Pins fixed upstream")
            )
            .unwrap()
        )
        .1,
        3
    );
    let last = team.read();
    assert_eq!(revision_state(&last, IPN), (3, vec![]));
    let kinds: Vec<&str> = last
        .changes
        .iter()
        .skip(team.seeded as usize)
        .map(|change| change.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "edit",
            "archive",
            "edit",
            "unarchive",
            "archive",
            "unarchive"
        ]
    );
    // Details never moved, and every revision is still there.
    assert_eq!(last.part_details, initial.part_details);
    assert_eq!(
        last.part_revisions
            .iter()
            .filter(|revision| revision.part == part.id)
            .count(),
        3
    );
    assert_ne!(symbol, "Device:R");
    team.server.stop();
}

/// The Change in a realtime notification.
fn notified_change(event: &collaboration::RealtimeEvent) -> StoredChange {
    assert_eq!(event.data["action"], "create", "{:?}", event.data);
    serde_json::from_value(event.data["record"].clone()).unwrap()
}

#[test]
fn a_second_client_follows_archive_only_changes() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let watcher = team.member("Vic Viewer", false);
    let wait = Duration::from_secs(20);
    change_symbol(&editor, &team.read(), IPN, "Device:R");

    // Catching up from nothing applies the sample parts and the edit.
    let mut replica = CatalogReplica::new();
    assert_eq!(
        replica.catch_up(&watcher).unwrap(),
        team.seeded as usize + 1
    );
    let part_id = team.read().part(IPN).unwrap().id.clone();
    assert_eq!(replica.parts()[&part_id].number, 2);
    let mut connection = RealtimeConnection::connect(&watcher, wait).unwrap();
    connection
        .subscribe(&watcher, &["catalog_changes"])
        .unwrap();

    // An archive arrives as a notification: a Change with only an archive
    // entry, which moves the current revision back to revision 1, with its
    // part revision.
    applied(&submit_archive(&editor, &ArchiveCommand::new(IPN, 2, true, "Bad symbol")).unwrap());
    let change = notified_change(
        &connection
            .next_on("catalog_changes", wait)
            .unwrap()
            .unwrap(),
    );
    assert_eq!(change.kind, "archive");
    assert_eq!(
        replica.receive(&change, &watcher).unwrap(),
        ApplyResult::Applied
    );
    let known = &replica.parts()[&part_id];
    assert_eq!(known.number, 1);
    assert_eq!(
        known.revision.as_ref().unwrap().field("Symbol"),
        Some("g-res:R_US")
    );
    // While disconnected, an unarchive happens; catching up from the
    // checkpoint applies it.
    connection.disconnect();
    applied(&submit_archive(&editor, &ArchiveCommand::new(IPN, 2, false, "It was fine")).unwrap());
    assert_eq!(replica.catch_up(&watcher).unwrap(), 1);
    let known = &replica.parts()[&part_id];
    assert_eq!(known.number, 2);
    assert_eq!(
        known.revision.as_ref().unwrap().field("Symbol"),
        Some("Device:R")
    );
    // The details were never touched.
    let details: &PartDetails = known.details.as_ref().unwrap();
    assert_eq!(details.ipn, IPN);
    // The sample parts, the edit, the archive and the unarchive.
    let every: Vec<u64> = (1..=team.seeded + 3).collect();
    assert_eq!(replica.applied_sequences(), every);
    team.server.stop();
}

/// A hook that saves part revisions and moves the part's current revision
/// directly, as buggy server code might, for the test below.
const DIRECT_SAVE_HOOK: &str = r#"
routerAdd("POST", "/api/test/part-revision", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  const part = $app.findFirstRecordByData("catalog_parts", "ipn", body.ipn);
  try {
    if (body.create) {
      const newest = lib.newestRevision($app, part.id);
      const made = new Record($app.findCollectionByNameOrId("catalog_part_revisions"));
      made.set("id", body.id || lib.deriveId("part-revision", part.id + "\n" + body.create));
      for (const name of ["change", "snapshot", "snapshot_sha256", "source"]) {
        made.set(name, newest.getString(name));
      }
      made.set("part", part.id);
      made.set("number", body.create);
      made.set("predecessor", body.noPredecessor ? "" : newest.id);
      $app.save(made);
    } else {
      const target = $app.findFirstRecordByFilter(
        "catalog_part_revisions",
        "part = {:part} && number = {:number}",
        { part: part.id, number: body.pointTo },
      );
      part.set("current_revision", target.id);
      part.set("current_revision_number", body.pointTo);
      $app.save(part);
    }
  } catch (error) {
    return e.json(409, { refused: String(error) });
  }
  return e.json(200, { saved: true });
}, $apis.requireSuperuserAuth());

// Saves an archive entry directly, in a new Change, as buggy server code
// might: a valid entry unless the body overrides a field. With movePointer
// it also moves the part to the entry's current_revision_after.
routerAdd("POST", "/api/test/archive-entry", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  try {
    $app.runInTransaction((txApp) => {
      const part = txApp.findFirstRecordByData("catalog_parts", "ipn", body.ipn);
      const revisionOf = (number) =>
        txApp.findFirstRecordByFilter("catalog_part_revisions", "part = {:part} && number = {:number}", {
          part: part.id,
          number: number,
        });
      const last = txApp.findRecordsByFilter("catalog_changes", "seq > 0", "-seq", 1, 0)[0];
      const seq = last.getInt("seq") + 1;
      const correlation = "test-" + seq;
      const archived = body.archived !== false;
      const change = new Record(txApp.findCollectionByNameOrId("catalog_changes"));
      change.set("id", lib.deriveId("change", correlation));
      change.set("seq", seq);
      change.set("kind", body.kind || (archived ? "archive" : "unarchive"));
      change.set("reason", "test");
      change.set("correlation_id", correlation);
      change.set("actor", "test");
      change.set("sources", "{}");
      change.set("summary", "{}");
      txApp.save(change);
      const target = revisionOf(body.revision);
      const after = revisionOf(body.currentAfter);
      const entry = new Record(txApp.findCollectionByNameOrId("catalog_archive_entries"));
      entry.set("id", body.id || lib.deriveId("archive-entry", correlation));
      entry.set("part", part.id);
      entry.set("part_revision", body.revisionOf ? revisionOf(body.revisionOf).id : target.id);
      entry.set("revision_number", body.revision);
      entry.set("archived", archived);
      entry.set("change", change.id);
      entry.set("seq", body.seq || seq);
      entry.set("current_revision_after", after.id);
      entry.set("current_revision_number_after", body.currentAfter);
      txApp.save(entry);
      if (body.movePointer) {
        part.set("current_revision", after.id);
        part.set("current_revision_number", body.currentAfter);
        txApp.save(part);
      }
    });
  } catch (error) {
    return e.json(409, { refused: String(error) });
  }
  return e.json(200, { saved: true });
}, $apis.requireSuperuserAuth());
"#;

#[test]
fn server_code_can_only_add_the_next_revision_and_point_at_the_newest_not_archived() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let hooks = state.path().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for entry in std::fs::read_dir(root.join("infra/pocketbase/pb_hooks")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), hooks.join(entry.file_name())).unwrap();
    }
    std::fs::write(hooks.join("test_part_revision.pb.js"), DIRECT_SAVE_HOOK).unwrap();
    let mut options = services::options(&root, state.path());
    options.hooks_dir = hooks;
    let superuser = native_services::Superuser::generate();
    let binary = native_services::pocketbase_binary(&root).unwrap();
    let server = PocketBaseServer::start(&binary, &options, &superuser).unwrap();
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    sample_library::seed(&admin, &["res"]).unwrap();
    // Revisions 2 and 3, with 3 archived: revision 2 is current.
    change_symbol(&admin, &read_back(&admin).unwrap(), IPN, "Device:R");
    change_symbol(&admin, &read_back(&admin).unwrap(), IPN, "Device:R_Small");
    applied(&submit_archive(&admin, &ArchiveCommand::new(IPN, 3, true, "Wrong")).unwrap());
    let before = read_back(&admin).unwrap();
    assert_eq!(revision_state(&before, IPN), (2, vec![3]));

    let save = |body: serde_json::Value| admin.post_json("/api/test/part-revision", &body).unwrap();
    // A new revision must be the newest + 1 (4, after the archived 3), name
    // the newest as its predecessor and have the derived ID.
    let skip = save(serde_json::json!({ "ipn": IPN, "create": 5 }));
    assert_eq!(skip.status, 409, "{}", skip.body);
    assert!(skip.body.contains("must be number 4"), "{}", skip.body);
    let unlinked = save(serde_json::json!({ "ipn": IPN, "create": 4, "noPredecessor": true }));
    assert_eq!(unlinked.status, 409, "{}", unlinked.body);
    let forged = save(serde_json::json!({ "ipn": IPN, "create": 4, "id": "aaaaaaaaaaaaaaa" }));
    assert_eq!(forged.status, 409, "{}", forged.body);
    assert!(forged.body.contains("ID derived"), "{}", forged.body);
    // The current revision may only be the newest one not archived: not the
    // archived 3, and not the older 1.
    for number in [3, 1] {
        let moved = save(serde_json::json!({ "ipn": IPN, "pointTo": number }));
        assert_eq!(moved.status, 409, "{number}: {}", moved.body);
        assert!(
            moved.body.contains("newest revision that is not archived"),
            "{}",
            moved.body
        );
    }
    assert_eq!(read_back(&admin).unwrap(), before);

    // Archive entries are held to what the archive route saves. Revisions
    // 1, 2 and 3 exist, 3 is archived and 2 is current.
    let entry =
        |body: serde_json::Value| admin.post_json("/api/test/archive-entry", &body).unwrap();
    let refused_entry = |body: serde_json::Value, says: &str| {
        let reply = entry(body);
        assert_eq!(reply.status, 409, "{}", reply.body);
        assert!(reply.body.contains(says), "{says}: {}", reply.body);
    };
    // Another revision's record under this number.
    refused_entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "revisionOf": 1, "currentAfter": 1 }),
        "must name revision 2 of its own part",
    );
    // A Change of the other kind, or a seq that is not the Change's.
    refused_entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "kind": "unarchive", "currentAfter": 1 }),
        "a Change of kind archive",
    );
    refused_entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "seq": 999, "currentAfter": 1 }),
        "copy its seq",
    );
    // A made-up ID.
    refused_entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "id": "aaaaaaaaaaaaaaa", "currentAfter": 1 }),
        "ID derived from its Change",
    );
    // No change of state: 3 is already archived, 2 is not.
    refused_entry(
        serde_json::json!({ "ipn": IPN, "revision": 3, "currentAfter": 2 }),
        "must change the state of revision 3",
    );
    refused_entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "archived": false, "currentAfter": 2 }),
        "must change the state of revision 2",
    );
    // The wrong current revision afterwards: archiving 2 makes 1 current.
    refused_entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "currentAfter": 2 }),
        "must name revision 1, the newest not archived",
    );
    // Archiving a part's only revision.
    refused_entry(
        serde_json::json!({ "ipn": "RES-0000-1000", "revision": 1, "currentAfter": 1 }),
        "only revision",
    );
    assert_eq!(read_back(&admin).unwrap(), before);
    // A valid entry, with the part moved to match, is accepted.
    let valid = entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "currentAfter": 1, "movePointer": true }),
    );
    assert_eq!(valid.status, 200, "{}", valid.body);
    assert_eq!(
        revision_state(&read_back(&admin).unwrap(), IPN),
        (1, vec![2, 3])
    );
    // Saving an entry moves the part itself, so server code that forgets to
    // cannot leave it on an older revision: unarchiving 2 makes 2 current...
    let unarchived = entry(
        serde_json::json!({ "ipn": IPN, "revision": 2, "archived": false, "currentAfter": 2 }),
    );
    assert_eq!(unarchived.status, 200, "{}", unarchived.body);
    assert_eq!(
        revision_state(&read_back(&admin).unwrap(), IPN),
        (2, vec![3])
    );
    // ...nor on an archived one: archiving the current revision 2 makes 1
    // current.
    let archived = entry(serde_json::json!({ "ipn": IPN, "revision": 2, "currentAfter": 1 }));
    assert_eq!(archived.status, 200, "{}", archived.body);
    assert_eq!(
        revision_state(&read_back(&admin).unwrap(), IPN),
        (1, vec![2, 3])
    );
    server.stop();
}

#[test]
fn a_details_edit_with_unchanged_cad_is_never_taken_for_archived_cad() {
    // Revision 3 has the same CAD content as revision 1, which is then
    // archived. A details edit that sends the unchanged CAD section of
    // revision 3 saves its details: the CAD does not change, so it brings
    // nothing archived back.
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    change_symbol(&editor, &team.read(), IPN, "Device:R");
    change_symbol(&editor, &team.read(), IPN, "g-res:R_US");
    applied(&submit_archive(&editor, &ArchiveCommand::new(IPN, 1, true, "Old drawing")).unwrap());
    let stored = team.read();
    assert_eq!(revision_state(&stored, IPN), (3, vec![1]));
    let part = stored.part(IPN).unwrap();
    let details: PartDetails =
        serde_json::from_str(&stored.part_details(&part.current_details).unwrap().snapshot)
            .unwrap();
    let revision: PartRevision = serde_json::from_str(
        &stored
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut edit = EditCommand::new(IPN, "Note the exact value")
        .editing_details(&details, part.current_details_number)
        .editing_cad(&revision, 3);
    assert!(edit.set_field("Value", "47 Ohm"));
    let reply = submit_edit(&editor, &edit).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let after = team.read();
    assert_eq!(revision_state(&after, IPN), (3, vec![1]));
    assert_eq!(after.part(IPN).unwrap().current_details_number, 2);
    team.server.stop();
}

#[test]
fn editors_archiving_both_revisions_at_once_always_leave_one_in_use() {
    // Two editors look at a part with revisions 1 and 2, both in use. Each
    // archives a different one at the same moment. Alone, each request
    // leaves one revision in use; together they must not archive both.
    let team = Team::start();
    let editors: Vec<PocketBaseClient> = (0..8)
        .map(|n| team.member(&format!("Editor {n}"), true))
        .collect();
    change_symbol(&editors[0], &team.read(), IPN, "Device:R");
    let before = team.read();
    assert_eq!(revision_state(&before, IPN), (2, vec![]));
    let barrier = std::sync::Barrier::new(editors.len());
    let replies: Vec<Result<ArchiveReply, CollabError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = editors
            .iter()
            .enumerate()
            .map(|(n, client)| {
                // Half archive revision 1, half revision 2.
                let revision = 1 + (n as u32 % 2);
                let command = ArchiveCommand::new(IPN, revision, true, &format!("Editor {n}"));
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    submit_archive(client, &command)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let mut applied_count = 0;
    let mut noops = 0;
    let mut refusals = 0;
    for reply in &replies {
        match reply {
            Ok(reply) if matches!(reply.outcome, ArchiveOutcome::Applied { .. }) => {
                applied_count += 1
            }
            Ok(reply) if matches!(reply.outcome, ArchiveOutcome::Noop { .. }) => noops += 1,
            Err(CollabError::Http {
                status: 400,
                message,
            }) if message.contains("only revision that is not archived") => refusals += 1,
            other => panic!("unexpected answer: {other:?}"),
        }
    }
    // The first archives its revision; the others asking for the same one
    // find it already archived, and the others asking for the other one
    // are refused, because it is now the only one in use.
    assert_eq!((applied_count, noops, refusals), (1, 3, 4));
    let after = team.read();
    let (current, archived) = revision_state(&after, IPN);
    assert_eq!(archived.len(), 1, "exactly one revision is archived");
    assert_ne!(archived[0], current, "the current revision is in use");
    assert_eq!(after.changes.len(), before.changes.len() + 1);
    team.server.stop();
}

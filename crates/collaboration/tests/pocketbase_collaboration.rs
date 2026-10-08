//! The collaboration mechanics against a real, disposable PocketBase.
//!
//! The first run downloads the pinned PocketBase (about 12 MB) into
//! `target/native-services`; later runs reuse it. Each test starts its own
//! instance on a random 127.0.0.1 port with a temp data directory.

mod support;

use collaboration::{
    ApplyResult, ChangeEntry, CollabError, CommandOutcome, CorrelationId, EditRecord,
    PocketBaseClient, RealtimeConnection, RecordSnapshot, Replica,
};
use native_services::Superuser;
use std::time::Duration;
use support::{CATEGORY_MIGRATION, INIT_MIGRATION, all_migrations, create_member, sign_in};

const EVENT_TIMEOUT: Duration = Duration::from_secs(15);

fn applied_seq(outcome: &CommandOutcome) -> u64 {
    match outcome {
        CommandOutcome::Applied { seq, .. } => *seq,
        other => panic!("expected an applied command, got {other:?}"),
    }
}

fn change_of(event: &collaboration::RealtimeEvent) -> ChangeEntry {
    assert_eq!(event.data["action"], "create");
    serde_json::from_value(event.data["record"].clone()).unwrap()
}

#[test]
fn authenticated_commands_notify_and_reconcile_a_second_client() {
    let temporary = tempfile::tempdir().unwrap();
    let migrations: Vec<String> = all_migrations();
    let migrations: Vec<&str> = migrations.iter().map(String::as_str).collect();
    let options = support::options(temporary.path(), &migrations);
    let superuser = Superuser::generate();
    let (server, admin) = support::start(&options, &superuser);
    let url = server.url().to_owned();
    let alice_member = create_member(&admin, "alice");
    let bob_member = create_member(&admin, "bob");

    // Authentication: a wrong password is refused; real ones sign in.
    assert!(
        PocketBaseClient::new(&url)
            .authenticate("users", &alice_member.email, "wrong-password")
            .is_err()
    );
    let alice = sign_in(&url, &alice_member);
    let bob = sign_in(&url, &bob_member);
    assert_eq!(alice.auth_id(), Some(alice_member.id.as_str()));

    // Unauthorised calls are rejected: no token, a superuser token on the
    // users-only route, and a direct write that bypasses the command.
    let anonymous = PocketBaseClient::new(&url);
    let first = EditRecord::new("res-1", 0, "Resistor", "10k 0603");
    assert!(matches!(
        anonymous.execute(&first),
        Err(CollabError::Unauthorized(_))
    ));
    assert!(matches!(
        admin.execute(&first),
        Err(CollabError::Forbidden(_))
    ));
    assert!(matches!(
        anonymous.list_records::<RecordSnapshot>("collab_records", "", "key"),
        Ok(records) if records.is_empty()
    ));
    assert!(matches!(
        alice
            .post_json(
                "/api/collections/collab_records/records",
                &serde_json::json!({ "key": "res-x", "revision": 1 }),
            )
            .unwrap()
            .success(),
        Err(CollabError::Forbidden(_))
    ));

    // Bob watches the change log and starts from an empty replica.
    let mut bob_stream = RealtimeConnection::connect(&url, EVENT_TIMEOUT).unwrap();
    bob_stream.subscribe(&bob, &["collab_changes"]).unwrap();
    let mut bob_replica = Replica::new();
    assert_eq!(bob_replica.catch_up(&bob).unwrap(), 0);

    // An optimistic create (expected revision 0) is applied as revision 1.
    let created = alice.execute(&first).unwrap();
    assert_eq!(created.status, 200);
    assert!(!created.replayed);
    let CommandOutcome::Applied {
        revision,
        seq,
        ref record_id,
        ..
    } = created.outcome
    else {
        panic!("expected applied, got {:?}", created.outcome);
    };
    assert_eq!((revision, seq), (1, 1));

    // Bob is notified in realtime and applies it.
    let event = bob_stream
        .next_on("collab_changes", EVENT_TIMEOUT)
        .unwrap()
        .expect("bob is notified of alice's change");
    let change = change_of(&event);
    assert_eq!(
        (change.seq, change.revision, change.actor.as_str()),
        (1, 1, alice_member.id.as_str())
    );
    assert_eq!(
        bob_replica.receive(&change, &bob).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(bob_replica.records()["res-1"].title, "Resistor");

    // A duplicate correlation ID returns the stored result byte for byte
    // and applies nothing.
    let replay = alice.execute(&first).unwrap();
    assert!(replay.replayed);
    assert_eq!((replay.status, &replay.body), (200, &created.body));
    assert_eq!(alice.changes_since(0).unwrap().len(), 1);
    assert_eq!(alice.record_by_key("res-1").unwrap().unwrap().revision, 1);

    // Reusing the ID for a different command is refused.
    let mut reused = EditRecord::new("res-1", 1, "Other", "x");
    reused.correlation_id = first.correlation_id.clone();
    let reused = alice.execute(&reused).unwrap();
    assert_eq!(reused.status, 409);
    assert!(matches!(
        reused.outcome,
        CommandOutcome::CorrelationReused { .. }
    ));

    // A no-op is detected and rejected without a new revision; its replay
    // is identical too.
    let noop = EditRecord::new("res-1", 1, "Resistor", "10k 0603");
    let rejected = alice.execute(&noop).unwrap();
    assert_eq!(rejected.status, 422);
    assert!(matches!(
        rejected.outcome,
        CommandOutcome::Noop { revision: 1, .. }
    ));
    let rejected_again = alice.execute(&noop).unwrap();
    assert!(rejected_again.replayed);
    assert_eq!(
        (rejected_again.status, &rejected_again.body),
        (422, &rejected.body)
    );
    assert_eq!(alice.changes_since(0).unwrap().len(), 1);

    // Stale conflict: bob edits revision 1 first; alice's edit of revision 1
    // then conflicts, and succeeds after she reloads.
    let bob_edit = bob
        .execute(&EditRecord::new("res-1", 1, "Resistor", "10k 0603 1%"))
        .unwrap();
    assert_eq!(applied_seq(&bob_edit.outcome), 2);
    let stale = alice
        .execute(&EditRecord::new("res-1", 1, "Resistor 10k", "10k 0603"))
        .unwrap();
    assert_eq!(stale.status, 409);
    assert!(matches!(
        &stale.outcome,
        CommandOutcome::Conflict {
            record_key,
            expected_revision: 1,
            current_revision: 2,
            ..
        } if record_key == "res-1"
    ));
    let reloaded = alice.record_by_key("res-1").unwrap().unwrap();
    assert_eq!(
        (reloaded.revision, reloaded.content.as_str()),
        (2, "10k 0603 1%")
    );
    let retried = alice
        .execute(&EditRecord::new(
            "res-1",
            reloaded.revision,
            "Resistor 10k",
            reloaded.content.clone(),
        ))
        .unwrap();
    assert_eq!(applied_seq(&retried.outcome), 3);
    for expected in [2, 3] {
        let change = change_of(
            &bob_stream
                .next_on("collab_changes", EVENT_TIMEOUT)
                .unwrap()
                .expect("bob sees every change"),
        );
        assert_eq!(change.seq, expected);
        assert_eq!(
            bob_replica.receive(&change, &bob).unwrap(),
            ApplyResult::Applied
        );
    }

    // Concurrent writers with the same expected revision: exactly one wins.
    let racers: Vec<_> = (0..8)
        .map(|index| {
            let client = if index % 2 == 0 {
                alice.clone()
            } else {
                bob.clone()
            };
            std::thread::spawn(move || {
                client
                    .execute(&EditRecord::new(
                        "res-1",
                        3,
                        format!("racer {index}"),
                        "race",
                    ))
                    .unwrap()
            })
        })
        .collect();
    let outcomes: Vec<_> = racers
        .into_iter()
        .map(|racer| racer.join().unwrap())
        .collect();
    assert_eq!(
        outcomes.iter().filter(|reply| reply.status == 200).count(),
        1,
        "{outcomes:#?}"
    );
    assert!(
        outcomes
            .iter()
            .filter(|reply| reply.status != 200)
            .all(|reply| matches!(
                reply.outcome,
                CommandOutcome::Conflict {
                    current_revision: 4,
                    ..
                }
            ))
    );
    let change = change_of(
        &bob_stream
            .next_on("collab_changes", EVENT_TIMEOUT)
            .unwrap()
            .unwrap(),
    );
    assert_eq!(
        bob_replica.receive(&change, &bob).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(bob_replica.checkpoint(), 4);

    // Disconnect: bob's stream drops, alice keeps working.
    bob_stream.disconnect();
    let mut revision = 4;
    for index in 0..5 {
        let reply = alice
            .execute(&EditRecord::new(
                "res-1",
                revision,
                "Resistor 10k",
                format!("offline edit {index}"),
            ))
            .unwrap();
        applied_seq(&reply.outcome);
        revision += 1;
    }
    alice
        .execute(&EditRecord::new("cap-1", 0, "Capacitor", "100n 0402"))
        .unwrap();
    assert_eq!(
        bob_replica.checkpoint(),
        4,
        "bob missed these while offline"
    );

    // Reconnect: subscribe first, then catch up from the checkpoint, so a
    // change made in between arrives both ways and is applied once.
    let mut bob_stream = RealtimeConnection::connect(&url, EVENT_TIMEOUT).unwrap();
    bob_stream.subscribe(&bob, &["collab_changes"]).unwrap();
    let overlap = alice
        .execute(&EditRecord::new("cap-1", 1, "Capacitor", "100n 0402 X7R"))
        .unwrap();
    let overlap_seq = applied_seq(&overlap.outcome);
    assert_eq!(overlap_seq, 11);
    assert_eq!(bob_replica.catch_up(&bob).unwrap(), 7);
    let late = change_of(
        &bob_stream
            .next_on("collab_changes", EVENT_TIMEOUT)
            .unwrap()
            .expect("the overlapping change is also delivered in realtime"),
    );
    assert_eq!(late.seq, overlap_seq);
    assert_eq!(
        bob_replica.receive(&late, &bob).unwrap(),
        ApplyResult::Duplicate
    );

    // A notification that arrives after a gap triggers a catch-up.
    bob_stream.disconnect();
    for content in ["gap 1", "gap 2"] {
        let current = alice.record_by_key("cap-1").unwrap().unwrap();
        alice
            .execute(&EditRecord::new(
                "cap-1",
                current.revision,
                "Capacitor",
                content,
            ))
            .unwrap();
    }
    let mut bob_stream = RealtimeConnection::connect(&url, EVENT_TIMEOUT).unwrap();
    bob_stream.subscribe(&bob, &["collab_changes"]).unwrap();
    let current = alice.record_by_key("cap-1").unwrap().unwrap();
    alice
        .execute(&EditRecord::new(
            "cap-1",
            current.revision,
            "Capacitor",
            "gap 3",
        ))
        .unwrap();
    let after_gap = change_of(
        &bob_stream
            .next_on("collab_changes", EVENT_TIMEOUT)
            .unwrap()
            .unwrap(),
    );
    assert_eq!(after_gap.seq, 14);
    assert_eq!(
        bob_replica.apply(&after_gap),
        ApplyResult::Gap {
            checkpoint: 11,
            received: 14
        }
    );
    assert_eq!(
        bob_replica.receive(&after_gap, &bob).unwrap(),
        ApplyResult::Duplicate
    );

    // Nothing missed, nothing applied twice, and the replica equals the
    // server.
    let server_changes = alice.changes_since(0).unwrap();
    let all_seqs: Vec<u64> = server_changes.iter().map(|change| change.seq).collect();
    assert_eq!(all_seqs, (1..=14).collect::<Vec<_>>());
    assert_eq!(bob_replica.applied_sequences(), all_seqs.as_slice());
    let server_records: Vec<RecordSnapshot> =
        bob.list_records("collab_records", "", "key").unwrap();
    assert_eq!(server_records.len(), bob_replica.records().len());
    for record in &server_records {
        let local = &bob_replica.records()[&record.key];
        assert_eq!(
            (
                &local.record_id,
                local.revision,
                &local.title,
                &local.content
            ),
            (&record.id, record.revision, &record.title, &record.content)
        );
    }
    assert!(
        server_records
            .iter()
            .any(|record| record.key == "res-1" && &record.id == record_id)
    );

    // Each ledger entry is visible only to the user who issued it.
    let alice_ledger: Vec<serde_json::Value> = alice
        .list_records("collab_commands", "", "created")
        .unwrap();
    assert!(
        alice_ledger
            .iter()
            .all(|entry| entry["actor"] == alice_member.id.as_str())
    );
    assert!(
        alice_ledger
            .iter()
            .any(|entry| entry["correlation_id"] == first.correlation_id.as_str())
    );
    drop(bob_stream);
    server.check_no_installer().unwrap();
    server.stop();
}

#[test]
fn a_migration_preserves_a_populated_instance() {
    let temporary = tempfile::tempdir().unwrap();
    let superuser = Superuser::generate();

    // Version 1 of the schema, populated through the protected command.
    let options = support::options(temporary.path(), &[INIT_MIGRATION]);
    let (server, admin) = support::start(&options, &superuser);
    let member = create_member(&admin, "carol");
    let carol = sign_in(server.url(), &member);
    for (key, content) in [("res-1", "10k"), ("cap-1", "100n"), ("nokey", "x")] {
        carol
            .execute(&EditRecord::new(key, 0, key.to_uppercase(), content))
            .unwrap();
    }
    let edit = carol
        .execute(&EditRecord::new("res-1", 1, "RES-1", "10k 1%"))
        .unwrap();
    assert_eq!(applied_seq(&edit.outcome), 4);
    let before_records = support::records(&admin, "collab_records");
    let before_changes = support::records(&admin, "collab_changes");
    let before_ledger = support::records(&admin, "collab_commands");
    assert!(
        before_records
            .iter()
            .all(|record| record.get("category").is_none())
    );
    server.stop();

    // Upgrade: add the second migration and start on the same data.
    std::fs::copy(
        native_services::repository_root()
            .join("infra/pocketbase/pb_migrations")
            .join(CATEGORY_MIGRATION),
        options.migrations_dir.join(CATEGORY_MIGRATION),
    )
    .unwrap();
    let (server, admin) = support::start(&options, &superuser);
    let after_records = support::records(&admin, "collab_records");
    assert_eq!(after_records.len(), before_records.len());
    for (before, after) in before_records.iter().zip(&after_records) {
        let mut stripped = after.clone();
        let category = stripped
            .as_object_mut()
            .unwrap()
            .remove("category")
            .expect("the migration added the field");
        // Every existing value, including `updated`, is unchanged.
        assert_eq!(&stripped, before);
        let key = before["key"].as_str().unwrap();
        let expected = key.split_once('-').map_or("", |(prefix, _)| prefix);
        assert_eq!(category, expected, "backfilled category of {key}");
    }
    assert_eq!(support::records(&admin, "collab_changes"), before_changes);
    assert_eq!(support::records(&admin, "collab_commands"), before_ledger);

    // The upgraded instance keeps working where it left off, and a
    // replayed command from before the upgrade is still recognised.
    let carol = sign_in(server.url(), &member);
    let next = carol
        .execute(&EditRecord::new("res-1", 2, "RES-1", "10k 1% 0603"))
        .unwrap();
    assert_eq!(applied_seq(&next.outcome), 5);
    let mut replay = EditRecord::new("res-1", 1, "RES-1", "10k 1%");
    replay.correlation_id = CorrelationId::new(
        before_ledger
            .iter()
            .find(|entry| entry["result"].as_str().unwrap().contains("\"seq\":4"))
            .unwrap()["correlation_id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let replayed = carol.execute(&replay).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.body, edit.body);
    server.stop();

    // A restart does not apply the migration again: values stay put.
    let (server, admin) = support::start(&options, &superuser);
    let restarted = support::records(&admin, "collab_records");
    assert_eq!(restarted.len(), after_records.len());
    assert!(
        restarted
            .iter()
            .all(|record| record.get("category").is_some())
    );
    server.stop();
}

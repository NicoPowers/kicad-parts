//! The desktop's team catalog use cases (catalog-browser's TeamCatalog)
//! against a disposable PocketBase with the resistors imported: sign-in,
//! history and differences, edits, a conflict resolved by saving on top,
//! restore, read-only members, and changes by others arriving by
//! themselves.

use catalog::pocketbase::{EditCommand, ImportCommand, read_back, submit_edit, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    LiveUpdates, PublishResult, SignInRequest, TeamEditRequest, TeamRestoreRequest, TeamSession,
};
use std::time::{Duration, Instant};

const IPN: &str = "RES-0000-0470";

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn checkpoint(team: &TeamCatalog) -> u64 {
    match team.status().session {
        TeamSession::SignedIn { checkpoint, .. } => checkpoint,
        other => panic!("not signed in: {other:?}"),
    }
}

#[test]
fn the_desktop_edits_resolves_a_conflict_and_restores() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let import = services::plan(&root, "g-res").unwrap();
    submit_import(&admin, &ImportCommand::new(&import, "Import the resistors")).unwrap();
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let other = services::add_member(&admin, "Grace Editor", "grace@example.test", true).unwrap();
    let viewer = services::add_member(&admin, "Vic Viewer", "vic@example.test", false).unwrap();
    let url = server.url().to_owned();

    let settings = state.path().join("desktop").join("team.json");
    let team = TeamCatalog::open(settings.clone(), Some(url.clone()), None);
    // A wrong password is a plain problem, and nothing is saved.
    let status = team.sign_in(SignInRequest {
        server_url: url.clone(),
        email: editor.email.clone(),
        password: "wrong".into(),
    });
    assert_eq!(
        status.session,
        TeamSession::SignedOut {
            problem: Some("The email or password is not right.".into())
        }
    );
    assert!(team.part(IPN).is_err(), "signed out");
    let status = team.sign_in(SignInRequest {
        server_url: url.clone(),
        email: editor.email.clone(),
        password: editor.password.clone(),
    });
    let TeamSession::SignedIn { name, can_edit, .. } = &status.session else {
        panic!("{status:?}");
    };
    assert_eq!((name.as_str(), *can_edit), ("Ada Editor", true));
    // Only the address and email are saved.
    let saved = std::fs::read_to_string(&settings).unwrap();
    assert!(saved.contains(&url) && saved.contains(&editor.email));
    assert!(!saved.contains(&editor.password), "{saved}");
    wait_until("live updates", || {
        matches!(
            team.status().session,
            TeamSession::SignedIn {
                live: LiveUpdates::Live,
                checkpoint: 1,
                ..
            }
        )
    });

    // The part's history starts with the import.
    let part = team.part(IPN).unwrap().unwrap();
    assert_eq!(part.current.number, 1);
    assert_eq!(part.history.len(), 1);
    assert_eq!(part.history[0].action, "Imported");
    assert!(
        part.current
            .cad_items
            .iter()
            .any(|item| item.binding == "revision 1")
    );
    assert!(team.part("CAP-0000-0001").unwrap().is_none());

    // An edit saves revision 2, and the history says what changed.
    let mut request = TeamEditRequest {
        ipn: IPN.into(),
        expected_revision: 1,
        reason: "Exact value".into(),
        fields: part.current.fields.clone(),
        manufacturer_parts: part.current.manufacturer_parts.clone(),
    };
    request
        .fields
        .iter_mut()
        .find(|field| field.name == "Value")
        .unwrap()
        .value = "47R".into();
    let PublishResult::Saved { revision, part } = team.edit(request.clone()).unwrap() else {
        panic!("not saved");
    };
    assert_eq!(revision, 2);
    let latest = &part.history[0];
    assert_eq!(
        (latest.action.as_str(), latest.actor_name.as_str()),
        ("Edited", "Ada Editor")
    );
    assert_eq!(latest.changes.len(), 1);
    assert_eq!(
        (
            latest.changes[0].label.as_str(),
            latest.changes[0].before.as_str(),
            latest.changes[0].after.as_str()
        ),
        ("Value", "47", "47R")
    );
    // Saving the same details again saves nothing.
    request.expected_revision = 2;
    assert!(matches!(
        team.edit(request.clone()).unwrap(),
        PublishResult::NothingToSave { .. }
    ));
    // A symbol the Catalog does not have is refused with the server's words.
    let mut unknown = request.clone();
    unknown
        .fields
        .iter_mut()
        .find(|field| field.name == "Symbol")
        .unwrap()
        .value = "g-pas:NOPE".into();
    let PublishResult::Refused { message } = team.edit(unknown).unwrap() else {
        panic!("not refused");
    };
    assert!(message.contains("g-pas:NOPE"), "{message}");

    // Someone else saves revision 3 while this edit still starts from 2.
    let other_client = PocketBaseClient::new(&url)
        .authenticate("users", &other.email, &other.password)
        .unwrap();
    let readback = read_back(&admin).unwrap();
    let stored = readback.part(IPN).unwrap();
    let details = serde_json::from_str(
        &readback
            .part_revision(&stored.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut theirs = EditCommand::starting_from(&details, 2, "Better description");
    theirs.set_field("Description", "Thick film resistor 47 ohm");
    submit_edit(&other_client, &theirs).unwrap();
    // It arrives by itself.
    wait_until("their change", || checkpoint(&team) == 3);

    let mut mine = request.clone();
    mine.fields
        .iter_mut()
        .find(|field| field.name == "Tolerance")
        .unwrap()
        .value = "0.5%".into();
    let PublishResult::Conflict(conflict) = team.edit(mine).unwrap() else {
        panic!("no conflict");
    };
    assert_eq!((conflict.started_from, conflict.theirs.number), (2, 3));
    assert_eq!(conflict.theirs.actor_name, "Grace Editor");
    let labels: Vec<&str> = conflict.rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, ["Description", "Tolerance"]);
    assert!(conflict.rows.iter().all(|row| !row.both_changed));
    // Saving on top keeps both changes.
    assert_eq!(conflict.merged.expected_revision, 3);
    let PublishResult::Saved { revision, part } = team.edit(conflict.merged.clone()).unwrap()
    else {
        panic!("not saved on top");
    };
    assert_eq!(revision, 4);
    assert_eq!(
        part.current.field("Description"),
        Some("Thick film resistor 47 ohm")
    );
    assert_eq!(part.current.field("Tolerance"), Some("0.5%"));

    // Restoring revision 1 adds revision 5 with its details.
    let first = part.history.iter().find(|r| r.number == 1).unwrap();
    assert!(
        first
            .restore_changes
            .iter()
            .any(|change| change.label == "Value")
    );
    let restored = team
        .restore(TeamRestoreRequest {
            ipn: IPN.into(),
            expected_revision: 4,
            restore_revision: 1,
            reason: "Back to the import".into(),
        })
        .unwrap();
    let PublishResult::Saved { revision, part } = restored else {
        panic!("not restored");
    };
    assert_eq!(revision, 5);
    assert_eq!(part.history[0].action, "Restored");
    assert_eq!(part.history[0].restored_from, Some(1));
    assert_eq!(part.current.field("Value"), Some("47"));
    // A stale restore is a conflict that names the restore.
    let PublishResult::Conflict(conflict) = team
        .restore(TeamRestoreRequest {
            ipn: IPN.into(),
            expected_revision: 4,
            restore_revision: 2,
            reason: "stale".into(),
        })
        .unwrap()
    else {
        panic!("no conflict");
    };
    assert_eq!(conflict.restore_revision, Some(2));

    // A read-only member sees everything but cannot save.
    let reader = TeamCatalog::open(state.path().join("reader.json"), None, None);
    let status = reader.sign_in(SignInRequest {
        server_url: url.clone(),
        email: viewer.email.clone(),
        password: viewer.password.clone(),
    });
    assert!(matches!(
        status.session,
        TeamSession::SignedIn {
            can_edit: false,
            ..
        }
    ));
    let part = reader.part(IPN).unwrap().unwrap();
    assert_eq!(part.history.len(), 5);
    let PublishResult::Refused { message } = reader
        .edit(TeamEditRequest {
            expected_revision: 5,
            ..request
        })
        .unwrap()
    else {
        panic!("not refused");
    };
    assert!(message.contains("not edit"), "{message}");

    // Signing out ends the session and stops following changes.
    assert!(!team.sign_out().session.is_signed_in());
    server.stop();
}

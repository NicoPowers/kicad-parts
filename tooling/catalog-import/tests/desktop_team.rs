//! The desktop's team catalog use cases (catalog-browser's TeamCatalog)
//! against a disposable PocketBase with the sample library's resistors:
//! sign-in, history and differences, edits, a conflict resolved by saving
//! on top, read-only members and changes by others arriving by themselves.
//! (CSV changes that were not applied are in `desktop_team_csv.rs`.)

use catalog::pocketbase::{EditCommand, read_back, submit_edit};
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    LiveUpdates, PreviewKind, PublishResult, RevisionStatus, SignInRequest, TeamArchiveRequest,
    TeamEditRequest, TeamSession,
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
fn the_desktop_edits_and_resolves_a_conflict() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let seeded = sample_library::seed(&admin, &["res"]).unwrap().last_seq;
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let other = services::add_member(&admin, "Grace Editor", "grace@example.test", true).unwrap();
    let viewer = services::add_member(&admin, "Vic Viewer", "vic@example.test", false).unwrap();
    let url = server.url().to_owned();

    let team = TeamCatalog::new();
    // A wrong password is a plain problem.
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
    wait_until("live updates", || {
        matches!(
            team.status().session,
            TeamSession::SignedIn {
                live: LiveUpdates::Live,
                checkpoint,
                ..
            } if checkpoint == seeded
        )
    });

    // The part's history starts with Add part, which added revision 1 and
    // its first details.
    let part = team.part(IPN).unwrap().unwrap();
    assert_eq!((part.details.number, part.revision.number), (1, 1));
    assert_eq!(part.history.len(), 1);
    assert_eq!(
        (
            part.history[0].action.as_str(),
            part.history[0].revision,
            part.history[0].details
        ),
        ("Added", Some(1), Some(1))
    );
    assert!(
        part.revision
            .cad_items
            .iter()
            .any(|item| item.binding == "symbol revision 1")
    );
    // Symbol and Footprint belong to the revision, not the details.
    assert!(part.details.field("Symbol").is_none());
    // Its local symbol and footprint say nothing about the BOM.
    assert_eq!(part.revision.excluded_from_bom, "No");
    assert!(part.revision.field("Symbol").is_some());
    assert!(team.part("CAP-0000-0001").unwrap().is_none());

    // A details edit adds no revision, and the history says what changed.
    let mut request = TeamEditRequest {
        correlation_id: None,
        ipn: IPN.into(),
        expected_details: 1,
        expected_revision: 1,
        reason: "Exact value".into(),
        category: String::new(),
        fields: part.details.fields.clone(),
        manufacturer_parts: part.details.manufacturer_parts.clone(),
        cad_fields: part.revision.cad_fields.clone(),
        files: Vec::new(),
    };
    request
        .fields
        .iter_mut()
        .find(|field| field.name == "Value")
        .unwrap()
        .value = "47R".into();
    // A save sent twice with the same correlation ID (as after a lost
    // answer) is replayed, not a conflict with yourself.
    request.correlation_id = Some("desktop-retry-1".into());
    let PublishResult::Saved {
        revision: again, ..
    } = team.edit(request.clone()).unwrap()
    else {
        panic!("not saved");
    };
    let PublishResult::Saved { revision, part, .. } = team.edit(request.clone()).unwrap() else {
        panic!("not saved");
    };
    assert_eq!((again, revision), (None, None));
    assert_eq!((part.details.number, part.revision.number), (2, 1));
    assert_eq!(part.history.len(), 2, "the repeated save saved nothing new");
    assert_eq!(
        (part.history[0].revision, part.history[0].details),
        (None, Some(2))
    );
    request.correlation_id = None;
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
    request.expected_details = 2;
    assert!(matches!(
        team.edit(request.clone()).unwrap(),
        PublishResult::NothingToSave { .. }
    ));
    // A symbol the Catalog does not have is refused with the server's words.
    let mut unknown = request.clone();
    unknown
        .cad_fields
        .iter_mut()
        .find(|field| field.name == "Symbol")
        .unwrap()
        .value = "g-res:NOPE".into();
    let PublishResult::Refused { message } = team.edit(unknown).unwrap() else {
        panic!("not refused");
    };
    assert!(message.contains("g-res:NOPE"), "{message}");

    // Someone else saves details 3 while this edit still starts from 2.
    let other_client = PocketBaseClient::new(&url)
        .authenticate("users", &other.email, &other.password)
        .unwrap();
    let readback = read_back(&admin).unwrap();
    let stored = readback.part(IPN).unwrap();
    let details = serde_json::from_str(
        &readback
            .part_details(&stored.current_details)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut theirs = EditCommand::new(IPN, "Better description").editing_details(&details, 2);
    theirs.set_field("Description", "Thick film resistor 47 ohm");
    submit_edit(&other_client, &theirs).unwrap();
    // It arrives by itself.
    wait_until("their change", || checkpoint(&team) == seeded + 2);

    let mut mine = request.clone();
    mine.fields
        .iter_mut()
        .find(|field| field.name == "Tolerance")
        .unwrap()
        .value = "0.5%".into();
    let PublishResult::Conflict(conflict) = team.edit(mine).unwrap() else {
        panic!("no conflict");
    };
    assert_eq!(
        (conflict.theirs.details, conflict.theirs.revision),
        (Some(3), None)
    );
    assert_eq!(conflict.theirs.actor_name, "Grace Editor");
    let labels: Vec<&str> = conflict.rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, ["Description", "Tolerance"]);
    assert!(conflict.rows.iter().all(|row| !row.both_changed));
    // Only the field you changed counts as yours.
    let changed: Vec<bool> = conflict.rows.iter().map(|row| row.you_changed).collect();
    assert_eq!(changed, [false, true]);
    // Saving on top keeps both changes.
    assert_eq!(
        (
            conflict.merged.expected_details,
            conflict.merged.expected_revision
        ),
        (3, 1)
    );
    let PublishResult::Saved { revision, part, .. } = team.edit(conflict.merged.clone()).unwrap()
    else {
        panic!("not saved on top");
    };
    assert_eq!(revision, None);
    assert_eq!(
        part.details.field("Description"),
        Some("Thick film resistor 47 ohm")
    );
    assert_eq!(part.details.field("Tolerance"), Some("0.5%"));

    // A details edit does not conflict with someone else's new symbol: the
    // two are saved apart. Theirs adds revision 2...
    let readback = read_back(&admin).unwrap();
    let stored = readback.part(IPN).unwrap();
    let revision: catalog::PartRevision = serde_json::from_str(
        &readback
            .part_revision(&stored.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let symbol = revision.field("Symbol").unwrap().to_owned();
    let mut theirs = EditCommand::new(IPN, "Use KiCad's symbol").editing_cad(&revision, 1);
    theirs.set_field("Symbol", "Device:R");
    submit_edit(&other_client, &theirs).unwrap();
    // ...while mine, started from revision 1, changes only a detail.
    let mut mine = conflict.merged.clone();
    mine.expected_details = part.details.number;
    mine.fields
        .iter_mut()
        .find(|field| field.name == "Datasheet")
        .unwrap()
        .value = "https://example.test/47.pdf".into();
    let PublishResult::Saved { revision, part, .. } = team.edit(mine.clone()).unwrap() else {
        panic!("not saved");
    };
    assert_eq!(revision, None);
    assert_eq!(part.revision.field("Symbol"), Some("Device:R"));
    assert_eq!(part.revision.number, 2);
    // KiCad's own symbol is not read, so the app does not claim to know.
    assert_eq!(
        part.revision.excluded_from_bom,
        "Not known: the symbol comes from KiCad's own library, which this app does not read yet"
    );
    // Changing the symbol back from a stale start is a conflict about the
    // symbol only; saving on top adds revision 3.
    let mut stale = mine.clone();
    stale.expected_details = part.details.number;
    stale
        .cad_fields
        .iter_mut()
        .find(|field| field.name == "Footprint")
        .unwrap()
        .value = "Device:R_0603".into();
    let PublishResult::Conflict(conflict) = team.edit(stale).unwrap() else {
        panic!("no conflict");
    };
    assert_eq!(conflict.theirs.revision, Some(2));
    let labels: Vec<&str> = conflict.rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, ["Symbol", "Footprint"]);
    let PublishResult::Saved { revision, part, .. } = team.edit(conflict.merged.clone()).unwrap()
    else {
        panic!("not saved on top");
    };
    assert_eq!(revision, Some(3));
    assert_eq!(part.revision.field("Symbol"), Some("Device:R"));
    assert_eq!(part.revision.field("Footprint"), Some("Device:R_0603"));
    assert_ne!(symbol, "Device:R");
    assert_eq!(
        part.history[0].changes[0].label, "Footprint",
        "{:?}",
        part.history[0]
    );
    // Searching finds team parts by the team catalog's details and lists
    // them with those details: the value is now 47R.
    wait_until("the saved changes", || checkpoint(&team) == seeded + 6);
    let cache = tempfile::tempdir().unwrap();
    let browser = catalog_browser::CatalogBrowser::open(catalog_browser::BrowserConfig {
        catalog_root: sample_library::root(),
        cache_dir: cache.path().join("cache"),
        settings_file: cache.path().join("settings.json"),
        kicad_locations: kicad_integration::SearchLocations::Only(Vec::new()),
        kicad_config_dir: None,
        kicad_documents_dir: None,
    });
    // The part list reads the team's last-known parts.
    let parts = [team
        .last_known_parts(&team.status().server_url, "TE")
        .unwrap()];
    assert_eq!(
        parts[0].category_names.get("res").map(String::as_str),
        Some("Resistors")
    );
    let found = browser.search_with("47R thick", 10, &parts);
    assert_eq!(found.total, 1, "{:?}", found.sections);
    assert_eq!(found.sections[0].name, "Resistors");
    let hit = found.hits().next().unwrap();
    assert_eq!(
        (
            hit.ipn.as_str(),
            hit.description.as_str(),
            hit.value.as_str(),
            hit.mpn.as_str()
        ),
        (IPN, "Thick film resistor 47 ohm", "47R", "RC0603FR-0747RL")
    );
    assert_eq!(browser.search("47R thick", 10).total, 0);
    // Words only the old description had no longer find it.
    assert!(
        browser
            .search_with("1/10W 47", 10, &parts)
            .hits()
            .all(|hit| hit.ipn != IPN)
    );

    // Archiving revision 3 makes revision 2 current again; the history says
    // so, the part lists revision 3 as archived, and revision 3 stays
    // readable, with its previews.
    let archive = |revision: u32, archived: bool, reason: &str| {
        team.archive(TeamArchiveRequest {
            correlation_id: None,
            ipn: IPN.into(),
            revision,
            archived,
            reason: reason.into(),
        })
        .unwrap()
    };
    let PublishResult::Archived {
        revision,
        archived,
        current,
        part,
    } = archive(3, true, "The footprint is the wrong size")
    else {
        panic!("not archived");
    };
    assert_eq!((revision, archived, current), (3, true, 2));
    assert_eq!(part.revision.number, 2);
    assert_eq!(
        part.revisions,
        [
            RevisionStatus {
                number: 1,
                archived: false
            },
            RevisionStatus {
                number: 2,
                archived: false
            },
            RevisionStatus {
                number: 3,
                archived: true
            },
        ]
    );
    let latest = &part.history[0];
    assert_eq!(
        (
            latest.action.as_str(),
            latest.revision,
            latest.reason.as_str()
        ),
        ("Archived", None, "The footprint is the wrong size")
    );
    let note = latest.archive.as_ref().unwrap();
    assert_eq!(
        (note.revision, note.archived, note.current_after),
        (3, true, 2)
    );
    assert!(matches!(
        team.cad_for_preview(IPN, 3, PreviewKind::Footprint),
        Ok(catalog_browser::team::TeamCad::Library { .. })
    ));
    assert!(matches!(
        team.cad_for_preview(IPN, 1, PreviewKind::Symbol),
        Ok(catalog_browser::team::TeamCad::Stored(_))
    ));
    // Archiving it again saves nothing; archiving 1 and 2 leaves 2 as the
    // only one, which is refused in plain words.
    assert!(matches!(
        archive(3, true, "again"),
        PublishResult::NothingToSave { .. }
    ));
    assert!(matches!(
        archive(1, true, "Old symbol"),
        PublishResult::Archived { current: 2, .. }
    ));
    let PublishResult::Refused { message } = archive(2, true, "Everything is wrong") else {
        panic!("not refused");
    };
    assert!(
        message.starts_with("Revision 2 is the only revision that is not archived"),
        "{message}"
    );
    wait_until("the archives", || checkpoint(&team) == seeded + 8);

    // A read-only member sees everything but cannot save.
    let reader = TeamCatalog::new();
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
    assert_eq!(part.history.len(), 9);
    let PublishResult::Refused { message } = reader
        .edit(TeamEditRequest {
            expected_details: 5,
            expected_revision: 3,
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

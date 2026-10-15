//! The desktop's team catalog use cases (catalog-browser's TeamCatalog)
//! against a disposable PocketBase with the resistors imported: sign-in,
//! history and differences, edits, a conflict resolved by saving on top,
//! read-only members, changes by others arriving by themselves, and CSV
//! changes that were not applied, reviewed field by field.

use catalog::pocketbase::{EditCommand, ImportCommand, read_back, submit_edit, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    LiveUpdates, PreviewKind, PublishResult, RevisionStatus, SignInRequest, TeamArchiveRequest,
    TeamCsvReviewRequest, TeamEditRequest, TeamSession,
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
    let import = services::plan(&root, "g-res").unwrap();
    submit_import(
        &admin,
        &ImportCommand::new(&import, "Import the resistors")
            .allowing_uncommitted_changes(services::has_uncommitted_changes(&import)),
    )
    .unwrap();
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

    // The part's history starts with the import, which added revision 1
    // and its first details.
    let part = team.part(IPN).unwrap().unwrap();
    assert_eq!((part.details.number, part.revision.number), (1, 1));
    assert_eq!(part.history.len(), 1);
    assert_eq!(
        (
            part.history[0].action.as_str(),
            part.history[0].revision,
            part.history[0].details
        ),
        ("Imported", Some(1), Some(1))
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
    // them with those details: the value is now 47R, which the CSV catalog
    // does not say.
    wait_until("the saved changes", || checkpoint(&team) == 7);
    let cache = tempfile::tempdir().unwrap();
    let browser = catalog_browser::CatalogBrowser::open(catalog_browser::BrowserConfig {
        catalog_root: root.clone(),
        cache_dir: cache.path().join("cache"),
        settings_file: cache.path().join("settings.json"),
        kicad_locations: kicad_integration::SearchLocations::Only(Vec::new()),
        kicad_config_dir: None,
        kicad_documents_dir: None,
    });
    let current = team.current_values();
    let names = team.category_names();
    assert_eq!(names.get("res").map(String::as_str), Some("Resistors"));
    let found = browser.search_with("47R thick", 10, &current, &names);
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
    // Words only the CSV description has no longer find it.
    assert!(
        browser
            .search_with("1/10W 47", 10, &current, &names)
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
    wait_until("the archives", || checkpoint(&team) == 9);

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

fn imports(team: &TeamCatalog) -> u64 {
    match team.status().session {
        TeamSession::SignedIn { imports, .. } => imports,
        other => panic!("not signed in: {other:?}"),
    }
}

/// Imports the resistors from `root` as an importer does, reading the team
/// catalog first.
fn import_from(admin: &PocketBaseClient, root: &std::path::Path) {
    let import = catalog::CsvCatalog::new(root)
        .import("g-res", None)
        .unwrap();
    let state = catalog::pocketbase::read_import_state(admin).unwrap();
    let command = ImportCommand::new(&import, "Import the resistors")
        .allowing_uncommitted_changes(services::has_uncommitted_changes(&import))
        .expecting(&state);
    let reply = submit_import(admin, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
}

#[test]
fn the_desktop_shows_csv_changes_live_and_takes_them_field_by_field() {
    let state = tempfile::tempdir().unwrap();
    // A copy of what the resistor import reads, which the test changes.
    let copy = tempfile::tempdir().unwrap();
    for path in [
        "database/g-res.csv",
        "database/substitutes.csv",
        "symbols/g-res.kicad_sym",
        "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod",
    ] {
        let destination = copy.path().join(path);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(services::repository_root().join(path), destination).unwrap();
    }
    let (server, admin, _superuser) =
        services::start(&services::repository_root(), state.path()).unwrap();
    import_from(&admin, copy.path());
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let other = services::add_member(&admin, "Grace Editor", "grace@example.test", true).unwrap();
    let url = server.url().to_owned();
    let team = TeamCatalog::open(state.path().join("team.json"), None, None);
    team.sign_in(SignInRequest {
        server_url: url.clone(),
        email: editor.email.clone(),
        password: editor.password.clone(),
    });
    wait_until("live updates", || {
        matches!(
            team.status().session,
            TeamSession::SignedIn {
                live: LiveUpdates::Live,
                ..
            }
        )
    });
    assert!(team.part(IPN).unwrap().unwrap().csv_changes.is_empty());

    // Someone edits the description and the value in the app.
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
    let mut theirs = EditCommand::new(IPN, "Plainer words").editing_details(&details, 1);
    theirs.set_field("Description", "Resistor 47 ohm");
    theirs.set_field("Value", "47 ohm");
    submit_edit(&other_client, &theirs).unwrap();
    wait_until("their edit", || checkpoint(&team) == 2);

    // Importing the unchanged CSV records that the part differs, but the
    // CSV has no new values, so the app offers nothing: only the part was
    // edited.
    let before = imports(&team);
    import_from(&admin, copy.path());
    wait_until("the unchanged import run", || imports(&team) > before);
    let part = team.part(IPN).unwrap().unwrap();
    assert!(part.csv_changes.is_empty(), "{:?}", part.csv_changes);
    let problems = read_back(&admin).unwrap().problems;
    assert!(
        problems
            .iter()
            .any(|problem| problem.code == catalog::pocketbase::CSV_DETAILS_DIFFER),
        "the import problem is still recorded"
    );

    // The CSV changes the value; the import leaves the edited part alone
    // and publishes no Change, yet the app hears about the run and offers
    // the new values.
    let csv = copy.path().join("database/g-res.csv");
    let text = std::fs::read_to_string(&csv).unwrap();
    std::fs::write(&csv, text.replacen("\"47\",", "\"47R\",", 1)).unwrap();
    let before = imports(&team);
    import_from(&admin, copy.path());
    wait_until("the import run", || imports(&team) > before);
    assert_eq!(checkpoint(&team), 2, "the import published no Change");
    let part = team.part(IPN).unwrap().unwrap();
    assert_eq!(part.csv_changes.len(), 1, "{:?}", part.csv_changes);
    let changes = &part.csv_changes[0];
    assert_eq!(
        (changes.csv_file.as_str(), changes.cad, changes.expected),
        ("g-res.csv", false, 2)
    );
    // The value the CSV changed comes first; the description is listed
    // too, marked as not changed in the CSV (accepting it undoes the edit).
    let rows: Vec<(&str, &str, &str, bool)> = changes
        .rows
        .iter()
        .map(|row| {
            (
                row.field.as_str(),
                row.now.as_str(),
                row.csv.as_str(),
                row.changed_in_csv,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Value", "47 ohm", "47R", true),
            (
                "Description",
                "Resistor 47 ohm",
                "RES 47 OHM 1% 1/10W 0603",
                false
            ),
        ]
    );

    // Taking the value saves details 3 and adds no revision; the
    // description stays, and the notice is gone.
    let PublishResult::CsvReviewed {
        accepted,
        details_saved,
        revision,
        part,
    } = team
        .csv_review(TeamCsvReviewRequest {
            correlation_id: None,
            ipn: IPN.into(),
            problem: changes.problem.clone(),
            accept: vec!["Value".into()],
            expected: changes.expected,
            reason: "Accept CSV changes from g-res.csv".into(),
        })
        .unwrap()
    else {
        panic!("not reviewed");
    };
    assert_eq!(
        (accepted, details_saved, revision),
        (vec!["Value".to_owned()], true, None)
    );
    assert_eq!((part.details.number, part.revision.number), (3, 1));
    assert_eq!(part.details.field("Value"), Some("47R"));
    assert_eq!(part.details.field("Description"), Some("Resistor 47 ohm"));
    assert!(part.csv_changes.is_empty());
    let latest = &part.history[0];
    assert_eq!(
        (latest.action.as_str(), latest.details, latest.revision),
        ("Accepted CSV values", Some(3), None)
    );
    // Reviewing the same CSV values again saves nothing.
    assert!(matches!(
        team.csv_review(TeamCsvReviewRequest {
            correlation_id: None,
            ipn: IPN.into(),
            problem: changes.problem.clone(),
            accept: Vec::new(),
            expected: 3,
            reason: "Keep".into(),
        })
        .unwrap(),
        PublishResult::NothingToSave { .. }
    ));

    // When the CSV changes again, the notice comes back; keeping the
    // current values is recorded in the history and clears it.
    let text = std::fs::read_to_string(&csv).unwrap();
    std::fs::write(
        &csv,
        text.replacen(
            "\"47R\",\"g-res:R_US\",\"g-Resistor_SMD:R_0603_1608Metric\",\"1%\"",
            "\"47R\",\"g-res:R_US\",\"g-Resistor_SMD:R_0603_1608Metric\",\"0.5%\"",
            1,
        ),
    )
    .unwrap();
    let before = imports(&team);
    import_from(&admin, copy.path());
    wait_until("the second import run", || imports(&team) > before);
    let part = team.part(IPN).unwrap().unwrap();
    let changes = part.csv_changes[0].clone();
    let fields: Vec<(&str, bool)> = changes
        .rows
        .iter()
        .map(|row| (row.field.as_str(), row.changed_in_csv))
        .collect();
    assert_eq!(fields, [("Tolerance", true), ("Description", false)]);
    let PublishResult::CsvReviewed {
        details_saved,
        part,
        ..
    } = team
        .csv_review(TeamCsvReviewRequest {
            correlation_id: None,
            ipn: IPN.into(),
            problem: changes.problem.clone(),
            accept: Vec::new(),
            expected: changes.expected,
            reason: "Keep the values edited in this app".into(),
        })
        .unwrap()
    else {
        panic!("not kept");
    };
    assert!(!details_saved);
    assert!(part.csv_changes.is_empty());
    assert_eq!(part.history[0].action, "Kept current part details");
    assert_eq!(part.details.number, 3);

    // The CSV goes back to what it said at the first import. That is a real
    // change since the last review (the part keeps 47R from the CSV's
    // earlier values), so the app asks again, though the first import saved
    // exactly this.
    std::fs::copy(services::repository_root().join("database/g-res.csv"), &csv).unwrap();
    let before = imports(&team);
    import_from(&admin, copy.path());
    wait_until("the import of the first CSV again", || {
        imports(&team) > before
    });
    let part = team.part(IPN).unwrap().unwrap();
    assert_eq!(part.csv_changes.len(), 1, "{:?}", part.csv_changes);
    let rows: Vec<(&str, &str, &str, bool)> = part.csv_changes[0]
        .rows
        .iter()
        .map(|row| {
            (
                row.field.as_str(),
                row.now.as_str(),
                row.csv.as_str(),
                row.changed_in_csv,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Value", "47R", "47", true),
            (
                "Description",
                "Resistor 47 ohm",
                "RES 47 OHM 1% 1/10W 0603",
                false
            ),
        ]
    );

    assert!(!team.sign_out().session.is_signed_in());
    server.stop();
}

//! The desktop's team catalog with CSV changes that were not applied,
//! shown live and reviewed field by field. Split from `desktop_team.rs`
//! (kicad-parts-fbc2): this tests the CSV import and the CSV review, which
//! kicad-parts-e257 removes with the CSV catalog.

use catalog::pocketbase::{EditCommand, ImportCommand, read_back, submit_edit, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    LiveUpdates, PublishResult, SignInRequest, TeamCsvReviewRequest, TeamSession,
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
    let team = TeamCatalog::new();
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

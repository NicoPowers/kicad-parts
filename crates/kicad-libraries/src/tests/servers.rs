//! A team's folder takes updates only from the team server it records
//! (kicad-parts-092b). Every member of a team knows its ID, so another
//! server could claim it; only the person can say the team moved.

use super::layout::files;
use super::{resistors, team, test_folder};
use crate::{STATE_FILE, Team, UpdateError, read_state, same_server, update, use_new_server};
use std::path::Path;

/// The team `team()` at another server, as if it moved, or as if another
/// server claimed its ID.
fn at(server: &str) -> Team {
    Team {
        server: server.into(),
        ..team()
    }
}

fn run_for(root: &Path, team: &Team) -> Result<crate::Outcome, UpdateError> {
    let fixture = resistors();
    let mut blobs = fixture.blobs();
    update(root, &fixture.catalog, team, &mut blobs)
}

fn recorded_server(root: &Path) -> String {
    read_state(&root.join("MA")).unwrap().server
}

#[test]
fn an_older_folder_records_the_server_of_its_first_update() {
    let root = test_folder("server-adopted");
    std::fs::create_dir_all(root.join("MA")).unwrap();
    // Made before servers were recorded: the team's ID and code only.
    std::fs::write(
        root.join("MA").join(STATE_FILE),
        r#"{"team_id":"team-settings-ma","code":"MA","updated_at_ms":1}"#,
    )
    .unwrap();
    run_for(&root, &team()).unwrap();
    assert_eq!(recorded_server(&root), "http://127.0.0.1:8090");
    // From then on, only that server writes.
    assert!(matches!(
        run_for(&root, &at("https://other.example.com")),
        Err(UpdateError::OtherServer { .. })
    ));
}

#[test]
fn another_server_cannot_write_until_the_person_says_the_team_moved() {
    let root = test_folder("server-moved");
    run_for(&root, &team()).unwrap();
    // The same server writes again.
    run_for(&root, &team()).unwrap();
    let before = files(&root.join("MA"));
    let moved = at("https://new.example.com");
    match run_for(&root, &moved) {
        Err(error @ UpdateError::OtherServer { .. }) => assert_eq!(
            error.to_string(),
            "The KiCad libraries for MA were made from http://127.0.0.1:8090, so they were not changed from https://new.example.com. If the team moved to the new address, choose Use the new address in the Teams menu."
        ),
        other => panic!("expected the other server to be refused: {other:?}"),
    }
    assert_eq!(files(&root.join("MA")), before, "nothing was written");
    // The person confirms: the folder records the new server.
    use_new_server(&root, &moved).unwrap();
    assert_eq!(recorded_server(&root), "https://new.example.com");
    run_for(&root, &moved).unwrap();
    // And the old server no longer writes.
    assert!(matches!(
        run_for(&root, &team()),
        Err(UpdateError::OtherServer { .. })
    ));
}

#[test]
fn confirming_a_new_server_never_takes_another_teams_folder() {
    let root = test_folder("server-other-team");
    run_for(&root, &team()).unwrap();
    let claimed = Team {
        id: "team-settings-other".into(),
        ..at("https://new.example.com")
    };
    assert!(matches!(
        use_new_server(&root, &claimed),
        Err(UpdateError::OtherTeam { .. })
    ));
    assert_eq!(recorded_server(&root), "http://127.0.0.1:8090");
    // A team with no folder yet has nothing to confirm.
    let empty = test_folder("server-no-folder");
    use_new_server(&empty, &team()).unwrap();
    assert!(!empty.join("MA").exists());
}

#[test]
fn an_address_written_the_older_way_names_the_same_server() {
    assert!(same_server(
        "https://Parts.Example.com:443/acme",
        "https://parts.example.com/acme"
    ));
    assert!(!same_server(
        "https://parts.example.com/acme",
        "https://parts.example.com/beta"
    ));
    let root = test_folder("server-older-address");
    std::fs::create_dir_all(root.join("MA")).unwrap();
    std::fs::write(
        root.join("MA").join(STATE_FILE),
        r#"{"team_id":"team-settings-ma","server":"https://Parts.example.com:443","code":"MA"}"#,
    )
    .unwrap();
    run_for(&root, &at("https://parts.example.com")).unwrap();
    assert_eq!(recorded_server(&root), "https://parts.example.com");
}

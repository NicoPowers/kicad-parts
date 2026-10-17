//! The team's settings and its code (kicad-parts-f397): every member reads
//! them; only members who can edit the Catalog choose the code, which is
//! set once and never changed through the route. Each test starts its own
//! disposable PocketBase on 127.0.0.1.

use catalog::pocketbase::{TeamCodeOutcome, read_back, read_team, submit_team_code};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};

fn member(
    server: &native_services::PocketBaseServer,
    admin: &PocketBaseClient,
    name: &str,
    can_edit: bool,
) -> PocketBaseClient {
    let email = format!("{}@example.test", name.to_lowercase().replace(' ', "."));
    let member = services::add_member(admin, name, &email, can_edit).unwrap();
    PocketBaseClient::new(server.url())
        .authenticate("users", &member.email, &member.password)
        .unwrap()
}

#[test]
fn the_team_code_is_set_once_by_someone_who_can_edit() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let editor = member(&server, &admin, "Ada Editor", true);
    let viewer = member(&server, &admin, "Vic Viewer", false);

    // Every member reads the settings: no code yet, and who may choose it.
    let settings = read_team(&viewer).unwrap();
    assert_eq!(settings.code, "");
    assert_eq!(settings.id.len(), 15);
    assert_eq!(settings.editors, ["Ada Editor"]);

    // A member who cannot edit is refused.
    match submit_team_code(&viewer, "MA") {
        Err(CollabError::Forbidden(_)) => {}
        other => panic!("expected 403: {other:?}"),
    }
    // Not a team code.
    for code in ["ma", "M", "MABC", "1A", "M-"] {
        match submit_team_code(&editor, code) {
            Err(CollabError::Http { status: 400, .. }) => {}
            other => panic!("expected 400 for {code}: {other:?}"),
        }
    }
    assert_eq!(read_team(&editor).unwrap().code, "");

    // Set once; the same code again changes nothing; another is refused.
    assert_eq!(
        submit_team_code(&editor, "MA").unwrap(),
        TeamCodeOutcome::Applied
    );
    assert_eq!(
        submit_team_code(&editor, "MA").unwrap(),
        TeamCodeOutcome::Noop
    );
    assert_eq!(
        submit_team_code(&admin, "SL").unwrap(),
        TeamCodeOutcome::AlreadySet { code: "MA".into() }
    );
    let after = read_team(&viewer).unwrap();
    assert_eq!(
        (after.id, after.code.as_str(), after.code_set_by.as_str()),
        (settings.id, "MA", "Ada Editor")
    );
    // Members cannot write the record themselves.
    let direct = viewer
        .post_json(
            "/api/collections/team_settings/records",
            &serde_json::json!({ "code": "SL" }),
        )
        .unwrap();
    assert!(direct.status >= 400, "{direct:?}");
    // The choice is in the audit entries.
    let readback = read_back(&admin).unwrap();
    let audit: Vec<_> = readback
        .audit_entries
        .iter()
        .filter(|entry| entry.action == "team.code")
        .collect();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_name, "Ada Editor");
    server.stop();
}

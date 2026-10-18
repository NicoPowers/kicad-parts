//! Several teams at once (kicad-parts-f10d): the desktop's list of teams
//! against two real, disposable PocketBase servers on 127.0.0.1, each with
//! the resistors imported. Both teams are followed live at the same time,
//! and a change on one reaches only that team. Both teams' KiCad libraries
//! are kept up to date, each in its own folder. A restart carries on both
//! sign-ins from the kept tokens, with no password; a rejected token asks
//! only that team to sign in again; and a stopped server leaves the other
//! team working.
//!
//! The kept tokens live in a store in memory, never the system's
//! credential store.

use catalog::pocketbase::{ImportCommand, submit_import, submit_team_code};
use catalog_browser::kicad_libraries::KicadLibraries;
use catalog_browser::team::{SIGN_IN_AGAIN, TeamCatalog};
use catalog_browser::teams::{MemorySignIns, SignInStore, SignIns, Teams};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    KicadLibrariesState, LiveUpdates, PublishResult, SignInRequest, TeamEditRequest, TeamSession,
    TeamStatus,
};
use native_services::PocketBaseServer;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const IPN: &str = "RES-0000-0470";
const EMAIL: &str = "ada@example.test";

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One client's team server, with the resistors imported and the team code
/// chosen.
struct Client {
    server: Option<PocketBaseServer>,
    url: String,
    admin: PocketBaseClient,
    ada: services::Member,
    _state: tempfile::TempDir,
}

impl Client {
    fn start(root: &Path, code: &str) -> Self {
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(root, state.path()).unwrap();
        let import = services::plan(root, "g-res").unwrap();
        submit_import(
            &admin,
            &ImportCommand::new(&import, "Import the resistors")
                .allowing_uncommitted_changes(services::has_uncommitted_changes(&import)),
        )
        .unwrap();
        let ada = services::add_member(&admin, "Ada Editor", EMAIL, true).unwrap();
        let url = server.url().to_owned();
        let member = PocketBaseClient::new(&url)
            .authenticate("users", EMAIL, &ada.password)
            .unwrap();
        submit_team_code(&member, code).unwrap();
        Self {
            server: Some(server),
            url,
            admin,
            ada,
            _state: state,
        }
    }

    fn sign_in(&self) -> SignInRequest {
        SignInRequest {
            server_url: self.url.clone(),
            email: EMAIL.into(),
            password: self.ada.password.clone(),
        }
    }

    /// A new password ends every earlier sign-in.
    fn change_password(&mut self) {
        let users = self
            .admin
            .list_records::<serde_json::Value>("users", &format!("email = '{EMAIL}'"), "email")
            .unwrap();
        let id = users[0]["id"].as_str().unwrap();
        let password = native_services::random_token(20);
        self.admin
            .patch_json(
                &format!("/api/collections/users/records/{id}"),
                &serde_json::json!({ "password": password, "passwordConfirm": password }),
            )
            .unwrap()
            .success()
            .unwrap();
        self.ada.password = password;
    }

    /// Another member saves a new Value for the part.
    fn someone_else_edits(&self, value: &str) {
        let grace = services::add_member(
            &self.admin,
            "Grace Editor",
            &format!("grace-{value}@example.test"),
            true,
        )
        .unwrap();
        let other = TeamCatalog::new();
        other.sign_in(SignInRequest {
            server_url: self.url.clone(),
            email: grace.email.clone(),
            password: grace.password.clone(),
        });
        let part = other.part(IPN).unwrap().unwrap();
        let mut request = TeamEditRequest {
            correlation_id: None,
            ipn: IPN.into(),
            expected_details: part.details.number,
            expected_revision: part.revision.number,
            reason: "Write the value the way the BOM does".into(),
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
            .value = value.into();
        assert!(matches!(
            other.edit(request).unwrap(),
            PublishResult::Saved { .. }
        ));
    }
}

fn team<'a>(teams: &'a [TeamStatus], client: &Client) -> &'a TeamStatus {
    teams
        .iter()
        .find(|team| team.server_url == client.url)
        .unwrap()
}

fn live(teams: &Teams, client: &Client) -> Option<u64> {
    match &team(&teams.status().teams, client).session {
        TeamSession::SignedIn {
            live: LiveUpdates::Live,
            checkpoint,
            ..
        } => Some(*checkpoint),
        _ => None,
    }
}

fn catalog_of(teams: &Teams, client: &Client) -> Arc<TeamCatalog> {
    teams
        .every()
        .into_iter()
        .find(|(server, _)| *server == client.url)
        .unwrap()
        .1
}

fn value_of(catalog: &TeamCatalog) -> Option<String> {
    catalog
        .current_values()
        .get(IPN)?
        .fields
        .as_ref()?
        .iter()
        .find_map(|(name, value)| (name == "Value").then(|| value.clone()))
}

/// Clears every read-only mark under `folder`, so it can be deleted.
fn clear_read_only(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            clear_read_only(&path);
        } else if let Ok(metadata) = std::fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&path, permissions);
        }
    }
}

#[test]
fn two_teams_are_followed_side_by_side_and_stay_signed_in() {
    let root = services::repository_root();
    let mut acme = Client::start(&root, "AC");
    let mut beta = Client::start(&root, "BE");
    let store = Arc::new(MemorySignIns::default());
    let kept_in = || SignIns::kept_in(store.clone());
    let config = tempfile::tempdir().unwrap();
    let list = config.path().join("team.json");
    let folder = root
        .join("target")
        .join("kicad-libraries-tests")
        .join("desktop-teams");
    clear_read_only(&folder);
    let _ = std::fs::remove_dir_all(&folder);

    // Sign in to both: the second sign-in keeps the first team signed in.
    let teams = Arc::new(Teams::open(list.clone(), kept_in(), None, None));
    let libraries = KicadLibraries::start(folder.clone(), teams.clone());
    teams.sign_in(acme.sign_in()).unwrap();
    teams.sign_in(beta.sign_in()).unwrap();
    wait_until("both teams to be live", || {
        live(&teams, &acme).is_some() && live(&teams, &beta).is_some()
    });
    assert_eq!(teams.status().shown, beta.url, "the last sign-in is shown");

    // Both teams' KiCad libraries are made, each in its own folder.
    let up_to_date = |client: &Client| {
        libraries
            .status_of(&client.url)
            .is_some_and(|status| matches!(status.state, KicadLibrariesState::UpToDate { .. }))
    };
    wait_until("both teams' libraries", || {
        up_to_date(&acme) && up_to_date(&beta)
    });
    assert!(folder.join("AC").join("AC-Parts.kicad_dbl").is_file());
    assert!(folder.join("BE").join("BE-Parts.kicad_dbl").is_file());
    let status = teams.status();
    let codes: Vec<&str> = status.teams.iter().map(|team| team.code.as_str()).collect();
    assert_eq!(codes, ["AC", "BE"]);

    // A change on acme reaches acme only.
    let (acme_at, beta_at) = (live(&teams, &acme).unwrap(), live(&teams, &beta).unwrap());
    acme.someone_else_edits("47R");
    wait_until("acme to follow the change", || {
        live(&teams, &acme).is_some_and(|at| at > acme_at)
    });
    assert_eq!(value_of(&catalog_of(&teams, &acme)).as_deref(), Some("47R"));
    assert_eq!(live(&teams, &beta), Some(beta_at));
    assert_eq!(value_of(&catalog_of(&teams, &beta)).as_deref(), Some("47"));
    // Switching shows the other team at once.
    teams.show(&acme.url).unwrap();
    assert!(Arc::ptr_eq(&teams.shown(), &catalog_of(&teams, &acme)));
    wait_until("both sign-ins to be kept", || {
        store.read(&acme.url).unwrap().is_some() && store.read(&beta.url).unwrap().is_some()
    });

    // A restart carries on both sign-ins from the kept tokens.
    drop(libraries);
    drop(teams);
    let teams = Teams::open(list.clone(), kept_in(), None, None);
    wait_until("both teams to be live again with no password", || {
        live(&teams, &acme).is_some() && live(&teams, &beta).is_some()
    });
    assert_eq!(teams.status().shown, acme.url, "acme is still shown");
    let values = catalog_of(&teams, &acme);
    assert_eq!(value_of(&values).as_deref(), Some("47R"));

    // Acme's password changes while the app is closed: only acme asks to
    // sign in again.
    drop(teams);
    acme.change_password();
    let teams = Teams::open(list.clone(), kept_in(), None, None);
    wait_until("beta to be live", || live(&teams, &beta).is_some());
    wait_until("acme to ask for its password", || {
        team(&teams.status().teams, &acme).session
            == TeamSession::SignedOut {
                problem: Some(SIGN_IN_AGAIN.into()),
            }
    });
    assert_eq!(
        store.read(&acme.url).unwrap(),
        None,
        "its token is forgotten"
    );
    assert_eq!(teams.status().shown, beta.url, "beta is shown meanwhile");
    teams.sign_in(acme.sign_in()).unwrap();
    wait_until("acme to be live again", || live(&teams, &acme).is_some());

    // Beta's server stops: beta is offline with its parts kept, and acme
    // keeps working.
    beta.server.take().unwrap().stop();
    wait_until("beta to be offline", || {
        matches!(
            team(&teams.status().teams, &beta).session,
            TeamSession::SignedIn {
                live: LiveUpdates::Reconnecting { .. },
                ..
            }
        )
    });
    assert_eq!(
        value_of(&catalog_of(&teams, &beta)).as_deref(),
        Some("47"),
        "beta's last-known parts stay"
    );
    let part = catalog_of(&teams, &acme).part(IPN).unwrap().unwrap();
    assert_eq!(part.ipn, IPN);
    acme.someone_else_edits("47.0");
    wait_until("acme to follow a change while beta is offline", || {
        value_of(&catalog_of(&teams, &acme)).as_deref() == Some("47.0")
    });
    drop(teams);
    clear_read_only(&folder);
    let _ = std::fs::remove_dir_all(&folder);
}

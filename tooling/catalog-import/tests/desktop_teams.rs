//! Several teams at once (kicad-parts-f10d): the desktop's list of teams
//! against two real, disposable PocketBase servers on 127.0.0.1, each with
//! the sample library's resistors. Both teams are followed live at the same time,
//! and a change on one reaches only that team. Both teams' KiCad libraries
//! are kept up to date, each in its own folder. A restart carries on both
//! sign-ins from the kept tokens, with no password; a rejected token asks
//! only that team to sign in again; and a stopped server leaves the other
//! team working. A second server that claims a team's ID cannot write that
//! team's libraries until the person says the team moved (kicad-parts-092b).
//!
//! One part list shows both teams' parts (kicad-parts-8a71): the same IPN
//! once per team, an edit saved only to its own team's server, a
//! teammate's save changing only that team's rows, and a stopped server
//! leaving its rows in the list, marked offline, while the other team can
//! still be edited.
//!
//! A team whose server is down at start shows the parts the last run kept
//! on disk, offline, until its server is back (kicad-parts-ed46).
//!
//! The kept tokens live in a store in memory, never the system's
//! credential store.

use catalog::pocketbase::submit_team_code;
use catalog_browser::kicad_libraries::KicadLibraries;
use catalog_browser::team::{SIGN_IN_AGAIN, TeamCatalog};
use catalog_browser::teams::{MemorySignIns, SignInStore, SignIns, Teams};
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    GroupBy, KicadLibrariesState, LibrariesMadeFrom, LiveUpdates, PartSummary, PublishResult,
    SearchRequest, SignInRequest, TeamEditRequest, TeamSession, TeamStatus, UseNewServerRequest,
    two_servers_claim_a_team,
};
use native_services::{PocketBaseServer, StoppedPocketBase, Superuser};
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

/// One client's team server, with the sample resistors and the team code
/// chosen.
struct Client {
    server: Option<PocketBaseServer>,
    url: String,
    admin: PocketBaseClient,
    superuser: Superuser,
    ada: services::Member,
    _state: tempfile::TempDir,
}

impl Client {
    fn start(root: &Path, code: &str) -> Self {
        let state = tempfile::tempdir().unwrap();
        let (server, admin, superuser) = services::start(root, state.path()).unwrap();
        sample_library::seed(&admin, &["res"]).unwrap();
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
            superuser,
            ada,
            _state: state,
        }
    }

    /// Starts the server stopped by `stop_for_a_while` again, at the same
    /// address. The superuser signs in afresh: its old connections went
    /// with the old process.
    fn start_again(&mut self, stopped: StoppedPocketBase) {
        self.server = Some(stopped.start_again().unwrap());
        self.admin = PocketBaseClient::new(&self.url)
            .authenticate(
                "_superusers",
                &self.superuser.email,
                &self.superuser.password,
            )
            .unwrap();
    }

    fn sign_in(&self) -> SignInRequest {
        SignInRequest {
            server_url: self.url.clone(),
            email: EMAIL.into(),
            password: self.ada.password.clone(),
        }
    }

    /// Makes this server report the team settings record `id` with `code`,
    /// as a server claiming another team would. Only a superuser can.
    fn claim_team(&self, id: &str, code: &str) {
        let settings = self
            .admin
            .list_records::<serde_json::Value>("team_settings", "", "")
            .unwrap();
        for record in settings {
            let own = record["id"].as_str().unwrap();
            self.admin
                .delete(&format!("/api/collections/team_settings/records/{own}"))
                .unwrap()
                .success()
                .unwrap();
        }
        self.admin
            .post_json(
                "/api/collections/team_settings/records",
                &serde_json::json!({ "id": id, "code": code }),
            )
            .unwrap()
            .success()
            .unwrap();
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
        assert!(matches!(
            value_edit(&self.another_member(value), value),
            PublishResult::Saved { .. }
        ));
    }

    /// Another member of this team, new each time, signed in on their own.
    fn another_member(&self, name: &str) -> TeamCatalog {
        let unique = native_services::random_token(10).to_lowercase();
        let grace = services::add_member(
            &self.admin,
            &format!("Grace {name}"),
            &format!("grace-{unique}@example.test"),
            true,
        )
        .unwrap();
        let other = TeamCatalog::new();
        other.sign_in(SignInRequest {
            server_url: self.url.clone(),
            email: grace.email.clone(),
            password: grace.password.clone(),
        });
        other
    }

    /// The part's Value as this team's server has it now, read afresh.
    fn value_on_server(&self) -> String {
        let reader = self.another_member("reader");
        let part = reader.part(IPN).unwrap().unwrap();
        part.details.field("Value").unwrap_or_default().to_owned()
    }
}

/// Saves `value` as the part's Value through `team`.
fn value_edit(team: &TeamCatalog, value: &str) -> PublishResult {
    let part = team.part(IPN).unwrap().unwrap();
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
    team.edit(request).unwrap()
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
    let teams = Arc::new(Teams::open(
        list.clone(),
        config.path().join("parts-files"),
        kept_in(),
        None,
        None,
    ));
    let libraries = KicadLibraries::start(folder.clone(), teams.clone());
    teams.sign_in(acme.sign_in()).unwrap();
    teams.sign_in(beta.sign_in()).unwrap();
    wait_until("both teams to be live", || {
        live(&teams, &acme).is_some() && live(&teams, &beta).is_some()
    });
    // Each team got its own colour, kept with the list.
    let colours: Vec<String> = teams
        .status()
        .teams
        .into_iter()
        .map(|team| team.colour)
        .collect();
    assert_eq!(colours, ["blue", "green"]);

    // Both teams' KiCad libraries are made, each in its own folder.
    let up_to_date = |client: &Client| {
        matches!(
            libraries.status(&client.url).state,
            KicadLibrariesState::UpToDate { .. }
        )
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
    wait_until("both sign-ins to be kept", || {
        store.read(&acme.url).unwrap().is_some() && store.read(&beta.url).unwrap().is_some()
    });

    // A restart carries on both sign-ins from the kept tokens.
    drop(libraries);
    drop(teams);
    let teams = Teams::open(
        list.clone(),
        config.path().join("parts-files"),
        kept_in(),
        None,
        None,
    );
    wait_until("both teams to be live again with no password", || {
        live(&teams, &acme).is_some() && live(&teams, &beta).is_some()
    });
    let values = catalog_of(&teams, &acme);
    assert_eq!(value_of(&values).as_deref(), Some("47R"));

    // Acme's password changes while the app is closed: only acme asks to
    // sign in again.
    drop(teams);
    acme.change_password();
    let teams = Teams::open(
        list.clone(),
        config.path().join("parts-files"),
        kept_in(),
        None,
        None,
    );
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
    // Beta's parts are in the list meanwhile; acme's are not.
    let shown: Vec<String> = teams
        .known_parts()
        .iter()
        .map(|parts| parts.team.clone())
        .collect();
    assert_eq!(shown, [beta.url.clone()]);
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

/// A team's KiCad libraries take updates only from the server they were
/// made from (kicad-parts-092b). Every member knows the team's settings
/// ID, so a second server can claim it; that server writes nothing until
/// the person says the team moved there. While both servers are in the
/// person's teams, no answer is taken, so the libraries cannot flip back
/// and forth.
#[test]
fn a_second_server_claiming_a_team_cannot_write_its_libraries_until_confirmed() {
    let root = services::repository_root();
    let acme = Client::start(&root, "AC");
    // Another server, with a team code of its own for now.
    let claimer = Client::start(&root, "CL");
    let config = tempfile::tempdir().unwrap();
    let folder = root
        .join("target")
        .join("kicad-libraries-tests")
        .join("desktop-teams-claimed");
    clear_read_only(&folder);
    let _ = std::fs::remove_dir_all(&folder);
    let teams = Arc::new(Teams::open(
        config.path().join("team.json"),
        config.path().join("parts-files"),
        SignIns::kept_in(Arc::new(MemorySignIns::default())),
        None,
        None,
    ));
    let libraries = KicadLibraries::start(folder.clone(), teams.clone());
    let state_of = |client: &Client| libraries.status(&client.url).state;
    teams.sign_in(acme.sign_in()).unwrap();
    wait_until("acme's libraries", || {
        matches!(state_of(&acme), KicadLibrariesState::UpToDate { .. })
    });
    let recorded = kicad_libraries::read_state(&folder.join("AC")).unwrap();
    assert_eq!(recorded.server, acme.url);

    // The second server now claims acme's team: same ID, same code.
    claimer.claim_team(&recorded.team_id, "AC");
    teams.sign_in(claimer.sign_in()).unwrap();
    // Both servers are in the person's teams, so two servers say they are
    // AC. The app keeps acme's libraries as they are.
    let both_listed = KicadLibrariesState::OtherServer {
        code: "AC".into(),
        recorded_server: acme.url.clone(),
        server: claimer.url.clone(),
        recorded_server_in_your_teams: true,
    };
    wait_until("the claimer's libraries to wait", || {
        state_of(&claimer) == both_listed
    });
    // Asking the claimer's libraries for an update changes nothing. The
    // status Update now gives says so too, as every panel status does.
    assert_eq!(libraries.update_now(&claimer.url).state, both_listed);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(state_of(&claimer), both_listed);
    let unchanged = || {
        assert_eq!(
            kicad_libraries::read_state(&folder.join("AC")).unwrap(),
            recorded,
            "the folder was not written"
        );
    };
    unchanged();
    // The claimer's row says two servers claim AC, and offers no answer;
    // an answer sent anyway is refused.
    let made_from = |client: &Client| {
        let mut status = teams.status();
        libraries.note_other_servers(&mut status);
        team(&status.teams, client).libraries_made_from.clone()
    };
    assert_eq!(
        made_from(&claimer),
        Some(LibrariesMadeFrom {
            code: "AC".into(),
            server: acme.url.clone(),
            in_your_teams: true,
        })
    );
    assert_eq!(made_from(&acme), None);
    let answer = |client: &Client, from: &Client| UseNewServerRequest {
        server_url: client.url.clone(),
        code: "AC".into(),
        made_from: from.url.clone(),
    };
    assert_eq!(
        libraries.use_new_server(&answer(&claimer, &acme)),
        Err(two_servers_claim_a_team("AC", &acme.url, &claimer.url))
    );
    unchanged();

    // The team moved: the person removes acme. Now the claimer's row asks.
    teams.remove(&acme.url);
    assert_eq!(
        made_from(&claimer).map(|made| made.in_your_teams),
        Some(false)
    );
    // An answer to a question that changed meanwhile is refused.
    let out_of_date = UseNewServerRequest {
        made_from: "https://elsewhere.example.com".into(),
        ..answer(&claimer, &acme)
    };
    assert_eq!(
        libraries.use_new_server(&out_of_date),
        Err("The KiCad libraries for AC changed meanwhile, so your answer was not saved. Read the question again.".into())
    );
    unchanged();
    // The person says the team moved: the folder records the new server,
    // and the new server's update runs.
    libraries.use_new_server(&answer(&claimer, &acme)).unwrap();
    wait_until("the libraries to update from the new server", || {
        matches!(state_of(&claimer), KicadLibrariesState::UpToDate { .. })
    });
    assert_eq!(
        kicad_libraries::read_state(&folder.join("AC"))
            .unwrap()
            .server,
        claimer.url
    );

    // Adding acme back cannot hand the libraries back with one click:
    // two servers say they are AC again, and the claimer keeps them.
    teams.sign_in(acme.sign_in()).unwrap();
    wait_until("acme's libraries to wait", || {
        made_from(&acme).is_some_and(|made| made.in_your_teams)
    });
    assert_eq!(
        libraries.use_new_server(&answer(&acme, &claimer)),
        Err(two_servers_claim_a_team("AC", &claimer.url, &acme.url))
    );
    assert_eq!(
        kicad_libraries::read_state(&folder.join("AC"))
            .unwrap()
            .server,
        claimer.url
    );
    drop(libraries);
    drop(teams);
    clear_read_only(&folder);
    let _ = std::fs::remove_dir_all(&folder);
}

/// The part list as the app asks for it: every team, by category.
fn list(browser: &CatalogBrowser, teams: &Teams, query: &str) -> Vec<PartSummary> {
    let request = SearchRequest {
        query: query.into(),
        limit: 50,
        selected: None,
        section: None,
        uses: None,
        team: None,
        group_by: GroupBy::Category,
    };
    browser
        .search_request(&request, &teams.known_parts())
        .hits()
        .cloned()
        .collect()
}

/// The row of the part in the team at `client`.
fn row<'a>(rows: &'a [PartSummary], client: &Client) -> &'a PartSummary {
    rows.iter()
        .find(|row| row.team == client.url && row.ipn == IPN)
        .unwrap_or_else(|| panic!("no row for {} in {rows:?}", client.url))
}

#[test]
fn one_list_holds_both_teams_and_each_edit_goes_to_its_own_server() {
    let root = services::repository_root();
    let mut acme = Client::start(&root, "AC");
    let beta = Client::start(&root, "BE");
    let config = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        config.path().join("team.json"),
        config.path().join("parts-files"),
        SignIns::kept_in(Arc::new(MemorySignIns::default())),
        None,
        None,
    );
    teams.sign_in(acme.sign_in()).unwrap();
    teams.sign_in(beta.sign_in()).unwrap();
    wait_until("both teams to be live", || {
        live(&teams, &acme).is_some() && live(&teams, &beta).is_some()
    });
    let cache = tempfile::tempdir().unwrap();
    let browser = CatalogBrowser::open(BrowserConfig {
        catalog_root: sample_library::root(),
        cache_dir: cache.path().join("cache"),
        settings_file: cache.path().join("settings.json"),
        kicad_locations: kicad_integration::SearchLocations::Only(Vec::new()),
        kicad_config_dir: None,
        kicad_documents_dir: None,
    });

    // Both teams' parts are in one list: the same IPN once per team, in
    // team order. The sample library has no CSV catalog, so every row is a
    // team's.
    let rows = list(&browser, &teams, IPN);
    let both: Vec<&str> = rows
        .iter()
        .filter(|row| row.ipn == IPN)
        .map(|row| row.team.as_str())
        .collect();
    assert_eq!(both, [acme.url.as_str(), beta.url.as_str()]);
    let everything = list(&browser, &teams, "");
    assert!(
        everything.iter().all(|row| !row.team.is_empty()),
        "every row is a team's"
    );
    assert_eq!(everything.len(), 2 * 6, "both teams' sample resistors");

    // Editing acme's part saves to acme's server only.
    let acme_team = teams.team(&acme.url).unwrap();
    let saved = browser
        .edit(
            {
                let part = acme_team.part(IPN).unwrap().unwrap();
                let mut request = TeamEditRequest {
                    correlation_id: None,
                    ipn: IPN.into(),
                    expected_details: part.details.number,
                    expected_revision: part.revision.number,
                    reason: "Acme writes 47R".into(),
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
                request
            },
            &acme_team,
        )
        .unwrap();
    assert!(matches!(saved, PublishResult::Saved { .. }), "{saved:?}");
    assert_eq!(acme.value_on_server(), "47R");
    assert_eq!(beta.value_on_server(), "47", "beta's server is untouched");
    wait_until("acme's row to show the edit", || {
        row(&list(&browser, &teams, IPN), &acme).value == "47R"
    });
    assert_eq!(row(&list(&browser, &teams, IPN), &beta).value, "47");

    // A teammate's save on beta changes beta's row only.
    beta.someone_else_edits("47 ohm");
    wait_until("beta's row to show the teammate's save", || {
        row(&list(&browser, &teams, IPN), &beta).value == "47 ohm"
    });
    assert_eq!(row(&list(&browser, &teams, IPN), &acme).value, "47R");

    // Acme's server stops: its rows stay, from the parts last seen, and
    // acme is marked offline. Beta can still be edited.
    acme.server.take().unwrap().stop();
    wait_until("acme to be offline", || {
        matches!(
            team(&teams.status().teams, &acme).session,
            TeamSession::SignedIn {
                live: LiveUpdates::Reconnecting { .. },
                ..
            }
        )
    });
    assert_eq!(row(&list(&browser, &teams, IPN), &acme).value, "47R");
    let beta_team = teams.team(&beta.url).unwrap();
    assert!(matches!(
        value_edit(&beta_team, "47R0"),
        PublishResult::Saved { .. }
    ));
    assert_eq!(beta.value_on_server(), "47R0");
    wait_until("beta's row to show the edit while acme is offline", || {
        row(&list(&browser, &teams, IPN), &beta).value == "47R0"
    });
}

/// The time now, in milliseconds since 1970.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Whether a parts file in `folder` holds `text`.
fn parts_file_holds(folder: &Path, text: &str) -> bool {
    std::fs::read_dir(folder).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            std::fs::read_to_string(entry.path()).is_ok_and(|json| json.contains(text))
        })
    })
}

/// A team whose server is down when the app starts shows the parts the
/// last run kept on disk, marked offline and read-only, with when they
/// last matched the server (kicad-parts-ed46). When the server is back,
/// the catch-up replaces them, and edits work again.
#[test]
fn a_team_down_at_start_shows_its_parts_file_until_its_server_is_back() {
    let root = services::repository_root();
    let mut acme = Client::start(&root, "AC");
    let store = Arc::new(MemorySignIns::default());
    let kept_in = || SignIns::kept_in(store.clone());
    // Never the person's own data folder.
    let scratch = root.join("target").join("test-folders");
    std::fs::create_dir_all(&scratch).unwrap();
    let config = tempfile::tempdir_in(&scratch).unwrap();
    let list_file = config.path().join("team.json");
    // The parts files live in the local data folder, apart from the list.
    let data = tempfile::tempdir_in(&scratch).unwrap();
    let parts_folder = data.path().join("parts-files");
    let cache = tempfile::tempdir_in(&scratch).unwrap();
    let browser = CatalogBrowser::open(BrowserConfig {
        catalog_root: sample_library::root(),
        cache_dir: cache.path().join("cache"),
        settings_file: cache.path().join("settings.json"),
        kicad_locations: kicad_integration::SearchLocations::Only(Vec::new()),
        kicad_config_dir: None,
        kicad_documents_dir: None,
    });

    // The first run catches up, follows a teammate's save, and keeps the
    // parts on disk once they stay the same for a few seconds.
    let first_run_at = now_ms();
    let teams = Teams::open(
        list_file.clone(),
        parts_folder.clone(),
        kept_in(),
        None,
        None,
    );
    teams.sign_in(acme.sign_in()).unwrap();
    wait_until("acme to be live", || live(&teams, &acme).is_some());
    acme.someone_else_edits("47R");
    wait_until("the teammate's save to be kept on disk", || {
        parts_file_holds(&parts_folder, "\"47R\"")
    });
    wait_until("the sign-in to be kept", || {
        store.read(&acme.url).unwrap().is_some()
    });
    drop(teams);
    let closed_at = now_ms();

    // While the app is closed, another save, then the server goes down.
    acme.someone_else_edits("47.0");
    let stopped = acme.server.take().unwrap().stop_for_a_while();

    // The next start shows the parts file, offline, as of the first run.
    let teams = Teams::open(
        list_file.clone(),
        parts_folder.clone(),
        kept_in(),
        None,
        None,
    );
    wait_until("acme to be found unreachable", || {
        matches!(teams.reaching(&acme.url), Some(Some(_)))
    });
    let status = team(&teams.status().teams, &acme).clone();
    assert!(
        matches!(status.session, TeamSession::Reaching { problem: Some(_) }),
        "{status:?}"
    );
    let as_of = status.parts_as_of.expect("says when the parts are from");
    assert!(
        (first_run_at..=closed_at).contains(&as_of),
        "{as_of} is not between {first_run_at} and {closed_at}"
    );
    assert_eq!(row(&list(&browser, &teams, IPN), &acme).value, "47R");
    // Read-only: no session, so nothing can be saved to acme meanwhile.
    assert!(catalog_of(&teams, &acme).part(IPN).is_err());

    // The server is back: the kept sign-in carries on, and the catch-up
    // replaces the parts file with the server's.
    acme.start_again(stopped);
    wait_until("acme to be live again", || live(&teams, &acme).is_some());
    wait_until("the catch-up to replace the parts file", || {
        row(&list(&browser, &teams, IPN), &acme).value == "47.0"
    });
    assert_eq!(team(&teams.status().teams, &acme).parts_as_of, None);
    // Edits work again, and the kept file follows.
    assert!(matches!(
        value_edit(&catalog_of(&teams, &acme), "47 ohm"),
        PublishResult::Saved { .. }
    ));
    assert_eq!(acme.value_on_server(), "47 ohm");
    wait_until("the edit to be kept on disk", || {
        parts_file_holds(&parts_folder, "\"47 ohm\"")
    });

    // Signing out deletes the parts file, and no late write brings it
    // back, even after the writer's quiet time.
    teams.sign_out(&acme.url);
    assert!(!parts_file_holds(&parts_folder, "47"));
    std::thread::sleep(Duration::from_secs(5));
    assert!(!parts_file_holds(&parts_folder, "47"), "it stays gone");

    // Removing the team deletes the file too. Added back at the same
    // address, the team starts afresh, and its new session writes again.
    teams.sign_in(acme.sign_in()).unwrap();
    wait_until("acme to be live after signing in again", || {
        live(&teams, &acme).is_some()
    });
    wait_until("the new session's parts to be written", || {
        parts_file_holds(&parts_folder, "\"47 ohm\"")
    });
    teams.remove(&acme.url);
    std::thread::sleep(Duration::from_secs(5));
    assert!(
        !parts_file_holds(&parts_folder, "47"),
        "removed with the team"
    );
    teams.sign_in(acme.sign_in()).unwrap();
    wait_until("the team added back to write its parts file", || {
        parts_file_holds(&parts_folder, "\"47 ohm\"")
    });
}

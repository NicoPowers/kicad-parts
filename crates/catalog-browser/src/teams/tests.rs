//! The list of teams and their kept sign-ins, against stand-in team
//! servers and a store in memory. No test touches the person's real
//! credential store: that code is in the desktop app, not in this crate.

mod fake_server;
mod parts_files;

use super::*;
use crate::team::SIGN_IN_AGAIN;
use fake_server::{EMAIL, FakeServer, PASSWORD};
use std::time::{Duration, Instant};

/// A store in memory that outlives each `Teams`, as the system's store
/// outlives the app.
fn store() -> Arc<MemorySignIns> {
    Arc::new(MemorySignIns::default())
}

fn kept_in(store: &Arc<MemorySignIns>) -> SignIns {
    SignIns::kept_in(store.clone())
}

fn memory() -> SignIns {
    kept_in(&store())
}

fn request(server: &FakeServer) -> SignInRequest {
    SignInRequest {
        server_url: server.url.clone(),
        email: EMAIL.into(),
        password: PASSWORD.into(),
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn kept_token(store: &MemorySignIns, server: &FakeServer) -> Option<String> {
    store.read(&server.url).unwrap()
}

fn is_signed_in(teams: &Teams, server: &FakeServer) -> bool {
    teams
        .signed_in()
        .iter()
        .any(|(server_url, _)| *server_url == server.url)
}

/// The catalog of the first team in the list, for tests with one team.
fn first(teams: &Teams) -> Arc<TeamCatalog> {
    teams.every()[0].1.clone()
}

/// Each team's colour, in list order.
fn colours(teams: &Teams) -> Vec<String> {
    teams
        .status()
        .teams
        .into_iter()
        .map(|team| team.colour)
        .collect()
}

#[test]
fn a_failed_sign_in_adds_no_team_and_saves_no_password() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    let teams = Teams::open(
        file.clone(),
        dir.path().join("parts-files"),
        memory(),
        None,
        None,
    );
    let problem = teams
        .sign_in(SignInRequest {
            // Port 9 (discard) on loopback is closed on any normal machine.
            server_url: "http://127.0.0.1:9".into(),
            email: "a@example.test".into(),
            password: "secret-password".into(),
        })
        .unwrap_err();
    assert!(!problem.is_empty());
    assert!(!file.exists(), "a failed sign-in saves nothing at all");
    let status = teams.status();
    assert_eq!(status.problem, Some(problem));
    assert!(status.teams.is_empty());
    assert!(teams.signed_in().is_empty());
    assert!(teams.known_parts().is_empty());
}

#[test]
fn an_address_that_may_not_receive_a_password_is_refused_first() {
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        memory(),
        None,
        None,
    );
    let problem = teams
        .sign_in(SignInRequest {
            server_url: "http://team.example.com".into(),
            email: "a@example.test".into(),
            password: "secret-password".into(),
        })
        .unwrap_err();
    assert!(
        problem.starts_with("Addresses starting with http://"),
        "{problem}"
    );
}

#[test]
fn suggested_values_fill_in_the_sign_in_form() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    saved::write(
        &file,
        &SavedList {
            teams: vec![SavedTeam {
                server_url: "https://parts.example.com".into(),
                email: "a@example.test".into(),
                code: "PE".into(),
                colour: String::new(),
                signed_out: false,
            }],
            choices: TeamChoices::default(),
        },
    )
    .unwrap();
    let status = Teams::open(
        file.clone(),
        dir.path().join("parts-files"),
        memory(),
        None,
        None,
    )
    .status();
    assert_eq!(
        (status.form_server.as_str(), status.form_email.as_str()),
        ("https://parts.example.com", "a@example.test")
    );
    // Nothing is kept for it, so it needs signing in; its saved code
    // names it.
    assert_eq!(status.teams[0].label(), "PE");
    assert!(!status.teams[0].session.is_signed_in());
    // A team saved before colours were gets its colour once, and keeps it.
    assert_eq!(status.teams[0].colour, "blue");
    assert_eq!(saved::read(&file).teams[0].colour, "blue");
    let suggested = Some("http://127.0.0.1:1".into());
    let status = Teams::open(
        file,
        dir.path().join("parts-files"),
        memory(),
        suggested,
        None,
    )
    .status();
    assert_eq!(
        (status.form_server.as_str(), status.form_email.as_str()),
        ("http://127.0.0.1:1", "a@example.test")
    );
}

#[test]
fn signing_in_keeps_the_sign_in_in_the_store_never_in_a_file() {
    let server = FakeServer::start();
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    let teams = Teams::open(
        file.clone(),
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    teams.sign_in(request(&server)).unwrap();
    assert!(first(&teams).is_signed_in());
    wait_until("the sign-in to be kept", || {
        kept_token(&store, &server).is_some()
    });
    assert_eq!(kept_token(&store, &server), first(&teams).token());
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains(&server.url) && text.contains(EMAIL), "{text}");
    let token = kept_token(&store, &server).unwrap();
    assert!(
        !text.contains(PASSWORD) && !text.contains(&token) && !text.contains("token"),
        "{text}"
    );
    assert_eq!(teams.kept(), SignInsKept::AfterRestart);
}

#[test]
fn a_restart_carries_on_every_kept_sign_in_without_a_password() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    {
        let teams = Teams::open(
            file.clone(),
            dir.path().join("parts-files"),
            kept_in(&store),
            None,
            None,
        );
        teams.sign_in(request(&acme)).unwrap();
        teams.sign_in(request(&beta)).unwrap();
        // Signing in to beta kept acme signed in. Each team got its own
        // colour.
        assert!(is_signed_in(&teams, &acme) && is_signed_in(&teams, &beta));
        assert_eq!(colours(&teams), ["blue", "green"]);
        wait_until("both sign-ins to be kept", || {
            kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
        });
    }
    let teams = Teams::open(
        file,
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    assert_eq!(teams.reaching(&acme.url), Some(None), "connecting at first");
    wait_until("both teams to be signed in again", || {
        is_signed_in(&teams, &acme) && is_signed_in(&teams, &beta)
    });
    assert_eq!((acme.password_sign_ins(), beta.password_sign_ins()), (1, 1));
    assert!(acme.refreshes() >= 1 && beta.refreshes() >= 1);
    assert_eq!(colours(&teams), ["blue", "green"], "colours stay");
    // The renewed tokens are kept for the next start.
    wait_until("the renewed tokens to be kept", || {
        kept_token(&store, &acme) == teams.signed_in()[0].1.token()
    });
}

#[test]
fn every_request_names_its_team_and_the_list_choices_are_kept() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    let teams = Teams::open(
        file.clone(),
        dir.path().join("parts-files"),
        memory(),
        None,
        None,
    );
    teams.sign_in(request(&acme)).unwrap();
    teams.sign_in(request(&beta)).unwrap();
    // Each team is reached by its address, with no team "shown".
    let signed_in = teams.signed_in();
    assert!(Arc::ptr_eq(
        &teams.team(&acme.url).unwrap(),
        &signed_in[0].1
    ));
    assert!(Arc::ptr_eq(
        &teams.team(&beta.url).unwrap(),
        &signed_in[1].1
    ));
    assert!(teams.team("https://other.example.com").is_err());
    // What the person chose in the part list is kept for the next start.
    let chosen = TeamChoices {
        filter: acme.url.clone(),
        group_by: desktop_contract::GroupBy::Team,
        last_used: [("add_part".to_owned(), beta.url.clone())].into(),
    };
    teams.choose(chosen.clone());
    assert_eq!(teams.status().choices, chosen);
    assert_eq!(saved::read(&file).choices, chosen);
    // A team not in the list is never chosen: the list shows every team.
    teams.choose(TeamChoices {
        filter: "https://other.example.com".into(),
        ..chosen.clone()
    });
    assert_eq!(teams.status().choices.filter, "");
    // Removing a team forgets it wherever it was chosen.
    teams.choose(chosen);
    teams.remove(&acme.url);
    teams.remove(&beta.url);
    let left = teams.status().choices;
    assert_eq!((left.filter.as_str(), left.last_used.len()), ("", 0));
    assert_eq!(left.group_by, desktop_contract::GroupBy::Team);
}

#[test]
fn signing_out_and_removing_touch_one_team_only() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    let teams = Teams::open(
        file.clone(),
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    teams.sign_in(request(&acme)).unwrap();
    teams.sign_in(request(&beta)).unwrap();
    wait_until("both sign-ins to be kept", || {
        kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
    });

    // Signing out of acme forgets its sign-in; it stays in the list, and
    // beta is still signed in.
    teams.sign_out(&acme.url);
    assert_eq!(kept_token(&store, &acme), None);
    assert!(kept_token(&store, &beta).is_some());
    assert!(!is_signed_in(&teams, &acme) && is_signed_in(&teams, &beta));
    assert_eq!(saved::read(&file).teams.len(), 2);
    // Its keeper never brings the forgotten sign-in back.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(kept_token(&store, &acme), None);

    // Removing beta signs out of it and takes it off the list.
    teams.remove(&beta.url);
    assert_eq!(kept_token(&store, &beta), None);
    assert!(teams.signed_in().is_empty());
    let saved = saved::read(&file);
    assert_eq!(saved.teams.len(), 1);
    assert_eq!(saved.teams[0].server_url, acme.url);
}

#[test]
fn a_rejected_sign_in_asks_only_that_team_again() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    {
        let teams = Teams::open(
            file.clone(),
            dir.path().join("parts-files"),
            kept_in(&store),
            None,
            None,
        );
        teams.sign_in(request(&acme)).unwrap();
        teams.sign_in(request(&beta)).unwrap();
        wait_until("both sign-ins to be kept", || {
            kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
        });
    }
    // Acme's password changed while the app was closed.
    acme.reject_all();
    let teams = Teams::open(
        file,
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    wait_until("beta to be signed in", || is_signed_in(&teams, &beta));
    wait_until("acme to give up its kept sign-in", || {
        teams.reaching(&acme.url).is_none() && kept_token(&store, &acme).is_none()
    });
    assert!(!is_signed_in(&teams, &acme));
    // Acme shows no parts until the person signs in again; beta's show.
    let shown: Vec<String> = teams
        .known_parts()
        .iter()
        .map(|parts| parts.team.clone())
        .collect();
    assert_eq!(shown, [beta.url.clone()]);
    let acme_problem = teams
        .lock()
        .find(&acme.url)
        .and_then(|team| team.catalog.problem());
    assert_eq!(acme_problem.as_deref(), Some(SIGN_IN_AGAIN));
    assert_eq!(
        teams.status().teams[0].session,
        TeamSession::SignedOut {
            problem: Some(SIGN_IN_AGAIN.into())
        }
    );
    // Signing in again works as usual.
    teams.sign_in(request(&acme)).unwrap();
    assert!(is_signed_in(&teams, &acme));
}

#[test]
fn a_server_that_cannot_be_reached_is_tried_again_while_others_work() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    {
        let teams = Teams::open(
            file.clone(),
            dir.path().join("parts-files"),
            kept_in(&store),
            None,
            None,
        );
        teams.sign_in(request(&acme)).unwrap();
        teams.sign_in(request(&beta)).unwrap();
        wait_until("both sign-ins to be kept", || {
            kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
        });
    }
    acme.set_down(true);
    let teams = Teams::open(
        file,
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    wait_until("beta to be signed in", || is_signed_in(&teams, &beta));
    wait_until("acme to be offline", || {
        matches!(teams.reaching(&acme.url), Some(Some(_)))
    });
    let acme_status = teams.status().teams[0].clone();
    assert!(
        matches!(
            acme_status.session,
            TeamSession::Reaching { problem: Some(_) }
        ),
        "{acme_status:?}"
    );
    // The kept sign-in stays, for when the server is back.
    assert!(kept_token(&store, &acme).is_some());
    acme.set_down(false);
    wait_until("acme to be signed in again", || is_signed_in(&teams, &acme));
    assert_eq!(teams.reaching(&acme.url), None);
    assert_eq!(acme.password_sign_ins(), 1);
}

#[test]
fn a_sign_in_is_renewed_before_it_ends() {
    let server = FakeServer::start();
    server.set_token_lifetime(4);
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    teams.sign_in(request(&server)).unwrap();
    wait_until("the sign-in to be kept", || {
        kept_token(&store, &server).is_some()
    });
    let first_token = kept_token(&store, &server).unwrap();
    // Half its life is 2 seconds.
    wait_until("a renewed token to be kept", || {
        kept_token(&store, &server).is_some_and(|token| token != first_token)
    });
    assert!(server.refreshes() >= 1);
    assert_eq!(kept_token(&store, &server), first(&teams).token());
    assert_eq!(server.password_sign_ins(), 1);
}

/// A store that refuses to keep anything, like a locked keychain.
struct LockedStore;

impl SignInStore for LockedStore {
    fn read(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn keep(&self, _: &str, _: &str) -> Result<(), String> {
        Err("the keychain is locked".into())
    }
    fn forget(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn a_store_that_fails_says_sign_ins_last_until_the_app_closes() {
    let server = FakeServer::start();
    let dir = tempfile::tempdir().unwrap();
    let sign_ins = SignIns::kept_in(Arc::new(LockedStore));
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        sign_ins,
        None,
        None,
    );
    teams.sign_in(request(&server)).unwrap();
    wait_until("the failure to be said", || {
        teams.kept() != SignInsKept::AfterRestart
    });
    let SignInsKept::UntilClosed { reason } = teams.kept() else {
        unreachable!()
    };
    assert!(reason.contains("the keychain is locked"), "{reason}");
    // The session itself carries on.
    assert!(first(&teams).is_signed_in());
    let until_closed = SignIns::until_closed("this computer has no password store");
    let teams = Teams::open(
        dir.path().join("other.json"),
        dir.path().join("parts-files"),
        until_closed,
        None,
        None,
    );
    assert_eq!(
        teams.kept(),
        SignInsKept::UntilClosed {
            reason: "this computer has no password store".into()
        }
    );
}

/// Signs in to `server`, whose first token lasts 2 seconds, and waits
/// until the keeper's renewal of it is held by the server. Returns the
/// token being renewed.
fn renewal_in_flight(teams: &Teams, server: &FakeServer, store: &MemorySignIns) -> String {
    server.set_token_lifetime(2);
    server.hold_refreshes();
    teams.sign_in(request(server)).unwrap();
    wait_until("the sign-in to be kept", || {
        kept_token(store, server).is_some()
    });
    let renewing = kept_token(store, server).unwrap();
    wait_until("a renewal to be in flight", || server.held() == 1);
    // Later tokens last long, so they are not renewed during the test.
    server.set_token_lifetime(7 * 24 * 3600);
    renewing
}

/// Lets the held renewal answer, and gives the keeper time to act on it.
fn answer_the_renewal(server: &FakeServer) {
    server.release_refreshes();
    wait_until("the renewal to be answered", || server.held() == 0);
    std::thread::sleep(Duration::from_millis(800));
}

#[test]
fn a_renewal_answered_after_a_new_sign_in_leaves_the_new_sign_in_alone() {
    let server = FakeServer::start();
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    let old = renewal_in_flight(&teams, &server, &store);
    // The person signs in again meanwhile, and the old sign-in ends.
    teams.sign_in(request(&server)).unwrap();
    let new = first(&teams).token().unwrap();
    assert_ne!(new, old);
    server.reject(&old);
    answer_the_renewal(&server);
    // The rejection was about the old sign-in: the new one stays, kept.
    assert!(is_signed_in(&teams, &server));
    assert_eq!(first(&teams).token(), Some(new.clone()));
    assert_eq!(kept_token(&store, &server), Some(new));
    assert!(teams.status().teams[0].session.is_signed_in());
}

#[test]
fn a_renewal_answered_after_signing_out_says_nothing_and_keeps_nothing() {
    let server = FakeServer::start();
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    let old = renewal_in_flight(&teams, &server, &store);
    teams.sign_out(&server.url);
    server.reject(&old);
    answer_the_renewal(&server);
    // Signed out, as the person chose: not "sign in again".
    assert_eq!(
        teams.status().teams[0].session,
        TeamSession::SignedOut { problem: None }
    );
    assert_eq!(kept_token(&store, &server), None);
}

#[test]
fn a_renewal_answered_after_removing_and_adding_the_team_keeps_the_new_sign_in() {
    let server = FakeServer::start();
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    let old = renewal_in_flight(&teams, &server, &store);
    teams.remove(&server.url);
    teams.sign_in(request(&server)).unwrap();
    wait_until("the new sign-in to be kept", || {
        kept_token(&store, &server).is_some_and(|token| token != old)
    });
    let new = kept_token(&store, &server).unwrap();
    server.reject(&old);
    answer_the_renewal(&server);
    // The removed team's keeper is stopped and writes nothing.
    assert_eq!(kept_token(&store, &server), Some(new));
    assert!(is_signed_in(&teams, &server));
}

#[test]
fn a_wrong_password_leaves_a_kept_sign_in_being_carried_on() {
    let server = FakeServer::start();
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    {
        let teams = Teams::open(
            file.clone(),
            dir.path().join("parts-files"),
            kept_in(&store),
            None,
            None,
        );
        teams.sign_in(request(&server)).unwrap();
        wait_until("the sign-in to be kept", || {
            kept_token(&store, &server).is_some()
        });
    }
    // At the next start, carrying on the kept sign-in is in flight when
    // someone types a wrong password.
    server.hold_refreshes();
    let teams = Teams::open(
        file,
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    wait_until("the kept sign-in to be in flight", || server.held() == 1);
    let wrong = SignInRequest {
        password: "wrong".into(),
        ..request(&server)
    };
    assert!(teams.sign_in(wrong).is_err());
    server.release_refreshes();
    wait_until("the kept sign-in to be carried on", || {
        is_signed_in(&teams, &server)
    });
    assert!(kept_token(&store, &server).is_some());
}

/// A store that cannot forget, like a locked keyring whose prompt was
/// dismissed.
#[derive(Default)]
struct UnforgettingStore(MemorySignIns);

impl SignInStore for UnforgettingStore {
    fn read(&self, server_url: &str) -> Result<Option<String>, String> {
        self.0.read(server_url)
    }
    fn keep(&self, server_url: &str, token: &str) -> Result<(), String> {
        self.0.keep(server_url, token)
    }
    fn forget(&self, _: &str) -> Result<(), String> {
        Err("the keyring is locked".into())
    }
}

#[test]
fn a_sign_out_the_store_cannot_forget_is_still_never_carried_on() {
    let server = FakeServer::start();
    let store = Arc::new(UnforgettingStore::default());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    {
        let teams = Teams::open(
            file.clone(),
            dir.path().join("parts-files"),
            SignIns::kept_in(store.clone()),
            None,
            None,
        );
        teams.sign_in(request(&server)).unwrap();
        wait_until("the sign-in to be kept", || {
            store.read(&server.url).unwrap().is_some()
        });
        teams.sign_out(&server.url);
        let note = teams.status().teams[0].note.clone().unwrap_or_default();
        assert!(
            note.starts_with("Your sign-in for this team could not be removed")
                && note.contains("the keyring is locked"),
            "{note}"
        );
    }
    assert!(store.read(&server.url).unwrap().is_some(), "it lingers");
    let teams = Teams::open(
        file.clone(),
        dir.path().join("parts-files"),
        SignIns::kept_in(store.clone()),
        None,
        None,
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(!is_signed_in(&teams, &server));
    assert_eq!(teams.reaching(&server.url), None);
    assert_eq!(server.refreshes(), 0, "the lingering sign-in was not used");
    // Signing in again clears the mark.
    teams.sign_in(request(&server)).unwrap();
    assert!(!saved::read(&file).teams[0].signed_out);
}

#[test]
fn a_wrong_password_leaves_a_signed_in_team_signed_in() {
    let server = FakeServer::start();
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        kept_in(&store),
        None,
        None,
    );
    teams.sign_in(request(&server)).unwrap();
    wait_until("the sign-in to be kept", || {
        kept_token(&store, &server).is_some()
    });
    let catalog = first(&teams);
    let (session, token) = catalog.session_token().unwrap();
    // "Add a team" with the same address and a mistyped password.
    let wrong = SignInRequest {
        password: "wrong".into(),
        ..request(&server)
    };
    assert_eq!(
        teams.sign_in(wrong).unwrap_err(),
        "The email or password is not right."
    );
    assert_eq!(catalog.session_token(), Some((session, token.clone())));
    assert!(is_signed_in(&teams, &server));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(kept_token(&store, &server), Some(token));
}

#[test]
fn sessions_made_in_the_same_second_are_told_apart() {
    let server = FakeServer::start();
    let catalog = TeamCatalog::new();
    let sign_in = || {
        catalog.try_sign_in(request(&server)).unwrap();
        catalog.session_token().unwrap()
    };
    // PocketBase tokens made in the same second are the same text; try
    // until two are, in case a second ends between them.
    let (first, second) = (0..5)
        .map(|_| (sign_in(), sign_in()))
        .find(|((_, a), (_, b))| a == b)
        .expect("two sign-ins in the same second");
    assert_ne!(first.0, second.0, "each session has its own number");
    // A judgement about the first session does nothing to the second.
    assert!(!catalog.sign_in_again(Some(first.0)));
    assert_eq!(catalog.session_number(), Some(second.0));
    assert!(catalog.sign_in_again(Some(second.0)));
    assert_eq!(catalog.session_number(), None);
}

#[test]
fn a_removed_team_whose_sign_in_lingers_is_said_once() {
    let server = FakeServer::start();
    let store = Arc::new(UnforgettingStore::default());
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        SignIns::kept_in(store.clone()),
        None,
        None,
    );
    teams.sign_in(request(&server)).unwrap();
    teams.remove(&server.url);
    let note = teams.status().note.unwrap_or_default();
    assert!(
        note.starts_with(&format!(
            "Your sign-in for {} could not be removed",
            server.url
        )) && note.contains("the keyring is locked"),
        "{note}"
    );
    assert!(teams.status().teams.is_empty());
    // The next action clears it.
    teams.sign_in(request(&server)).unwrap();
    assert_eq!(teams.status().note, None);
}

/// A store whose forgetting waits, like a keyring asking to be unlocked.
#[derive(Default)]
struct SlowStore {
    tokens: MemorySignIns,
    hold: Mutex<bool>,
    released: std::sync::Condvar,
    waiting: std::sync::atomic::AtomicU64,
}

impl SlowStore {
    fn hold(&self) {
        *self.hold.lock().unwrap() = true;
    }

    fn release(&self) {
        *self.hold.lock().unwrap() = false;
        self.released.notify_all();
    }

    fn waiting(&self) -> u64 {
        self.waiting.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl SignInStore for SlowStore {
    fn read(&self, server_url: &str) -> Result<Option<String>, String> {
        self.tokens.read(server_url)
    }
    fn keep(&self, server_url: &str, token: &str) -> Result<(), String> {
        self.tokens.keep(server_url, token)
    }
    fn forget(&self, server_url: &str) -> Result<(), String> {
        let mut hold = self.hold.lock().unwrap();
        self.waiting
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        while *hold {
            hold = self.released.wait(hold).unwrap();
        }
        self.waiting
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        drop(hold);
        self.tokens.forget(server_url)
    }
}

#[test]
fn adding_a_team_back_waits_until_its_removal_forgot_the_old_sign_in() {
    let server = FakeServer::start();
    let store = Arc::new(SlowStore::default());
    let dir = tempfile::tempdir().unwrap();
    let teams = Arc::new(Teams::open(
        dir.path().join("team.json"),
        dir.path().join("parts-files"),
        SignIns::kept_in(store.clone()),
        None,
        None,
    ));
    teams.sign_in(request(&server)).unwrap();
    wait_until("the sign-in to be kept", || {
        store.read(&server.url).unwrap().is_some()
    });
    store.hold();
    let remover = {
        let (teams, url) = (teams.clone(), server.url.clone());
        std::thread::spawn(move || teams.remove(&url))
    };
    wait_until("the removal to wait on the store", || store.waiting() == 1);
    let adder = {
        let (teams, request) = (teams.clone(), request(&server));
        std::thread::spawn(move || teams.sign_in(request))
    };
    std::thread::sleep(Duration::from_millis(300));
    assert!(!adder.is_finished(), "adding it back waits for the removal");
    store.release();
    remover.join().unwrap();
    adder.join().unwrap().unwrap();
    wait_until("the new sign-in to be kept", || {
        store.read(&server.url).unwrap().is_some()
    });
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        store.read(&server.url).unwrap().is_some(),
        "never forgotten"
    );
    assert!(is_signed_in(&teams, &server));
}

#[test]
fn signing_in_again_keeps_the_teams_code_until_its_settings_are_read() {
    // The stand-in server never says its code, as a real one does only
    // after signing in.
    let server = FakeServer::start();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    saved::write(
        &file,
        &SavedList {
            teams: vec![SavedTeam {
                server_url: server.url.clone(),
                email: EMAIL.into(),
                code: "AC".into(),
                colour: "blue".into(),
                signed_out: true,
            }],
            choices: TeamChoices::default(),
        },
    )
    .unwrap();
    let teams = Teams::open(
        file.clone(),
        dir.path().join("parts-files"),
        memory(),
        None,
        None,
    );
    teams.sign_in(request(&server)).unwrap();
    assert_eq!(teams.status().teams[0].label(), "AC");
    assert_eq!(saved::read(&file).teams[0].code, "AC");
    assert_eq!(teams.known_parts()[0].label, "AC");
}

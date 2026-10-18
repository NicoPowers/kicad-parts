//! The list of teams and their kept sign-ins, against stand-in team
//! servers and a store in memory. No test touches the person's real
//! credential store: that code is in the desktop app, not in this crate.

mod fake_server;

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

fn same(a: &Arc<TeamCatalog>, b: &Arc<TeamCatalog>) -> bool {
    Arc::ptr_eq(a, b)
}

#[test]
fn a_failed_sign_in_adds_no_team_and_saves_no_password() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    let teams = Teams::open(file.clone(), memory(), None, None);
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
    assert!(status.teams.is_empty() && status.shown.is_empty());
    assert!(teams.signed_in().is_empty());
    assert!(!teams.shown().is_signed_in());
}

#[test]
fn an_address_that_may_not_receive_a_password_is_refused_first() {
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(dir.path().join("team.json"), memory(), None, None);
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
                signed_out: false,
            }],
            shown: "https://parts.example.com".into(),
        },
    )
    .unwrap();
    let status = Teams::open(file.clone(), memory(), None, None).status();
    assert_eq!(
        (status.form_server.as_str(), status.form_email.as_str()),
        ("https://parts.example.com", "a@example.test")
    );
    // Nothing is kept for it, so it needs signing in; its saved code
    // names it.
    assert_eq!(status.teams[0].label(), "PE");
    assert!(status.shown.is_empty(), "no team is signed in");
    let suggested = Some("http://127.0.0.1:1".into());
    let status = Teams::open(file, memory(), suggested, None).status();
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
    let teams = Teams::open(file.clone(), kept_in(&store), None, None);
    teams.sign_in(request(&server)).unwrap();
    assert!(teams.shown().is_signed_in());
    wait_until("the sign-in to be kept", || {
        kept_token(&store, &server).is_some()
    });
    assert_eq!(kept_token(&store, &server), teams.shown().token());
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
        let teams = Teams::open(file.clone(), kept_in(&store), None, None);
        teams.sign_in(request(&acme)).unwrap();
        teams.sign_in(request(&beta)).unwrap();
        // Signing in to beta kept acme signed in, and shows beta.
        assert!(is_signed_in(&teams, &acme) && is_signed_in(&teams, &beta));
        assert_eq!(teams.status().shown, beta.url);
        wait_until("both sign-ins to be kept", || {
            kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
        });
    }
    let teams = Teams::open(file, kept_in(&store), None, None);
    assert_eq!(teams.reaching(&acme.url), Some(None), "connecting at first");
    wait_until("both teams to be signed in again", || {
        is_signed_in(&teams, &acme) && is_signed_in(&teams, &beta)
    });
    assert_eq!((acme.password_sign_ins(), beta.password_sign_ins()), (1, 1));
    assert!(acme.refreshes() >= 1 && beta.refreshes() >= 1);
    assert_eq!(teams.status().shown, beta.url, "beta is still shown");
    // The renewed tokens are kept for the next start.
    wait_until("the renewed tokens to be kept", || {
        kept_token(&store, &acme) == teams.signed_in()[0].1.token()
    });
}

#[test]
fn switching_teams_needs_no_sign_in() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(dir.path().join("team.json"), memory(), None, None);
    teams.sign_in(request(&acme)).unwrap();
    teams.sign_in(request(&beta)).unwrap();
    let signed_in = teams.signed_in();
    let (acme_catalog, beta_catalog) = (&signed_in[0].1, &signed_in[1].1);
    assert!(same(&teams.shown(), beta_catalog));
    teams.show(&acme.url).unwrap();
    assert!(same(&teams.shown(), acme_catalog));
    teams.show(&beta.url).unwrap();
    assert!(same(&teams.shown(), beta_catalog));
    assert_eq!((acme.password_sign_ins(), beta.password_sign_ins()), (1, 1));
    assert!(teams.show("https://other.example.com").is_err());
}

#[test]
fn signing_out_and_removing_touch_one_team_only() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("team.json");
    let teams = Teams::open(file.clone(), kept_in(&store), None, None);
    teams.sign_in(request(&acme)).unwrap();
    teams.sign_in(request(&beta)).unwrap();
    teams.show(&acme.url).unwrap();
    wait_until("both sign-ins to be kept", || {
        kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
    });

    // Signing out of acme forgets its sign-in; it stays in the list, and
    // the catalog view shows beta, which is still signed in.
    teams.sign_out(&acme.url);
    assert_eq!(kept_token(&store, &acme), None);
    assert!(kept_token(&store, &beta).is_some());
    assert!(!is_signed_in(&teams, &acme) && is_signed_in(&teams, &beta));
    assert_eq!(saved::read(&file).teams.len(), 2);
    assert_eq!(teams.status().shown, beta.url);
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
        let teams = Teams::open(file.clone(), kept_in(&store), None, None);
        teams.sign_in(request(&acme)).unwrap();
        teams.sign_in(request(&beta)).unwrap();
        wait_until("both sign-ins to be kept", || {
            kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
        });
    }
    // Acme's password changed while the app was closed.
    acme.reject_all();
    let teams = Teams::open(file, kept_in(&store), None, None);
    wait_until("beta to be signed in", || is_signed_in(&teams, &beta));
    wait_until("acme to give up its kept sign-in", || {
        teams.reaching(&acme.url).is_none() && kept_token(&store, &acme).is_none()
    });
    assert!(!is_signed_in(&teams, &acme));
    teams.show(&acme.url).unwrap();
    // Acme is not signed in, so the catalog view shows beta; the sign-in
    // form asks for acme's password.
    assert_eq!(teams.status().shown, beta.url);
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
        let teams = Teams::open(file.clone(), kept_in(&store), None, None);
        teams.sign_in(request(&acme)).unwrap();
        teams.sign_in(request(&beta)).unwrap();
        wait_until("both sign-ins to be kept", || {
            kept_token(&store, &acme).is_some() && kept_token(&store, &beta).is_some()
        });
    }
    acme.set_down(true);
    let teams = Teams::open(file, kept_in(&store), None, None);
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
    let teams = Teams::open(dir.path().join("team.json"), kept_in(&store), None, None);
    teams.sign_in(request(&server)).unwrap();
    wait_until("the sign-in to be kept", || {
        kept_token(&store, &server).is_some()
    });
    let first = kept_token(&store, &server).unwrap();
    // Half its life is 2 seconds.
    wait_until("a renewed token to be kept", || {
        kept_token(&store, &server).is_some_and(|token| token != first)
    });
    assert!(server.refreshes() >= 1);
    assert_eq!(kept_token(&store, &server), teams.shown().token());
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
    let teams = Teams::open(dir.path().join("team.json"), sign_ins, None, None);
    teams.sign_in(request(&server)).unwrap();
    wait_until("the failure to be said", || {
        teams.kept() != SignInsKept::AfterRestart
    });
    let SignInsKept::UntilClosed { reason } = teams.kept() else {
        unreachable!()
    };
    assert!(reason.contains("the keychain is locked"), "{reason}");
    // The session itself carries on.
    assert!(teams.shown().is_signed_in());
    let until_closed = SignIns::until_closed("this computer has no password store");
    let teams = Teams::open(dir.path().join("other.json"), until_closed, None, None);
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
    let teams = Teams::open(dir.path().join("team.json"), kept_in(&store), None, None);
    let old = renewal_in_flight(&teams, &server, &store);
    // The person signs in again meanwhile, and the old sign-in ends.
    teams.sign_in(request(&server)).unwrap();
    let new = teams.shown().token().unwrap();
    assert_ne!(new, old);
    server.reject(&old);
    answer_the_renewal(&server);
    // The rejection was about the old sign-in: the new one stays, kept.
    assert!(is_signed_in(&teams, &server));
    assert_eq!(teams.shown().token(), Some(new.clone()));
    assert_eq!(kept_token(&store, &server), Some(new));
    assert!(teams.status().teams[0].session.is_signed_in());
}

#[test]
fn a_renewal_answered_after_signing_out_says_nothing_and_keeps_nothing() {
    let server = FakeServer::start();
    let store = store();
    let dir = tempfile::tempdir().unwrap();
    let teams = Teams::open(dir.path().join("team.json"), kept_in(&store), None, None);
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
    let teams = Teams::open(dir.path().join("team.json"), kept_in(&store), None, None);
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
        let teams = Teams::open(file.clone(), kept_in(&store), None, None);
        teams.sign_in(request(&server)).unwrap();
        wait_until("the sign-in to be kept", || {
            kept_token(&store, &server).is_some()
        });
    }
    // At the next start, carrying on the kept sign-in is in flight when
    // someone types a wrong password.
    server.hold_refreshes();
    let teams = Teams::open(file, kept_in(&store), None, None);
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
        let teams = Teams::open(file.clone(), SignIns::kept_in(store.clone()), None, None);
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
    let teams = Teams::open(file.clone(), SignIns::kept_in(store.clone()), None, None);
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
    let teams = Teams::open(dir.path().join("team.json"), kept_in(&store), None, None);
    teams.sign_in(request(&server)).unwrap();
    wait_until("the sign-in to be kept", || {
        kept_token(&store, &server).is_some()
    });
    let catalog = teams.shown();
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

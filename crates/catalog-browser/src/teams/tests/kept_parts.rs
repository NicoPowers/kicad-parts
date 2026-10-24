//! Each team's last-known parts, kept on disk (kicad-parts-ed46): when the
//! list of teams shows them, and when it deletes them. The stand-in server
//! never catches up, so these tests write the file themselves; the
//! desktop_teams test in tooling/catalog-import has the app write it,
//! against real servers.

use super::*;
use crate::PartValues;
use crate::team::KeptParts;
use crate::teams::kept_parts as disk;
use std::collections::BTreeMap;

const AS_OF_MS: u64 = 1_790_000_000_000;

/// A folder for the list of teams under `target/`, never the person's own
/// data folder.
fn data_folder() -> tempfile::TempDir {
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-folders");
    std::fs::create_dir_all(&target).unwrap();
    tempfile::tempdir_in(target).unwrap()
}

/// One part with `value`, as kept for `server`.
fn kept(server: &FakeServer, value: &str) -> KeptParts {
    let values = PartValues {
        fields: Some(vec![("Value".into(), value.into())]),
        ..PartValues::default()
    };
    KeptParts {
        server_url: server.url.clone(),
        code: "AC".into(),
        as_of_ms: AS_OF_MS,
        values: [("RES-0000-0470".to_owned(), values)].into(),
        category_names: BTreeMap::new(),
    }
}

fn keep(file: &Path, parts: &KeptParts) {
    disk::write(&parts_folder_of(file), parts).unwrap();
}

fn is_kept(file: &Path, server: &FakeServer) -> bool {
    disk::file_for(&parts_folder_of(file), &server.url).is_file()
}

/// The Value of each part the list shows for `server`.
fn shown_values(teams: &Teams, server: &FakeServer) -> Vec<String> {
    teams
        .known_parts()
        .iter()
        .filter(|parts| parts.team == server.url)
        .flat_map(|parts| parts.values.values().cloned().collect::<Vec<_>>())
        .filter_map(|values| values.fields?.into_iter().next().map(|(_, value)| value))
        .collect()
}

fn parts_as_of(teams: &Teams, server: &FakeServer) -> Option<u64> {
    teams
        .status()
        .teams
        .into_iter()
        .find(|team| team.server_url == server.url)?
        .parts_as_of
}

/// Signs in to `server` in a first run, so a later run has its sign-in.
fn signed_in_before(file: &Path, store: &Arc<MemorySignIns>, server: &FakeServer) {
    let teams = Teams::open(file.to_owned(), kept_in(store), None, None);
    teams.sign_in(request(server)).unwrap();
    wait_until("the sign-in to be kept", || {
        kept_token(store, server).is_some()
    });
}

#[test]
fn a_team_whose_server_is_down_at_start_shows_its_kept_parts_offline() {
    let server = FakeServer::start();
    let store = store();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store, &server);
    keep(&file, &kept(&server, "47"));
    server.set_down(true);
    let teams = Teams::open(file, kept_in(&store), None, None);
    wait_until("the server to be found unreachable", || {
        matches!(teams.reaching(&server.url), Some(Some(_)))
    });
    assert_eq!(shown_values(&teams, &server), ["47"]);
    assert_eq!(parts_as_of(&teams, &server), Some(AS_OF_MS));
    assert!(!first(&teams).is_signed_in(), "read-only: no session");
}

#[test]
fn a_team_with_no_kept_sign_in_shows_no_kept_parts() {
    let server = FakeServer::start();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store(), &server);
    keep(&file, &kept(&server, "47"));
    // A new store: as on a computer whose password store lost the sign-in.
    let teams = Teams::open(file.clone(), memory(), None, None);
    wait_until("the keeper to find no sign-in", || {
        teams.reaching(&server.url).is_none()
    });
    assert!(teams.known_parts().is_empty());
    assert_eq!(parts_as_of(&teams, &server), None);
    // The file stays: it is deleted only by signing out or removing.
    assert!(is_kept(&file, &server));
}

#[test]
fn parts_kept_from_another_server_are_never_shown() {
    let (server, other) = (FakeServer::start(), FakeServer::start());
    let store = store();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store, &server);
    // A file in the team's place, holding another server's parts.
    keep(&file, &kept(&other, "47"));
    let folder = parts_folder_of(&file);
    std::fs::rename(
        disk::file_for(&folder, &other.url),
        disk::file_for(&folder, &server.url),
    )
    .unwrap();
    server.set_down(true);
    let teams = Teams::open(file, kept_in(&store), None, None);
    wait_until("the server to be found unreachable", || {
        matches!(teams.reaching(&server.url), Some(Some(_)))
    });
    assert!(teams.known_parts().is_empty());
}

#[test]
fn signing_out_and_removing_delete_the_kept_parts() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let dir = data_folder();
    let file = dir.path().join("team.json");
    let teams = Teams::open(file.clone(), memory(), None, None);
    teams.sign_in(request(&acme)).unwrap();
    teams.sign_in(request(&beta)).unwrap();
    keep(&file, &kept(&acme, "47"));
    keep(&file, &kept(&beta, "47"));
    teams.sign_out(&acme.url);
    assert!(!is_kept(&file, &acme));
    assert!(is_kept(&file, &beta), "only that team's");
    teams.remove(&beta.url);
    assert!(!is_kept(&file, &beta));
}

#[test]
fn a_team_asked_to_sign_in_again_shows_none_but_keeps_them_for_the_sign_in() {
    let server = FakeServer::start();
    let store = store();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store, &server);
    keep(&file, &kept(&server, "47"));
    // The password changed while the app was closed.
    server.reject_all();
    let teams = Teams::open(file.clone(), kept_in(&store), None, None);
    wait_until("the team to ask for its password", || {
        first(&teams).problem().as_deref() == Some(SIGN_IN_AGAIN)
    });
    assert!(teams.known_parts().is_empty());
    assert_eq!(parts_as_of(&teams, &server), None);
    assert!(is_kept(&file, &server));
    // Signing in again shows them at once, before the first catch-up.
    teams.sign_in(request(&server)).unwrap();
    assert_eq!(shown_values(&teams, &server), ["47"]);
}

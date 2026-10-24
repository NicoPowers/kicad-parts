//! Each team's parts file (kicad-parts-ed46): when the
//! list of teams shows them, and when it deletes them. The stand-in server
//! never catches up, so these tests write the file themselves; the
//! desktop_teams test in tooling/catalog-import has the app write it,
//! against real servers.

use super::*;
use crate::PartValues;
use crate::team::PartsFile;
use crate::teams::parts_file as disk;
use std::collections::BTreeMap;

const AS_OF_MS: u64 = 1_790_000_000_000;

/// A folder for the list of teams under `target/`, never the person's own
/// data folder.
fn data_folder() -> tempfile::TempDir {
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-folders");
    std::fs::create_dir_all(&target).unwrap();
    tempfile::tempdir_in(target).unwrap()
}

/// A parts file with one part with `value`, from `server`.
fn parts_file(server: &FakeServer, value: &str) -> PartsFile {
    let values = PartValues {
        fields: Some(vec![("Value".into(), value.into())]),
        ..PartValues::default()
    };
    PartsFile {
        server_url: server.url.clone(),
        as_of_ms: AS_OF_MS,
        values: [("RES-0000-0470".to_owned(), values)].into(),
        category_names: BTreeMap::new(),
    }
}

/// The parts folder these tests pass beside the list `file`.
fn parts_folder(file: &Path) -> PathBuf {
    file.with_file_name("parts-files")
}

fn write_file(file: &Path, parts: &PartsFile) {
    disk::write(&parts_folder(file), parts).unwrap();
}

fn has_file(file: &Path, server: &FakeServer) -> bool {
    disk::file_for(&parts_folder(file), &server.url).is_file()
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
    let teams = Teams::open(
        file.to_owned(),
        parts_folder(file),
        kept_in(store),
        None,
        None,
    );
    teams.sign_in(request(server)).unwrap();
    wait_until("the sign-in to be kept", || {
        kept_token(store, server).is_some()
    });
}

#[test]
fn a_team_whose_server_is_down_at_start_shows_its_parts_file_offline() {
    let server = FakeServer::start();
    let store = store();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store, &server);
    write_file(&file, &parts_file(&server, "47"));
    server.set_down(true);
    let teams = Teams::open(
        file.clone(),
        parts_folder(&file),
        kept_in(&store),
        None,
        None,
    );
    wait_until("the server to be found unreachable", || {
        matches!(teams.reaching(&server.url), Some(Some(_)))
    });
    assert_eq!(shown_values(&teams, &server), ["47"]);
    assert_eq!(parts_as_of(&teams, &server), Some(AS_OF_MS));
    assert!(!first(&teams).is_signed_in(), "read-only: no session");
}

#[test]
fn a_team_with_no_kept_sign_in_shows_no_parts_file() {
    let server = FakeServer::start();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store(), &server);
    write_file(&file, &parts_file(&server, "47"));
    // A new store: as on a computer whose password store lost the sign-in.
    let teams = Teams::open(file.clone(), parts_folder(&file), memory(), None, None);
    wait_until("the keeper to find no sign-in", || {
        teams.reaching(&server.url).is_none()
    });
    assert!(teams.known_parts().is_empty());
    assert_eq!(parts_as_of(&teams, &server), None);
    // The file stays: it is deleted only by signing out or removing.
    assert!(has_file(&file, &server));
}

#[test]
fn a_parts_file_from_another_server_is_never_shown() {
    let (server, other) = (FakeServer::start(), FakeServer::start());
    let store = store();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store, &server);
    // A file in the team's place, holding another server's parts.
    write_file(&file, &parts_file(&other, "47"));
    let folder = parts_folder(&file);
    std::fs::rename(
        disk::file_for(&folder, &other.url),
        disk::file_for(&folder, &server.url),
    )
    .unwrap();
    server.set_down(true);
    let teams = Teams::open(
        file.clone(),
        parts_folder(&file),
        kept_in(&store),
        None,
        None,
    );
    wait_until("the server to be found unreachable", || {
        matches!(teams.reaching(&server.url), Some(Some(_)))
    });
    assert!(teams.known_parts().is_empty());
}

#[test]
fn signing_out_and_removing_delete_the_parts_file() {
    let (acme, beta) = (FakeServer::start(), FakeServer::start());
    let dir = data_folder();
    let file = dir.path().join("team.json");
    let teams = Teams::open(file.clone(), parts_folder(&file), memory(), None, None);
    teams.sign_in(request(&acme)).unwrap();
    teams.sign_in(request(&beta)).unwrap();
    write_file(&file, &parts_file(&acme, "47"));
    write_file(&file, &parts_file(&beta, "47"));
    teams.sign_out(&acme.url);
    assert!(!has_file(&file, &acme));
    assert!(has_file(&file, &beta), "only that team's");
    teams.remove(&beta.url);
    assert!(!has_file(&file, &beta));
}

#[test]
fn a_team_asked_to_sign_in_again_shows_none_but_keeps_them_for_the_sign_in() {
    let server = FakeServer::start();
    let store = store();
    let dir = data_folder();
    let file = dir.path().join("team.json");
    signed_in_before(&file, &store, &server);
    write_file(&file, &parts_file(&server, "47"));
    // The password changed while the app was closed.
    server.reject_all();
    let teams = Teams::open(
        file.clone(),
        parts_folder(&file),
        kept_in(&store),
        None,
        None,
    );
    wait_until("the team to ask for its password", || {
        first(&teams).problem().as_deref() == Some(SIGN_IN_AGAIN)
    });
    assert!(teams.known_parts().is_empty());
    assert_eq!(parts_as_of(&teams, &server), None);
    assert!(has_file(&file, &server));
    // Signing in again shows them at once, before the first catch-up.
    teams.sign_in(request(&server)).unwrap();
    assert_eq!(shown_values(&teams, &server), ["47"]);
}

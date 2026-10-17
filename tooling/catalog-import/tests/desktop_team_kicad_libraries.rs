//! The desktop keeps the KiCad libraries up to date while signed in
//! (kicad-parts-f397): after the sign-in catches up, after a Change by
//! someone else, and on Update now. A disposable PocketBase on 127.0.0.1
//! has the resistors imported; the libraries are written under target/.

use catalog::PartRevision;
use catalog::pocketbase::{EditCommand, ImportCommand, read_back, submit_edit, submit_import};
use catalog_browser::kicad_libraries::KicadLibraries;
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use desktop_contract::{KicadLibrariesState, SignInRequest};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wait_for(
    libraries: &KicadLibraries,
    what: &str,
    mut done: impl FnMut(&KicadLibrariesState) -> bool,
) -> KicadLibrariesState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = libraries.status().state;
        if done(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn updated_at(state: &KicadLibrariesState) -> Option<u64> {
    match state {
        KicadLibrariesState::UpToDate { at_ms, .. } => Some(*at_ms),
        _ => None,
    }
}

#[test]
fn the_libraries_follow_the_team_catalog() {
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
    let member = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let folder = root
        .join("target")
        .join("kicad-libraries-tests")
        .join("desktop");
    let _ = std::fs::remove_dir_all(&folder);

    let team = Arc::new(TeamCatalog::open(
        state.path().join("team.json"),
        Some(server.url().to_owned()),
        None,
    ));
    let libraries = KicadLibraries::start(folder.clone(), team.clone());
    assert_eq!(libraries.status().state, KicadLibrariesState::SignedOut);
    assert!(libraries.status().symbol_table.is_empty());

    team.sign_in(SignInRequest {
        server_url: server.url().to_owned(),
        email: member.email.clone(),
        password: member.password.clone(),
    });
    let first = wait_for(&libraries, "the first update", |state| {
        updated_at(state).is_some()
    });
    let KicadLibrariesState::UpToDate {
        current,
        older,
        archived,
        restart_kicad,
        ..
    } = first
    else {
        unreachable!()
    };
    assert!(current > 10, "{current}");
    assert_eq!((older, archived, restart_kicad), (0, 0, false));
    let status = libraries.status();
    assert!(folder.join("team-parts.kicad_dbl").is_file());
    assert!(folder.join("symbols").join("g-res.kicad_sym").is_file());
    assert!(
        status
            .symbol_table
            .contains("(lib (name \"team-parts\")(type \"Database\")")
    );
    assert!(status.footprint_table.contains("(name \"g-Resistor_SMD\")"));
    assert_eq!(status.libraries[0].kind, "parts");
    assert_eq!(status.problems, Vec::<String>::new());

    // Someone else changes a symbol, which adds a revision: one update
    // follows by itself, about two seconds later.
    let before = updated_at(&first);
    let readback = read_back(&admin).unwrap();
    let part = readback.part("RES-0000-0470").unwrap();
    let revision: PartRevision = serde_json::from_str(
        &readback
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut edit = EditCommand::new("RES-0000-0470", "Use KiCad's resistor")
        .editing_cad(&revision, part.current_revision_number);
    assert!(edit.set_field("Symbol", "Device:R"));
    let changed = Instant::now();
    assert_eq!(submit_edit(&admin, &edit).unwrap().status, 200);
    let second = wait_for(&libraries, "the update after the Change", |state| {
        updated_at(state).is_some_and(|at| Some(at) != before)
    });
    assert!(
        changed.elapsed() >= Duration::from_secs(2),
        "it waited for quiet"
    );
    let KicadLibrariesState::UpToDate { older, .. } = second else {
        unreachable!()
    };
    assert_eq!(older, 1);

    // Update now runs at once, even with nothing new.
    let before = updated_at(&second);
    libraries.update_now();
    wait_for(&libraries, "Update now", |state| {
        updated_at(state).is_some_and(|at| Some(at) != before)
    });

    team.sign_out();
    wait_for(&libraries, "signing out", |state| {
        *state == KicadLibrariesState::SignedOut
    });
    server.stop();
}

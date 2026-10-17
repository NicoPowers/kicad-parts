//! The desktop keeps the signed-in team's KiCad libraries up to date
//! (kicad-parts-f397): nothing is made until the team has a code; then
//! after the sign-in catches up, after a Change by someone else, and on
//! Update now. Each team gets its own folder, and a code another team's
//! libraries use stops the updates. A disposable PocketBase on
//! 127.0.0.1 has the resistors imported; the libraries are written under
//! target/.

use catalog::PartRevision;
use catalog::pocketbase::{EditCommand, ImportCommand, read_back, submit_edit, submit_import};
use catalog_browser::kicad_libraries::KicadLibraries;
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use desktop_contract::{KicadLibrariesState, SignInRequest, TeamCode};
use std::path::Path;
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
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let viewer = services::add_member(&admin, "Vic Viewer", "vic@example.test", false).unwrap();
    let folder = root
        .join("target")
        .join("kicad-libraries-tests")
        .join("desktop");
    clear_read_only(&folder);
    let _ = std::fs::remove_dir_all(&folder);
    // Another team's libraries are already on this computer, with the code
    // SL.
    std::fs::create_dir_all(folder.join("SL")).unwrap();
    std::fs::write(
        folder.join("SL").join(kicad_libraries::STATE_FILE),
        r#"{"team_id":"another-team","server":"http://127.0.0.1:9999","code":"SL","updated_at_ms":1}"#,
    )
    .unwrap();

    let sign_in = |team: &TeamCatalog, member: &services::Member| {
        team.sign_in(SignInRequest {
            server_url: server.url().to_owned(),
            email: member.email.clone(),
            password: member.password.clone(),
        });
    };

    // A member who cannot edit sees who may choose the code, and cannot.
    let team = Arc::new(TeamCatalog::open(
        state.path().join("viewer.json"),
        Some(server.url().to_owned()),
        None,
    ));
    let libraries = KicadLibraries::start(folder.clone(), team.clone());
    assert_eq!(libraries.status().state, KicadLibrariesState::SignedOut);
    sign_in(&team, &viewer);
    wait_for(&libraries, "the team to need a code", |state| {
        *state == KicadLibrariesState::NeedsCode
    });
    assert_eq!(
        libraries.status().team_code,
        TeamCode::Missing {
            can_set: false,
            editors: vec!["Ada Editor".to_owned()],
            unnamed_editors: 0,
        }
    );
    assert_eq!(
        libraries.set_code("MA").unwrap_err(),
        "Only team members who can edit the catalog can choose the team code."
    );
    team.sign_out();

    // Someone who can edit chooses it.
    let team = Arc::new(TeamCatalog::open(
        state.path().join("team.json"),
        Some(server.url().to_owned()),
        None,
    ));
    let libraries = KicadLibraries::start(folder.clone(), team.clone());
    sign_in(&team, &editor);
    wait_for(&libraries, "the team to need a code", |state| {
        *state == KicadLibrariesState::NeedsCode
    });
    assert!(libraries.status().symbol_table.is_empty());
    assert!(!folder.join("MA").exists(), "no libraries without a code");
    let check = libraries.check_code("ma");
    assert!(check.problem.is_some());
    let check = libraries.check_code("SL");
    assert_eq!(check.problem, None);
    assert!(
        check.warning.unwrap().contains(
            "The SL libraries of another team, from the team server http://127.0.0.1:9999"
        )
    );
    assert_eq!(libraries.check_code("MA").warning, None);
    libraries.set_code("MA").unwrap();
    let first = wait_for(&libraries, "the first update", |state| {
        updated_at(state).is_some()
    });
    let KicadLibrariesState::UpToDate {
        current,
        older,
        archived,
        ..
    } = first
    else {
        unreachable!()
    };
    assert!(current > 10, "{current}");
    assert_eq!((older, archived), (0, 0));
    let status = libraries.status();
    assert!(!status.restart_kicad);
    assert_eq!(
        status.team_code,
        TeamCode::Set {
            code: "MA".into(),
            set_by: "Ada Editor".into()
        }
    );
    let team_folder = folder.join("MA");
    assert_eq!(status.folder, team_folder.display().to_string());
    assert!(team_folder.join("MA-Parts.kicad_dbl").is_file());
    assert!(
        team_folder
            .join("symbols")
            .join("MA-res-20241209.kicad_sym")
            .is_file()
    );
    assert!(
        status
            .symbol_table
            .contains("(lib (name \"MA-Parts\")(type \"Database\")")
    );
    assert!(
        status
            .footprint_table
            .contains("(name \"MA-Resistor_SMD\")")
    );
    assert_eq!(status.libraries[0].kind, "parts");
    assert_eq!(status.problems, Vec::<String>::new());
    // The other team's libraries are listed and left as they are.
    assert_eq!(status.other_teams.len(), 1);
    assert_eq!(status.other_teams[0].code, "SL");
    assert_eq!(std::fs::read_dir(folder.join("SL")).unwrap().count(), 1);

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

    // On a computer where another team's libraries already use MA, nothing
    // is written, and asking again changes nothing.
    let crowded = folder.with_file_name("desktop-code-taken");
    let _ = std::fs::remove_dir_all(&crowded);
    std::fs::create_dir_all(crowded.join("MA")).unwrap();
    let theirs = r#"{"team_id":"another-team","server":"http://127.0.0.1:9999","code":"MA","updated_at_ms":1}"#;
    std::fs::write(crowded.join("MA").join(kicad_libraries::STATE_FILE), theirs).unwrap();
    let clash = KicadLibraries::start(crowded.clone(), team.clone());
    let taken = KicadLibrariesState::CodeTaken {
        code: "MA".into(),
        other_server: "http://127.0.0.1:9999".into(),
        server: server.url().to_owned(),
    };
    wait_for(&clash, "the code to be taken", |state| *state == taken);
    clash.update_now();
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(clash.status().state, taken);
    let left: Vec<_> = std::fs::read_dir(crowded.join("MA"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, [".kicad-parts.json"]);
    assert_eq!(
        std::fs::read_to_string(crowded.join("MA").join(kicad_libraries::STATE_FILE)).unwrap(),
        theirs
    );

    team.sign_out();
    wait_for(&libraries, "signing out", |state| {
        *state == KicadLibrariesState::SignedOut
    });
    server.stop();
    clear_read_only(&folder);
    let _ = std::fs::remove_dir_all(&crowded);
}

//! The desktop draws a team part's symbol and footprint from the stored
//! bytes of the exact CAD item revisions it uses, not from the library on
//! this computer. These tests import the resistors into a disposable
//! PocketBase, then change a temporary copy of the catalog's libraries, and
//! check which drawing each request gets.

use catalog::pocketbase::{ImportCommand, read_back, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use desktop_contract::{
    LiveUpdates, PreviewKind, PreviewOutcome, PreviewProblemReason, PreviewRequest, PreviewSet,
    SignInRequest, TeamSession,
};
use kicad_integration::SearchLocations;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

const IPN: &str = "RES-0000-0470";

/// The SVG inside a preview's data URI, without the `<title>` that holds
/// the export time.
fn drawing(set: &PreviewSet) -> String {
    use base64::Engine;
    let encoded = set.images[0]
        .data_uri
        .strip_prefix("data:image/svg+xml;base64,")
        .unwrap();
    let svg = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    String::from_utf8(svg)
        .unwrap()
        .lines()
        .filter(|line| !line.trim_start().starts_with("<title>"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ready(outcome: PreviewOutcome) -> PreviewSet {
    match outcome {
        PreviewOutcome::Ready(set) => set,
        PreviewOutcome::Problem(problem) => panic!("no preview: {problem:?}"),
    }
}

fn open_browser(catalog_root: &Path, data: &Path) -> CatalogBrowser {
    CatalogBrowser::open(BrowserConfig {
        catalog_root: catalog_root.to_path_buf(),
        cache_dir: data.join("cache"),
        settings_file: data.join("config").join("settings.json"),
        kicad_locations: SearchLocations::Standard,
        kicad_config_dir: None,
    })
}

fn copy_folder(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A temporary copy of the catalog in which someone has since redrawn the
/// R_US symbol and the R_0603_1608Metric footprint.
fn changed_catalog(root: &Path, into: &Path) {
    for folder in ["database", "symbols", "footprints"] {
        copy_folder(&root.join(folder), &into.join(folder));
    }
    let symbols = into.join("symbols").join("g-res.kicad_sym");
    let text = fs::read_to_string(&symbols).unwrap();
    let changed = text.replacen(
        "(symbol \"R_US_0_1\"",
        "(symbol \"R_US_0_1\"\n\t\t\t(circle (center 0 0) (radius 5) (stroke (width 0.5) (type default)) (fill (type none)))",
        1,
    );
    assert_ne!(changed, text);
    fs::write(&symbols, changed).unwrap();
    let footprint = into
        .join("footprints")
        .join("g-Resistor_SMD.pretty")
        .join("R_0603_1608Metric.kicad_mod");
    let text = fs::read_to_string(&footprint).unwrap();
    let changed = text.replacen(
        "\t(fp_line",
        "\t(fp_circle (center 0 0) (end 2 0) (stroke (width 0.3) (type solid)) (fill no) (layer \"F.SilkS\"))\n\t(fp_line",
        1,
    );
    assert_ne!(changed, text);
    fs::write(&footprint, changed).unwrap();
}

fn request(kind: PreviewKind, team_revision: Option<u32>) -> PreviewRequest {
    PreviewRequest {
        ipn: IPN.into(),
        kind,
        field: None,
        team_revision,
        selection: Some(1),
    }
}

/// Signs `team` in and waits until it follows changes live.
fn sign_in_live(team: &TeamCatalog, url: &str, member: &services::Member) {
    team.sign_in(SignInRequest {
        server_url: url.to_owned(),
        email: member.email.clone(),
        password: member.password.clone(),
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(
        team.status().session,
        TeamSession::SignedIn {
            live: LiveUpdates::Live,
            ..
        }
    ) {
        assert!(Instant::now() < deadline, "live updates never connected");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_stored_revision_record_that_does_not_match_its_hash_is_not_drawn() {
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
    // Find the stored revision of the R_US symbol, then change its snapshot
    // in the stopped server's database, keeping the hash the part names.
    let readback = read_back(&admin).unwrap();
    let cad_item = readback
        .cad_items
        .iter()
        .find(|item| item.kind == "symbol" && item.key == "g-res:R_US")
        .unwrap();
    let revision = readback
        .cad_item_revisions
        .iter()
        .find(|revision| revision.cad_item == cad_item.id)
        .unwrap();
    server.stop();
    let connection =
        rusqlite::Connection::open(state.path().join("pb_data").join("data.db")).unwrap();
    let changed = connection
        .execute(
            "UPDATE catalog_cad_item_revisions SET snapshot = ?1 WHERE id = ?2",
            rusqlite::params![revision.snapshot.replace("R_US", "R_EU"), revision.id],
        )
        .unwrap();
    assert_eq!(changed, 1);
    drop(connection);
    let (server, _admin, _superuser) = services::start(&root, state.path()).unwrap();
    let url = server.url().to_owned();

    let data = tempfile::tempdir().unwrap();
    let browser = open_browser(&root, data.path());
    let team = TeamCatalog::open(data.path().join("team.json"), Some(url.clone()), None);
    sign_in_live(&team, &url, &member);
    let revision = team.part(IPN).unwrap().unwrap().revision.number;
    let symbol =
        ready(browser.preview_request(&request(PreviewKind::Symbol, Some(revision)), Some(&team)));
    assert_eq!(
        symbol.cad_item_revision, None,
        "not drawn as the stored revision"
    );
    assert_eq!(
        symbol.note.as_deref(),
        Some(
            "The team catalog's revision of g-res:R_US could not be used because its record \
             does not match its SHA-256. This drawing is from your local library, which may \
             differ from the revision this part uses."
        )
    );
    // The footprint's record is untouched and is drawn from its stored bytes.
    let footprint = ready(browser.preview_request(
        &request(PreviewKind::Footprint, Some(revision)),
        Some(&team),
    ));
    assert_eq!(footprint.cad_item_revision, Some(1));
    team.sign_out();
    server.stop();
}

#[test]
fn team_previews_show_the_stored_revision_after_the_local_library_changes() {
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
    let url = server.url().to_owned();

    let changed = tempfile::tempdir().unwrap();
    changed_catalog(&root, changed.path());
    let data = tempfile::tempdir().unwrap();
    let browser = open_browser(changed.path(), data.path());
    let unchanged = open_browser(&root, &data.path().join("unchanged"));

    let team = TeamCatalog::open(data.path().join("team.json"), Some(url.clone()), None);
    // Signed out, a team preview is a plain message, never the local file.
    for kind in [PreviewKind::Symbol, PreviewKind::Footprint] {
        match browser.preview_request(&request(kind, Some(1)), Some(&team)) {
            PreviewOutcome::Problem(problem) => {
                assert_eq!(problem.reason, PreviewProblemReason::TeamSignInNeeded);
                assert!(problem.message.starts_with("Sign in to the team catalog"));
            }
            other => panic!("expected a sign-in message, got {other:?}"),
        }
    }
    sign_in_live(&team, &url, &member);
    let revision = team.part(IPN).unwrap().unwrap().revision.number;

    for (kind, reference) in [
        (PreviewKind::Symbol, "g-res:R_US"),
        (PreviewKind::Footprint, "g-Resistor_SMD:R_0603_1608Metric"),
    ] {
        let local = ready(browser.preview_request(&request(kind, None), Some(&team)));
        let stored = ready(browser.preview_request(&request(kind, Some(revision)), Some(&team)));
        let original = ready(unchanged.preview_request(&request(kind, None), None));
        assert_eq!(stored.reference, reference);
        assert_eq!(stored.cad_item_revision, Some(1), "{reference}");
        assert_eq!(stored.note, None);
        assert!(stored.library_path.is_empty());
        assert_eq!(local.cad_item_revision, None);
        // The local file changed, but the team part still shows the bytes
        // the team catalog stored, which are the original library's.
        assert_ne!(drawing(&stored), drawing(&local), "{reference}");
        assert_eq!(drawing(&stored), drawing(&original), "{reference}");
        // A repeat view comes from the cache, keyed by the stored bytes' SHA-256.
        let again = ready(browser.preview_request(&request(kind, Some(revision)), Some(&team)));
        assert!(again.from_cache);
        assert_eq!(again.library_sha256, stored.library_sha256);
    }

    // The team server stops. A preview the cache already has still shows
    // the stored revision.
    server.stop();
    let cached =
        ready(browser.preview_request(&request(PreviewKind::Symbol, Some(revision)), Some(&team)));
    assert_eq!(cached.cad_item_revision, Some(1));

    // One the cache does not have falls back to the local library and says
    // that it differs from the stored revision.
    let fresh = open_browser(changed.path(), &data.path().join("fresh"));
    let fallback =
        ready(fresh.preview_request(&request(PreviewKind::Symbol, Some(revision)), Some(&team)));
    assert_eq!(fallback.cad_item_revision, None);
    let note = fallback.note.unwrap();
    assert_eq!(
        note,
        "Symbol revision 1 of g-res:R_US could not be fetched because the team server could not be \
         reached. This drawing is from your local library, which differs from that revision."
    );
    // With an unchanged local library, the note says the bytes match.
    let same = open_browser(&root, &data.path().join("same"));
    let note = ready(same.preview_request(
        &request(PreviewKind::Footprint, Some(revision)),
        Some(&team),
    ))
    .note
    .unwrap();
    assert!(note.ends_with("which has the same bytes."), "{note}");

    // A revision this app never fetched cannot be looked up at all.
    let unknown = ready(fresh.preview_request(
        &PreviewRequest {
            field: Some("g-res:R_US".into()),
            ..request(PreviewKind::Symbol, Some(revision + 1))
        },
        Some(&team),
    ));
    let note = unknown.note.unwrap();
    assert!(
        note.starts_with(
            "The team catalog could not be checked because the team server could not be reached."
        ),
        "{note}"
    );
    team.sign_out();
}

#[test]
fn after_going_back_to_earlier_content_previews_draw_each_revisions_own_cad() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let member = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let changed = tempfile::tempdir().unwrap();
    changed_catalog(&root, changed.path());
    // Content A, then B (the redrawn symbol and footprint), then A again:
    // CAD item revisions 1, 2 and 3, and part revisions 1, 2 and 3.
    for (step, catalog_root) in [
        ("A", &root),
        ("B", &changed.path().to_path_buf()),
        ("A again", &root),
    ] {
        let import = catalog::CsvCatalog::new(catalog_root)
            .import("g-res", None)
            .unwrap();
        let state = catalog::pocketbase::read_import_state(&admin).unwrap();
        let mut command = ImportCommand::new(&import, "Import the resistors")
            .allowing_uncommitted_changes(services::has_uncommitted_changes(&import))
            .expecting(&state);
        command
            .correlation_id
            .push_str(&format!("-{}", step.replace(' ', "-")));
        let reply = submit_import(&admin, &command).unwrap();
        assert_eq!(reply.status, 200, "{step}: {}", reply.body);
    }
    let url = server.url().to_owned();
    let data = tempfile::tempdir().unwrap();
    // The local library is the redrawn one, so a drawing from it would show.
    let browser = open_browser(changed.path(), data.path());
    let original = open_browser(&root, &data.path().join("original"));
    let redrawn = open_browser(changed.path(), &data.path().join("redrawn"));
    let team = TeamCatalog::open(data.path().join("team.json"), Some(url.clone()), None);
    sign_in_live(&team, &url, &member);
    assert_eq!(team.part(IPN).unwrap().unwrap().revision.number, 3);

    for kind in [PreviewKind::Symbol, PreviewKind::Footprint] {
        let content_a = drawing(&ready(original.preview_request(&request(kind, None), None)));
        let content_b = drawing(&ready(redrawn.preview_request(&request(kind, None), None)));
        assert_ne!(content_a, content_b);
        // Part revision 3 binds CAD item revision 3, which has content A.
        for (part_revision, cad_revision, content) in
            [(3, 3, &content_a), (2, 2, &content_b), (1, 1, &content_a)]
        {
            let stored =
                ready(browser.preview_request(&request(kind, Some(part_revision)), Some(&team)));
            assert_eq!(stored.cad_item_revision, Some(cad_revision), "{kind:?}");
            assert_eq!(stored.note, None);
            assert_eq!(&drawing(&stored), content, "{kind:?} {part_revision}");
        }
    }
    team.sign_out();
    server.stop();
}

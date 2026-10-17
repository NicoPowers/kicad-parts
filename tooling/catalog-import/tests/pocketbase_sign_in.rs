//! Signing in (kicad-parts-a3ba): the desktop reads every part's current
//! revision and details in pages, then replays the Changes saved since it
//! began. Others may save between two pages; the replica must still end
//! exactly like the server, and the realtime copies of those Changes must
//! change nothing. A proxy on 127.0.0.1 saves Changes just before the
//! second page is read; a disposable PocketBase on 127.0.0.1 has the
//! resistors.

mod support;

use catalog::PartDetails;
use catalog::PartRevision;
use catalog::pocketbase::{
    ArchiveCommand, CatalogReadback, EditCommand, ImportCommand, PublishOutcome, StoredChange,
    latest_change_seq, read_back, submit_archive, submit_edit, submit_import,
};
use catalog::replica::CatalogReplica;
use catalog_import::services;
use collaboration::{ApplyResult, PocketBaseClient, RealtimeConnection};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::proxy::Proxy;

const IPN: &str = "RES-0000-0470";
/// Small pages, so the resistors take several and others can save between
/// two of them.
const PARTS_PER_PAGE: usize = 10;

fn signed_in(url: &str, member: &services::Member) -> PocketBaseClient {
    PocketBaseClient::new(url)
        .authenticate("users", &member.email, &member.password)
        .unwrap()
}

/// An edit that sets the first describing field of `ipn`'s details.
fn details_edit(readback: &CatalogReadback, ipn: &str, value: &str) -> EditCommand {
    let part = readback.part(ipn).unwrap();
    let details: PartDetails = serde_json::from_str(
        &readback
            .part_details(&part.current_details)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let field = details.fields[0].name.clone();
    let mut edit = EditCommand::new(ipn, "Correct the description")
        .editing_details(&details, part.current_details_number);
    assert!(edit.set_field(&field, value));
    edit
}

/// An edit that changes the symbol of `ipn`, which adds a revision.
fn symbol_edit(readback: &CatalogReadback, ipn: &str, symbol: &str) -> EditCommand {
    let part = readback.part(ipn).unwrap();
    let revision: PartRevision = serde_json::from_str(
        &readback
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut edit = EditCommand::new(ipn, &format!("Use {symbol}"))
        .editing_cad(&revision, part.current_revision_number);
    assert!(edit.set_field("Symbol", symbol));
    edit
}

fn assert_applied(outcome: &PublishOutcome) {
    assert!(
        matches!(outcome, PublishOutcome::Applied { .. }),
        "{outcome:?}"
    );
}

/// Every part's current revision and details, by part ID, as the server
/// holds them.
fn server_state(readback: &CatalogReadback) -> Vec<(String, String, u32, String, u32)> {
    let mut state: Vec<_> = readback
        .parts
        .iter()
        .map(|part| {
            (
                part.id.clone(),
                part.current_revision.clone(),
                part.current_revision_number,
                part.current_details.clone(),
                part.current_details_number,
            )
        })
        .collect();
    state.sort();
    state
}

fn replica_state(replica: &CatalogReplica) -> Vec<(String, String, u32, String, u32)> {
    replica
        .parts()
        .iter()
        .map(|(id, part)| {
            (
                id.clone(),
                part.revision_id.clone(),
                part.number,
                part.details_id.clone(),
                part.details_number,
            )
        })
        .collect()
}

#[test]
fn signing_in_while_others_save_ends_like_the_server() {
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
    let editor = signed_in(
        server.url(),
        &services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap(),
    );
    let viewer_account =
        services::add_member(&admin, "Vic Viewer", "vic@example.test", false).unwrap();
    let viewer = signed_in(server.url(), &viewer_account);
    // A second revision, so it can be archived during the sign-in.
    assert_applied(
        &submit_edit(
            &editor,
            &symbol_edit(&read_back(&admin).unwrap(), IPN, "Device:R"),
        )
        .unwrap()
        .outcome,
    );
    let before = read_back(&admin).unwrap();
    let mut ids: Vec<&str> = before.parts.iter().map(|part| part.id.as_str()).collect();
    ids.sort();
    assert!(ids.len() > 2 * PARTS_PER_PAGE, "the resistors fill pages");
    let first_page_ipn = before
        .parts
        .iter()
        .find(|part| part.id == ids[0])
        .unwrap()
        .ipn
        .clone();
    // The ID the second page starts after: the last one the first page read.
    let cursor = ids[PARTS_PER_PAGE - 1].to_owned();

    // Just before the second page is read, others save three Changes: new
    // details for a part the first page already read, an archive that
    // moves a part's current revision back, and an import of the
    // capacitors, whose new parts land before and after the pages read.
    let saved = Arc::new(Mutex::new(false));
    let marked = saved.clone();
    let (hook_editor, hook_admin) = (editor.clone(), admin.clone());
    let capacitors = services::plan(&root, "g-cap").unwrap();
    let proxy = Proxy::start(
        server.url(),
        "/api/collections/catalog_parts/records",
        2,
        Box::new(move || {
            let now = read_back(&hook_admin).unwrap();
            assert_applied(
                &submit_edit(
                    &hook_editor,
                    &details_edit(&now, &first_page_ipn, "Changed during sign-in"),
                )
                .unwrap()
                .outcome,
            );
            let archived = submit_archive(
                &hook_editor,
                &ArchiveCommand::new(IPN, 2, true, "Wrong symbol"),
            )
            .unwrap();
            assert_eq!(archived.status, 200, "{}", archived.body);
            submit_import(
                &hook_admin,
                &ImportCommand::new(&capacitors, "Import the capacitors")
                    .allowing_uncommitted_changes(services::has_uncommitted_changes(&capacitors)),
            )
            .unwrap();
            *marked.lock().unwrap() = true;
        }),
    );

    // The desktop subscribes first, then signs in, as its follower does.
    let wait = Duration::from_secs(20);
    let mut connection = RealtimeConnection::connect(server.url(), wait).unwrap();
    connection.subscribe(&viewer, &["catalog_changes"]).unwrap();
    let noted = latest_change_seq(&viewer).unwrap();
    let mut replica = CatalogReplica::from_server_in_pages(
        &signed_in(&proxy.url, &viewer_account),
        PARTS_PER_PAGE,
    )
    .unwrap();
    assert!(*saved.lock().unwrap(), "the saves ran during the sign-in");

    // It ends exactly like the server: every part, its current revision
    // and its current details, and the newest Change.
    let after = read_back(&admin).unwrap();
    // Some new parts sort before the cursor, so the pages missed them and
    // only the replay can add them. Without one, this test would no longer
    // cover that case.
    let added: Vec<&str> = after
        .parts
        .iter()
        .map(|part| part.id.as_str())
        .filter(|id| !ids.contains(id))
        .collect();
    assert!(!added.is_empty(), "the capacitors came");
    assert!(
        added.iter().any(|id| *id < cursor.as_str()),
        "no part added during the sign-in sorts before the cursor {cursor}"
    );
    assert_eq!(replica_state(&replica), server_state(&after));
    assert_eq!(replica.checkpoint(), latest_change_seq(&viewer).unwrap());
    assert_eq!(replica.checkpoint(), noted + 3);
    let archived = replica.part_by_ipn(IPN).unwrap();
    assert_eq!(archived.number, 1, "the archive moved it back");
    // The same as replaying every Change from the first.
    let mut replayed = CatalogReplica::new();
    replayed.catch_up(&viewer).unwrap();
    assert_eq!(replica.parts(), replayed.parts());
    // Only the three Changes saved during the sign-in were replayed.
    assert_eq!(
        replica.applied_sequences(),
        [noted + 1, noted + 2, noted + 3]
    );

    // Their realtime notifications arrive afterwards and change nothing.
    let signed_in_state = replica.clone();
    for _ in 0..3 {
        let event = connection
            .next_on("catalog_changes", wait)
            .unwrap()
            .expect("a notification of a Change saved during the sign-in");
        let change: StoredChange = serde_json::from_value(event.data["record"].clone()).unwrap();
        assert_eq!(
            replica.receive(&change, &viewer).unwrap(),
            ApplyResult::Duplicate
        );
    }
    assert_eq!(replica, signed_in_state);

    // The next Change follows on.
    assert_applied(
        &submit_edit(&editor, &details_edit(&after, IPN, "Changed after sign-in"))
            .unwrap()
            .outcome,
    );
    let event = connection
        .next_on("catalog_changes", wait)
        .unwrap()
        .expect("a notification of the edit");
    let change: StoredChange = serde_json::from_value(event.data["record"].clone()).unwrap();
    assert_eq!(
        replica.receive(&change, &viewer).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        replica_state(&replica),
        server_state(&read_back(&admin).unwrap())
    );
    connection.disconnect();
    server.stop();
}

#[test]
fn signing_in_reads_the_current_state_in_a_few_requests() {
    // However long the history, signing in reads the newest Change, one
    // page per PARTS_PER_PAGE parts (the last one shorter, possibly empty),
    // and the Changes saved since the newest one: here none.
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
    let editor = signed_in(
        server.url(),
        &services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap(),
    );
    for value in ["1", "2", "3", "4", "5"] {
        assert_applied(
            &submit_edit(
                &editor,
                &details_edit(&read_back(&admin).unwrap(), IPN, value),
            )
            .unwrap()
            .outcome,
        );
    }
    let viewer = services::add_member(&admin, "Vic Viewer", "vic@example.test", false).unwrap();
    let proxy = Proxy::start(server.url(), "/never", 1, Box::new(|| {}));
    let client = signed_in(&proxy.url, &viewer);
    let before = proxy.requests();
    let replica = CatalogReplica::from_server_in_pages(&client, PARTS_PER_PAGE).unwrap();
    let parts = read_back(&admin).unwrap().parts.len();
    assert_eq!(replica.parts().len(), parts);
    assert_eq!(replica.checkpoint(), 6);
    assert_eq!(
        proxy.requests() - before,
        1 + (parts / PARTS_PER_PAGE + 1) + 1,
        "the newest Change, the pages, and the Changes after it"
    );
    server.stop();
}

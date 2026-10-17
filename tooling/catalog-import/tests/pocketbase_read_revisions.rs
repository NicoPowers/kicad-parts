//! Reading the Catalog for the KiCad libraries (kicad-parts-f397) while
//! someone saves: every CAD item read has its first Change read too, so the
//! libraries never name an item by a guess and rename it on the next
//! update. A proxy on 127.0.0.1 saves a Change just before the CAD items
//! are read; a disposable PocketBase on 127.0.0.1 has the resistors.

mod support;

use catalog::pocketbase::{ImportCommand, read_back, read_revisions, submit_import};
use catalog_import::services;
use collaboration::PocketBaseClient;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use support::proxy::Proxy;

#[test]
fn every_cad_item_read_has_its_first_change() {
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
    let viewer = services::add_member(&admin, "Vic Viewer", "vic@example.test", false).unwrap();
    let before = read_back(&admin).unwrap();
    let items_before = before.cad_items.len();

    // Just before the CAD items are read, someone imports more parts, with
    // CAD items the Catalog did not have: a new Change and new CAD items.
    let more = services::plan(&root, "g-cap").unwrap();
    let saved = Arc::new(Mutex::new(false));
    let marked = saved.clone();
    let hook_admin = admin.clone();
    let proxy = Proxy::start(
        server.url(),
        "/api/collections/catalog_cad_items/records",
        1,
        Box::new(move || {
            let command = ImportCommand::new(&more, "Import the capacitors")
                .allowing_uncommitted_changes(services::has_uncommitted_changes(&more));
            submit_import(&hook_admin, &command).unwrap();
            *marked.lock().unwrap() = true;
        }),
    );
    let reader = PocketBaseClient::new(&proxy.url)
        .authenticate("users", &viewer.email, &viewer.password)
        .unwrap();
    let readback = read_revisions(&reader).unwrap();
    assert!(*saved.lock().unwrap(), "the save ran during the reading");
    assert!(
        readback.cad_items.len() > items_before,
        "the reading has the new CAD items"
    );
    let changes: BTreeSet<&str> = readback
        .changes
        .iter()
        .map(|change| change.id.as_str())
        .collect();
    for item in &readback.cad_items {
        assert!(
            changes.contains(item.first_change.as_str()),
            "{} was read without its first Change",
            item.key
        );
    }
    server.stop();
}

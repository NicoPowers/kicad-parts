//! Reading the Catalog for the KiCad libraries (kicad-parts-f397) while
//! someone saves: every CAD item read has its first Change read too, so the
//! libraries never name an item by a guess and rename it on the next
//! update. A proxy on 127.0.0.1 saves a Change just before the CAD items
//! are read; a disposable PocketBase on 127.0.0.1 has the resistors.

use catalog::pocketbase::{ImportCommand, read_back, read_revisions, submit_import};
use catalog_import::services;
use collaboration::PocketBaseClient;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

type Hook = Box<dyn FnOnce() + Send>;

/// A proxy on 127.0.0.1 in front of `upstream` that runs `hook` once, just
/// before it passes on the first request that reads the CAD items.
fn proxy(upstream: &str, hook: Hook) -> String {
    let upstream = upstream.trim_start_matches("http://").to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hook = Arc::new(Mutex::new(Some(hook)));
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let upstream = upstream.clone();
            let hook = hook.clone();
            std::thread::spawn(move || {
                let _ = pass_on(client, &upstream, &hook);
            });
        }
    });
    url
}

fn pass_on(
    mut client: TcpStream,
    upstream: &str,
    hook: &Mutex<Option<Hook>>,
) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let read = client.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while request.len() < head_end + length {
        let read = client.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    if head.contains("/api/collections/catalog_cad_items/records") {
        let taken = hook.lock().unwrap().take();
        if let Some(hook) = taken {
            hook();
        }
    }
    let mut lines: Vec<&str> = head
        .trim_end()
        .lines()
        .filter(|line| !line.to_ascii_lowercase().starts_with("connection:"))
        .collect();
    lines.push("Connection: close");
    let mut server = TcpStream::connect(upstream)?;
    server.write_all(format!("{}\r\n\r\n", lines.join("\r\n")).as_bytes())?;
    server.write_all(&request[head_end..])?;
    std::io::copy(&mut server, &mut client)?;
    Ok(())
}

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
    let url = proxy(
        server.url(),
        Box::new(move || {
            let command = ImportCommand::new(&more, "Import the capacitors")
                .allowing_uncommitted_changes(services::has_uncommitted_changes(&more));
            submit_import(&hook_admin, &command).unwrap();
            *marked.lock().unwrap() = true;
        }),
    );
    let reader = PocketBaseClient::new(&url)
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

//! What an importer reads from the team catalog before importing
//! (`read_import_state`), on a real, disposable PocketBase with history:
//! several revisions of parts, of a footprint and of its 3D model, an
//! archived part revision, and a part whose current revision binds an older
//! footprint revision. The read must give the same import state as one
//! worked out from every stored record, while downloading only current
//! records: a counting proxy between the importer and PocketBase lists
//! every request and how many records each returned.
//!
//! The test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; every changed input is a temp copy.

use catalog::pocketbase::{
    ArchiveCommand, CatalogReadback, EditCommand, ImportCommand, ImportOutcome, ImportState,
    LatestImport, read_back, read_import_state, submit_archive, submit_edit, submit_import,
};
use catalog::{
    Binding, CadItemDetails, CsvCatalog, CsvImport, CurrentCad, CurrentCadItemRevision,
    ModelTarget, PartRevision,
};
use catalog_import::services;
use collaboration::PocketBaseClient;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

const REASON: &str = "Import the resistors from database/g-res.csv";
const FOOTPRINT: &str = "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod";
const MODEL: &str = "3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step";
/// Every file the resistor import reads, once its footprint names `MODEL`.
const INPUTS: [&str; 5] = [
    "database/g-res.csv",
    "database/substitutes.csv",
    "symbols/g-res.kicad_sym",
    FOOTPRINT,
    MODEL,
];
/// A resistor whose CAD an edit keeps, so it stays on footprint revision 1.
const EDITED: &str = "RES-0000-1000";
/// A resistor whose revision 2 is archived.
const ARCHIVED: &str = "RES-0000-0470";

#[test]
fn the_import_state_is_read_from_current_records_only() {
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    for path in INPUTS {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(services::repository_root().join(path), destination).unwrap();
    }
    // The resistor footprint names KiCad's own 3D model, which is not
    // stored; point it at a model in the CSV catalog, so the footprint binds
    // a 3D model revision.
    replace_once(
        root,
        FOOTPRINT,
        "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
        "${GITPLM_PARTS}/3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step",
    );
    let state = tempfile::tempdir().unwrap();
    let (server, admin, superuser) =
        services::start(&services::repository_root(), state.path()).unwrap();

    // Footprint 1, model 1, every part at revision 1.
    import(&admin, root, "applied");
    // An edit points one resistor's footprint elsewhere and back, so the
    // edit, not the CSV, keeps its CAD: it stays on footprint revision 1.
    let footprint = current_revision(&read_back(&admin).unwrap(), EDITED)
        .field("Footprint")
        .unwrap()
        .to_owned();
    edit_cad(&admin, EDITED, "Device:R_0603");
    edit_cad(&admin, EDITED, &footprint);
    // Footprint 2, and part revision 2 of the other resistors.
    replace_once(
        root,
        FOOTPRINT,
        "(tags \"resistor\")",
        "(tags \"resistor B\")",
    );
    import(&admin, root, "applied");
    // Revision 2 of one resistor is archived, so revision 1 is current.
    let reply = submit_archive(
        &admin,
        &ArchiveCommand::new(ARCHIVED, 2, true, "Footprint B is wrong for this part"),
    )
    .unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    // Model 2, so footprint 3 (which names it), and part revision 3.
    let mut model = fs::read(root.join(MODEL)).unwrap();
    model.extend_from_slice(b"/* content B */\n");
    fs::write(root.join(MODEL), model).unwrap();
    import(&admin, root, "applied");

    let stored = read_back(&admin).unwrap();
    let expected = state_from(&stored);
    // The history the read must not download.
    let older_bound = older_bound_revisions(&stored);
    assert!(older_bound >= 1, "a part binds an older CAD item revision");
    assert!(stored.cad_item_revisions.len() > stored.cad_items.len() + older_bound);
    assert!(stored.part_revisions.len() > stored.parts.len());
    let runs = stored
        .audit_entries
        .iter()
        .filter(|entry| entry.csv_import_number > 0)
        .count();
    assert_eq!(runs, 3);
    assert!(stored.archive_entries.iter().any(|entry| entry.archived));

    let proxy = CountingProxy::start(admin.base_url());
    let counted = PocketBaseClient::new(&proxy.url)
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    proxy.clear();
    let read = read_import_state(&counted).unwrap();
    let fetches = proxy.fetches();
    assert_eq!(read, expected);

    let fetched = |collection: &str| -> usize {
        fetches
            .iter()
            .filter(|fetch| fetch.collection == collection)
            .map(|fetch| fetch.records)
            .sum()
    };
    let summary = format!("{fetches:#?}");
    assert_eq!(fetched("catalog_parts"), stored.parts.len(), "{summary}");
    // Each part's current revision, not its history.
    assert_eq!(
        fetched("catalog_part_revisions"),
        stored.parts.len(),
        "{summary}"
    );
    // Each CAD item's current revision, and the older ones that parts'
    // current revisions bind.
    assert_eq!(
        fetched("catalog_cad_item_revisions"),
        stored.cad_items.len() + older_bound,
        "{summary}"
    );
    // The latest run of the one CSV file imported, not every run.
    assert_eq!(fetched("catalog_audit_entries"), 1, "{summary}");
    // Parts, current CAD item revisions, the current CAD item revisions
    // made in the app (none here, so no last imported revision is read),
    // current part revisions, the older bound CAD item revisions, the latest
    // run of g-res and the check that no other CSV file was imported.
    assert_eq!(fetches.len(), 7, "{summary}");
    // Revisions are always asked for by filter, never listed whole.
    assert!(
        fetches
            .iter()
            .filter(|fetch| fetch.collection.ends_with("_revisions"))
            .all(|fetch| fetch.query.contains("filter=")),
        "{summary}"
    );

    // The state read this way imports as before: nothing changed, so the
    // import is a no-op.
    let reply = submit_import(
        &admin,
        &ImportCommand::new(&plan(root), REASON).expecting(&read),
    )
    .unwrap();
    assert!(
        matches!(reply.outcome, ImportOutcome::Noop { .. }),
        "{}",
        reply.body
    );
    server.stop();
}

/// CAD items whose current revision was made in the app are read with
/// their newest imported revision in one request, however many there are.
#[test]
fn cad_items_made_in_the_app_are_read_in_one_request() {
    let copy = tempfile::tempdir().unwrap();
    let root = copy.path();
    for path in &INPUTS[..4] {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(services::repository_root().join(path), destination).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let (server, admin, superuser) =
        services::start(&services::repository_root(), state.path()).unwrap();
    import(&admin, root, "applied");
    // The app makes new revisions of the footprint and the symbol.
    let footprint = String::from_utf8(fs::read(root.join(FOOTPRINT)).unwrap())
        .unwrap()
        .replacen("(tags \"resistor\")", "(tags \"resistor app\")", 1);
    let symbols = String::from_utf8(fs::read(root.join("symbols/g-res.kicad_sym")).unwrap())
        .unwrap()
        .replacen("(symbol \"R_US\"", "(symbol \"R_US\"\n\t\t(in_bom yes)", 1);
    for (ipn, upload) in [
        (
            EDITED,
            catalog::cad_file::footprint_upload(
                footprint.as_bytes(),
                "g-Resistor_SMD:R_0603_1608Metric",
                "R.kicad_mod",
            )
            .unwrap(),
        ),
        (
            ARCHIVED,
            catalog::cad_file::symbol_upload(
                symbols.as_bytes(),
                "R_US",
                "g-res:R_US",
                "s.kicad_sym",
            )
            .unwrap(),
        ),
    ] {
        let stored = read_back(&admin).unwrap();
        let number = stored.part(ipn).unwrap().current_revision_number;
        let mut command = EditCommand::new(ipn, "Use the file")
            .editing_cad(&current_revision(&stored, ipn), number);
        command.cad.as_mut().unwrap().uploads = vec![upload];
        let reply = submit_edit(&admin, &command).unwrap();
        assert_eq!(reply.status, 200, "{}", reply.body);
    }

    let proxy = CountingProxy::start(admin.base_url());
    let counted = PocketBaseClient::new(&proxy.url)
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    proxy.clear();
    let read = read_import_state(&counted).unwrap();
    let fetches = proxy.fetches();
    let summary = format!("{fetches:#?}");
    assert_eq!(read.current_cad.edited_in_app.len(), 2, "{summary}");
    for edited in read.current_cad.edited_in_app.values() {
        assert_eq!(edited.current.number, 2);
        assert_eq!(
            edited
                .last_imported
                .as_ref()
                .map(|last| last.binding.number),
            Some(1)
        );
    }
    // Parts, current CAD item revisions, the ones made in the app, both
    // newest imported revisions in one request (the older revisions the
    // other parts bind, so no request of their own), current part
    // revisions, and the two reads of the latest import runs.
    assert_eq!(fetches.len(), 7, "{summary}");
    server.stop();
}

/// Replaces exactly one occurrence of `from` in `path` under `root`.
fn replace_once(root: &Path, path: &str, from: &str, to: &str) {
    let file = root.join(path);
    let text = fs::read_to_string(&file).unwrap();
    assert_eq!(text.matches(from).count(), 1, "{from:?} in {path}");
    fs::write(&file, text.replacen(from, to, 1)).unwrap();
}

fn plan(root: &Path) -> CsvImport {
    CsvCatalog::new(root).import("g-res", None).unwrap()
}

/// Imports `root` as an importer does, after reading the team catalog, and
/// expects `outcome`.
fn import(admin: &PocketBaseClient, root: &Path, outcome: &str) {
    let state = read_import_state(admin).unwrap();
    let reply = submit_import(
        admin,
        &ImportCommand::new(&plan(root), REASON).expecting(&state),
    )
    .unwrap();
    let got = match reply.outcome {
        ImportOutcome::Applied { .. } => "applied",
        ImportOutcome::NothingChanged { .. } => "nothing_changed",
        ImportOutcome::Noop { .. } => "noop",
        _ => "refused",
    };
    assert_eq!(got, outcome, "{}", reply.body);
}

fn current_revision(stored: &CatalogReadback, ipn: &str) -> PartRevision {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    serde_json::from_str(&revision.snapshot).unwrap()
}

/// Sets the Footprint of `ipn` to `value` with an edit, as a person in the
/// app does.
fn edit_cad(admin: &PocketBaseClient, ipn: &str, value: &str) {
    let stored = read_back(admin).unwrap();
    let number = stored.part(ipn).unwrap().current_revision_number;
    let mut command =
        EditCommand::new(ipn, "Set Footprint").editing_cad(&current_revision(&stored, ipn), number);
    assert!(command.set_field("Footprint", value));
    let reply = submit_edit(admin, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
}

/// The import state worked out from every stored record: each part's
/// current revision and details, each CAD item's newest revision, the CAD
/// item revisions each part's current revision binds, and the latest run
/// of each CSV file.
fn state_from(stored: &CatalogReadback) -> ImportState {
    let binding = |id: &str| {
        let revision = stored.cad_item_revision(id).unwrap();
        let details: CadItemDetails = serde_json::from_str(&revision.snapshot).unwrap();
        (
            revision.cad_item.clone(),
            Binding {
                number: revision.number,
                snapshot_sha256: revision.snapshot_sha256.clone(),
                content_sha256: details.content_sha256(),
            },
        )
    };
    let mut current_cad = CurrentCad::default();
    for cad_item in &stored.cad_items {
        let newest = stored
            .cad_item_revisions
            .iter()
            .filter(|revision| revision.cad_item == cad_item.id)
            .max_by_key(|revision| revision.number)
            .unwrap();
        let details: CadItemDetails = serde_json::from_str(&newest.snapshot).unwrap();
        let models = details
            .model_references
            .iter()
            .filter_map(|model| match &model.target {
                ModelTarget::CadItemRevision {
                    cad_item,
                    number,
                    snapshot_sha256,
                } => Some((
                    cad_item.clone(),
                    Binding {
                        number: *number,
                        snapshot_sha256: snapshot_sha256.clone(),
                        content_sha256: snapshot_sha256.clone(),
                    },
                )),
                ModelTarget::Unresolved { .. } => None,
            })
            .collect();
        current_cad.cad_items.insert(
            cad_item.id.clone(),
            CurrentCadItemRevision {
                number: newest.number,
                snapshot_sha256: newest.snapshot_sha256.clone(),
                models,
            },
        );
    }
    for part in &stored.parts {
        let revision = stored.part_revision(&part.current_revision).unwrap();
        let bindings = revision
            .cad_item_revisions
            .iter()
            .map(|id| binding(id))
            .collect();
        current_cad.parts.insert(part.ipn.clone(), bindings);
    }
    let mut latest: BTreeMap<&str, LatestImport> = BTreeMap::new();
    for entry in &stored.audit_entries {
        let newer = latest
            .get(entry.csv_file.as_str())
            .is_none_or(|run| run.csv_import_number < entry.csv_import_number);
        if entry.csv_import_number > 0 && newer {
            latest.insert(
                &entry.csv_file,
                LatestImport {
                    csv_file: entry.csv_file.clone(),
                    csv_import_number: entry.csv_import_number,
                    source_commit: entry.source_commit.clone(),
                    audit_entry: entry.id.clone(),
                },
            );
        }
    }
    let mut latest_imports: Vec<LatestImport> = latest.into_values().collect();
    latest_imports.sort_by_key(|run| run.csv_import_number);
    ImportState {
        current_revisions: stored
            .parts
            .iter()
            .map(|part| (part.ipn.clone(), part.current_revision_number))
            .collect(),
        current_details: stored
            .parts
            .iter()
            .map(|part| (part.ipn.clone(), part.current_details_number))
            .collect(),
        current_cad,
        latest_imports,
    }
}

/// How many CAD item revisions that are not their CAD item's newest the
/// parts' current revisions bind.
fn older_bound_revisions(stored: &CatalogReadback) -> usize {
    let mut older = std::collections::BTreeSet::new();
    for part in &stored.parts {
        let revision = stored.part_revision(&part.current_revision).unwrap();
        for id in &revision.cad_item_revisions {
            let bound = stored.cad_item_revision(id).unwrap();
            let newest = stored
                .cad_item_revisions
                .iter()
                .filter(|other| other.cad_item == bound.cad_item)
                .map(|other| other.number)
                .max()
                .unwrap();
            if bound.number < newest {
                older.insert(id.clone());
            }
        }
    }
    older.len()
}

/// One record list request that went through the [`CountingProxy`].
#[derive(Debug)]
struct Fetch {
    collection: String,
    /// The request's query, still URL-encoded.
    query: String,
    records: usize,
}

/// A loopback HTTP proxy in front of PocketBase that notes each record list
/// request and how many records it returned. It asks PocketBase to close
/// each connection after one answer, so every request is seen whole.
struct CountingProxy {
    url: String,
    fetches: Arc<Mutex<Vec<Fetch>>>,
}

impl CountingProxy {
    fn start(upstream_url: &str) -> Self {
        let upstream = upstream_url
            .strip_prefix("http://")
            .unwrap()
            .trim_end_matches('/')
            .to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let fetches = Arc::new(Mutex::new(Vec::new()));
        let log = fetches.clone();
        // The thread ends with the test process.
        std::thread::spawn(move || {
            for client in listener.incoming() {
                let Ok(client) = client else { continue };
                let (upstream, log) = (upstream.clone(), log.clone());
                std::thread::spawn(move || forward(client, &upstream, &log));
            }
        });
        Self { url, fetches }
    }

    fn clear(&self) {
        self.fetches.lock().unwrap().clear();
    }

    fn fetches(&self) -> Vec<Fetch> {
        std::mem::take(&mut *self.fetches.lock().unwrap())
    }
}

/// Forwards one request from `client` to `upstream` and its answer back,
/// noting it in `log` if it lists records.
fn forward(mut client: TcpStream, upstream: &str, log: &Mutex<Vec<Fetch>>) {
    let mut request = Vec::new();
    let head_end = loop {
        if let Some(end) = find(&request, b"\r\n\r\n") {
            break end;
        }
        let mut chunk = [0u8; 8192];
        let read = client.read(&mut chunk).unwrap();
        if read == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8(request[..head_end].to_vec()).unwrap();
    let mut body = request[head_end + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap().to_owned();
    let mut headers: Vec<&str> = Vec::new();
    let mut length = 0;
    for line in lines {
        let name = line.split(':').next().unwrap().to_ascii_lowercase();
        if name == "content-length" {
            length = line[line.find(':').unwrap() + 1..].trim().parse().unwrap();
        }
        // One answer per connection, and plain JSON to count.
        if name != "connection" && name != "accept-encoding" {
            headers.push(line);
        }
    }
    while body.len() < length {
        let mut chunk = vec![0u8; length - body.len()];
        let read = client.read(&mut chunk).unwrap();
        body.extend_from_slice(&chunk[..read]);
    }
    let mut server = TcpStream::connect(upstream).unwrap();
    let head = format!(
        "{request_line}\r\n{}\r\nConnection: close\r\n\r\n",
        headers.join("\r\n")
    );
    server.write_all(head.as_bytes()).unwrap();
    server.write_all(&body).unwrap();
    let mut answer = Vec::new();
    server.read_to_end(&mut answer).unwrap();
    client.write_all(&answer).unwrap();
    client.flush().unwrap();

    let target = request_line.split(' ').nth(1).unwrap();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let Some(collection) = path
        .strip_prefix("/api/collections/")
        .and_then(|rest| rest.strip_suffix("/records"))
    else {
        return;
    };
    if !request_line.starts_with("GET ") {
        return;
    }
    let answer_head_end = find(&answer, b"\r\n\r\n").unwrap();
    let answer_head = String::from_utf8_lossy(&answer[..answer_head_end]).to_ascii_lowercase();
    let mut json = answer[answer_head_end + 4..].to_vec();
    if answer_head.contains("transfer-encoding: chunked") {
        json = unchunk(&json);
    }
    let list: serde_json::Value = serde_json::from_slice(&json).unwrap();
    log.lock().unwrap().push(Fetch {
        collection: collection.to_owned(),
        query: query.to_owned(),
        records: list["items"].as_array().unwrap().len(),
    });
}

fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The body of a chunked HTTP answer.
fn unchunk(mut chunked: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let line_end = find(chunked, b"\r\n").unwrap();
        let size_text = String::from_utf8_lossy(&chunked[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap().trim(), 16).unwrap();
        if size == 0 {
            return body;
        }
        let start = line_end + 2;
        body.extend_from_slice(&chunked[start..start + size]);
        chunked = &chunked[start + size + 2..];
    }
}

//! The Catalog's PocketBase contract: the protected routes
//! (`infra/pocketbase/pb_hooks/catalog_csv_imports.pb.js` for CSV imports,
//! `catalog_edits.pb.js` for edits and restores) and the `catalog_*`
//! collections (`infra/pocketbase/pb_migrations/1791500000_catalog_init.js`
//! and `1791600000_catalog_edits.js`).

use crate::cad_item::Blob;
use crate::csv_import::{CsvImport, CurrentCadItemRevision, SourceFile};
use crate::part::{DetailField, ManufacturerPart, PartDetails};
use crate::sha256_hex;
use collaboration::{CollabError, CollabResult, CorrelationId, PocketBaseClient};
use serde::Deserialize;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// The route that publishes a CSV catalog import.
pub const CSV_IMPORT_ROUTE: &str = "/api/catalog/csv-imports";

/// The largest import request the route accepts, in bytes (32 MiB, set by
/// `IMPORT_BODY_LIMIT` in `catalog_csv_imports.pb.js`). An import is one
/// request so that it is one transaction; the largest import of the CSV
/// catalog today (g-con, with a 1.5 MiB 3D model) is 1.8 MiB (1,900,969 bytes).
pub const IMPORT_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Every Catalog collection, in dependency order.
pub const COLLECTIONS: [&str; 9] = [
    "catalog_changes",
    "catalog_blobs",
    "catalog_cad_items",
    "catalog_cad_item_revisions",
    "catalog_parts",
    "catalog_part_revisions",
    "catalog_audit_entries",
    "catalog_import_problems",
    "catalog_commands",
];

/// One import request: what to publish, why, and its idempotency key.
#[derive(Clone, Debug)]
pub struct ImportCommand<'a> {
    pub correlation_id: String,
    pub reason: String,
    /// The import to send. [`ImportCommand::expecting`] replaces it with a
    /// copy whose CAD item revisions are numbered against the team catalog.
    pub import: Cow<'a, CsvImport>,
    /// The current revision number the importer read for each IPN. A part
    /// missing here is expected not to exist yet (number 0). This and the
    /// next two fields are private so that only [`ImportCommand::expecting`]
    /// and [`ImportCommand::allowing_older_checkout`] set them, which derive
    /// the correlation ID again. If any part's
    /// current revision differs when the import arrives, the route answers
    /// 409 conflict and writes nothing. See [`ImportCommand::expecting`].
    expected_revisions: BTreeMap<String, u32>,
    /// The number of the latest import run of this CSV file that the
    /// importer read (0 if none). If another import of the file lands
    /// first, the route answers 409 and writes nothing, because the checkout
    /// check was made against an older state.
    expected_csv_import: u64,
    /// Import even though the checkout does not include the team catalog's
    /// latest imports. The run's audit entry records it.
    allow_older_checkout: bool,
}

impl<'a> ImportCommand<'a> {
    /// A command whose correlation ID is derived from the import and what
    /// it expects, so sending the same command again replays the stored
    /// answer. It expects every part to be new; call
    /// [`ImportCommand::expecting`] before importing into a catalog that
    /// already has parts.
    pub fn new(import: &'a CsvImport, reason: &str) -> Self {
        let mut command = Self {
            correlation_id: String::new(),
            reason: reason.to_owned(),
            import: Cow::Borrowed(import),
            expected_revisions: BTreeMap::new(),
            expected_csv_import: 0,
            allow_older_checkout: false,
        };
        command.correlation_id = command.derived_correlation_id();
        command
    }

    /// Expects each part's current revision, each CAD item's current
    /// revision, and the latest import run of this CSV file, to be the ones
    /// in `state`. Numbers the CAD item revisions against `state` (see
    /// [`CsvImport::numbered_against`]), so the parts bind each CAD item's
    /// current revision. Derives the correlation ID again.
    pub fn expecting(mut self, state: &ImportState) -> Self {
        self.import = Cow::Owned(
            self.import
                .numbered_against(&state.current_cad_item_revisions),
        );
        self.expected_revisions = self
            .import
            .parts
            .iter()
            .filter_map(|part| {
                let ipn = part.ipn.as_str();
                let number = *state.current_revisions.get(ipn)?;
                Some((ipn.to_owned(), number))
            })
            .collect();
        self.expected_csv_import = state
            .latest_import(&self.import.csv_file)
            .map_or(0, |run| run.csv_import_number);
        self.correlation_id = self.derived_correlation_id();
        self
    }

    /// The current revision number expected for each IPN; a part missing
    /// here is expected to be new.
    pub fn expected_revisions(&self) -> &BTreeMap<String, u32> {
        &self.expected_revisions
    }

    /// The number of the latest import run of this CSV file expected.
    pub fn expected_csv_import(&self) -> u64 {
        self.expected_csv_import
    }

    /// Whether the import is allowed from an older checkout.
    pub fn allow_older_checkout(&self) -> bool {
        self.allow_older_checkout
    }

    /// Imports even from a checkout that does not include the team catalog's
    /// latest imports, if `allow`. Derives the correlation ID again.
    pub fn allowing_older_checkout(mut self, allow: bool) -> Self {
        self.allow_older_checkout = allow;
        self.correlation_id = self.derived_correlation_id();
        self
    }

    /// `csv-import-` and 32 hex digits of a hash of everything the route
    /// keys the command on: the import, the expected revisions, the
    /// expected import run and the older-checkout setting. The same import
    /// sent again after the catalog moved on gets a new ID, so it is
    /// compared afresh rather than refused as a reused ID.
    fn derived_correlation_id(&self) -> String {
        let mut key = self.import.fingerprint();
        for (ipn, number) in &self.expected_revisions {
            key.push_str(&format!("\n{ipn}={number}"));
        }
        key.push_str(&format!(
            "\nimport={}\nallow-older-checkout={}",
            self.expected_csv_import, self.allow_older_checkout
        ));
        format!("csv-import-{}", &sha256_hex(key)[..32])
    }

    pub fn body(&self) -> Value {
        let import = self.import.as_ref();
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "csvFile": import.csv_file,
            "sourceCommit": import.source_commit,
            "allowOlderCheckout": self.allow_older_checkout,
            "expectedCsvImport": self.expected_csv_import,
            "sources": import.sources.iter().map(|source| {
                let mut file = json!({
                    "role": source.role,
                    "path": source.path,
                    "byteSha256": source.byte_sha256,
                    "size": source.size,
                    "headerRaw": source.header_raw,
                });
                if let Some(rows) = &source.rows {
                    file["rows"] = json!(rows);
                }
                file
            }).collect::<Vec<_>>(),
            "blobs": import.blobs.iter().map(blob_body).collect::<Vec<_>>(),
            "cadItemRevisions": import.cad_item_revisions.iter().map(|revision| json!({
                "id": revision.id,
                "cadItemId": revision.cad_item.id,
                "kind": revision.cad_item.kind.as_str(),
                "key": revision.cad_item.key,
                "expectedRevision": revision.expected_revision,
                "number": revision.number,
                "snapshotSha256": revision.snapshot_sha256,
                "snapshot": revision.snapshot_json,
                "source": revision.source_json,
            })).collect::<Vec<_>>(),
            "parts": import.parts.iter().map(|part| json!({
                "id": part.id,
                "ipn": part.ipn.as_str(),
                "expectedRevision": self.expected_revisions.get(part.ipn.as_str()).copied().unwrap_or(0),
                "snapshot": part.snapshot_json,
                "snapshotSha256": part.snapshot_sha256,
                "source": part.source_json,
            })).collect::<Vec<_>>(),
            "problems": import.problems.iter().map(|problem| json!({
                "id": problem.id,
                "code": problem.code,
                "subject": problem.subject,
                "path": problem.path,
                "location": problem.location,
                "message": problem.message,
                "detail": problem.detail_json,
            })).collect::<Vec<_>>(),
        })
    }
}

/// One blob as the import route takes it: as `text` when its bytes are
/// UTF-8, which the route hashes and stores with PocketBase's own fast code,
/// and as `base64` otherwise, which the route decodes and hashes in
/// JavaScript at about 3 s per MiB.
fn blob_body(blob: &Blob) -> Value {
    match std::str::from_utf8(&blob.bytes) {
        Ok(text) => json!({ "sha256": blob.sha256, "size": blob.bytes.len(), "text": text }),
        Err(_) => json!({
            "sha256": blob.sha256,
            "size": blob.bytes.len(),
            "base64": base64(&blob.bytes),
        }),
    }
}

/// What an importer reads from the team catalog before importing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportState {
    /// Each part's current revision number, by IPN.
    pub current_revisions: BTreeMap<String, u32>,
    /// Each CAD item's current revision (its newest), by CAD item ID.
    pub current_cad_item_revisions: BTreeMap<String, CurrentCadItemRevision>,
    /// The latest import run of each CSV file, oldest first.
    pub latest_imports: Vec<LatestImport>,
}

impl ImportState {
    /// The latest import run of `csv_file` (such as `g-res`), if any.
    pub fn latest_import(&self, csv_file: &str) -> Option<&LatestImport> {
        self.latest_imports
            .iter()
            .find(|run| run.csv_file == csv_file)
    }
}

/// The latest import run of one CSV file.
#[derive(Clone, Debug, PartialEq)]
pub struct LatestImport {
    /// Such as `g-res`.
    pub csv_file: String,
    pub csv_import_number: u64,
    /// The commit its files were read at, possibly ending in `+dirty`;
    /// empty if it was not recorded.
    pub source_commit: String,
    pub audit_entry: String,
    /// `(path, byte_sha256)` of every file it read, sorted by path.
    pub file_hashes: Vec<(String, String)>,
}

/// The few fields of an audit entry that [`read_import_state`] downloads.
#[derive(Deserialize)]
struct LatestImportFields {
    id: String,
    csv_file: String,
    csv_import_number: u64,
    #[serde(default)]
    source_commit: String,
    #[serde(default)]
    file_hashes: String,
}

/// The few fields of a CAD item revision that [`read_import_state`]
/// downloads.
#[derive(Deserialize)]
struct CadItemRevisionNumber {
    cad_item: String,
    number: u32,
    snapshot_sha256: String,
}

/// Reads each part's current revision number, each CAD item's current
/// revision, and the latest import run of each CSV file. Downloads only the
/// fields it needs, never the recorded rows or snapshots.
pub fn read_import_state(client: &PocketBaseClient) -> CollabResult<ImportState> {
    let parts: Vec<StoredPart> = client.list_record_fields(
        "catalog_parts",
        "",
        "ipn",
        "id,ipn,ipn_prefix,variant,current_revision,current_revision_number,first_change",
    )?;
    let entries: Vec<LatestImportFields> = client.list_record_fields(
        "catalog_audit_entries",
        "csv_import_number > 0",
        "csv_import_number",
        "id,csv_file,csv_import_number,source_commit,file_hashes",
    )?;
    let cad_item_revisions: Vec<CadItemRevisionNumber> = client.list_record_fields(
        "catalog_cad_item_revisions",
        "",
        "cad_item,number",
        "cad_item,number,snapshot_sha256",
    )?;
    // Sorted by number, so the newest revision of each CAD item comes last.
    let mut current_cad_item_revisions = BTreeMap::new();
    for revision in cad_item_revisions {
        current_cad_item_revisions.insert(
            revision.cad_item,
            CurrentCadItemRevision {
                number: revision.number,
                snapshot_sha256: revision.snapshot_sha256,
            },
        );
    }
    let mut latest: BTreeMap<String, LatestImport> = BTreeMap::new();
    for entry in entries {
        let file_hashes = serde_json::from_str(&entry.file_hashes).map_err(|error| {
            CollabError::Protocol(format!(
                "import run {} records no readable file hashes: {error}",
                entry.csv_import_number
            ))
        })?;
        latest.insert(
            entry.csv_file.clone(),
            LatestImport {
                csv_file: entry.csv_file,
                csv_import_number: entry.csv_import_number,
                source_commit: entry.source_commit,
                audit_entry: entry.id,
                file_hashes,
            },
        );
    }
    let mut latest_imports: Vec<LatestImport> = latest.into_values().collect();
    latest_imports.sort_by_key(|run| run.csv_import_number);
    Ok(ImportState {
        current_revisions: parts
            .into_iter()
            .map(|part| (part.ipn, part.current_revision_number))
            .collect(),
        current_cad_item_revisions,
        latest_imports,
    })
}

/// What the import route answered.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportReply {
    pub status: u16,
    pub outcome: ImportOutcome,
    /// The answer was replayed from the correlation-ID ledger.
    pub replayed: bool,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ImportOutcome {
    /// 200: one Change with at least one revision, plus an audit entry for
    /// the import run that also holds its new import problems.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
        created: Counts,
        unchanged: Value,
    },
    /// 200: no part details changed, so there is no Change, but the run read
    /// other CSV files than the last import run of this CSV file, or found
    /// new import problems (for example a row whose supplier data changed).
    /// The run's audit entry records the files, and the problems point to it.
    #[serde(rename_all = "camelCase")]
    NothingChanged {
        correlation_id: String,
        audit_entry_id: String,
        problems_recorded: u64,
        unchanged: Value,
    },
    /// 422: everything was already stored and the last import run of this
    /// CSV file read the same files; nothing was written.
    #[serde(rename_all = "camelCase")]
    Noop {
        correlation_id: String,
        unchanged: Value,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
    /// A stored record has a derived ID of this request but other content.
    /// Nothing was written.
    #[serde(rename_all = "camelCase")]
    IdCollision {
        correlation_id: String,
        collisions: Vec<String>,
    },
    /// 409: some parts' or CAD items' current revisions are not the ones
    /// the importer read, because someone changed them in between. Nothing
    /// was written; read the team catalog again and import again.
    #[serde(rename_all = "camelCase")]
    Conflict {
        correlation_id: String,
        conflicts: Vec<ImportConflict>,
        #[serde(default)]
        cad_item_conflicts: Vec<CadItemConflict>,
        message: String,
    },
    /// 409: another import of this CSV file landed after the importer read
    /// the team catalog, so its checkout check is out of date. Nothing was
    /// written; read the team catalog again and import again.
    #[serde(rename_all = "camelCase")]
    NewerImportRun {
        correlation_id: String,
        csv_file: String,
        expected_csv_import: u64,
        latest_csv_import: u64,
        message: String,
    },
}

/// One part whose current revision changed after the importer read it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportConflict {
    pub ipn: String,
    /// 0 when the importer expected no such part.
    pub expected_revision: u32,
    /// The part's current revision; `None` if there is no such part.
    pub current_revision: Option<ConflictingRevision>,
}

/// One CAD item whose current revision changed after the importer read it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CadItemConflict {
    /// `symbol`, `footprint` or `model-3d`.
    pub kind: String,
    pub key: String,
    /// 0 when the importer expected no such CAD item.
    pub expected_revision: u32,
    /// 0 when there is no such CAD item.
    pub current_revision: u32,
}

/// What one import created.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub blobs: u64,
    pub cad_items: u64,
    pub cad_item_revisions: u64,
    /// New parts, each with revision 1.
    pub parts: u64,
    /// Every new part revision: one per new part plus one per updated part.
    pub part_revisions: u64,
    /// Existing parts that got a new revision.
    pub updated_parts: u64,
    pub problems: u64,
}

/// Submits `command`. Every answer in [`ImportOutcome`] is `Ok`; inspect it.
pub fn submit_import(
    client: &PocketBaseClient,
    command: &ImportCommand<'_>,
) -> CollabResult<ImportReply> {
    let body = command.body().to_string();
    let size = body.len();
    if size > IMPORT_BODY_LIMIT {
        return Err(CollabError::Protocol(format!(
            "the import of {} is {size} bytes, more than the {IMPORT_BODY_LIMIT} bytes the import route accepts in one request; nothing was sent",
            command.import.csv_file
        )));
    }
    let reply = client.post_json_text(CSV_IMPORT_ROUTE, &body)?;
    match reply.status {
        200 | 409 | 422 => {
            let outcome: ImportOutcome = serde_json::from_str(&reply.body)
                .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
            Ok(ImportReply {
                status: reply.status,
                outcome,
                replayed: reply.replayed,
                body: reply.body,
            })
        }
        _ => Err(reply.success().err().unwrap_or_else(|| {
            CollabError::Protocol(format!("unexpected status {}", reply.status))
        })),
    }
}

/// The route that publishes a change to one part's details.
pub const EDIT_ROUTE: &str = "/api/catalog/edits";

/// The route that publishes a copy of an earlier revision as a new one.
pub const RESTORE_ROUTE: &str = "/api/catalog/restores";

/// A change to one part's details: every field of the current revision (the
/// same names in the same order, with new values) and the full list of
/// manufacturer parts. The server reads the new CAD references from the
/// `Symbol` and `Footprint` fields and binds exact CAD item revisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditCommand {
    pub correlation_id: CorrelationId,
    pub reason: String,
    pub ipn: String,
    /// The number of the revision the edit starts from.
    pub expected_revision: u32,
    pub fields: Vec<DetailField>,
    pub manufacturer_parts: Vec<ManufacturerPart>,
}

impl EditCommand {
    /// An edit of `current` (the part's current revision, numbered
    /// `expected_revision`) with a fresh correlation ID, starting from its
    /// details unchanged.
    pub fn starting_from(current: &PartDetails, expected_revision: u32, reason: &str) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            reason: reason.to_owned(),
            ipn: current.ipn.clone(),
            expected_revision,
            fields: current.fields.clone(),
            manufacturer_parts: current.manufacturer_parts.clone(),
        }
    }

    /// Sets the value of the field named exactly `name`. Returns false if
    /// the details have no such field.
    pub fn set_field(&mut self, name: &str, value: &str) -> bool {
        match self.fields.iter_mut().find(|field| field.name == name) {
            Some(field) => {
                field.value = value.to_owned();
                true
            }
            None => false,
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "ipn": self.ipn,
            "expectedRevision": self.expected_revision,
            "fields": self.fields.iter().map(|field| json!({
                "name": field.name,
                "value": field.value,
            })).collect::<Vec<_>>(),
            "manufacturerParts": self.manufacturer_parts.iter().map(|part| json!({
                "manufacturer": part.manufacturer,
                "mpn": part.mpn,
            })).collect::<Vec<_>>(),
        })
    }
}

/// A new revision of one part whose details are a copy of revision
/// `restore_revision`. Each CAD binding names the CAD item's current
/// revision if that revision has the same content, and otherwise keeps the
/// exact older revision, so the part gets back exactly the CAD it had. A
/// restore never adds a CAD item revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreCommand {
    pub correlation_id: CorrelationId,
    pub reason: String,
    pub ipn: String,
    pub expected_revision: u32,
    pub restore_revision: u32,
}

impl RestoreCommand {
    pub fn new(ipn: &str, expected_revision: u32, restore_revision: u32, reason: &str) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            reason: reason.to_owned(),
            ipn: ipn.to_owned(),
            expected_revision,
            restore_revision,
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "ipn": self.ipn,
            "expectedRevision": self.expected_revision,
            "restoreRevision": self.restore_revision,
        })
    }
}

/// What the edit or restore route answered.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishReply {
    pub status: u16,
    pub outcome: PublishOutcome,
    /// The answer was replayed from the correlation-ID ledger.
    pub replayed: bool,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PublishOutcome {
    /// 200: one Change holding the new revision, the part moved to it, and
    /// one audit entry.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        ipn: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
        revision: PublishedRevision,
        /// The exact CAD item revisions the new revision binds.
        cad_item_revisions: Vec<String>,
    },
    /// 422: the details would not change; nothing was written.
    #[serde(rename_all = "camelCase")]
    Noop {
        correlation_id: String,
        ipn: String,
        revision: RevisionNumber,
        message: String,
    },
    /// 409: someone else published a revision first. Nothing was written.
    #[serde(rename_all = "camelCase")]
    Conflict {
        correlation_id: String,
        ipn: String,
        expected_revision: u32,
        current_revision: ConflictingRevision,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublishedRevision {
    pub id: String,
    pub number: u32,
    pub snapshot_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RevisionNumber {
    pub id: String,
    pub number: u32,
}

/// The part's current revision when a command expected another one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictingRevision {
    pub id: String,
    pub number: u32,
    /// Its part details as canonical JSON.
    pub snapshot: String,
    pub snapshot_sha256: String,
    pub change_kind: String,
    pub reason: String,
    /// `<auth collection>:<record id>` of whoever published it.
    #[serde(default)]
    pub actor: String,
    pub actor_name: String,
    pub created: String,
}

impl ConflictingRevision {
    pub fn details(&self) -> CollabResult<PartDetails> {
        serde_json::from_str(&self.snapshot)
            .map_err(|error| CollabError::Protocol(format!("revision {}: {error}", self.number)))
    }
}

/// Submits an edit. Applied, no-op, conflict and correlation-reuse answers
/// are `Ok`; inspect the outcome. Bad input (400), 401, 403 and 404 are
/// errors.
pub fn submit_edit(client: &PocketBaseClient, command: &EditCommand) -> CollabResult<PublishReply> {
    publish(client, EDIT_ROUTE, &command.body())
}

/// Submits a restore; answers as [`submit_edit`].
pub fn submit_restore(
    client: &PocketBaseClient,
    command: &RestoreCommand,
) -> CollabResult<PublishReply> {
    publish(client, RESTORE_ROUTE, &command.body())
}

fn publish(client: &PocketBaseClient, route: &str, body: &Value) -> CollabResult<PublishReply> {
    let reply = client.post_json(route, body)?;
    match reply.status {
        200 | 409 | 422 => {
            let outcome: PublishOutcome = serde_json::from_str(&reply.body)
                .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
            Ok(PublishReply {
                status: reply.status,
                outcome,
                replayed: reply.replayed,
                body: reply.body,
            })
        }
        _ => Err(reply.success().err().unwrap_or_else(|| {
            CollabError::Protocol(format!("unexpected status {}", reply.status))
        })),
    }
}

/// The part with `ipn`, if the Catalog has it.
pub fn part_by_ipn(client: &PocketBaseClient, ipn: &str) -> CollabResult<Option<StoredPart>> {
    let filter = format!("ipn = {}", Value::from(ipn));
    Ok(client
        .list_records("catalog_parts", &filter, "ipn")?
        .into_iter()
        .next())
}

/// Every revision of the part with ID `part_id`, oldest first.
pub fn part_revisions(
    client: &PocketBaseClient,
    part_id: &str,
) -> CollabResult<Vec<StoredPartRevision>> {
    let filter = format!("part = {}", Value::from(part_id));
    client.list_records("catalog_part_revisions", &filter, "number")
}

/// `id = "a" || id = "b" || ...` filters of at most 40 IDs each, which keeps
/// request URLs short.
fn id_filters(ids: &[&str]) -> Vec<String> {
    ids.chunks(40)
        .map(|chunk| {
            chunk
                .iter()
                .map(|id| format!("id = {}", Value::from(*id)))
                .collect::<Vec<_>>()
                .join(" || ")
        })
        .collect()
}

/// The Changes with these IDs, in `seq` order.
pub fn changes_by_id(client: &PocketBaseClient, ids: &[&str]) -> CollabResult<Vec<StoredChange>> {
    let mut changes = Vec::new();
    for filter in id_filters(ids) {
        changes.extend(client.list_records::<StoredChange>("catalog_changes", &filter, "seq")?);
    }
    changes.sort_by_key(|change| change.seq);
    Ok(changes)
}

/// The CAD item revisions with these IDs.
pub fn cad_item_revisions_by_id(
    client: &PocketBaseClient,
    ids: &[&str],
) -> CollabResult<Vec<StoredCadItemRevision>> {
    let mut revisions = Vec::new();
    for filter in id_filters(ids) {
        revisions.extend(client.list_records::<StoredCadItemRevision>(
            "catalog_cad_item_revisions",
            &filter,
            "cad_item,number",
        )?);
    }
    Ok(revisions)
}

/// Every Change after `checkpoint`, in `seq` order.
pub fn changes_since(
    client: &PocketBaseClient,
    checkpoint: u64,
) -> CollabResult<Vec<StoredChange>> {
    client.list_records("catalog_changes", &format!("seq > {checkpoint}"), "seq")
}

/// The part revisions one Change published.
pub fn revisions_in_change(
    client: &PocketBaseClient,
    change_id: &str,
) -> CollabResult<Vec<StoredPartRevision>> {
    let filter = format!("change = {}", Value::from(change_id));
    client.list_records("catalog_part_revisions", &filter, "part,number")
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredChange {
    pub id: String,
    pub seq: u64,
    pub kind: String,
    pub reason: String,
    pub correlation_id: String,
    pub actor: String,
    /// The actor's name or email when the Change was made.
    #[serde(default)]
    pub actor_name: String,
    /// When the Change was saved, as PocketBase writes dates.
    #[serde(default)]
    pub created: String,
    pub source_commit: String,
    /// JSON: `{ csv_file, files: [SourceFile] }`.
    pub sources: String,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredPart {
    pub id: String,
    pub ipn: String,
    pub ipn_prefix: String,
    pub variant: String,
    pub current_revision: String,
    pub current_revision_number: u32,
    pub first_change: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredPartRevision {
    pub id: String,
    pub part: String,
    pub number: u32,
    pub predecessor: String,
    pub change: String,
    pub snapshot: String,
    pub snapshot_sha256: String,
    pub source: String,
    pub cad_item_revisions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredCadItem {
    pub id: String,
    pub kind: String,
    pub key: String,
    pub first_change: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredCadItemRevision {
    pub id: String,
    pub cad_item: String,
    pub number: u32,
    pub predecessor: String,
    pub change: String,
    pub blob: String,
    pub snapshot_sha256: String,
    pub snapshot: String,
    pub license: String,
    pub source: String,
}

#[derive(Clone, Deserialize, PartialEq)]
pub struct StoredBlob {
    pub id: String,
    pub sha256: String,
    pub size: u64,
    pub file: String,
    pub first_change: String,
    /// The stored file's bytes, downloaded by [`read_back`].
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for StoredBlob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredBlob")
            .field("id", &self.id)
            .field("sha256", &self.sha256)
            .field("size", &self.size)
            .field("downloaded", &self.bytes.len())
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredImportProblem {
    pub id: String,
    pub code: String,
    pub subject: String,
    pub path: String,
    pub location: String,
    pub message: String,
    pub detail: String,
    /// The audit entry of the import run that first found it.
    pub audit_entry: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredAuditEntry {
    pub id: String,
    pub action: String,
    pub actor: String,
    /// The actor's name or email when the entry was made.
    #[serde(default)]
    pub actor_name: String,
    /// The Change the run made, or empty if it made no revision.
    pub change: String,
    pub reason: String,
    pub correlation_id: String,
    /// JSON counts of what the run created.
    pub summary: String,
    /// For a CSV import run: the CSV file it imported, such as `g-res`.
    #[serde(default)]
    pub csv_file: String,
    /// For a CSV import run: its number among all CSV import runs, counting
    /// from 1. 0 for edits and restores.
    #[serde(default)]
    pub csv_import_number: u64,
    /// For a CSV import run: the commit its files were read at.
    #[serde(default)]
    pub source_commit: String,
    /// For a CSV import run: JSON [`ImportRun`], exactly what the CSV said.
    #[serde(default)]
    pub sources: String,
    /// For a CSV import run: JSON `[[path, byte_sha256], ...]` of the files
    /// it read, sorted by path.
    #[serde(default)]
    pub file_hashes: String,
    /// The importer was told to import from a checkout older than the team
    /// catalog's latest imports.
    #[serde(default)]
    pub allowed_older_checkout: bool,
}

impl StoredAuditEntry {
    /// What this CSV import run read, or `None` if the entry records no
    /// readable import run (edits, restores, or a damaged record).
    pub fn import_run(&self) -> Option<ImportRun> {
        serde_json::from_str(&self.sources).ok()
    }
}

/// What one CSV import run read and what it left each part at, as its audit
/// entry records it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ImportRun {
    pub csv_file: String,
    /// Every file read. The CSV files carry their header line and every
    /// row, so the header followed by the rows is the file, byte for byte.
    pub files: Vec<SourceFile>,
    /// Every part a row of the CSV file made, in row order, with the number
    /// of its current revision just after the run.
    pub parts: Vec<ImportedPart>,
    /// The IDs of every import problem the run found, new or already
    /// stored. Problems are shared by content, so only these explain the
    /// run's rows.
    #[serde(default)]
    pub problems: Vec<String>,
    /// Parts that earlier runs of this CSV file made but no row makes now.
    #[serde(default)]
    pub removed_parts: Vec<String>,
}

impl ImportRun {
    pub fn file(&self, path: &str) -> Option<&SourceFile> {
        self.files.iter().find(|file| file.path == path)
    }
}

/// A part that a row of an import run's CSV file made, and the number of
/// its current revision just after that run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ImportedPart {
    pub ipn: String,
    pub revision: u32,
}

/// Every Catalog record as stored, with each blob's bytes downloaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogReadback {
    pub changes: Vec<StoredChange>,
    pub parts: Vec<StoredPart>,
    pub part_revisions: Vec<StoredPartRevision>,
    pub cad_items: Vec<StoredCadItem>,
    pub cad_item_revisions: Vec<StoredCadItemRevision>,
    pub blobs: Vec<StoredBlob>,
    pub problems: Vec<StoredImportProblem>,
    pub audit_entries: Vec<StoredAuditEntry>,
}

impl CatalogReadback {
    pub fn part(&self, ipn: &str) -> Option<&StoredPart> {
        self.parts.iter().find(|part| part.ipn == ipn)
    }

    pub fn part_revision(&self, id: &str) -> Option<&StoredPartRevision> {
        self.part_revisions
            .iter()
            .find(|revision| revision.id == id)
    }

    pub fn cad_item(&self, id: &str) -> Option<&StoredCadItem> {
        self.cad_items.iter().find(|cad_item| cad_item.id == id)
    }

    pub fn cad_item_revision(&self, id: &str) -> Option<&StoredCadItemRevision> {
        self.cad_item_revisions
            .iter()
            .find(|revision| revision.id == id)
    }

    pub fn blob(&self, id: &str) -> Option<&StoredBlob> {
        self.blobs.iter().find(|blob| blob.id == id)
    }

    /// The number of records in each Catalog collection (blobs counted
    /// once each), for idempotency checks.
    pub fn counts(&self) -> [usize; 8] {
        [
            self.changes.len(),
            self.parts.len(),
            self.part_revisions.len(),
            self.cad_items.len(),
            self.cad_item_revisions.len(),
            self.blobs.len(),
            self.problems.len(),
            self.audit_entries.len(),
        ]
    }
}

/// Reads every Catalog record back, in a fixed order, and downloads every
/// blob. Fails if a stored file's bytes do not hash to its `sha256`.
pub fn read_back(client: &PocketBaseClient) -> CollabResult<CatalogReadback> {
    let mut readback = CatalogReadback {
        changes: client.list_records("catalog_changes", "", "seq")?,
        parts: client.list_records("catalog_parts", "", "ipn")?,
        part_revisions: client.list_records("catalog_part_revisions", "", "part,number")?,
        cad_items: client.list_records("catalog_cad_items", "", "kind,key")?,
        cad_item_revisions: client.list_records(
            "catalog_cad_item_revisions",
            "",
            "cad_item,number",
        )?,
        blobs: client.list_records("catalog_blobs", "", "sha256")?,
        problems: client.list_records("catalog_import_problems", "", "code,location,id")?,
        audit_entries: client.list_records("catalog_audit_entries", "", "id")?,
    };
    if !readback.blobs.is_empty() {
        let token = client.file_token()?;
        for blob in &mut readback.blobs {
            blob.bytes =
                client.download_file("catalog_blobs", &blob.id, &blob.file, Some(&token))?;
            if sha256_hex(&blob.bytes) != blob.sha256 || blob.bytes.len() as u64 != blob.size {
                return Err(CollabError::Protocol(format!(
                    "stored blob {} does not match its SHA-256 or size",
                    blob.sha256
                )));
            }
        }
    }
    Ok(readback)
}

/// Standard base64 with padding, as the import route decodes it.
pub fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648() {
        for (input, output) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), output);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn outcomes_parse() {
        let applied: ImportOutcome = serde_json::from_str(
            r#"{"outcome":"applied","correlationId":"c","changeId":"x","seq":1,"auditEntryId":"a","created":{"blobs":2,"cadItems":2,"cadItemRevisions":2,"parts":29,"partRevisions":29,"updatedParts":0,"problems":1},"unchanged":{}}"#,
        )
        .unwrap();
        assert!(matches!(applied, ImportOutcome::Applied { seq: 1, .. }));
        let noop: ImportOutcome =
            serde_json::from_str(r#"{"outcome":"noop","correlationId":"c","unchanged":{}}"#)
                .unwrap();
        assert!(matches!(noop, ImportOutcome::Noop { .. }));
        let nothing: ImportOutcome = serde_json::from_str(
            r#"{"outcome":"nothing_changed","correlationId":"c","auditEntryId":"a","problemsRecorded":1,"unchanged":{}}"#,
        )
        .unwrap();
        assert!(matches!(
            nothing,
            ImportOutcome::NothingChanged {
                problems_recorded: 1,
                ..
            }
        ));
        let conflict: ImportOutcome = serde_json::from_str(
            r#"{"outcome":"conflict","correlationId":"c","conflicts":[{"ipn":"RES-0000-0470","expectedRevision":1,"currentRevision":{"id":"r","number":2,"snapshot":"{}","snapshotSha256":"s","changeKind":"edit","reason":"x","actor":"users:u","actorName":"A","created":"t"}},{"ipn":"RES-0000-9999","expectedRevision":3,"currentRevision":null}],"message":"m"}"#,
        )
        .unwrap();
        let ImportOutcome::Conflict { conflicts, .. } = conflict else {
            panic!("not a conflict");
        };
        assert_eq!(
            conflicts
                .iter()
                .map(|c| (
                    c.expected_revision,
                    c.current_revision.as_ref().map(|r| r.number)
                ))
                .collect::<Vec<_>>(),
            [(1, Some(2)), (3, None)]
        );
    }

    #[test]
    fn blobs_go_as_text_when_they_are_utf8_and_as_base64_otherwise() {
        let text = blob_body(&Blob::new("ISO-10303-21;\né\n".into()));
        assert_eq!(text["text"], "ISO-10303-21;\né\n");
        assert_eq!(text["size"], 17);
        assert!(text.get("base64").is_none());
        let binary = blob_body(&Blob::new(vec![0xff, 0xfe, 0x00]));
        assert_eq!(binary["base64"], "//4A");
        assert!(binary.get("text").is_none());
    }

    #[test]
    fn an_import_over_the_size_limit_is_not_sent() {
        let root = tempfile::tempdir().unwrap();
        for directory in ["database", "footprints/g-big.pretty", "3d-models"] {
            std::fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        std::fs::write(
            root.path().join("database/g-big.csv"),
            "IPN,Footprint\nBIG-0000-0001,g-big:Big\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("database/substitutes.csv"),
            "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("footprints/g-big.pretty/Big.kicad_mod"),
            "(footprint \"Big\" (model \"${GITPLM_PARTS}/3d-models/big.step\"))",
        )
        .unwrap();
        std::fs::write(
            root.path().join("3d-models/big.step"),
            "x".repeat(IMPORT_BODY_LIMIT),
        )
        .unwrap();
        let import = crate::CsvCatalog::new(root.path())
            .import("g-big", None)
            .unwrap();
        // Nothing listens on this port; the size check comes first.
        let client = PocketBaseClient::new("http://127.0.0.1:9");
        let error = submit_import(&client, &ImportCommand::new(&import, "r")).unwrap_err();
        assert!(error.to_string().contains("nothing was sent"), "{error}");
    }

    #[test]
    fn import_commands_send_rows_and_expected_revisions() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("database")).unwrap();
        std::fs::write(
            root.path().join("database/g-res.csv"),
            "IPN,Value\nRES-0000-0470,47\nRES-0000-1000,100\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("database/substitutes.csv"),
            "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n",
        )
        .unwrap();
        let import = crate::CsvCatalog::new(root.path())
            .import("g-res", None)
            .unwrap();
        let state = ImportState {
            current_revisions: [("RES-0000-1000".to_owned(), 4)].into(),
            ..ImportState::default()
        };
        let body = ImportCommand::new(&import, "r").expecting(&state).body();
        let expected: Vec<(&str, u64)> = body["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| {
                (
                    part["ipn"].as_str().unwrap(),
                    part["expectedRevision"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(expected, [("RES-0000-0470", 0), ("RES-0000-1000", 4)]);
        assert_eq!(body["allowOlderCheckout"], false);
        let csv = body["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["role"] == "catalog-csv")
            .unwrap();
        assert_eq!(
            csv["rows"],
            json!(["RES-0000-0470,47\n", "RES-0000-1000,100\n"])
        );
        let substitutes = body["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["role"] == "substitutes-csv")
            .unwrap();
        assert_eq!(substitutes["rows"], json!([]));
    }
}

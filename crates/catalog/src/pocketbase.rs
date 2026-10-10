//! The Catalog's PocketBase contract: the protected routes
//! (`infra/pocketbase/pb_hooks/catalog_csv_imports.pb.js` for CSV imports,
//! `catalog_edits.pb.js` for edits and restores) and the `catalog_*`
//! collections (`infra/pocketbase/pb_migrations/1791500000_catalog_init.js`
//! and `1791600000_catalog_edits.js`).

use crate::csv_import::CsvImport;
use crate::part::{DetailField, ManufacturerPart, PartDetails};
use crate::sha256_hex;
use collaboration::{CollabError, CollabResult, CorrelationId, PocketBaseClient};
use serde::Deserialize;
use serde_json::{Value, json};

/// The route that publishes a CSV catalog import.
pub const CSV_IMPORT_ROUTE: &str = "/api/catalog/csv-imports";

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
    pub import: &'a CsvImport,
}

impl<'a> ImportCommand<'a> {
    /// A command whose correlation ID is derived from the import itself, so
    /// retrying the same import replays the stored answer.
    pub fn new(import: &'a CsvImport, reason: &str) -> Self {
        Self {
            correlation_id: format!("csv-import-{}", &import.fingerprint()[..32]),
            reason: reason.to_owned(),
            import,
        }
    }

    pub fn body(&self) -> Value {
        let import = self.import;
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "csvFile": import.csv_file,
            "sourceCommit": import.source_commit,
            "sources": import.sources.iter().map(|source| json!({
                "role": source.role,
                "path": source.path,
                "byteSha256": source.byte_sha256,
                "size": source.size,
                "headerRaw": source.header_raw,
            })).collect::<Vec<_>>(),
            "blobs": import.blobs.iter().map(|blob| json!({
                "sha256": blob.sha256,
                "size": blob.bytes.len(),
                "base64": base64(&blob.bytes),
            })).collect::<Vec<_>>(),
            "cadItemRevisions": import.cad_item_revisions.iter().map(|revision| json!({
                "id": revision.id,
                "cadItemId": revision.cad_item.id,
                "kind": revision.cad_item.kind.as_str(),
                "key": revision.cad_item.key,
                "snapshotSha256": revision.snapshot_sha256,
                "snapshot": revision.snapshot_json,
                "source": revision.source_json,
            })).collect::<Vec<_>>(),
            "parts": import.parts.iter().map(|part| json!({
                "id": part.id,
                "ipn": part.ipn.as_str(),
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
    /// 200: nothing in the catalog changed, so there is no Change, but the
    /// run found new import problems (for example a row whose supplier data
    /// changed). They are recorded against the run's audit entry.
    #[serde(rename_all = "camelCase")]
    NothingChanged {
        correlation_id: String,
        audit_entry_id: String,
        problems_recorded: u64,
        unchanged: Value,
    },
    /// 422: everything was already stored; nothing was written.
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
    let reply = client.post_json(CSV_IMPORT_ROUTE, &command.body())?;
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

/// A new revision of one part whose details are an exact copy of revision
/// `restore_revision`, CAD item revisions included.
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
    /// The Change the run made, or empty if it only recorded import problems.
    pub change: String,
    pub reason: String,
    pub correlation_id: String,
    /// JSON counts of what the run created.
    pub summary: String,
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
    }
}

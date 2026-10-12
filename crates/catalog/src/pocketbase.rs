//! The Catalog's PocketBase contract: the protected routes
//! (`infra/pocketbase/pb_hooks/catalog_csv_imports.pb.js` for CSV imports,
//! `catalog_edits.pb.js` for edits) and the `catalog_*`
//! collections (`infra/pocketbase/pb_migrations/1791500000_catalog_init.js`
//! `1791600000_catalog_edits.js`, `1791800000_shared_part_details.js`,
//! `1791900000_archived_revisions.js`, `1792000000_csv_reviews.js`,
//! `1792100000_uncommitted_changes.js` and
//! `1792200000_cad_item_revision_successor.js`).

use crate::cad_item::Blob;
use crate::cad_item::{CadItemDetails, ModelTarget};
use crate::csv_import::{
    Binding, CsvImport, CurrentCad, CurrentCadItemRevision, EditedInApp, LastImported, SourceFile,
};
use crate::part::{DetailField, ManufacturerPart, PartDetails, PartRevision};
use crate::sha256_hex;
use collaboration::{CollabError, CollabResult, CorrelationId, PocketBaseClient};
use serde::Deserialize;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

/// The route that publishes a CSV catalog import.
pub const CSV_IMPORT_ROUTE: &str = "/api/catalog/csv-imports";

/// The largest import request the route accepts, in bytes (32 MiB, set by
/// `IMPORT_BODY_LIMIT` in `catalog_csv_imports.pb.js`). An import is one
/// request so that it is one transaction; the largest import of the CSV
/// catalog today (g-con, with a 1.5 MiB 3D model) is 1.8 MiB (1,900,969 bytes).
pub const IMPORT_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Every Catalog collection, in dependency order.
pub const COLLECTIONS: [&str; 12] = [
    "catalog_changes",
    "catalog_blobs",
    "catalog_cad_items",
    "catalog_cad_item_revisions",
    "catalog_parts",
    "catalog_part_details",
    "catalog_part_revisions",
    "catalog_archive_entries",
    "catalog_csv_reviews",
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
    /// next four fields are private so that only [`ImportCommand::expecting`],
    /// [`ImportCommand::allowing_older_checkout`] and
    /// [`ImportCommand::allowing_uncommitted_changes`] set them, which derive
    /// the correlation ID again. If any part's
    /// current revision differs when the import arrives, the route answers
    /// 409 conflict and writes nothing. See [`ImportCommand::expecting`].
    expected_revisions: BTreeMap<String, u32>,
    /// The number of the current part details the importer read for each
    /// IPN, checked like `expected_revisions`.
    expected_details: BTreeMap<String, u32>,
    /// The number of the latest import run of this CSV file that the
    /// importer read (0 if none). If another import of the file lands
    /// first, the route answers 409 and writes nothing, because the checkout
    /// check was made against an older state.
    expected_csv_import: u64,
    /// Import even though the checkout does not include the team catalog's
    /// latest imports. The run's audit entry records it.
    allow_older_checkout: bool,
    /// Import even though the files have changes that are not committed
    /// (the source commit ends in `+dirty`). The route refuses such an
    /// import without it, and the run's audit entry records it.
    allow_uncommitted_changes: bool,
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
            expected_details: BTreeMap::new(),
            expected_csv_import: 0,
            allow_older_checkout: false,
            allow_uncommitted_changes: false,
        };
        command.correlation_id = command.derived_correlation_id();
        command
    }

    /// Expects each part's current revision and current details, each CAD
    /// item's current revision, and the latest import run of this CSV file,
    /// to be the ones in `state`. Numbers the CAD item revisions against `state` (see
    /// [`CsvImport::numbered_against`]): a binding keeps its number while its
    /// content is the same, and otherwise names the CAD item's current
    /// revision. Derives the correlation ID again.
    pub fn expecting(mut self, state: &ImportState) -> Self {
        self.import = Cow::Owned(self.import.numbered_against(&state.current_cad));
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
        self.expected_details = self
            .import
            .parts
            .iter()
            .filter_map(|part| {
                let ipn = part.ipn.as_str();
                let number = *state.current_details.get(ipn)?;
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

    /// The current details number expected for each IPN; a part missing
    /// here is expected to be new.
    pub fn expected_details(&self) -> &BTreeMap<String, u32> {
        &self.expected_details
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
    ///
    /// Older CAD content then becomes the current revision of its CAD item
    /// (a new number), so every part that uses that symbol, footprint or 3D
    /// model, in any CSV file, binds the older content when it is next
    /// imported, and KiCad Integration writes it. No CAD item is shared
    /// between CSV files today.
    pub fn allowing_older_checkout(mut self, allow: bool) -> Self {
        self.allow_older_checkout = allow;
        self.correlation_id = self.derived_correlation_id();
        self
    }

    /// Whether the import is allowed from files with uncommitted changes.
    pub fn allow_uncommitted_changes(&self) -> bool {
        self.allow_uncommitted_changes
    }

    /// Imports even from files with changes that are not committed, if
    /// `allow`. Derives the correlation ID again.
    ///
    /// Until those changes are committed and imported again, a later
    /// import from a checkout without them undoes them, like an import from
    /// an older checkout. The route refuses this setting when the source
    /// commit does not end in `+dirty`.
    pub fn allowing_uncommitted_changes(mut self, allow: bool) -> Self {
        self.allow_uncommitted_changes = allow;
        self.correlation_id = self.derived_correlation_id();
        self
    }

    /// `csv-import-` and 32 hex digits of a hash of everything the route
    /// keys the command on: the import, the expected revisions, the
    /// expected import run and the two settings that allow it anyway. The same import
    /// sent again after the catalog moved on gets a new ID, so it is
    /// compared afresh rather than refused as a reused ID.
    fn derived_correlation_id(&self) -> String {
        let mut key = self.import.fingerprint();
        for (ipn, number) in &self.expected_revisions {
            key.push_str(&format!("\n{ipn}={number}"));
        }
        for (ipn, number) in &self.expected_details {
            key.push_str(&format!("\n{ipn} details={number}"));
        }
        key.push_str(&format!(
            "\nimport={}\nallow-older-checkout={}\nallow-uncommitted-changes={}",
            self.expected_csv_import, self.allow_older_checkout, self.allow_uncommitted_changes
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
            "allowUncommittedChanges": self.allow_uncommitted_changes,
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
                "expectedDetails": self.expected_details.get(part.ipn.as_str()).copied().unwrap_or(0),
                "details": {
                    "snapshot": part.details_json,
                    "snapshotSha256": part.details_sha256,
                },
                "revision": {
                    "snapshot": part.revision_json,
                    "snapshotSha256": part.revision_sha256,
                },
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
    /// Each part's current details number, by IPN.
    pub current_details: BTreeMap<String, u32>,
    /// Each CAD item's current revision (its newest), and the CAD item
    /// revisions each part's current revision binds.
    pub current_cad: CurrentCad,
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
}

/// The few fields of an audit entry that [`read_import_state`] downloads.
#[derive(Deserialize)]
struct LatestImportFields {
    id: String,
    csv_file: String,
    csv_import_number: u64,
    #[serde(default)]
    source_commit: String,
}

/// The few fields of a CAD item revision that [`read_import_state`]
/// downloads (its snapshot is small: it names the blob, not its bytes).
#[derive(Deserialize)]
struct CadItemRevisionFields {
    id: String,
    cad_item: String,
    number: u32,
    snapshot_sha256: String,
    snapshot: String,
}

/// The fields of [`CadItemRevisionFields`], for `fields=`.
const CAD_ITEM_REVISION_FIELDS: &str = "id,cad_item,number,snapshot_sha256,snapshot";

/// The few fields of a part revision that [`read_import_state`] downloads.
#[derive(Deserialize)]
struct PartRevisionBindings {
    part: String,
    #[serde(default)]
    cad_item_revisions: Vec<String>,
}

/// Reads each part's current revision and details numbers and the CAD item
/// revisions its current revision binds, each CAD item's current revision,
/// and the latest import run of each CSV file. Downloads only these
/// records, and only the fields it needs: never older revisions (unless a
/// part's current revision binds one), earlier import runs, the recorded
/// rows or the stored bytes. So it downloads no more as history grows, and
/// the unique index on a CAD item revision's predecessor keeps finding the
/// newest revisions fast.
pub fn read_import_state(client: &PocketBaseClient) -> CollabResult<ImportState> {
    let parts: Vec<StoredPart> = client.list_record_fields(
        "catalog_parts",
        "",
        "ipn",
        "id,ipn,ipn_prefix,variant,current_revision,current_revision_number,current_details,current_details_number,first_change",
    )?;
    let current_cad = read_current_cad(client, &parts)?;
    let latest_imports = read_latest_imports(client)?;
    Ok(ImportState {
        current_revisions: parts
            .iter()
            .map(|part| (part.ipn.clone(), part.current_revision_number))
            .collect(),
        current_details: parts
            .into_iter()
            .map(|part| (part.ipn, part.current_details_number))
            .collect(),
        current_cad,
        latest_imports,
    })
}

/// The latest import run of each CSV file, oldest first. Goes through the
/// CSV files in name order, asking each time for the newest run of the
/// first file named after the one found last, so it reads one run per CSV
/// file however many runs there were, with a filter that stays short. (The
/// import route takes only plain CSV file names, letters, digits and
/// `._-`, so the name compares and quotes safely.)
fn read_latest_imports(client: &PocketBaseClient) -> CollabResult<Vec<LatestImport>> {
    let mut latest: Vec<LatestImport> = Vec::new();
    loop {
        let filter = match latest.last() {
            None => "csv_import_number > 0".to_owned(),
            Some(run) => format!(
                "csv_import_number > 0 && csv_file > {}",
                Value::from(run.csv_file.as_str())
            ),
        };
        let Some(entry) = client.first_record_fields::<LatestImportFields>(
            "catalog_audit_entries",
            &filter,
            "csv_file,-csv_import_number",
            "id,csv_file,csv_import_number,source_commit",
        )?
        else {
            break;
        };
        // A file not named after the last one would loop forever.
        if latest
            .last()
            .is_some_and(|run| entry.csv_file <= run.csv_file)
        {
            return Err(CollabError::Protocol(format!(
                "the team server listed import run {} of {} out of order",
                entry.csv_import_number, entry.csv_file
            )));
        }
        latest.push(LatestImport {
            csv_file: entry.csv_file,
            csv_import_number: entry.csv_import_number,
            source_commit: entry.source_commit,
            audit_entry: entry.id,
        });
    }
    latest.sort_by_key(|run| run.csv_import_number);
    Ok(latest)
}

/// Each CAD item's current revision, with a footprint's 3D model bindings,
/// and the CAD item revisions each part's current revision binds.
///
/// Reads three sets of records:
/// - each CAD item's current revision: its newest, the one that no other
///   revision names as its predecessor;
/// - each part's current revision, the one its part points to, for the IDs
///   of the CAD item revisions it binds;
/// - the bound CAD item revisions that are not current, by ID.
///
/// A footprint's snapshot names the 3D model revisions it binds, so they
/// need no read of their own.
fn read_current_cad(client: &PocketBaseClient, parts: &[StoredPart]) -> CollabResult<CurrentCad> {
    let mut current = CurrentCad::default();
    let mut by_id: BTreeMap<String, (String, Binding)> = BTreeMap::new();
    let newest: Vec<CadItemRevisionFields> = client.list_record_fields(
        "catalog_cad_item_revisions",
        "catalog_cad_item_revisions_via_predecessor.id = \"\"",
        "cad_item,number",
        CAD_ITEM_REVISION_FIELDS,
    )?;
    let mut current_bindings: BTreeMap<String, Binding> = BTreeMap::new();
    for revision in newest {
        let (cad_item, binding, models) = read_cad_item_revision(revision, &mut by_id)?;
        current_bindings.insert(cad_item.clone(), binding.clone());
        current.cad_items.insert(
            cad_item,
            CurrentCadItemRevision {
                number: binding.number,
                snapshot_sha256: binding.snapshot_sha256,
                models,
            },
        );
    }

    // The CAD items whose current revision was made in the app, and for
    // each, the newest revision a CSV import saved (kicad-parts-3b8b).
    let edited: Vec<CadItemRevisionFields> = client.list_record_fields(
        "catalog_cad_item_revisions",
        "catalog_cad_item_revisions_via_predecessor.id = \"\" && change.kind != \"csv-import\"",
        "cad_item,number",
        CAD_ITEM_REVISION_FIELDS,
    )?;
    for revision in edited {
        let Some(binding) = current_bindings.get(&revision.cad_item).cloned() else {
            continue;
        };
        let filter = format!(
            "cad_item = {} && change.kind = \"csv-import\"",
            Value::from(revision.cad_item.as_str())
        );
        let last_imported = client
            .first_record_fields::<CadItemRevisionFields>(
                "catalog_cad_item_revisions",
                &filter,
                "-number",
                CAD_ITEM_REVISION_FIELDS,
            )?
            .map(|last| {
                read_cad_item_revision(last, &mut by_id)
                    .map(|(_, binding, models)| LastImported { binding, models })
            })
            .transpose()?;
        current.edited_in_app.insert(
            revision.cad_item,
            EditedInApp {
                current: binding,
                last_imported,
            },
        );
    }

    let part_revisions: Vec<PartRevisionBindings> = client.list_record_fields(
        "catalog_part_revisions",
        "part.current_revision = id",
        "part,number",
        "part,cad_item_revisions",
    )?;
    let older: BTreeSet<&str> = part_revisions
        .iter()
        .flat_map(|revision| &revision.cad_item_revisions)
        .map(String::as_str)
        .filter(|id| !by_id.contains_key(*id))
        .collect();
    for filter in id_filters(&older.into_iter().collect::<Vec<_>>()) {
        let bound: Vec<CadItemRevisionFields> = client.list_record_fields(
            "catalog_cad_item_revisions",
            &filter,
            "cad_item,number",
            CAD_ITEM_REVISION_FIELDS,
        )?;
        for revision in bound {
            read_cad_item_revision(revision, &mut by_id)?;
        }
    }

    let ipns: BTreeMap<&str, &str> = parts
        .iter()
        .map(|part| (part.id.as_str(), part.ipn.as_str()))
        .collect();
    for revision in &part_revisions {
        let Some(ipn) = ipns.get(revision.part.as_str()) else {
            continue;
        };
        let bindings = revision
            .cad_item_revisions
            .iter()
            .filter_map(|id| by_id.get(id).cloned())
            .collect();
        current.parts.insert((*ipn).to_owned(), bindings);
    }
    Ok(current)
}

/// Reads one CAD item revision's snapshot. Gives its CAD item, its binding
/// (also kept in `by_id` under its ID) and, for a footprint, the 3D model
/// revisions it binds, by CAD item ID.
fn read_cad_item_revision(
    revision: CadItemRevisionFields,
    by_id: &mut BTreeMap<String, (String, Binding)>,
) -> CollabResult<(String, Binding, BTreeMap<String, Binding>)> {
    let details: CadItemDetails = serde_json::from_str(&revision.snapshot).map_err(|error| {
        CollabError::Protocol(format!(
            "CAD item revision {} cannot be read: {error}",
            revision.id
        ))
    })?;
    let binding = Binding {
        number: revision.number,
        snapshot_sha256: revision.snapshot_sha256,
        content_sha256: details.content_sha256(),
    };
    let models = details
        .model_references
        .into_iter()
        .filter_map(|model| match model.target {
            ModelTarget::CadItemRevision {
                cad_item,
                number,
                snapshot_sha256,
            } => Some((
                cad_item,
                Binding {
                    number,
                    // A 3D model names no other CAD item, so its content is
                    // its whole snapshot.
                    content_sha256: snapshot_sha256.clone(),
                    snapshot_sha256,
                },
            )),
            ModelTarget::Unresolved { .. } => None,
        })
        .collect();
    by_id.insert(revision.id, (revision.cad_item.clone(), binding.clone()));
    Ok((revision.cad_item, binding, models))
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
    /// 200: one Change with at least one revision or saved part details,
    /// plus an audit entry for the import run that also holds its new
    /// import problems.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
        created: Counts,
        unchanged: Value,
    },
    /// 200: nothing was saved, so there is no Change, but the run read
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
    /// 409: some parts' current revisions or details, or CAD items'
    /// current revisions, are not the ones the importer read, because
    /// someone changed them in between. Nothing was written; read the team
    /// catalog again and import again.
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

/// One part whose current revision or details changed after the importer
/// read it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportConflict {
    pub ipn: String,
    /// 0 when the importer expected no such part.
    pub expected_revision: u32,
    /// 0 when the importer expected no such part.
    pub expected_details: u32,
    /// The part's current revision; `None` if there is no such part.
    pub current_revision: Option<ConflictingRecord>,
    /// The part's current details; `None` if there is no such part.
    pub current_details: Option<ConflictingRecord>,
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
    /// New parts, each with details 1 and revision 1.
    pub parts: u64,
    /// Every new part revision: one per new part plus one per updated part.
    pub part_revisions: u64,
    /// Existing parts that got a new revision.
    pub updated_parts: u64,
    /// Every new saved part details: one per new part plus one per part
    /// with updated details.
    pub part_details: u64,
    /// Existing parts whose details were saved again.
    pub updated_details: u64,
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

/// The route that publishes a change to one part's details, symbol or
/// footprint.
pub const EDIT_ROUTE: &str = "/api/catalog/edits";

/// The largest edit request the route accepts, in bytes (32 MiB, set by
/// `EDIT_BODY_LIMIT` in `catalog_edits.pb.js`), files included.
/// [`submit_edit`] refuses a bigger one before sending it.
pub const EDIT_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// A change to one part: its details, its Symbol and Footprint, or both.
/// Each section carries the number it started from and is sent only when
/// something in it changed, so a details edit never conflicts with someone
/// else's change of the symbol or footprint, and the reverse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditCommand {
    pub correlation_id: CorrelationId,
    pub reason: String,
    pub ipn: String,
    pub details: Option<DetailsEdit>,
    pub cad: Option<CadEdit>,
}

/// New part details: every field of the current details (the same names in
/// the same order, with new values) and the full list of manufacturer
/// parts. Saving them adds no revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailsEdit {
    /// The number of the part details the edit starts from.
    pub expected_details: u32,
    pub fields: Vec<DetailField>,
    pub manufacturer_parts: Vec<ManufacturerPart>,
}

/// A new Symbol or Footprint: the CAD fields of the current revision (the
/// same names in the same order, with new values), and any files they use.
/// The server binds exact CAD item revisions and adds a revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CadEdit {
    /// The number of the revision the edit starts from.
    pub expected_revision: u32,
    pub fields: Vec<DetailField>,
    /// Files that make new CAD items or CAD item revisions; see
    /// [`crate::cad_file`].
    pub uploads: Vec<CadUpload>,
}

/// What an uploaded file is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadRole {
    Symbol,
    Footprint,
    Model3d,
}

impl UploadRole {
    /// The role as the edit route names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Footprint => "footprint",
            Self::Model3d => "3d-model",
        }
    }
}

/// One file sent with an edit: a symbol (the exact bytes of one top-level
/// `(symbol ...)` entry, with the `(version ...)` of its library), a whole
/// footprint file, or a whole 3D model file. The server reads and hashes
/// the bytes itself.
#[derive(Clone, PartialEq, Eq)]
pub struct CadUpload {
    pub role: UploadRole,
    /// `LIB:NAME` for a symbol or footprint; for a 3D model, the
    /// `3d-models/...` path a footprint sent with it names.
    pub reference: String,
    /// The name of the file it came from, shown as its source.
    pub file_name: String,
    /// For a symbol, the `(version ...)` of the library it came from.
    pub format_version: Option<String>,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for CadUpload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CadUpload")
            .field("role", &self.role)
            .field("reference", &self.reference)
            .field("file_name", &self.file_name)
            .field("format_version", &self.format_version)
            .field("size", &self.bytes.len())
            .finish()
    }
}

impl CadUpload {
    /// The upload as the edit route takes it: its bytes as `text` when
    /// they are UTF-8, otherwise as `base64`, like an import's blobs.
    fn body(&self) -> Value {
        let mut body = json!({
            "role": self.role.as_str(),
            "reference": self.reference,
            "fileName": self.file_name,
        });
        if let Some(version) = &self.format_version {
            body["formatVersion"] = json!(version);
        }
        match std::str::from_utf8(&self.bytes) {
            Ok(text) => body["text"] = json!(text),
            Err(_) => body["base64"] = json!(base64(&self.bytes)),
        }
        body
    }
}

impl EditCommand {
    /// An edit of the part `ipn` that changes nothing yet, with a fresh
    /// correlation ID.
    pub fn new(ipn: &str, reason: &str) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            reason: reason.to_owned(),
            ipn: ipn.to_owned(),
            details: None,
            cad: None,
        }
    }

    /// Changes the details too, starting from `current`, numbered
    /// `expected_details`, unchanged.
    pub fn editing_details(mut self, current: &PartDetails, expected_details: u32) -> Self {
        self.details = Some(DetailsEdit {
            expected_details,
            fields: current.fields.clone(),
            manufacturer_parts: current.manufacturer_parts.clone(),
        });
        self
    }

    /// Changes the Symbol or Footprint too, starting from revision
    /// `current`, numbered `expected_revision`, unchanged.
    pub fn editing_cad(mut self, current: &PartRevision, expected_revision: u32) -> Self {
        self.cad = Some(CadEdit {
            expected_revision,
            fields: current.cad_fields.clone(),
            uploads: Vec::new(),
        });
        self
    }

    /// Sets the value of the field named exactly `name`, in the details or
    /// in the CAD fields, whichever section has it. Returns false if no
    /// section has such a field.
    pub fn set_field(&mut self, name: &str, value: &str) -> bool {
        let details = self.details.as_mut().map(|edit| &mut edit.fields);
        let cad = self.cad.as_mut().map(|edit| &mut edit.fields);
        for fields in [details, cad].into_iter().flatten() {
            if let Some(field) = fields.iter_mut().find(|field| field.name == name) {
                field.value = value.to_owned();
                return true;
            }
        }
        false
    }

    pub fn body(&self) -> Value {
        let fields = |fields: &[DetailField]| {
            fields
                .iter()
                .map(|field| json!({ "name": field.name, "value": field.value }))
                .collect::<Vec<_>>()
        };
        let mut body = json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "ipn": self.ipn,
        });
        if let Some(details) = &self.details {
            body["details"] = json!({
                "expectedDetails": details.expected_details,
                "fields": fields(&details.fields),
                "manufacturerParts": details.manufacturer_parts.iter().map(|part| json!({
                    "manufacturer": part.manufacturer,
                    "mpn": part.mpn,
                })).collect::<Vec<_>>(),
            });
        }
        if let Some(cad) = &self.cad {
            body["cad"] = json!({
                "expectedRevision": cad.expected_revision,
                "fields": fields(&cad.fields),
            });
            if !cad.uploads.is_empty() {
                body["cad"]["uploads"] =
                    Value::Array(cad.uploads.iter().map(CadUpload::body).collect());
            }
        }
        body
    }
}

/// What the edit route answered.
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
    /// 200: one Change holding the new part details, the new revision, or
    /// both, the part moved to them, and one audit entry.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        ipn: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
        /// The new part details, if the edit changed them.
        details: Option<PublishedRecord>,
        /// The new revision, if the edit changed the symbol or footprint.
        revision: Option<PublishedRecord>,
        /// The exact CAD item revisions the part's current revision binds.
        cad_item_revisions: Vec<String>,
        /// The CAD item revisions the edit's files made.
        #[serde(default)]
        new_cad_item_revisions: Vec<NewCadItemRevision>,
    },
    /// 422: nothing would change; nothing was written.
    #[serde(rename_all = "camelCase")]
    Noop {
        correlation_id: String,
        ipn: String,
        message: String,
    },
    /// 409: someone else saved first the part details or the revision the
    /// edit started from. Nothing was written. Only the ones that moved are
    /// given.
    #[serde(rename_all = "camelCase")]
    Conflict {
        correlation_id: String,
        ipn: String,
        #[serde(default)]
        current_details: Option<Box<ConflictingRecord>>,
        #[serde(default)]
        current_revision: Option<Box<ConflictingRecord>>,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
}

/// A CAD item revision a file sent with an edit made.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewCadItemRevision {
    /// `symbol`, `footprint` or `model-3d`.
    pub kind: String,
    pub key: String,
    pub number: u32,
    pub id: String,
    /// For a symbol or footprint, how many other parts still use an older
    /// revision of it; they keep their exact binding.
    #[serde(default)]
    pub other_parts: Option<u32>,
}

/// A record an edit saved.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublishedRecord {
    pub id: String,
    pub number: u32,
    pub snapshot_sha256: String,
}

/// The route that archives or unarchives one part revision.
pub const ARCHIVE_ROUTE: &str = "/api/catalog/archives";

/// Archives (or unarchives) revision `revision` of part `ipn`, with a
/// reason. An archived revision is hidden from KiCad and the app, so nobody
/// new picks it; designs that already use it keep it, and nothing is
/// deleted. The part's current revision is its newest one not archived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveCommand {
    pub correlation_id: CorrelationId,
    pub reason: String,
    pub ipn: String,
    pub revision: u32,
    /// True to archive, false to unarchive.
    pub archived: bool,
}

impl ArchiveCommand {
    /// A command with a fresh correlation ID.
    pub fn new(ipn: &str, revision: u32, archived: bool, reason: &str) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            reason: reason.to_owned(),
            ipn: ipn.to_owned(),
            revision,
            archived,
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "ipn": self.ipn,
            "revision": self.revision,
            "archived": self.archived,
        })
    }
}

/// What the archive route answered.
#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveReply {
    pub status: u16,
    pub outcome: ArchiveOutcome,
    /// The answer was replayed from the correlation-ID ledger.
    pub replayed: bool,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ArchiveOutcome {
    /// 200: one Change with one archive entry, the part moved to its newest
    /// revision not archived (if that changed), and one audit entry.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        ipn: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
        revision: u32,
        archived: bool,
        /// The part's current revision afterwards.
        current_revision: PublishedRecord,
    },
    /// 422: the revision already has that state; nothing was written.
    #[serde(rename_all = "camelCase")]
    Noop {
        correlation_id: String,
        ipn: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
}

/// Submits an archive or unarchive. Applied, no-op and correlation-reuse
/// answers are `Ok`; bad input (400, including archiving the only revision
/// not archived), 401, 403 and 404 are errors.
pub fn submit_archive(
    client: &PocketBaseClient,
    command: &ArchiveCommand,
) -> CollabResult<ArchiveReply> {
    let reply = client.post_json(ARCHIVE_ROUTE, &command.body())?;
    match reply.status {
        200 | 409 | 422 => {
            let outcome: ArchiveOutcome = serde_json::from_str(&reply.body)
                .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
            Ok(ArchiveReply {
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

/// The route that reviews CSV changes that were not applied to an edited
/// part.
pub const CSV_REVIEW_ROUTE: &str = "/api/catalog/csv-reviews";

/// The import problem codes a person can review, and which part of the part
/// each one is about.
pub const CSV_DETAILS_DIFFER: &str = "csv-details-differ-from-edited-details";
pub const CSV_CAD_DIFFERS: &str = "csv-cad-differs-from-edited-revision";
/// A symbol, footprint or 3D model the part uses whose CSV file changed
/// while its current revision was made in the app (kicad-parts-3b8b). Its
/// CSV file is not stored, so a review can only keep the part as it is.
pub use crate::csv_import::CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM as CSV_CAD_ITEM_KEPT;

/// The field name that takes the CSV's whole list of manufacturer parts.
pub const MANUFACTURER_PARTS_FIELD: &str = "Manufacturer parts";

/// Reviews one CSV difference of part `ipn` (an import problem with code
/// [`CSV_DETAILS_DIFFER`] or [`CSV_CAD_DIFFERS`]): takes the CSV's values for
/// the fields in `accept` and keeps the part's values for the rest. An empty
/// `accept` keeps all of them. The route builds what it saves from the
/// problem's recorded CSV document, so the command names only fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvReviewCommand {
    pub correlation_id: CorrelationId,
    pub reason: String,
    pub ipn: String,
    /// The import problem's ID.
    pub problem: String,
    /// Details field names and [`MANUFACTURER_PARTS_FIELD`], or `Symbol` and
    /// `Footprint`.
    pub accept: Vec<String>,
    /// The part's current details number (details problem) or current
    /// revision number (CAD problem) the reviewer read.
    pub expected: u32,
}

impl CsvReviewCommand {
    /// A command with a fresh correlation ID.
    pub fn new(ipn: &str, problem: &str, accept: Vec<String>, expected: u32, reason: &str) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            reason: reason.to_owned(),
            ipn: ipn.to_owned(),
            problem: problem.to_owned(),
            accept,
            expected,
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "ipn": self.ipn,
            "problem": self.problem,
            "accept": self.accept,
            "expected": self.expected,
        })
    }
}

/// What the CSV review route answered.
#[derive(Clone, Debug, PartialEq)]
pub struct CsvReviewReply {
    pub status: u16,
    pub outcome: CsvReviewOutcome,
    /// The answer was replayed from the correlation-ID ledger.
    pub replayed: bool,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CsvReviewOutcome {
    /// 200: one Change with one CSV review, plus the new part details or the
    /// new revision when fields were taken, the part moved to them, and one
    /// audit entry.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        ipn: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
        accepted: Vec<String>,
        details: Option<PublishedRecord>,
        revision: Option<PublishedRecord>,
    },
    /// 422: these CSV values were already reviewed; nothing was written.
    #[serde(rename_all = "camelCase")]
    Noop {
        correlation_id: String,
        ipn: String,
        message: String,
    },
    /// 409: the part's details or revision moved since the difference was
    /// found or read. Nothing was written.
    #[serde(rename_all = "camelCase")]
    Conflict {
        correlation_id: String,
        ipn: String,
        #[serde(default)]
        current_details: Option<Box<ConflictingRecord>>,
        #[serde(default)]
        current_revision: Option<Box<ConflictingRecord>>,
        message: String,
    },
    /// 409: a later import run of the CSV file no longer finds this
    /// difference. Nothing was written.
    #[serde(rename_all = "camelCase")]
    OutOfDate {
        correlation_id: String,
        ipn: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
}

/// Submits a CSV review. Applied, no-op, conflict, out-of-date and
/// correlation-reuse answers are `Ok`; bad input (400, including a field the
/// CSV did not change or CAD that matches an archived revision), 401, 403
/// and 404 are errors.
pub fn submit_csv_review(
    client: &PocketBaseClient,
    command: &CsvReviewCommand,
) -> CollabResult<CsvReviewReply> {
    let reply = client.post_json(CSV_REVIEW_ROUTE, &command.body())?;
    match reply.status {
        200 | 409 | 422 => {
            let outcome: CsvReviewOutcome = serde_json::from_str(&reply.body)
                .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
            Ok(CsvReviewReply {
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

/// A part's current details or revision when a command expected another
/// one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictingRecord {
    pub id: String,
    pub number: u32,
    /// Its part details or part revision as canonical JSON.
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

impl ConflictingRecord {
    /// Its snapshot as part details.
    pub fn details(&self) -> CollabResult<PartDetails> {
        serde_json::from_str(&self.snapshot)
            .map_err(|error| CollabError::Protocol(format!("details {}: {error}", self.number)))
    }

    /// Its snapshot as a part revision.
    pub fn revision(&self) -> CollabResult<PartRevision> {
        serde_json::from_str(&self.snapshot)
            .map_err(|error| CollabError::Protocol(format!("revision {}: {error}", self.number)))
    }
}

/// Submits an edit. Applied, no-op, conflict and correlation-reuse answers
/// are `Ok`; inspect the outcome. Bad input (400), 401, 403 and 404 are
/// errors.
pub fn submit_edit(client: &PocketBaseClient, command: &EditCommand) -> CollabResult<PublishReply> {
    let body = command.body().to_string();
    if body.len() > EDIT_BODY_LIMIT {
        return Err(CollabError::Protocol(format!(
            "the edit of {} is {} bytes, more than the {EDIT_BODY_LIMIT} bytes the edit route accepts in one request; nothing was sent",
            command.ipn,
            body.len()
        )));
    }
    let reply = client.post_json_text(EDIT_ROUTE, &body)?;
    publish_reply(reply)
}

fn publish_reply(reply: collaboration::HttpReply) -> CollabResult<PublishReply> {
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

/// Every saved copy of the details of the part with ID `part_id`, oldest
/// first.
pub fn part_details_of(
    client: &PocketBaseClient,
    part_id: &str,
) -> CollabResult<Vec<StoredPartDetails>> {
    let filter = format!("part = {}", Value::from(part_id));
    client.list_records("catalog_part_details", &filter, "number")
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

/// Every archive entry of the part with ID `part_id`, oldest first.
pub fn archive_entries_of(
    client: &PocketBaseClient,
    part_id: &str,
) -> CollabResult<Vec<StoredArchiveEntry>> {
    let filter = format!("part = {}", Value::from(part_id));
    client.list_records("catalog_archive_entries", &filter, "seq")
}

/// The import problems with subject `ipn` and one of `codes`.
pub fn problems_of(
    client: &PocketBaseClient,
    ipn: &str,
    codes: &[&str],
) -> CollabResult<Vec<StoredImportProblem>> {
    let codes = codes
        .iter()
        .map(|code| format!("code = {}", Value::from(*code)))
        .collect::<Vec<_>>()
        .join(" || ");
    let filter = format!("subject = {} && ({codes})", Value::from(ipn));
    client.list_records("catalog_import_problems", &filter, "id")
}

/// Every CSV review of the part with ID `part_id`, oldest first.
pub fn csv_reviews_of(
    client: &PocketBaseClient,
    part_id: &str,
) -> CollabResult<Vec<StoredCsvReview>> {
    let filter = format!("part = {}", Value::from(part_id));
    client.list_records("catalog_csv_reviews", &filter, "seq")
}

/// The audit entry of the latest import run of `csv_file` (such as
/// `g-res`), if it was ever imported.
pub fn latest_import_run(
    client: &PocketBaseClient,
    csv_file: &str,
) -> CollabResult<Option<StoredAuditEntry>> {
    let filter = format!(
        "csv_file = {} && csv_import_number > 0",
        Value::from(csv_file)
    );
    client.first_record("catalog_audit_entries", &filter, "-csv_import_number")
}

/// The archive entries one Change saved.
pub fn archive_entries_in_change(
    client: &PocketBaseClient,
    change_id: &str,
) -> CollabResult<Vec<StoredArchiveEntry>> {
    let filter = format!("change = {}", Value::from(change_id));
    client.list_records("catalog_archive_entries", &filter, "part")
}

/// The part revisions with these IDs.
pub fn part_revisions_by_id(
    client: &PocketBaseClient,
    ids: &[&str],
) -> CollabResult<Vec<StoredPartRevision>> {
    let mut revisions = Vec::new();
    for filter in id_filters(ids) {
        revisions.extend(client.list_records::<StoredPartRevision>(
            "catalog_part_revisions",
            &filter,
            "part,number",
        )?);
    }
    Ok(revisions)
}

/// The part details one Change saved.
pub fn details_in_change(
    client: &PocketBaseClient,
    change_id: &str,
) -> CollabResult<Vec<StoredPartDetails>> {
    let filter = format!("change = {}", Value::from(change_id));
    client.list_records("catalog_part_details", &filter, "part,number")
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
    pub current_details: String,
    pub current_details_number: u32,
    pub first_change: String,
}

/// One archive or unarchive of a part revision. A revision's state is its
/// newest entry (by `seq`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredArchiveEntry {
    pub id: String,
    pub part: String,
    pub part_revision: String,
    pub revision_number: u32,
    /// True for an archive, false for an unarchive.
    pub archived: bool,
    pub change: String,
    /// The Change's seq.
    pub seq: u64,
    /// The part's current revision once this entry was saved.
    pub current_revision_after: String,
    pub current_revision_number_after: u32,
}

/// One review of CSV changes that were not applied to an edited part. It
/// covers what the CSV said (`csv_sha256`), so later imports that find the
/// same CSV values need no new review.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredCsvReview {
    pub id: String,
    pub part: String,
    /// The import problem reviewed.
    pub problem: String,
    /// [`CSV_DETAILS_DIFFER`] or [`CSV_CAD_DIFFERS`].
    pub code: String,
    /// SHA-256 of the CSV document the problem recorded.
    pub csv_sha256: String,
    /// JSON list of the field names whose CSV values were taken.
    pub accepted: String,
    pub change: String,
    /// The Change's seq.
    pub seq: u64,
}

/// The CSV document an import problem recorded: the CSV's part details for
/// [`CSV_DETAILS_DIFFER`], its part revision for [`CSV_CAD_DIFFERS`], and its
/// snapshot of the CAD item for [`CSV_CAD_ITEM_KEPT`].
pub fn recorded_csv_document(problem: &StoredImportProblem) -> Option<String> {
    let key = match problem.code.as_str() {
        CSV_DETAILS_DIFFER => "details",
        CSV_CAD_DIFFERS => "revision",
        CSV_CAD_ITEM_KEPT => "csv_cad_item",
        _ => return None,
    };
    let detail: Value = serde_json::from_str(&problem.detail).ok()?;
    detail.get(key)?.as_str().map(str::to_owned)
}

/// The revision numbers of a part that are archived now, from its archive
/// entries (any order).
pub fn archived_numbers(entries: &[&StoredArchiveEntry]) -> std::collections::BTreeSet<u32> {
    let mut sorted: Vec<&&StoredArchiveEntry> = entries.iter().collect();
    sorted.sort_by_key(|entry| entry.seq);
    let mut state = BTreeMap::new();
    for entry in sorted {
        state.insert(entry.revision_number, entry.archived);
    }
    state
        .into_iter()
        .filter_map(|(number, archived)| archived.then_some(number))
        .collect()
}

/// One saved copy of a part's details.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredPartDetails {
    pub id: String,
    pub part: String,
    pub number: u32,
    pub predecessor: String,
    pub change: String,
    pub snapshot: String,
    pub snapshot_sha256: String,
    pub source: String,
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
    /// from 1. 0 for edits.
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
    /// The importer was told to import from files with uncommitted changes.
    #[serde(default)]
    pub allowed_uncommitted_changes: bool,
}

impl StoredAuditEntry {
    /// What this CSV import run read, or `None` if the entry records no
    /// readable import run (edits, or a damaged record).
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
    /// Every part a row of the CSV file made, in row order, with the
    /// numbers of its current revision and details just after the run.
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

/// A part that a row of an import run's CSV file made, and the numbers of
/// its current revision and details just after that run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ImportedPart {
    pub ipn: String,
    pub revision: u32,
    pub details: u32,
}

/// Every Catalog record as stored, with each blob's bytes downloaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogReadback {
    pub changes: Vec<StoredChange>,
    pub parts: Vec<StoredPart>,
    pub part_details: Vec<StoredPartDetails>,
    pub part_revisions: Vec<StoredPartRevision>,
    pub archive_entries: Vec<StoredArchiveEntry>,
    pub csv_reviews: Vec<StoredCsvReview>,
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

    pub fn part_details(&self, id: &str) -> Option<&StoredPartDetails> {
        self.part_details.iter().find(|details| details.id == id)
    }

    /// The revision numbers of the part with ID `part_id` archived now.
    pub fn archived(&self, part_id: &str) -> std::collections::BTreeSet<u32> {
        let entries: Vec<&StoredArchiveEntry> = self
            .archive_entries
            .iter()
            .filter(|entry| entry.part == part_id)
            .collect();
        archived_numbers(&entries)
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
    pub fn counts(&self) -> [usize; 11] {
        [
            self.changes.len(),
            self.parts.len(),
            self.part_details.len(),
            self.part_revisions.len(),
            self.archive_entries.len(),
            self.csv_reviews.len(),
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
        part_details: client.list_records("catalog_part_details", "", "part,number")?,
        part_revisions: client.list_records("catalog_part_revisions", "", "part,number")?,
        archive_entries: client.list_records("catalog_archive_entries", "", "seq")?,
        csv_reviews: client.list_records("catalog_csv_reviews", "", "seq")?,
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
            r#"{"outcome":"applied","correlationId":"c","changeId":"x","seq":1,"auditEntryId":"a","created":{"blobs":2,"cadItems":2,"cadItemRevisions":2,"parts":29,"partRevisions":29,"updatedParts":0,"partDetails":29,"updatedDetails":0,"problems":1},"unchanged":{}}"#,
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
            r#"{"outcome":"conflict","correlationId":"c","conflicts":[{"ipn":"RES-0000-0470","expectedRevision":1,"expectedDetails":1,"currentRevision":{"id":"r","number":2,"snapshot":"{}","snapshotSha256":"s","changeKind":"edit","reason":"x","actor":"users:u","actorName":"A","created":"t"},"currentDetails":null},{"ipn":"RES-0000-9999","expectedRevision":3,"expectedDetails":3,"currentRevision":null,"currentDetails":null}],"message":"m"}"#,
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
            current_details: [("RES-0000-1000".to_owned(), 7)].into(),
            ..ImportState::default()
        };
        let body = ImportCommand::new(&import, "r").expecting(&state).body();
        let expected: Vec<(&str, u64, u64)> = body["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| {
                (
                    part["ipn"].as_str().unwrap(),
                    part["expectedRevision"].as_u64().unwrap(),
                    part["expectedDetails"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(expected, [("RES-0000-0470", 0, 0), ("RES-0000-1000", 4, 7)]);
        // Details and revision travel as two documents with one source.
        let part = &body["parts"][0];
        assert!(
            part["details"]["snapshot"]
                .as_str()
                .unwrap()
                .contains("part-details/2")
        );
        assert!(
            part["revision"]["snapshot"]
                .as_str()
                .unwrap()
                .contains("part-revision/1")
        );
        assert_eq!(body["allowOlderCheckout"], false);
        assert_eq!(body["allowUncommittedChanges"], false);
        // Allowing uncommitted changes is sent and gives a new correlation
        // ID, so it is never taken for the same request.
        let plain = ImportCommand::new(&import, "r").expecting(&state);
        let allowed = plain.clone().allowing_uncommitted_changes(true);
        assert!(allowed.allow_uncommitted_changes());
        assert_eq!(allowed.body()["allowUncommittedChanges"], true);
        assert_ne!(allowed.correlation_id, plain.correlation_id);
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

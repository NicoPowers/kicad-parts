//! The Catalog's PocketBase contract: the protected routes
//! (`infra/pocketbase/pb_hooks/catalog_edits.pb.js` for edits,
//! `catalog_new_parts.pb.js` for new parts, `catalog_archives.pb.js` for
//! archives and `catalog_categories.pb.js` for categories) and the
//! `catalog_*` collections (`infra/pocketbase/pb_migrations/`).

use crate::cad_item::{CadItemDetails, CadItemKind};
use crate::part::{DetailField, ManufacturerPart, PartDetails, PartRevision};
use crate::sha256_hex;
use collaboration::{CollabError, CollabResult, CorrelationId, PocketBaseClient};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Every Catalog collection, in dependency order.
pub const COLLECTIONS: [&str; 13] = [
    "catalog_changes",
    "catalog_blobs",
    "catalog_cad_items",
    "catalog_cad_item_revisions",
    "catalog_parts",
    "catalog_part_details",
    "catalog_part_revisions",
    "catalog_archive_entries",
    "catalog_categories",
    "catalog_category_revisions",
    "catalog_category_matches",
    "catalog_audit_entries",
    "catalog_commands",
];

/// For each CAD item, by CAD item ID, the SHA-256 of the bytes of its
/// current revision (its newest), for the library check
/// (`crate::library_check`). Reads only each CAD item's current revision,
/// and only its CAD item and snapshot: the snapshot names the blob, not its
/// bytes. The unique index on a revision's predecessor keeps finding the
/// newest revisions fast.
pub fn read_current_cad_bytes(client: &PocketBaseClient) -> CollabResult<BTreeMap<String, String>> {
    #[derive(Deserialize)]
    struct Fields {
        id: String,
        cad_item: String,
        snapshot: String,
    }
    let newest: Vec<Fields> = client.list_record_fields(
        "catalog_cad_item_revisions",
        "catalog_cad_item_revisions_via_predecessor.id = \"\"",
        "cad_item,number",
        "id,cad_item,snapshot",
    )?;
    newest
        .into_iter()
        .map(|revision| {
            let details: CadItemDetails =
                serde_json::from_str(&revision.snapshot).map_err(|error| {
                    CollabError::Protocol(format!(
                        "CAD item revision {} cannot be read: {error}",
                        revision.id
                    ))
                })?;
            Ok((revision.cad_item, details.blob_sha256))
        })
        .collect()
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

/// New part details: the category, every field of the current details
/// (the same names in the same order, with new values) and the full list of
/// manufacturer parts. Saving them adds no revision.
///
/// A new category keeps every field and adds, after them, the fields of the
/// new category the part lacks (see [`fields_for_category`]); the server
/// refuses any other list. The IPN never changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailsEdit {
    /// The number of the part details the edit starts from.
    pub expected_details: u32,
    /// The code of the part's category, changed or not.
    pub category: String,
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
    /// Where the file is in our libraries, for a symbol or footprint
    /// already there (Add part from our libraries, kicad-parts-37a7);
    /// `None` for a file chosen in the app.
    pub library: Option<LibrarySource>,
    /// Where the file was copied from, for a symbol, footprint or 3D model
    /// of another installed KiCad library (kicad-parts-bc6f); `None`
    /// otherwise.
    pub installed: Option<InstalledSource>,
}

/// Where an upload was copied from in one of the person's other installed
/// KiCad libraries (not ours, not KiCad's own): the library's nickname and
/// the file, which its source names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledSource {
    /// The library's nickname in the person's KiCad library tables.
    pub library: String,
    /// The file it was read from, such as `D:\KiCad\Mine.pretty\X.kicad_mod`.
    pub path: String,
}

/// Where an upload was read from in our own libraries in the local library
/// folder: the repository file, the commit it was read from, the whole
/// file's SHA-256 and, for a symbol, its byte range in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibrarySource {
    /// The repository path, such as `symbols/g-ics.kicad_sym`.
    pub path: String,
    /// The SHA-256 of that whole file.
    pub byte_sha256: String,
    /// The commit (`HEAD`) the file was read from, which holds it as it is.
    pub commit: String,
    /// For a symbol, the byte range of its entry in the file.
    pub byte_range: Option<[u64; 2]>,
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
            .field("library", &self.library)
            .field("installed", &self.installed)
            .finish()
    }
}

impl CadUpload {
    /// The upload as the edit route takes it: its bytes as `text` when
    /// they are UTF-8, otherwise as `base64`.
    fn body(&self) -> Value {
        let mut body = json!({
            "role": self.role.as_str(),
            "reference": self.reference,
            "fileName": self.file_name,
        });
        if let Some(version) = &self.format_version {
            body["formatVersion"] = json!(version);
        }
        if let Some(library) = &self.library {
            body["library"] = json!({
                "path": library.path,
                "byteSha256": library.byte_sha256,
                "commit": library.commit,
                "byteRange": library.byte_range,
            });
        }
        if let Some(installed) = &self.installed {
            body["installed"] = json!({
                "library": installed.library,
                "path": installed.path,
            });
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
            category: current.category.clone(),
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

    /// Moves the part to `category`, starting from the details `current`
    /// (the ones [`EditCommand::editing_details`] started from): the fields
    /// become [`fields_for_category`], keeping any value already set in
    /// this edit. Does nothing without a details section.
    pub fn choose_category(&mut self, current: &PartDetails, category: &crate::CategoryDetails) {
        let Some(details) = self.details.as_mut() else {
            return;
        };
        let fields = if category.code == current.category {
            current.fields.clone()
        } else {
            fields_for_category(&current.fields, &category.fields)
        };
        details.fields = fields
            .into_iter()
            .map(|field| {
                let typed = details.fields.iter().find(|typed| typed.name == field.name);
                typed.cloned().unwrap_or(field)
            })
            .collect();
        details.category.clone_from(&category.code);
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
                "category": details.category,
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

/// The fields a part has in a new category: every field it has, then,
/// blank, the fields of the new category it lacks, in the category's order.
/// Nothing is dropped.
pub fn fields_for_category(fields: &[DetailField], category_fields: &[String]) -> Vec<DetailField> {
    let mut result = fields.to_vec();
    for name in category_fields {
        if !fields.iter().any(|field| &field.name == name) {
            result.push(DetailField {
                name: name.clone(),
                value: String::new(),
            });
        }
    }
    result
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
        /// The clues remembered with a new category.
        #[serde(default)]
        remembered_matches: Vec<RememberedClue>,
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

/// A clue remembered with the category a person chose.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RememberedClue {
    /// See [`crate::guess::ClueKind::as_str`].
    pub clue: String,
    pub value: String,
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

/// The route that adds a new part from downloaded CAD files.
pub const NEW_PART_ROUTE: &str = "/api/catalog/parts";

/// A new part (kicad-parts-9693): its IPN, its category with exactly the
/// category's fields, its manufacturer parts, its Symbol and Footprint, the
/// files they use, and where the files came from. The server saves part
/// details 1, revision 1 and revision 1 of each new CAD item in one Change,
/// and remembers the clues of the category the person confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateCommand {
    pub correlation_id: CorrelationId,
    pub reason: String,
    pub ipn: String,
    /// The code of the part's category.
    pub category: String,
    /// Every field of the category, in its order.
    pub fields: Vec<DetailField>,
    pub manufacturer_parts: Vec<ManufacturerPart>,
    /// Symbol, then Footprint.
    pub cad_fields: Vec<DetailField>,
    pub uploads: Vec<CadUpload>,
    /// Where the files came from, such as `SnapEDA`; blank if not known.
    pub from: String,
    /// The names of the downloaded files.
    pub files: Vec<String>,
}

impl CreateCommand {
    pub fn new(ipn: &str, category: &str, reason: &str) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            reason: reason.to_owned(),
            ipn: ipn.to_owned(),
            category: category.to_owned(),
            fields: Vec::new(),
            manufacturer_parts: Vec::new(),
            cad_fields: vec![
                DetailField {
                    name: "Symbol".into(),
                    value: String::new(),
                },
                DetailField {
                    name: "Footprint".into(),
                    value: String::new(),
                },
            ],
            uploads: Vec::new(),
            from: String::new(),
            files: Vec::new(),
        }
    }

    pub fn body(&self) -> Value {
        let fields = |fields: &[DetailField]| {
            fields
                .iter()
                .map(|field| json!({ "name": field.name, "value": field.value }))
                .collect::<Vec<_>>()
        };
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "ipn": self.ipn,
            "category": self.category,
            "fields": fields(&self.fields),
            "manufacturerParts": self.manufacturer_parts.iter().map(|part| json!({
                "manufacturer": part.manufacturer,
                "mpn": part.mpn,
            })).collect::<Vec<_>>(),
            "cadFields": fields(&self.cad_fields),
            "uploads": self.uploads.iter().map(CadUpload::body).collect::<Vec<_>>(),
            "from": self.from,
            "files": self.files,
        })
    }
}

/// What the new-part route answered.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateReply {
    pub status: u16,
    pub outcome: CreateOutcome,
    /// The answer was replayed from the correlation-ID ledger.
    pub replayed: bool,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CreateOutcome {
    /// 200: one Change holding the new part, its details 1 and revision 1,
    /// the new CAD item revisions and the remembered matches.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        ipn: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
        details: Box<PublishedRecord>,
        revision: Box<PublishedRecord>,
        cad_item_revisions: Vec<String>,
        #[serde(default)]
        new_cad_item_revisions: Vec<NewCadItemRevision>,
        #[serde(default)]
        remembered_matches: Vec<RememberedClue>,
    },
    /// 409: the Catalog already has a part with the IPN; nothing was
    /// written. An IPN is never reused.
    #[serde(rename_all = "camelCase")]
    IpnTaken {
        correlation_id: String,
        ipn: String,
        message: String,
        /// The caller added that part in the app: a save whose answer was
        /// lost, sent again with changes.
        #[serde(default)]
        added_by_you: bool,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
}

/// Submits a new part. Applied, IPN-taken and correlation-reuse answers are
/// `Ok`; inspect the outcome. Bad input (400), 401 and 403 are errors.
pub fn submit_create(
    client: &PocketBaseClient,
    command: &CreateCommand,
) -> CollabResult<CreateReply> {
    let body = command.body().to_string();
    if body.len() > EDIT_BODY_LIMIT {
        return Err(CollabError::Protocol(format!(
            "the new part {} is {} bytes, more than the {EDIT_BODY_LIMIT} bytes the route accepts in one request; nothing was sent",
            command.ipn,
            body.len()
        )));
    }
    let reply = client.post_json_text(NEW_PART_ROUTE, &body)?;
    match reply.status {
        200 | 409 => {
            let outcome: CreateOutcome = serde_json::from_str(&reply.body)
                .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
            Ok(CreateReply {
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

/// The route that archives or unarchives one part revision.
pub const ARCHIVE_ROUTE: &str = "/api/catalog/archives";

/// Archives (or unarchives) revision `revision` of part `ipn`, with a
/// reason. An archived revision is hidden from the app and set apart in
/// KiCad's chooser, under Archived revisions, so nobody new picks it;
/// designs that already use it keep it, and nothing is deleted. The
/// part's current revision is its newest one not archived.
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

/// The route through which a superuser sets up a category.
pub const CATEGORY_ROUTE: &str = "/api/catalog/categories";

/// Sets up one new category, as a superuser: its revision 1 is `category`,
/// saved in one Change of kind category (kicad-parts-fbc2). A new team
/// server has the starting categories already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryCommand {
    pub correlation_id: CorrelationId,
    pub reason: String,
    pub category: crate::CategoryDetails,
}

impl CategoryCommand {
    /// A command with a fresh correlation ID.
    pub fn new(category: crate::CategoryDetails, reason: &str) -> Self {
        Self {
            correlation_id: CorrelationId::generate(),
            reason: reason.to_owned(),
            category,
        }
    }

    pub fn body(&self) -> Value {
        json!({
            "correlationId": self.correlation_id,
            "reason": self.reason,
            "category": self.category,
        })
    }
}

/// What the category route answered.
#[derive(Clone, Debug, PartialEq)]
pub struct CategoryReply {
    pub status: u16,
    pub outcome: CategoryOutcome,
    /// The answer was replayed from the correlation-ID ledger.
    pub replayed: bool,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CategoryOutcome {
    /// 200: one Change with the new category, its revision 1 and one audit
    /// entry.
    #[serde(rename_all = "camelCase")]
    Applied {
        correlation_id: String,
        change_id: String,
        seq: u64,
        audit_entry_id: String,
    },
    #[serde(rename_all = "camelCase")]
    CorrelationReused {
        correlation_id: String,
        message: String,
    },
}

/// Submits a new category. Applied and correlation-reuse answers are `Ok`;
/// bad input (400, including a category the Catalog has) and 401 are
/// errors.
pub fn submit_category(
    client: &PocketBaseClient,
    command: &CategoryCommand,
) -> CollabResult<CategoryReply> {
    let reply = client.post_json(CATEGORY_ROUTE, &command.body())?;
    match reply.status {
        200 | 409 => {
            let outcome: CategoryOutcome = serde_json::from_str(&reply.body)
                .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
            Ok(CategoryReply {
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
    any_of_filters("id", ids)
}

/// `field = "a" || field = "b" || ...` filters of at most 40 values each,
/// which keeps request URLs short.
fn any_of_filters(field: &str, values: &[&str]) -> Vec<String> {
    values
        .chunks(40)
        .map(|chunk| {
            chunk
                .iter()
                .map(|value| format!("{field} = {}", Value::from(*value)))
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

/// The CAD item revisions the Changes with these IDs saved, such as the
/// ones an edit's files made.
pub fn cad_item_revisions_in_changes(
    client: &PocketBaseClient,
    change_ids: &[&str],
) -> CollabResult<Vec<StoredCadItemRevision>> {
    let mut revisions = Vec::new();
    for filter in any_of_filters("change", change_ids) {
        revisions.extend(client.list_records::<StoredCadItemRevision>(
            "catalog_cad_item_revisions",
            &filter,
            "cad_item,number",
        )?);
    }
    Ok(revisions)
}

/// Whether the Catalog has the CAD item `kind` `key`.
pub fn has_cad_item(client: &PocketBaseClient, kind: CadItemKind, key: &str) -> CollabResult<bool> {
    let id = crate::CadItemId::new(kind, key);
    let filter = format!("id = {}", Value::from(id.as_str()));
    Ok(client
        .first_record::<StoredCadItem>("catalog_cad_items", &filter, "id")?
        .is_some())
}

/// Every Change after `checkpoint`, in `seq` order.
///
/// The list is read in pages by position, which is safe here: a new Change
/// always gets the next `seq`, so it lands after every page already read,
/// and Changes are never deleted.
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

/// The archive entries the Changes with these IDs saved.
///
/// This and the other `*_in_changes` reads page by position, which is safe:
/// a Change saves all its records in one transaction, so once it can be
/// read, nothing is ever added to or removed from what it saved.
pub fn archive_entries_in_changes(
    client: &PocketBaseClient,
    change_ids: &[&str],
) -> CollabResult<Vec<StoredArchiveEntry>> {
    let mut entries = Vec::new();
    for filter in any_of_filters("change", change_ids) {
        entries.extend(client.list_records("catalog_archive_entries", &filter, "seq")?);
    }
    Ok(entries)
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

/// The part details the Changes with these IDs saved.
pub fn details_in_changes(
    client: &PocketBaseClient,
    change_ids: &[&str],
) -> CollabResult<Vec<StoredPartDetails>> {
    let mut details = Vec::new();
    for filter in any_of_filters("change", change_ids) {
        details.extend(client.list_records("catalog_part_details", &filter, "part,number")?);
    }
    Ok(details)
}

/// The part revisions the Changes with these IDs published.
pub fn revisions_in_changes(
    client: &PocketBaseClient,
    change_ids: &[&str],
) -> CollabResult<Vec<StoredPartRevision>> {
    let mut revisions = Vec::new();
    for filter in any_of_filters("change", change_ids) {
        revisions.extend(client.list_records("catalog_part_revisions", &filter, "part,number")?);
    }
    Ok(revisions)
}

/// The `seq` of the newest Change, or 0 before the first.
pub fn latest_change_seq(client: &PocketBaseClient) -> CollabResult<u64> {
    #[derive(Deserialize)]
    struct Seq {
        seq: u64,
    }
    Ok(client
        .first_record_fields::<Seq>("catalog_changes", "", "-seq", "seq")?
        .map_or(0, |change| change.seq))
}

/// Up to `limit` parts whose IDs sort after `after` (from the first part
/// when `after` is empty), in ID order, each with its current revision and
/// current details. One request.
///
/// To read every part, pass the last ID of each page as the next `after`.
/// That pages by a key, not by position, so a part added between two pages
/// never makes another part show twice or not at all. The new part itself
/// is missed if its ID sorts before `after`.
pub fn current_parts_after(
    client: &PocketBaseClient,
    after: &str,
    limit: usize,
) -> CollabResult<Vec<StoredCurrentPart>> {
    // Only what the replica keeps, which leaves out the large `source`
    // documents.
    const FIELDS: &str = "id,\
        expand.current_revision.id,expand.current_revision.number,\
        expand.current_revision.snapshot,expand.current_revision.snapshot_sha256,\
        expand.current_details.id,expand.current_details.number,\
        expand.current_details.snapshot,expand.current_details.snapshot_sha256";
    let mut query = vec![
        ("perPage", limit.to_string()),
        ("sort", "id".to_owned()),
        ("expand", "current_revision,current_details".to_owned()),
        ("fields", FIELDS.to_owned()),
        ("skipTotal", "1".to_owned()),
    ];
    if !after.is_empty() {
        query.push(("filter", format!("id > {}", Value::from(after))));
    }
    let reply = client.get_with_query("/api/collections/catalog_parts/records", &query)?;
    let page: PartsPage = reply.json()?;
    page.items
        .into_iter()
        .map(|item| {
            // Every part gets both in the Change that adds it, so a gap
            // means the server answered something unexpected.
            let missing = |what: &str| {
                CollabError::Protocol(format!("part {} came without its {what}", item.id))
            };
            Ok(StoredCurrentPart {
                revision: item
                    .expand
                    .current_revision
                    .ok_or_else(|| missing("current revision"))?,
                details: item
                    .expand
                    .current_details
                    .ok_or_else(|| missing("current details"))?,
                id: item.id,
            })
        })
        .collect()
}

/// One reply to [`current_parts_after`], as PocketBase sends it.
#[derive(Deserialize)]
struct PartsPage {
    items: Vec<PartsPageItem>,
}

#[derive(Deserialize)]
struct PartsPageItem {
    id: String,
    #[serde(default)]
    expand: PartsPageExpand,
}

#[derive(Default, Deserialize)]
struct PartsPageExpand {
    current_revision: Option<StoredCurrentRecord>,
    current_details: Option<StoredCurrentRecord>,
}

/// A part with its current revision and current details, as
/// [`current_parts_after`] reads them.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredCurrentPart {
    pub id: String,
    pub revision: StoredCurrentRecord,
    pub details: StoredCurrentRecord,
}

/// What the replica keeps of a saved revision or of saved part details.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredCurrentRecord {
    pub id: String,
    pub number: u32,
    pub snapshot: String,
    pub snapshot_sha256: String,
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
    /// JSON of where what it saved came from, such as the files a new
    /// part was made from.
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

/// A category: its code gives its ID.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredCategory {
    pub id: String,
    pub code: String,
    pub first_change: String,
}

/// One revision of a category; the newest is the current one.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredCategoryRevision {
    pub id: String,
    pub category: String,
    pub number: u32,
    pub predecessor: String,
    pub change: String,
    /// [`crate::CategoryDetails`] as canonical JSON.
    pub snapshot: String,
    pub snapshot_sha256: String,
    pub source: String,
}

/// A person confirmed a category for a part with this clue (a remembered
/// match), in an edit.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StoredCategoryMatch {
    pub id: String,
    pub category: String,
    /// See [`crate::guess::ClueKind::as_str`].
    pub clue: String,
    pub value: String,
    pub part: String,
    pub change: String,
    pub seq: u64,
    pub actor: String,
    pub actor_name: String,
    #[serde(default)]
    pub created: String,
}

/// The current revision of every category, by name. Each is the newest
/// revision of its category.
pub fn current_categories(client: &PocketBaseClient) -> CollabResult<Vec<crate::CategoryDetails>> {
    let revisions: Vec<StoredCategoryRevision> =
        client.list_records("catalog_category_revisions", "", "category,-number")?;
    let mut seen = BTreeSet::new();
    let mut categories = Vec::new();
    for revision in revisions {
        if !seen.insert(revision.category.clone()) {
            continue;
        }
        let details: crate::CategoryDetails = serde_json::from_str(&revision.snapshot)
            .map_err(|error| CollabError::Protocol(format!("category {}: {error}", revision.id)))?;
        categories.push(details);
    }
    categories.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.code.cmp(&b.code))
    });
    Ok(categories)
}

/// Every remembered match, oldest first, with its category's code.
pub fn remembered_matches(
    client: &PocketBaseClient,
) -> CollabResult<Vec<crate::guess::RememberedMatch>> {
    let categories: Vec<StoredCategory> = client.list_records("catalog_categories", "", "code")?;
    let code_of: BTreeMap<String, String> = categories
        .into_iter()
        .map(|category| (category.id, category.code))
        .collect();
    let matches: Vec<StoredCategoryMatch> =
        client.list_records("catalog_category_matches", "", "seq,clue")?;
    Ok(matches
        .into_iter()
        .filter_map(|stored| {
            Some(crate::guess::RememberedMatch {
                category: code_of.get(&stored.category)?.clone(),
                clue: crate::guess::ClueKind::parse(&stored.clue)?,
                value: stored.value,
            })
        })
        .collect())
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
pub struct StoredAuditEntry {
    pub id: String,
    pub action: String,
    pub actor: String,
    /// The actor's name or email when the entry was made.
    #[serde(default)]
    pub actor_name: String,
    /// The Change the save made.
    pub change: String,
    pub reason: String,
    pub correlation_id: String,
    /// JSON counts of what the save created.
    pub summary: String,
}
/// Every Catalog record as stored, with each blob's bytes downloaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogReadback {
    pub changes: Vec<StoredChange>,
    pub parts: Vec<StoredPart>,
    pub part_details: Vec<StoredPartDetails>,
    pub part_revisions: Vec<StoredPartRevision>,
    pub archive_entries: Vec<StoredArchiveEntry>,
    pub categories: Vec<StoredCategory>,
    pub category_revisions: Vec<StoredCategoryRevision>,
    pub category_matches: Vec<StoredCategoryMatch>,
    pub cad_items: Vec<StoredCadItem>,
    pub cad_item_revisions: Vec<StoredCadItemRevision>,
    pub blobs: Vec<StoredBlob>,
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
    pub fn counts(&self) -> [usize; 12] {
        [
            self.changes.len(),
            self.parts.len(),
            self.part_details.len(),
            self.part_revisions.len(),
            self.archive_entries.len(),
            self.categories.len(),
            self.category_revisions.len(),
            self.category_matches.len(),
            self.cad_items.len(),
            self.cad_item_revisions.len(),
            self.blobs.len(),
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
        categories: client.list_records("catalog_categories", "", "code")?,
        category_revisions: client.list_records(
            "catalog_category_revisions",
            "",
            "category,number",
        )?,
        category_matches: client.list_records("catalog_category_matches", "", "seq,clue")?,
        cad_items: client.list_records("catalog_cad_items", "", "kind,key")?,
        cad_item_revisions: client.list_records(
            "catalog_cad_item_revisions",
            "",
            "cad_item,number",
        )?,
        blobs: client.list_records("catalog_blobs", "", "sha256")?,
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

/// The route that reads the team's settings.
pub const TEAM_ROUTE: &str = "/api/team";
/// The route that sets the team code, once.
pub const TEAM_CODE_ROUTE: &str = "/api/team/code";

/// The team's settings (`GET /api/team`, see `infra/pocketbase/pb_hooks/
/// team.pb.js`).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TeamSettings {
    /// The settings record's ID, which names the team on every computer.
    pub id: String,
    /// Such as `MA`; blank until someone who can edit chooses it.
    pub code: String,
    /// Who chose the code.
    #[serde(default)]
    pub code_set_by: String,
    /// The names of the members who may choose it.
    #[serde(default)]
    pub editors: Vec<String>,
    /// How many members may choose it but have no name. The route never
    /// gives their emails.
    #[serde(default)]
    pub unnamed_editors: u32,
}

/// Reads the team's settings.
pub fn read_team(client: &PocketBaseClient) -> CollabResult<TeamSettings> {
    client.get(TEAM_ROUTE)?.json()
}

/// What setting the team code did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeamCodeOutcome {
    Applied,
    /// The team already had this code.
    Noop,
    /// The team already has another code, which never changes.
    AlreadySet {
        code: String,
    },
}

/// Sets the team code. Only members who can edit the Catalog may.
pub fn submit_team_code(client: &PocketBaseClient, code: &str) -> CollabResult<TeamCodeOutcome> {
    let body = serde_json::json!({
        "correlationId": collaboration::CorrelationId::generate().to_string(),
        "code": code,
    });
    let reply = client.post_json(TEAM_CODE_ROUTE, &body)?;
    if reply.status == 409 {
        let answer: Value = serde_json::from_str(&reply.body)
            .map_err(|parse| CollabError::Protocol(format!("{parse}: {}", reply.body)))?;
        return Ok(TeamCodeOutcome::AlreadySet {
            code: answer["code"].as_str().unwrap_or_default().to_owned(),
        });
    }
    let answer: Value = reply.json()?;
    Ok(match answer["outcome"].as_str() {
        Some("noop") => TeamCodeOutcome::Noop,
        _ => TeamCodeOutcome::Applied,
    })
}

/// Reads the Catalog records that say what each part and CAD item is and
/// which revisions exist, without downloading any blob: parts, part
/// details, part revisions, archive entries, categories, CAD items, CAD
/// item revisions, blob records and Changes (which say when each CAD item
/// was first saved). Remembered matches and audit entries are left
/// empty. Use [`download_blob`] for the bytes that
/// are needed.
///
/// Changes are read last. A save writes its Change and its records in one
/// transaction, so every record read already has its Change, and a CAD
/// item's first Change is never missing. Reading Changes first would miss
/// the Change of an item saved during the reading; the libraries would then
/// guess its age, and could rename it on the next update.
pub fn read_revisions(client: &PocketBaseClient) -> CollabResult<CatalogReadback> {
    let mut readback = CatalogReadback {
        parts: client.list_records("catalog_parts", "", "ipn")?,
        part_details: client.list_records("catalog_part_details", "", "part,number")?,
        part_revisions: client.list_records("catalog_part_revisions", "", "part,number")?,
        archive_entries: client.list_records("catalog_archive_entries", "", "seq")?,
        categories: client.list_records("catalog_categories", "", "code")?,
        category_revisions: client.list_records(
            "catalog_category_revisions",
            "",
            "category,number",
        )?,
        cad_items: client.list_records("catalog_cad_items", "", "kind,key")?,
        cad_item_revisions: client.list_records(
            "catalog_cad_item_revisions",
            "",
            "cad_item,number",
        )?,
        blobs: client.list_records("catalog_blobs", "", "sha256")?,
        ..CatalogReadback::default()
    };
    readback.changes = client.list_records("catalog_changes", "", "seq")?;
    Ok(readback)
}

/// The bytes of one stored blob. Fails if they do not hash to its
/// `sha256`.
pub fn download_blob(client: &PocketBaseClient, blob: &StoredBlob) -> CollabResult<Vec<u8>> {
    let token = client.file_token()?;
    let bytes = client.download_file("catalog_blobs", &blob.id, &blob.file, Some(&token))?;
    if sha256_hex(&bytes) != blob.sha256 || bytes.len() as u64 != blob.size {
        return Err(CollabError::Protocol(format!(
            "stored blob {} does not match its SHA-256 or size",
            blob.sha256
        )));
    }
    Ok(bytes)
}

/// Standard base64 with padding, as the team server's routes decode it.
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
}

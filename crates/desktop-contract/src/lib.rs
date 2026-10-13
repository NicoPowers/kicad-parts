//! Messages exchanged between the Tauri shell and the Leptos UI.
//!
//! Both sides compile this crate (natively and to WebAssembly), so command
//! names and payload shapes cannot drift apart. It holds data only.

use serde::{Deserialize, Serialize};

/// Tauri command names.
pub mod command {
    pub const APP_STATUS: &str = "app_status";
    pub const REDISCOVER_KICAD: &str = "rediscover_kicad";
    pub const SET_KICAD_OVERRIDE: &str = "set_kicad_override";
    pub const SEARCH_PARTS: &str = "search_parts";
    pub const PART_DETAIL: &str = "part_detail";
    pub const PART_PREVIEW: &str = "part_preview";
    pub const TEAM_STATUS: &str = "team_status";
    pub const TEAM_SIGN_IN: &str = "team_sign_in";
    pub const TEAM_SIGN_OUT: &str = "team_sign_out";
    pub const TEAM_PART: &str = "team_part";
    pub const TEAM_EDIT: &str = "team_edit";
    pub const TEAM_ARCHIVE: &str = "team_archive";
    pub const TEAM_CSV_REVIEW: &str = "team_csv_review";
    pub const TEAM_READ_FILE: &str = "team_read_file";
    pub const TEAM_PREVIEW_FILE: &str = "team_preview_file";
    pub const TEAM_READ_DOWNLOAD: &str = "team_read_download";
    pub const TEAM_CHECK_IPN: &str = "team_check_ipn";
    pub const TEAM_ADD_PART: &str = "team_add_part";
    pub const TEAM_CHECK_MPN: &str = "team_check_mpn";
    pub const TEAM_LIBRARY_CHECK: &str = "team_library_check";
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppStatus {
    pub kicad: KicadStatus,
    /// The kicad-cli path the user chose, if any.
    pub kicad_override: Option<String>,
    /// Why the path the user just entered was not saved.
    pub override_rejected: Option<String>,
    /// Why the saved settings could not be read (defaults are in use).
    pub settings_error: Option<String>,
    pub catalog: CatalogStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum KicadStatus {
    Ready {
        version: String,
        cli_path: String,
        from_override: bool,
    },
    Missing {
        searched: Vec<String>,
    },
    Unsupported {
        version: String,
        cli_path: String,
        supported_major: u32,
    },
    OverrideInvalid {
        path: String,
        problem: String,
    },
    Broken {
        cli_path: String,
        message: String,
    },
}

impl KicadStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CatalogStatus {
    pub root: String,
    pub parts: usize,
    /// How many CSV files the catalog was read from.
    pub csv_files: usize,
    /// Why the catalog could not be loaded, if it could not.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchRequest {
    pub query: String,
    /// Most parts shown per category section.
    pub limit: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchResponse {
    pub query: String,
    /// Parts matching the query, in every section.
    pub total: usize,
    /// One section per category that has matches, in name order.
    pub sections: Vec<SearchSection>,
    pub elapsed_micros: u64,
}

impl SearchResponse {
    /// Every part shown, section by section.
    pub fn hits(&self) -> impl Iterator<Item = &PartSummary> {
        self.sections.iter().flat_map(|section| &section.hits)
    }

    /// How many parts are shown: at most `limit` per section.
    pub fn shown(&self) -> usize {
        self.sections.iter().map(|section| section.hits.len()).sum()
    }
}

/// The parts of one category that match a search.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchSection {
    /// The category's code, such as `res`.
    pub category: String,
    /// Its plain name, such as `Resistors`.
    pub name: String,
    /// Parts of this category matching the query, before `limit`.
    pub total: usize,
    /// At most `limit` of them, best match first.
    pub hits: Vec<PartSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartSummary {
    pub ipn: String,
    /// The CSV file the part was read from, such as `g-res.csv`.
    pub csv_file: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartDetail {
    pub summary: PartSummary,
    pub symbol: Option<String>,
    pub footprint: Option<String>,
    pub datasheet: String,
    /// Every non-blank catalog column, in source order.
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Field {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    Symbol,
    Footprint,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewRequest {
    pub ipn: String,
    pub kind: PreviewKind,
    /// The Symbol or Footprint field value to preview instead of the CSV
    /// catalog's, such as the team catalog's current revision.
    #[serde(default)]
    pub field: Option<String>,
    /// Draw what this revision of the part in the team catalog uses, from
    /// the stored bytes of its exact CAD item revision.
    #[serde(default)]
    pub team_revision: Option<u32>,
    /// Which selection the request is for; it grows with every new
    /// selection. Waiting exports for older selections are dropped.
    #[serde(default)]
    pub selection: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PreviewOutcome {
    Ready(PreviewSet),
    Problem(PreviewProblem),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewSet {
    pub kind: PreviewKind,
    /// The previewed library reference.
    pub reference: String,
    /// Other footprints the catalog lists for the part (after `;`).
    pub alternates: Vec<String>,
    /// The library file drawn from; blank when drawn from stored bytes.
    pub library_path: String,
    pub from_kicad_bundle: bool,
    /// When drawn from the team catalog's stored bytes, the number of the
    /// CAD item revision they belong to.
    #[serde(default)]
    pub cad_item_revision: Option<u32>,
    /// Something the person should know about where the drawing came from,
    /// such as the stored bytes not being available.
    #[serde(default)]
    pub note: Option<String>,
    pub kicad_version: String,
    pub library_sha256: String,
    /// True when served from the cache without running kicad-cli.
    pub from_cache: bool,
    pub images: Vec<PreviewImage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewImage {
    /// Symbol unit number (1-based); `None` for a footprint.
    pub unit: Option<u32>,
    /// The unit's alternate (De Morgan) body style.
    pub demorgan: bool,
    /// `data:image/svg+xml;base64,...` of the kicad-cli SVG.
    pub data_uri: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewProblem {
    pub kind: PreviewKind,
    pub reason: PreviewProblemReason,
    /// The library reference, when the part has one.
    pub reference: Option<String>,
    pub message: String,
    /// The kicad-cli command line, for failed exports.
    pub command: Option<String>,
    /// What kicad-cli printed, for failed exports.
    pub cli_message: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewProblemReason {
    /// The part has no symbol or footprint assigned.
    Unassigned,
    /// The library reference does not resolve to a library on disk.
    Unresolved,
    /// The team catalog's stored revision needs a sign-in to read.
    TeamSignInNeeded,
    /// A newer selection replaced the request before its export started.
    Replaced,
    /// KiCad is missing or unsupported; see [`AppStatus::kicad`].
    KicadUnavailable,
    /// kicad-cli ran and failed.
    ExportFailed,
    /// The name cannot be passed to kicad-cli (it begins with `-`).
    Unexportable,
    UnknownPart,
    Internal,
}

// ---- The team catalog ----------------------------------------------------
//
// The shared, revisioned Catalog on the team's PocketBase server. The app
// signs in with an email and password, keeps the session only in memory,
// and saves just the server address and email.

/// The team catalog connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamStatus {
    /// The saved or suggested server address, such as
    /// `http://127.0.0.1:8090`; blank if none.
    pub server_url: String,
    /// The saved or suggested email; blank if none.
    pub email: String,
    pub session: TeamSession,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TeamSession {
    SignedOut {
        /// Why the last sign-in failed, if it did.
        problem: Option<String>,
    },
    SignedIn {
        name: String,
        email: String,
        /// Marked as able to edit the Catalog; otherwise read-only.
        can_edit: bool,
        live: LiveUpdates,
        /// The `seq` of the last Change this app has seen. It grows when
        /// anyone publishes a Change, which tells the UI to reload.
        checkpoint: u64,
        /// Grows when a CSV import run finishes (which may find CSV changes
        /// without publishing a Change) and after catching up, which also
        /// tells the UI to reload.
        #[serde(default)]
        imports: u64,
    },
}

impl TeamSession {
    pub fn is_signed_in(&self) -> bool {
        matches!(self, Self::SignedIn { .. })
    }
}

/// Whether changes made by others arrive by themselves.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LiveUpdates {
    Connecting,
    Live,
    /// The connection dropped; the app reconnects and catches up.
    Reconnecting {
        message: String,
    },
    /// The server no longer accepts this sign-in (for example the password
    /// changed). The app stops retrying; sign out and sign in again.
    SignInAgain {
        message: String,
    },
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignInRequest {
    pub server_url: String,
    pub email: String,
    pub password: String,
}

/// Never prints the password.
impl std::fmt::Debug for SignInRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignInRequest")
            .field("server_url", &self.server_url)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// One part in the team catalog: its current details, its current
/// revision, and its history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamPart {
    pub ipn: String,
    /// What the part is, shared by every revision.
    pub details: CurrentDetails,
    /// The exact symbol and footprint new designs get: the newest revision
    /// that is not archived.
    pub revision: CurrentRevision,
    /// Every revision of the part, oldest first, and whether it is archived.
    pub revisions: Vec<RevisionStatus>,
    /// One entry per Change that saved this part, newest first.
    pub history: Vec<HistoryEntry>,
    /// New CSV values that were not applied because the part was edited in
    /// this app, and nobody has reviewed yet: the CSV changed after the
    /// edit. At most one for the details and one for the symbol and
    /// footprint.
    #[serde(default)]
    pub csv_changes: Vec<CsvChanges>,
    /// Every category, by name, for the Category choice in Edit part.
    #[serde(default)]
    pub categories: Vec<TeamCategory>,
}

/// A category a part can be in: the only grouping people see.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamCategory {
    /// Its code, such as `res`.
    pub code: String,
    /// Its plain name, such as `Resistors`.
    pub name: String,
    /// The fields a part in it has, in order.
    pub fields: Vec<String>,
}

/// New CSV values the latest import found for a part that was edited in
/// this app, so the import left the part as it is.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CsvChanges {
    /// The import problem that recorded them.
    pub problem: String,
    /// The CSV file, such as `g-res.csv`.
    pub csv_file: String,
    /// True for the symbol and footprint (taking them adds a revision),
    /// false for the part details.
    pub cad: bool,
    /// The current details number (`cad` false) or revision number (`cad`
    /// true) the rows compare with.
    pub expected: u32,
    /// Each field whose CSV value differs from the part's.
    pub rows: Vec<CsvChangeRow>,
}

/// One field whose value in the CSV differs from the part's.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CsvChangeRow {
    /// A details field name, `Category`, `Manufacturer parts`, `Symbol` or
    /// `Footprint`.
    pub field: String,
    pub now: String,
    pub csv: String,
    /// The CSV changed this value since it was last applied or reviewed.
    /// False for a value the CSV already had (the part was edited away from
    /// it): accepting it undoes that edit.
    #[serde(default)]
    pub changed_in_csv: bool,
    /// The CSV's value cannot be accepted here, only kept: the CSV's file
    /// for a symbol or footprint changed, but that symbol or footprint was
    /// changed in this app, so the Catalog does not have the CSV's file.
    /// Choose the file in Edit part to use it.
    #[serde(default)]
    pub keep_only: bool,
}

/// Accept the CSV's values for the fields in `accept` and keep the part's
/// for the rest; an empty `accept` keeps all of them. The server takes the
/// values from what the import recorded, not from this request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamCsvReviewRequest {
    /// As in [`TeamEditRequest`].
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub ipn: String,
    /// [`CsvChanges::problem`].
    pub problem: String,
    pub accept: Vec<String>,
    /// [`CsvChanges::expected`].
    pub expected: u32,
    pub reason: String,
}

/// One revision of a part and whether it is archived. An archived revision
/// is hidden from KiCad and the app, so nobody new picks it; designs that
/// already use it keep it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RevisionStatus {
    pub number: u32,
    pub archived: bool,
}

/// A part's current details: every describing field and its
/// manufacturer parts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CurrentDetails {
    /// Counts each save of the details; never shown as a revision.
    pub number: u32,
    /// The code of the part's category, such as `res`.
    #[serde(default)]
    pub category: String,
    /// Its plain name, such as `Resistors`.
    #[serde(default)]
    pub category_name: String,
    /// Every field, blanks included, in order.
    pub fields: Vec<Field>,
    /// The first is the main one.
    pub manufacturer_parts: Vec<ManufacturerPart>,
}

impl CurrentDetails {
    pub fn field(&self, name: &str) -> Option<&str> {
        find_field(&self.fields, name)
    }
}

/// A part's current revision: its Symbol and Footprint fields and the
/// exact CAD item revision each reference uses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CurrentRevision {
    pub number: u32,
    /// The Symbol and Footprint fields exactly as saved, in order.
    pub cad_fields: Vec<Field>,
    /// The exact CAD item revision each symbol and footprint reference uses.
    pub cad_items: Vec<CadItemUse>,
    /// Excluded from BOM, in plain words: set from the CAD's own flags
    /// when the revision was added, and never changed afterwards.
    #[serde(default)]
    pub excluded_from_bom: String,
}

impl CurrentRevision {
    pub fn field(&self, name: &str) -> Option<&str> {
        find_field(&self.cad_fields, name)
    }
}

fn find_field<'a>(fields: &'a [Field], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.value.as_str())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManufacturerPart {
    pub manufacturer: String,
    pub mpn: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CadItemUse {
    /// `Symbol` or `Footprint`.
    pub role: String,
    pub reference: String,
    /// Plain words, such as `revision 1`, `KiCad library` or `not found`.
    pub binding: String,
}

/// What one Change did to a part: added a revision, saved its details, or
/// both; or archived or unarchived one revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryEntry {
    /// The Change's number, which also tells entries apart.
    pub seq: u64,
    /// The revision this Change added, if it added one.
    pub revision: Option<u32>,
    /// The number of the part details this Change saved, if it saved them.
    pub details: Option<u32>,
    /// The revision this Change archived or unarchived, if it did.
    #[serde(default)]
    pub archive: Option<ArchiveNote>,
    /// `Imported`, `Edited`, `Archived`, `Unarchived`, `Accepted CSV
    /// values`, `Kept current part details` or `Kept current symbol and
    /// footprint`.
    pub action: String,
    pub actor_name: String,
    /// When it was saved, as `2026-10-09 18:02 UTC`.
    pub saved_at: String,
    pub reason: String,
    /// What changed compared with the details and revision before it; empty
    /// for the part's first entry.
    pub changes: Vec<DetailChange>,
}

/// What an archive or unarchive did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArchiveNote {
    pub revision: u32,
    /// True for an archive, false for an unarchive.
    pub archived: bool,
    /// The part's current revision afterwards.
    pub current_after: u32,
}

/// Archive (or unarchive) one revision of a part, with a reason.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamArchiveRequest {
    /// As in [`TeamEditRequest`].
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub ipn: String,
    pub revision: u32,
    /// True to archive, false to unarchive.
    pub archived: bool,
    pub reason: String,
}

/// One difference between a part's details or CAD before and after a change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DetailChange {
    pub label: String,
    pub before: String,
    pub after: String,
}

/// A change to a part: every details field and the full list of
/// manufacturer parts, and the Symbol and Footprint fields, each with the
/// number it started from. Only what changed is saved: new details add no
/// revision, and a new Symbol or Footprint adds one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamEditRequest {
    /// Names this save attempt. Retrying the same save with the same ID
    /// replays the answer instead of conflicting with yourself; `None`
    /// makes a new one.
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub ipn: String,
    /// The number of the part details the edit starts from.
    pub expected_details: u32,
    /// The revision the edit starts from.
    pub expected_revision: u32,
    pub reason: String,
    /// The code of the part's category, changed or not; blank keeps it. A
    /// new category keeps every field and the IPN, and adds the fields of
    /// the new category the part lacks.
    #[serde(default)]
    pub category: String,
    pub fields: Vec<Field>,
    pub manufacturer_parts: Vec<ManufacturerPart>,
    pub cad_fields: Vec<Field>,
    /// Files to use as the Symbol, the Footprint or a footprint's 3D model.
    /// A symbol or footprint file is for the reference its field names
    /// when saving.
    #[serde(default)]
    pub files: Vec<TeamFile>,
}

/// What a chosen file is for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Symbol,
    Footprint,
    Model3d,
}

/// A file chosen in Edit part.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamFile {
    pub kind: FileKind,
    /// The file's name, without its folder.
    pub file_name: String,
    /// The file's bytes, in standard base64.
    pub base64: String,
    /// For a symbol library, the symbol chosen in it.
    #[serde(default)]
    pub symbol: Option<String>,
    /// For a 3D model, the `3d-models/...` path the footprint names.
    #[serde(default)]
    pub model_key: Option<String>,
    /// For a symbol or footprint file, what its field said before the file
    /// was chosen, so the form can put it back; saving does not use it.
    #[serde(default)]
    pub replaces: Option<String>,
    /// Where the file came from when it is part of a download, such as
    /// `KiCad/LM317.kicad_sym in LM317.zip`; saved as its source instead of
    /// the file name.
    #[serde(default)]
    pub label: Option<String>,
}

/// Read a chosen symbol library or footprint file, for Edit part.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamFileRequest {
    pub ipn: String,
    /// [`FileKind::Symbol`] or [`FileKind::Footprint`].
    pub kind: FileKind,
    pub file_name: String,
    /// The file's bytes, in standard base64.
    pub base64: String,
    /// The reference the field names now, such as
    /// `g-Resistor_SMD:R_0603_1608Metric`.
    pub current_reference: String,
}

/// What a chosen file holds, and the reference it suggests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamFileReading {
    /// The symbols in a symbol library, in file order, or the footprint's
    /// one name.
    pub names: Vec<String>,
    /// The name chosen first: the one named like the current reference,
    /// otherwise the first.
    pub chosen: String,
    /// The library suggested for the reference: the current reference's
    /// when it is one of the Catalog's (`g-`) libraries, otherwise one from
    /// the part's CSV file (symbols) or the footprint's package. Blank when
    /// nothing says, so the person types it.
    pub library: String,
    /// For a footprint, the 3D models it names in the Catalog's 3D model
    /// folder that the Catalog does not have yet, as `3d-models/...` paths.
    pub missing_models: Vec<String>,
}

/// Draw a chosen symbol or footprint file before it is saved.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilePreviewRequest {
    /// The file and, for a symbol library, the symbol chosen.
    pub file: TeamFile,
    /// The reference it will have, such as `g-res:R_US`.
    pub reference: String,
    /// As in [`PreviewRequest`].
    #[serde(default)]
    pub selection: Option<u64>,
}

// ---- Add part ----------------------------------------------------------------
//
// An engineer adds a new part from the CAD files they downloaded (SnapEDA,
// Ultra Librarian, SamacSys or a manufacturer's site): a ZIP file or loose
// .kicad_sym, .kicad_mod and 3D model files (kicad-parts-9693).

/// A file chosen for Add part, as it was downloaded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DownloadedFile {
    /// The file's name, without its folder.
    pub file_name: String,
    /// The file's bytes, in standard base64.
    pub base64: String,
}

/// Read the files chosen for Add part.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamDownloadRequest {
    pub files: Vec<DownloadedFile>,
    /// The symbol and the footprint chosen, by their place in
    /// [`TeamDownload::symbols`] and [`TeamDownload::footprints`]; `None`
    /// takes the first.
    #[serde(default)]
    pub symbol: Option<usize>,
    #[serde(default)]
    pub footprint: Option<usize>,
    /// Leave out [`TeamDownload::model_files`], which a read again with
    /// another symbol or footprint chosen does not change.
    #[serde(default)]
    pub without_models: bool,
    /// Instead of `files`, a symbol and footprint already in our libraries
    /// (kicad-parts-37a7).
    #[serde(default)]
    pub library: Option<TeamLibraryChoice>,
}

/// A symbol and a footprint chosen from our libraries (the CSV catalog's
/// `symbols/g-*.kicad_sym` and `footprints/g-*.pretty`) for Add part,
/// such as `g-ics:IC_TP4333`; either may be left out.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamLibraryChoice {
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub footprint: Option<String>,
}

/// What Add part makes of a symbol and footprint from our libraries.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamLibraryReading {
    /// The symbol and footprint used, exactly as in our libraries, such as
    /// `g-ics:IC_TP4333`; blank when none is chosen. The part uses these
    /// references unchanged.
    pub symbol: String,
    pub footprint: String,
    /// The footprint's 3D model in our library, such as
    /// `3d-models/g-Package_SO.3dshapes/SOIC-8.step`; blank when it links
    /// none there.
    pub model: String,
    /// The commit the files were read from; blank when they are not all
    /// committed as they are.
    pub commit: String,
    /// The symbol, footprint and 3D model the Catalog already has, which the
    /// part uses as the Catalog has them.
    pub in_catalog: Vec<String>,
    /// What stops the save, in plain words, such as a 3D model link outside
    /// our library or files with changes that are not committed.
    pub problems: Vec<String>,
    /// What the library check found about these items that does not stop
    /// the save.
    pub findings: Vec<String>,
}

/// Which parts already have an MPN, for Add part.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamMpnRequest {
    pub mpn: String,
    pub manufacturer: String,
}

/// What a download holds and what the app makes of it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamDownload {
    /// Where the files came from, when a ZIP's layout says, such as
    /// `SnapEDA`; blank otherwise.
    pub from: String,
    /// How many files were left out, such as files for other CAD tools.
    pub ignored: usize,
    /// Each symbol in the download, such as `LM317 in KiCad/LM317.kicad_sym
    /// in LM317.zip`.
    pub symbols: Vec<String>,
    pub symbol: Option<usize>,
    /// Each footprint in the download.
    pub footprints: Vec<String>,
    pub footprint: Option<usize>,
    /// Each 3D model in the download.
    pub models: Vec<String>,
    /// The 3D model the footprint is linked to, if any.
    pub model: Option<usize>,
    /// The chosen symbol's library file, with the symbol named.
    pub symbol_file: Option<TeamFile>,
    /// The chosen footprint file.
    pub footprint_file: Option<TeamFile>,
    /// Every 3D model file, in the order of `models`.
    pub model_files: Vec<TeamFile>,
    /// The symbol's and footprint's own names.
    pub symbol_name: String,
    pub footprint_name: String,
    /// The footprint group suggested from the footprint's name (by package,
    /// such as `g-Resistor_SMD`); blank when the name does not say.
    pub footprint_library: String,
    /// What the symbol says, blank when it does not.
    pub mpn: String,
    pub manufacturer: String,
    /// Every category, with the fields a new part in it gets, filled from
    /// the symbol, and the next free IPN.
    pub categories: Vec<TeamNewCategory>,
    /// The best guesses of the category, best first, at most three; empty
    /// when no clue matches.
    pub guesses: Vec<TeamGuess>,
    /// Parts that already have the symbol's MPN.
    pub same_mpn: Vec<TeamSameMpn>,
    /// Footprints the Catalog or KiCad already has that match the chosen
    /// one.
    pub known_footprints: Vec<TeamKnownFootprint>,
    /// How many distinct pins the symbol has, and how many distinct
    /// numbered pads the footprint has.
    pub pins: Option<u32>,
    pub pads: Option<u32>,
    /// Plain notes about the files, such as a file that could not be read
    /// or how the footprint finds its 3D model.
    pub notes: Vec<String>,
    /// For a symbol and footprint from our libraries, what the app made of
    /// them; `None` for a download.
    #[serde(default)]
    pub library: Option<TeamLibraryReading>,
}

/// A category as Add part offers it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamNewCategory {
    pub code: String,
    pub name: String,
    /// Its IPN prefix, such as `CAP`; blank when it has none.
    #[serde(default)]
    pub ipn_prefix: String,
    /// The next free IPN for its IPN prefix, such as `ICS-0047-0001`; blank
    /// when it has no prefix.
    pub next_ipn: String,
    /// Its fields, in order, filled from the symbol where it says.
    pub fields: Vec<Field>,
    /// What the symbol says that fills none of the fields, so nothing is
    /// silently lost.
    pub unused: Vec<Field>,
    /// The footprint group to suggest in this category, such as
    /// `g-Capacitor_SMD`: the download's own when its footprint says, else
    /// one the category suggests; blank when neither does.
    #[serde(default)]
    pub footprint_library: String,
}

/// A guess of the new part's category.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamGuess {
    pub code: String,
    pub name: String,
    /// Why, strongest first, such as `footprint C_0603_1608Metric matches
    /// C_*`.
    pub reasons: Vec<String>,
}

/// A part that already has the MPN.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamSameMpn {
    pub ipn: String,
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    /// The part already lists this manufacturer with this MPN, so adding it
    /// as a manufacturer part would add nothing.
    pub listed: bool,
}

/// A footprint the Catalog or KiCad already has, which a new part can use
/// instead of storing a near-copy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamKnownFootprint {
    /// Such as `g-Resistor_SMD:R_0603_1608Metric`, or
    /// `Resistor_SMD:R_0603_1608Metric` for KiCad's own.
    pub reference: String,
    /// Why it matches, such as `the same pads`.
    pub why: String,
}

/// What the library check found (kicad-parts-da65): the CSV catalog's own
/// libraries compared with the team catalog. Shown to people who can edit.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryCheckView {
    /// Errors: they stop an import, and Add part or Edit part with them.
    pub to_fix: usize,
    /// Symbols, footprints and 3D models no part uses and the Catalog does
    /// not have yet.
    pub not_in_catalog: usize,
    pub worth_a_look: usize,
    /// Old problems of footprints the Catalog has unchanged.
    pub clean_up: usize,
    /// Whether KiCad's own libraries were found, so symbols naming one of
    /// KiCad's footprints were checked too.
    pub kicad_footprints_checked: bool,
    /// The groups that have items, in order: To fix, Not in the Catalog
    /// yet, Worth a look, Clean-up.
    pub groups: Vec<LibraryCheckGroup>,
}

impl LibraryCheckView {
    /// Whether the check found anything at all.
    pub fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.items.is_empty())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryCheckGroup {
    /// Such as `To fix`.
    pub title: String,
    /// One plain sentence saying what the group means.
    pub hint: String,
    pub items: Vec<LibraryCheckItem>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryCheckItem {
    /// Such as `g-ics`; blank for a 3D model or a file that could not be
    /// read, which `name` names by its path.
    pub library: String,
    /// Such as `IC_TP4333`, or a repository path such as
    /// `3d-models/g-Crystal.3dshapes/X.step`.
    pub name: String,
    /// The repository file, such as `symbols/g-ics.kicad_sym`.
    pub file: String,
    pub message: String,
}

/// Is this IPN free for a new part?
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamIpnRequest {
    pub ipn: String,
    /// The IPN prefix of the category chosen, such as `CAP`; blank when it
    /// has none.
    #[serde(default)]
    pub prefix: String,
}

/// The answer: `None` when the IPN can be used, otherwise why not.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamIpnCheck {
    pub problem: Option<String>,
}

/// Save a new part.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamNewPartRequest {
    /// As in [`TeamEditRequest`].
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub ipn: String,
    pub reason: String,
    /// The code of the category the person confirmed.
    pub category: String,
    /// Every field of the category, in its order.
    pub fields: Vec<Field>,
    pub manufacturer_parts: Vec<ManufacturerPart>,
    /// The Symbol and Footprint references, such as `g-reg:LM317`.
    pub symbol: String,
    pub footprint: String,
    /// The symbol file (with the symbol named), the footprint file and its
    /// 3D model file, for the references above; a footprint the Catalog or
    /// KiCad already has needs no file.
    pub files: Vec<TeamFile>,
    /// Where the files came from, such as `SnapEDA`; blank if not known.
    pub from: String,
    /// The names of the downloaded files.
    pub downloads: Vec<String>,
    /// For a part from our libraries: the symbol and footprint chosen
    /// there. The app reads them again from the folder when saving, so
    /// `files` and `downloads` are not used.
    #[serde(default)]
    pub library: Option<TeamLibraryChoice>,
}

/// A symbol or footprint that a saved edit gave a new revision, which other
/// parts still use in an older revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OlderUse {
    /// `symbol` or `footprint`.
    pub kind: String,
    pub reference: String,
    /// The new revision of the symbol or footprint.
    pub number: u32,
    /// How many other parts still use an older revision of it.
    pub other_parts: u32,
}

/// What publishing an edit, archive, unarchive or CSV review did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PublishResult {
    Saved {
        /// The revision the edit added, or `None` when it only changed the
        /// part details.
        revision: Option<u32>,
        part: TeamPart,
        /// Symbols and footprints the edit's files gave a new revision.
        #[serde(default)]
        older_uses: Vec<OlderUse>,
    },
    /// A revision was archived or unarchived.
    Archived {
        revision: u32,
        archived: bool,
        /// The part's current revision afterwards.
        current: u32,
        part: TeamPart,
    },
    /// CSV values were reviewed: some accepted (new details, a new
    /// revision, or both) or none (the part's values kept).
    CsvReviewed {
        /// The fields whose CSV values were accepted; empty if none.
        accepted: Vec<String>,
        /// The part details were saved again.
        details_saved: bool,
        /// The revision added, if a symbol or footprint was taken.
        revision: Option<u32>,
        part: TeamPart,
    },
    /// Nothing would change, so nothing was saved.
    NothingToSave { message: String },
    /// The save worked, but the team server could not be reached afterwards
    /// to show the part as it is now; the app updates by itself when it
    /// catches up.
    SavedNotShown { message: String },
    /// Every change the edit makes is already saved, so nothing
    /// more was saved. `part` is the part as it is now.
    AlreadySaved { message: String, part: TeamPart },
    /// Someone else saved the details or the revision the edit started
    /// from. Nothing was saved.
    Conflict(Box<TeamConflict>),
    /// The server refused it, for example a symbol the Catalog does not
    /// have, or no edit access.
    Refused { message: String },
    /// A new part was saved as `part`.
    Added { part: TeamPart },
    /// Someone took the new part's IPN first; nothing was saved.
    /// `next_free` is the next free IPN for its prefix. When `yours`, you
    /// added that part yourself (a save whose answer was lost, sent again
    /// with changes): open `ipn` rather than adding the part twice.
    IpnTaken {
        message: String,
        next_free: String,
        #[serde(default)]
        ipn: String,
        #[serde(default)]
        yours: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamConflict {
    /// The newest change someone else saved to what the edit changes.
    pub theirs: HistoryEntry,
    /// Every field or list that either side changed.
    pub rows: Vec<ConflictRow>,
    /// What they saved with only your changes applied on top: what "save my
    /// changes on top of theirs" publishes.
    pub merged: TeamEditRequest,
    /// The part as it is now.
    pub part: TeamPart,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConflictRow {
    pub label: String,
    pub started_from: String,
    pub theirs: String,
    pub yours: String,
    /// Both sides changed it differently; yours wins if you save on top.
    pub both_changed: bool,
    /// You changed it; otherwise "yours" is just where you started.
    pub you_changed: bool,
}

/// KiCad's letter for a symbol unit: 1 is A, 26 is Z, 27 is AA.
pub fn unit_label(unit: u32) -> String {
    let mut n = unit;
    let mut letters = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        letters.push(char::from(b'A' + rem as u8));
        n = (n - 1) / 26;
    }
    letters.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_requests_never_print_the_password() {
        let request = SignInRequest {
            server_url: "http://127.0.0.1:8090".into(),
            email: "a@example.test".into(),
            password: "secret-password".into(),
        };
        let printed = format!("{request:?}");
        assert!(printed.contains("a@example.test"), "{printed}");
        assert!(!printed.contains("secret-password"), "{printed}");
    }
    #[test]
    fn unit_labels_follow_kicad_lettering() {
        assert_eq!(unit_label(1), "A");
        assert_eq!(unit_label(26), "Z");
        assert_eq!(unit_label(27), "AA");
    }

    #[test]
    fn statuses_are_tagged_for_the_ui() {
        let json = serde_json::to_value(KicadStatus::Missing {
            searched: vec!["C:\\Program Files".into()],
        })
        .unwrap();
        assert_eq!(json["state"], "missing");
        let outcome = PreviewOutcome::Problem(PreviewProblem {
            kind: PreviewKind::Symbol,
            reason: PreviewProblemReason::ExportFailed,
            reference: Some("g-res:X".into()),
            message: "failed".into(),
            command: None,
            cli_message: Some("There is no symbol selected to save.".into()),
        });
        let round_trip: PreviewOutcome =
            serde_json::from_value(serde_json::to_value(&outcome).unwrap()).unwrap();
        assert_eq!(round_trip, outcome);
    }
}

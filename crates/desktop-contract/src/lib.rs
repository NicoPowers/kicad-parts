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
    pub const TEAM_RESTORE: &str = "team_restore";
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
    pub limit: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchResponse {
    pub query: String,
    /// Rows matching the query before `limit` was applied.
    pub total: usize,
    pub hits: Vec<PartSummary>,
    pub elapsed_micros: u64,
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
    Superseded,
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

/// One part in the team catalog: its current details and its history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamPart {
    pub ipn: String,
    pub current: RevisionDetails,
    /// Newest first; the first is the current revision.
    pub history: Vec<RevisionSummary>,
}

/// The part details one revision saved.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RevisionDetails {
    pub number: u32,
    /// Every field, blanks included, in order.
    pub fields: Vec<Field>,
    /// The first is the main one.
    pub manufacturer_parts: Vec<ManufacturerPart>,
    /// The exact CAD item revision each symbol and footprint reference uses.
    pub cad_items: Vec<CadItemUse>,
}

impl RevisionDetails {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.as_str())
    }
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RevisionSummary {
    pub number: u32,
    /// `Imported`, `Edited` or `Restored`.
    pub action: String,
    pub actor_name: String,
    /// When it was saved, as `2026-10-09 18:02 UTC`.
    pub saved_at: String,
    pub reason: String,
    /// For a restore, the revision whose details it copied.
    pub restored_from: Option<u32>,
    /// What changed compared with the revision before it.
    pub changes: Vec<DetailChange>,
    /// What restoring this revision would change in the current one; empty
    /// for the current revision.
    pub restore_changes: Vec<DetailChange>,
}

/// One difference between two versions of a part's details.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DetailChange {
    pub label: String,
    pub before: String,
    pub after: String,
}

/// A change to part details: every field with its new value, and the full
/// list of manufacturer parts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamEditRequest {
    /// Names this save attempt. Retrying the same save with the same ID
    /// replays the answer instead of conflicting with yourself; `None`
    /// makes a new one.
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub ipn: String,
    /// The revision the edit starts from.
    pub expected_revision: u32,
    pub reason: String,
    pub fields: Vec<Field>,
    pub manufacturer_parts: Vec<ManufacturerPart>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamRestoreRequest {
    /// As in [`TeamEditRequest`].
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub ipn: String,
    pub expected_revision: u32,
    pub restore_revision: u32,
    pub reason: String,
}

/// What publishing an edit or a restore did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PublishResult {
    Saved {
        revision: u32,
        part: TeamPart,
    },
    /// The details would not change, so nothing was saved.
    NothingToSave {
        message: String,
    },
    /// Someone else saved a revision after the one the edit started from.
    /// Nothing was saved.
    Conflict(Box<TeamConflict>),
    /// The server refused it, for example a symbol the Catalog does not
    /// have, or no edit access.
    Refused {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamConflict {
    /// The revision the edit started from.
    pub started_from: u32,
    /// The revision someone else saved, now the current one.
    pub theirs: RevisionSummary,
    /// Every field or list that either side changed.
    pub rows: Vec<ConflictRow>,
    /// Their revision with only your changes applied on top: what "save my
    /// changes on top of theirs" publishes.
    pub merged: TeamEditRequest,
    /// For a restore, the revision it would have copied; "restore anyway"
    /// tries again from their revision.
    pub restore_revision: Option<u32>,
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
            reference: Some("g-pas:X".into()),
            message: "failed".into(),
            command: None,
            cli_message: Some("There is no symbol selected to save.".into()),
        });
        let round_trip: PreviewOutcome =
            serde_json::from_value(serde_json::to_value(&outcome).unwrap()).unwrap();
        assert_eq!(round_trip, outcome);
    }
}

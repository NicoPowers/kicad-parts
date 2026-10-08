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
    pub categories: usize,
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
    pub category: String,
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
    pub library_path: String,
    pub from_kicad_bundle: bool,
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
    /// KiCad is missing or unsupported; see [`AppStatus::kicad`].
    KicadUnavailable,
    /// kicad-cli ran and failed.
    ExportFailed,
    /// The name cannot be passed to kicad-cli (it begins with `-`).
    Unexportable,
    UnknownPart,
    Internal,
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

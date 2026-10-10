//! Part details: what a revision of a part saves.

use crate::hash::sha256_hex;
use serde::{Deserialize, Serialize};

/// The schema tag of [`PartDetails`].
pub const PART_DETAILS_SCHEMA: &str = "kicad-parts/part-details/1";

/// What a part is and how you design with it, as one revision saves it.
///
/// Serialization is canonical: fields serialize in declaration order and
/// every collection is an ordered list, so [`PartDetails::canonical_json`]
/// is byte-for-byte reproducible. Its SHA-256 identifies the revision's
/// content, so any change here, including adding, removing or reordering a
/// manufacturer part, makes a new revision. Supplier data (supplier part
/// numbers and prices) is never part of it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartDetails {
    pub schema: String,
    pub ipn: String,
    /// Every source column that describes the part, in source order, blanks
    /// included, under its exact source name.
    pub fields: Vec<DetailField>,
    /// The first is the main manufacturer part.
    pub manufacturer_parts: Vec<ManufacturerPart>,
    /// The exact CAD this revision uses, one entry per symbol or footprint
    /// reference in source order.
    pub cad_references: Vec<CadReference>,
    /// Excluded from BOM: the part appears in designs but is never bought
    /// or assembled, such as a test point. It is set only from explicit
    /// evidence; when unset it is left out of the canonical JSON, so parts
    /// without it keep the details (and hashes) they always had.
    #[serde(default, skip_serializing_if = "is_unset")]
    pub excluded_from_bom: bool,
}

fn is_unset(setting: &bool) -> bool {
    !*setting
}

impl PartDetails {
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("part details always serialize")
    }

    pub fn sha256(&self) -> String {
        sha256_hex(self.canonical_json())
    }

    /// The value of the field named exactly `name`.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DetailField {
    pub name: String,
    pub value: String,
}

/// A manufacturer and their part number (MPN) approved for an IPN. The
/// part's one datasheet is in its fields, not here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManufacturerPart {
    pub manufacturer: String,
    pub mpn: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CadReferenceRole {
    Symbol,
    Footprint,
}

/// One symbol or footprint reference of a part, and what it resolved to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CadReference {
    pub role: CadReferenceRole,
    /// Position among the references of this role; 0 is the default (KiCad
    /// database libraries list footprint choices separated by `;`).
    pub ordinal: u32,
    /// The reference exactly as the source wrote it, such as `g-pas:R_US`.
    pub reference: String,
    pub target: CadReferenceTarget,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CadReferenceTarget {
    /// One exact revision of a CAD item, named by its CAD item and revision
    /// number (which give its ID) and the SHA-256 of that revision's
    /// snapshot (so a reader can check the record it finds).
    CadItemRevision {
        cad_item: String,
        number: u32,
        snapshot_sha256: String,
    },
    /// A library outside the CSV catalog, such as KiCad's bundled `Device`
    /// library. Library Sources will own these.
    External,
    /// A local reference that did not resolve; an import problem says why.
    Unresolved { status: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_is_stable_and_ordered() {
        let details = PartDetails {
            schema: PART_DETAILS_SCHEMA.into(),
            ipn: "RES-0000-0470".into(),
            fields: vec![
                DetailField {
                    name: "Value".into(),
                    value: "47".into(),
                },
                DetailField {
                    name: "Tolerance".into(),
                    value: String::new(),
                },
            ],
            manufacturer_parts: vec![ManufacturerPart {
                manufacturer: "Yageo".into(),
                mpn: "RC0603FR-0747RL".into(),
            }],
            cad_references: vec![CadReference {
                role: CadReferenceRole::Symbol,
                ordinal: 0,
                reference: "Device:R".into(),
                target: CadReferenceTarget::External,
            }],
            excluded_from_bom: false,
        };
        let json = details.canonical_json();
        assert_eq!(
            json,
            r#"{"schema":"kicad-parts/part-details/1","ipn":"RES-0000-0470","fields":[{"name":"Value","value":"47"},{"name":"Tolerance","value":""}],"manufacturer_parts":[{"manufacturer":"Yageo","mpn":"RC0603FR-0747RL"}],"cad_references":[{"role":"symbol","ordinal":0,"reference":"Device:R","target":{"kind":"external"}}]}"#
        );
        let parsed: PartDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, details);
        assert_eq!(parsed.sha256(), details.sha256());
        assert_eq!(details.field("Tolerance"), Some(""));
        // Reordering manufacturer parts changes the details, so it makes a
        // new revision.
        let mut reordered = details.clone();
        reordered.manufacturer_parts.push(ManufacturerPart {
            manufacturer: "Vishay".into(),
            mpn: "ALT-47".into(),
        });
        let before = reordered.sha256();
        reordered.manufacturer_parts.reverse();
        assert_ne!(reordered.sha256(), before);
        // Excluded from BOM is part details too: setting it makes a new
        // revision, and only a set value is written.
        let mut excluded = details.clone();
        excluded.excluded_from_bom = true;
        let excluded_json = excluded.canonical_json();
        assert!(
            excluded_json.ends_with(r#","excluded_from_bom":true}"#),
            "{excluded_json}"
        );
        assert_ne!(excluded.sha256(), details.sha256());
        let parsed: PartDetails = serde_json::from_str(&excluded_json).unwrap();
        assert!(parsed.excluded_from_bom);
    }
}

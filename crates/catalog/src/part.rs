//! Part details, which every revision of a part shares, and part
//! revisions, which hold only the CAD a part uses.

use crate::hash::sha256_hex;
use serde::{Deserialize, Serialize};

/// The schema tag of [`PartDetails`].
pub const PART_DETAILS_SCHEMA: &str = "kicad-parts/part-details/2";
/// The schema tag of [`PartRevision`].
pub const PART_REVISION_SCHEMA: &str = "kicad-parts/part-revision/1";
/// The CSV columns whose values are CAD references, in the order a part
/// revision keeps them. Every other describing column is part details.
pub const CAD_COLUMNS: [&str; 2] = ["Symbol", "Footprint"];

/// What a part is: every field that describes it and its manufacturer
/// parts. Every revision of the part shares them, so changing them saves
/// the part's next details but adds no revision.
///
/// Serialization is canonical: fields serialize in declaration order and
/// every collection is an ordered list, so [`PartDetails::canonical_json`]
/// is byte-for-byte reproducible. Its SHA-256 identifies the saved details,
/// so any change here, including adding, removing or reordering a
/// manufacturer part, saves new details. Supplier data (supplier part
/// numbers and prices) is never part of it, and neither are the Symbol and
/// Footprint fields, which belong to the [`PartRevision`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartDetails {
    pub schema: String,
    pub ipn: String,
    /// Every source column that describes the part, in source order, blanks
    /// included, under its exact source name; never Symbol or Footprint.
    pub fields: Vec<DetailField>,
    /// The first is the main manufacturer part.
    pub manufacturer_parts: Vec<ManufacturerPart>,
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
        find_field(&self.fields, name)
    }
}

/// A part revision: the exact symbol, footprint and 3D model revisions a
/// part uses. Only a change to CAD adds one.
///
/// Its canonical JSON is reproducible, like [`PartDetails`], and its
/// SHA-256 identifies the revision's content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartRevision {
    pub schema: String,
    pub ipn: String,
    /// The Symbol and Footprint fields exactly as the source wrote them
    /// (blanks included, footprint choices separated by `;`), in source
    /// order. A CSV file without such a column has no entry for it.
    pub cad_fields: Vec<DetailField>,
    /// The exact CAD this revision uses, one entry per symbol or footprint
    /// reference in source order.
    pub cad_references: Vec<CadReference>,
    /// Excluded from BOM: the part appears in designs but is never bought
    /// or assembled, such as a test point. It is set only from the CAD the
    /// revision uses; when unset it is left out of the canonical JSON.
    #[serde(default, skip_serializing_if = "is_unset")]
    pub excluded_from_bom: bool,
}

fn is_unset(setting: &bool) -> bool {
    !*setting
}

impl PartRevision {
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("part revisions always serialize")
    }

    pub fn sha256(&self) -> String {
        sha256_hex(self.canonical_json())
    }

    /// The value of the CAD field named exactly `name` (`Symbol` or
    /// `Footprint`).
    pub fn field(&self, name: &str) -> Option<&str> {
        find_field(&self.cad_fields, name)
    }
}

fn find_field<'a>(fields: &'a [DetailField], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.value.as_str())
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

    fn details() -> PartDetails {
        PartDetails {
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
        }
    }

    fn revision() -> PartRevision {
        PartRevision {
            schema: PART_REVISION_SCHEMA.into(),
            ipn: "RES-0000-0470".into(),
            cad_fields: vec![DetailField {
                name: "Symbol".into(),
                value: "Device:R".into(),
            }],
            cad_references: vec![CadReference {
                role: CadReferenceRole::Symbol,
                ordinal: 0,
                reference: "Device:R".into(),
                target: CadReferenceTarget::External,
            }],
            excluded_from_bom: false,
        }
    }

    #[test]
    fn part_details_serialize_canonically_without_cad() {
        let details = details();
        let json = details.canonical_json();
        assert_eq!(
            json,
            r#"{"schema":"kicad-parts/part-details/2","ipn":"RES-0000-0470","fields":[{"name":"Value","value":"47"},{"name":"Tolerance","value":""}],"manufacturer_parts":[{"manufacturer":"Yageo","mpn":"RC0603FR-0747RL"}]}"#
        );
        let parsed: PartDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, details);
        assert_eq!(parsed.sha256(), details.sha256());
        assert_eq!(details.field("Tolerance"), Some(""));
        // Reordering manufacturer parts changes the details.
        let mut reordered = details.clone();
        reordered.manufacturer_parts.push(ManufacturerPart {
            manufacturer: "Vishay".into(),
            mpn: "ALT-47".into(),
        });
        let before = reordered.sha256();
        reordered.manufacturer_parts.reverse();
        assert_ne!(reordered.sha256(), before);
        // A revision is never read as part details.
        assert!(serde_json::from_str::<PartDetails>(&revision().canonical_json()).is_err());
    }

    #[test]
    fn part_revisions_serialize_canonically_with_only_cad() {
        let revision = revision();
        let json = revision.canonical_json();
        assert_eq!(
            json,
            r#"{"schema":"kicad-parts/part-revision/1","ipn":"RES-0000-0470","cad_fields":[{"name":"Symbol","value":"Device:R"}],"cad_references":[{"role":"symbol","ordinal":0,"reference":"Device:R","target":{"kind":"external"}}]}"#
        );
        assert_eq!(revision.field("Symbol"), Some("Device:R"));
        assert_eq!(revision.field("Footprint"), None);
        // Excluded from BOM belongs to the revision, and only a set value
        // is written.
        let mut excluded = revision.clone();
        excluded.excluded_from_bom = true;
        let excluded_json = excluded.canonical_json();
        assert!(
            excluded_json.ends_with(r#","excluded_from_bom":true}"#),
            "{excluded_json}"
        );
        assert_ne!(excluded.sha256(), revision.sha256());
        let parsed: PartRevision = serde_json::from_str(&excluded_json).unwrap();
        assert!(parsed.excluded_from_bom);
        // Part details are never read as a revision.
        assert!(serde_json::from_str::<PartRevision>(&details().canonical_json()).is_err());
    }
}

//! The Part Revision snapshot.

use crate::hash::sha256_hex;
use serde::{Deserialize, Serialize};

/// The schema tag of [`PartRevisionSnapshot`].
pub const PART_REVISION_SCHEMA: &str = "kicad-parts/part-revision/1";

/// An immutable, complete snapshot of a Managed Part's catalog data.
///
/// Serialization is canonical: fields serialize in declaration order and
/// every collection is an ordered list, so [`PartRevisionSnapshot::canonical_json`]
/// is byte-for-byte reproducible and its SHA-256 identifies the revision's
/// content. Operational Facts (supplier part numbers and prices) are never
/// part of it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartRevisionSnapshot {
    pub schema: String,
    pub ipn: String,
    /// Every semantic source column in source order, blanks included, under
    /// its exact source name.
    pub fields: Vec<SnapshotField>,
    /// The first is the primary manufacturer and MPN.
    pub approved_manufacturer_parts: Vec<ApprovedManufacturerPart>,
    /// The exact CAD this revision uses, one entry per symbol or footprint
    /// reference in source order.
    pub asset_bindings: Vec<AssetBinding>,
}

impl PartRevisionSnapshot {
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("a snapshot always serializes")
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
pub struct SnapshotField {
    pub name: String,
    pub value: String,
}

/// A manufacturer and MPN approved to satisfy one IPN.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedManufacturerPart {
    pub manufacturer: String,
    pub mpn: String,
    /// Only Approved Manufacturer Parts after the primary carry their own
    /// datasheet (read from rows of the legacy `substitutes.csv`); the
    /// primary's datasheet is the part's `Datasheet` field.
    pub datasheet: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingRole {
    Symbol,
    Footprint,
}

/// One CAD reference of a Part Revision and what it resolved to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssetBinding {
    pub role: BindingRole,
    /// Position among the references of this role; 0 is the default (KiCad
    /// database libraries list footprint choices separated by `;`).
    pub ordinal: u32,
    /// The reference exactly as the source wrote it, such as `g-pas:R_US`.
    pub reference: String,
    pub target: BindingTarget,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BindingTarget {
    /// An exact Asset Revision of a Catalog Asset.
    AssetRevision {
        asset: String,
        asset_revision_sha256: String,
    },
    /// A library outside the Legacy Catalog, such as KiCad's bundled
    /// `Device` library. Library Sources will own these.
    External,
    /// A local reference that did not resolve; an Import Diagnostic
    /// explains why.
    Unresolved { status: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_is_stable_and_ordered() {
        let snapshot = PartRevisionSnapshot {
            schema: PART_REVISION_SCHEMA.into(),
            ipn: "RES-0000-0470".into(),
            fields: vec![
                SnapshotField {
                    name: "Value".into(),
                    value: "47".into(),
                },
                SnapshotField {
                    name: "Tolerance".into(),
                    value: String::new(),
                },
            ],
            approved_manufacturer_parts: vec![ApprovedManufacturerPart {
                manufacturer: "Yageo".into(),
                mpn: "RC0603FR-0747RL".into(),
                datasheet: None,
            }],
            asset_bindings: vec![AssetBinding {
                role: BindingRole::Symbol,
                ordinal: 0,
                reference: "Device:R".into(),
                target: BindingTarget::External,
            }],
        };
        let json = snapshot.canonical_json();
        assert_eq!(
            json,
            r#"{"schema":"kicad-parts/part-revision/1","ipn":"RES-0000-0470","fields":[{"name":"Value","value":"47"},{"name":"Tolerance","value":""}],"approved_manufacturer_parts":[{"manufacturer":"Yageo","mpn":"RC0603FR-0747RL","datasheet":null}],"asset_bindings":[{"role":"symbol","ordinal":0,"reference":"Device:R","target":{"kind":"external"}}]}"#
        );
        let parsed: PartRevisionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
        assert_eq!(parsed.sha256(), snapshot.sha256());
        assert_eq!(snapshot.field("Tolerance"), Some(""));
    }
}

//! CAD items, CAD item revisions and blobs.

use crate::hash::sha256_hex;
use crate::ids::{BlobId, CadItemId};
use serde::{Deserialize, Serialize};

/// The schema tag of [`CadItemDetails`]. Only the team server writes them;
/// a test keeps this and catalog_lib.js in step.
#[cfg(test)]
pub(crate) const CAD_ITEM_DETAILS_SCHEMA: &str = "kicad-parts/cad-item-details/1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CadItemKind {
    Symbol,
    Footprint,
    // Kebab case alone would give `model3d`; every other place says `model-3d`.
    #[serde(rename = "model-3d")]
    Model3d,
}

impl CadItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Footprint => "footprint",
            Self::Model3d => "model-3d",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Symbol, Self::Footprint, Self::Model3d]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// A symbol, footprint or 3D model that parts use. It keeps its identity
/// when its content changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CadItem {
    pub id: CadItemId,
    pub kind: CadItemKind,
    /// `LIBRARY:NAME` for symbols and footprints; the repository path under
    /// `3d-models/` for 3D models, such as
    /// `3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step`.
    pub key: String,
}

impl CadItem {
    pub fn new(kind: CadItemKind, key: &str) -> Self {
        Self {
            id: CadItemId::new(kind, key),
            kind,
            key: key.to_owned(),
        }
    }
}

/// The stated terms under which a CAD item revision may be used and shared.
///
/// Downloads and libraries rarely state a license the app can read, so
/// every revision says `unknown-needs-review` explicitly.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum License {
    /// No terms were stated; someone must review the CAD item before relying
    /// on any right to use or share it.
    UnknownNeedsReview,
}

impl License {
    /// The value stored in the `license` column.
    pub fn status(&self) -> &'static str {
        match self {
            Self::UnknownNeedsReview => "unknown-needs-review",
        }
    }
}

/// A 3D model reference inside a footprint, and what it resolved to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReference {
    /// Exactly as written in the footprint.
    pub reference: String,
    /// The oracle's lexical scope: `local`, `external-variable`,
    /// `posix-absolute`, and so on.
    pub scope: String,
    pub target: ModelTarget,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ModelTarget {
    /// One exact revision of a 3D model, named like a part's CAD binding
    /// (see `CadReferenceTarget::CadItemRevision`).
    CadItemRevision {
        cad_item: String,
        number: u32,
        snapshot_sha256: String,
    },
    Unresolved {
        status: String,
    },
}

/// What a revision of a CAD item saves: which bytes, how to read them, and
/// the license they come with. Its canonical JSON is stored exactly (the
/// revision's snapshot), and its SHA-256 lets a reader check it. A
/// footprint's snapshot names the exact 3D model revisions it uses, so a
/// new 3D model revision also makes a new footprint revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CadItemDetails {
    pub schema: String,
    pub kind: CadItemKind,
    pub key: String,
    /// How to read the blob: `kicad-symbol-entry` (one top-level
    /// `(symbol ...)` of a `.kicad_sym`), `kicad-footprint` (a whole
    /// `.kicad_mod`), or `3d-model` (a whole model file).
    pub format: String,
    /// The `(version ...)` of the KiCad file the bytes came from, needed to
    /// re-wrap a symbol entry into a library. `None` for 3D models.
    pub format_version: Option<String>,
    pub blob_sha256: String,
    pub blob_size: u64,
    /// For footprints, every 3D model reference in file order.
    pub model_references: Vec<ModelReference>,
    pub license: License,
}

impl CadItemDetails {
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("a snapshot always serializes")
    }

    pub fn sha256(&self) -> String {
        sha256_hex(self.canonical_json())
    }
}

/// Exact bytes, addressed by their SHA-256.
#[derive(Clone, Eq, PartialEq)]
pub struct Blob {
    pub sha256: String,
    pub bytes: Vec<u8>,
}

impl Blob {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            sha256: sha256_hex(&bytes),
            bytes,
        }
    }

    pub fn id(&self) -> BlobId {
        BlobId::new(&self.sha256)
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Blob")
            .field("sha256", &self.sha256)
            .field("size", &self.bytes.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn license_is_explicit_in_the_snapshot() {
        let snapshot = CadItemDetails {
            schema: CAD_ITEM_DETAILS_SCHEMA.into(),
            kind: CadItemKind::Footprint,
            key: "g-Resistor_SMD:R_0603_1608Metric".into(),
            format: "kicad-footprint".into(),
            format_version: Some("20240108".into()),
            blob_sha256: "ab".into(),
            blob_size: 2,
            model_references: vec![],
            license: License::UnknownNeedsReview,
        };
        let json = snapshot.canonical_json();
        assert!(
            json.ends_with(r#""license":{"status":"unknown-needs-review"}}"#),
            "{json}"
        );
        let parsed: CadItemDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sha256(), snapshot.sha256());
        assert_eq!(CadItemKind::parse("model-3d"), Some(CadItemKind::Model3d));
        for kind in [
            CadItemKind::Symbol,
            CadItemKind::Footprint,
            CadItemKind::Model3d,
        ] {
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{}\"", kind.as_str())
            );
        }
    }
}

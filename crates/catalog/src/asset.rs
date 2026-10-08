//! Catalog Assets, Asset Revisions and Asset Blobs.

use crate::hash::sha256_hex;
use crate::ids::{AssetBlobId, CatalogAssetId};
use serde::{Deserialize, Serialize};

/// The schema tag of [`AssetRevisionDeclaration`].
pub const ASSET_REVISION_SCHEMA: &str = "kicad-parts/asset-revision/1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssetKind {
    Symbol,
    Footprint,
    Model3d,
}

impl AssetKind {
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

/// A long-lived logical design artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogAsset {
    pub id: CatalogAssetId,
    pub kind: AssetKind,
    /// `LIBRARY:NAME` for symbols and footprints; the repository path under
    /// `3d-models/` for 3D models.
    pub key: String,
}

impl CatalogAsset {
    pub fn new(kind: AssetKind, key: &str) -> Self {
        Self {
            id: CatalogAssetId::new(kind, key),
            kind,
            key: key.to_owned(),
        }
    }
}

/// The stated terms under which an Asset Revision may be used and shared.
///
/// The Legacy Catalog declares no licence for any asset, so every imported
/// revision carries the explicit `undeclared-needs-review` declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LicenseDeclaration {
    /// No terms were stated; someone must review the asset before relying
    /// on any right to use or share it.
    UndeclaredNeedsReview,
}

impl LicenseDeclaration {
    /// The value stored in the `license_status` column.
    pub fn status(&self) -> &'static str {
        match self {
            Self::UndeclaredNeedsReview => "undeclared-needs-review",
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
    AssetRevision {
        asset: String,
        asset_revision_sha256: String,
    },
    Unresolved {
        status: String,
    },
}

/// Everything that identifies an Asset Revision: what its bytes are, how to
/// read them, and the terms they come with. Its canonical JSON is stored
/// verbatim and its SHA-256 is the revision hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRevisionDeclaration {
    pub schema: String,
    pub kind: AssetKind,
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
    pub license: LicenseDeclaration,
}

impl AssetRevisionDeclaration {
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("a declaration always serializes")
    }

    pub fn sha256(&self) -> String {
        sha256_hex(self.canonical_json())
    }
}

/// Exact bytes, addressed by their SHA-256.
#[derive(Clone, Eq, PartialEq)]
pub struct AssetBlob {
    pub sha256: String,
    pub bytes: Vec<u8>,
}

impl AssetBlob {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            sha256: sha256_hex(&bytes),
            bytes,
        }
    }

    pub fn id(&self) -> AssetBlobId {
        AssetBlobId::new(&self.sha256)
    }
}

impl std::fmt::Debug for AssetBlob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AssetBlob")
            .field("sha256", &self.sha256)
            .field("size", &self.bytes.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licence_is_explicit_in_the_declaration() {
        let declaration = AssetRevisionDeclaration {
            schema: ASSET_REVISION_SCHEMA.into(),
            kind: AssetKind::Footprint,
            key: "g-res:R_0603_1608Metric".into(),
            format: "kicad-footprint".into(),
            format_version: Some("20240108".into()),
            blob_sha256: "ab".into(),
            blob_size: 2,
            model_references: vec![],
            license: LicenseDeclaration::UndeclaredNeedsReview,
        };
        let json = declaration.canonical_json();
        assert!(
            json.ends_with(r#""license":{"status":"undeclared-needs-review"}}"#),
            "{json}"
        );
        let parsed: AssetRevisionDeclaration = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sha256(), declaration.sha256());
        assert_eq!(AssetKind::parse("model-3d"), Some(AssetKind::Model3d));
    }
}

//! Stable record identities.
//!
//! Every Catalog identity is derived from what the record *is*, never from
//! when it was stored: `derive_id(kind, key)` is the first
//! [`ID_LENGTH`] hex digits of `SHA-256("kicad-parts:<kind>:v1\n<key>")`.
//! The result is a valid PocketBase record ID, and the protected import
//! route recomputes it with the same rule (`infra/pocketbase/pb_hooks/
//! catalog_lib.js`), so a client and the server can never disagree about
//! which record an IPN or an asset maps to.

use crate::hash::sha256_hex;
use crate::ipn::Ipn;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Characters in every derived ID (60 bits). Before reusing a stored record
/// found by a derived ID, the import route compares its content hash or key
/// with the request and answers `id_collision` on a mismatch, so a
/// collision is refused rather than merged.
pub const ID_LENGTH: usize = 15;

/// The first [`ID_LENGTH`] hex digits of `SHA-256("kicad-parts:<kind>:v1\n<key>")`.
pub fn derive_id(kind: &str, key: &str) -> String {
    sha256_hex(format!("kicad-parts:{kind}:v1\n{key}"))[..ID_LENGTH].to_owned()
}

macro_rules! derived_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// An ID read back from the store.
            pub fn from_stored(id: &str) -> Self {
                Self(id.to_owned())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

derived_id!(
    /// A Managed Part, from its IPN.
    ManagedPartId
);
derived_id!(
    /// A Part Revision, from its Managed Part and revision number.
    PartRevisionId
);
derived_id!(
    /// A Catalog Asset, from its kind and library reference.
    CatalogAssetId
);
derived_id!(
    /// An Asset Revision, from its Catalog Asset and revision hash.
    AssetRevisionId
);
derived_id!(
    /// An Asset Blob, from the SHA-256 of its bytes.
    AssetBlobId
);
derived_id!(
    /// A Change Set, from the correlation ID of the command that created it.
    ChangeSetId
);
derived_id!(
    /// The Audit Event recording one Change Set.
    AuditEventId
);
derived_id!(
    /// An Import Diagnostic, from its code, subject, location and detail.
    ImportDiagnosticId
);

impl ManagedPartId {
    pub fn for_ipn(ipn: &Ipn) -> Self {
        Self(derive_id("managed-part", ipn.as_str()))
    }

    /// For a source IPN that does not parse; such rows never become parts,
    /// so this exists only for diagnostics and tests.
    pub fn for_text(ipn: &str) -> Self {
        Self(derive_id("managed-part", ipn))
    }
}

impl PartRevisionId {
    pub fn new(part: &ManagedPartId, number: u32) -> Self {
        Self(derive_id("part-revision", &format!("{part}\n{number}")))
    }
}

impl CatalogAssetId {
    pub fn new(kind: crate::AssetKind, key: &str) -> Self {
        Self(derive_id(
            "catalog-asset",
            &format!("{}\n{key}", kind.as_str()),
        ))
    }
}

impl AssetRevisionId {
    pub fn new(asset: &CatalogAssetId, revision_sha256: &str) -> Self {
        Self(derive_id(
            "asset-revision",
            &format!("{asset}\n{revision_sha256}"),
        ))
    }
}

impl AssetBlobId {
    pub fn new(sha256: &str) -> Self {
        Self(derive_id("asset-blob", sha256))
    }
}

impl ChangeSetId {
    pub fn new(correlation_id: &str) -> Self {
        Self(derive_id("change-set", correlation_id))
    }
}

impl AuditEventId {
    pub fn new(change_set: &ChangeSetId) -> Self {
        Self(derive_id("audit-event", change_set.as_str()))
    }
}

impl ImportDiagnosticId {
    pub fn new(code: &str, subject: &str, location: &str, detail: &str) -> Self {
        Self(derive_id(
            "import-diagnostic",
            &format!("{code}\n{subject}\n{location}\n{}", sha256_hex(detail)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_valid_pocketbase_ids() {
        let ipn = Ipn::parse("RES-0000-0470").unwrap();
        let id = ManagedPartId::for_ipn(&ipn);
        assert_eq!(id, ManagedPartId::for_ipn(&ipn));
        assert_eq!(id.as_str().len(), ID_LENGTH);
        assert!(
            id.as_str()
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch.is_ascii_lowercase())
        );
        // Pinned: the server derives the same value (see catalog_lib.js),
        // and stored records depend on it never changing.
        assert_eq!(id.as_str(), "d0469c29a4560b9");
        assert_eq!(
            derive_id("managed-part", "RES-0000-0470"),
            sha256_hex("kicad-parts:managed-part:v1\nRES-0000-0470")[..15]
        );
        assert_ne!(
            PartRevisionId::new(&id, 1).as_str(),
            PartRevisionId::new(&id, 2).as_str()
        );
    }
}

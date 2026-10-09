//! Stable record identities.
//!
//! Every Catalog identity is derived from what the record *is*, never from
//! when it was stored: `derive_id(kind, key)` is the first
//! [`ID_LENGTH`] hex digits of `SHA-256("kicad-parts:<kind>:v1\n<key>")`.
//! The result is a valid PocketBase record ID, and the protected import
//! route recomputes it with the same rule (`infra/pocketbase/pb_hooks/
//! catalog_lib.js`), so a client and the server can never disagree about
//! which record an IPN or a CAD item maps to.

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
    /// A part, from its IPN.
    PartId
);
derived_id!(
    /// A part revision, from its part and revision number.
    PartRevisionId
);
derived_id!(
    /// A CAD item, from its kind and library reference.
    CadItemId
);
derived_id!(
    /// A CAD item revision, from its CAD item and revision hash.
    CadItemRevisionId
);
derived_id!(
    /// A blob, from the SHA-256 of its bytes.
    BlobId
);
derived_id!(
    /// A Change, from the correlation ID of the command that created it.
    ChangeId
);
derived_id!(
    /// The audit entry of one import run, from its correlation ID.
    AuditEntryId
);
derived_id!(
    /// An import problem, from its code, subject, location and detail.
    ImportProblemId
);

impl PartId {
    pub fn for_ipn(ipn: &Ipn) -> Self {
        Self(derive_id("part", ipn.as_str()))
    }

    /// For a source IPN that does not parse; such rows never become parts,
    /// so this exists only for problems and tests.
    pub fn for_text(ipn: &str) -> Self {
        Self(derive_id("part", ipn))
    }
}

impl PartRevisionId {
    pub fn new(part: &PartId, number: u32) -> Self {
        Self(derive_id("part-revision", &format!("{part}\n{number}")))
    }
}

impl CadItemId {
    pub fn new(kind: crate::CadItemKind, key: &str) -> Self {
        Self(derive_id("cad-item", &format!("{}\n{key}", kind.as_str())))
    }
}

impl CadItemRevisionId {
    pub fn new(cad_item: &CadItemId, snapshot_sha256: &str) -> Self {
        Self(derive_id(
            "cad-item-revision",
            &format!("{cad_item}\n{snapshot_sha256}"),
        ))
    }
}

impl BlobId {
    pub fn new(sha256: &str) -> Self {
        Self(derive_id("blob", sha256))
    }
}

impl ChangeId {
    pub fn new(correlation_id: &str) -> Self {
        Self(derive_id("change", correlation_id))
    }
}

impl AuditEntryId {
    pub fn new(correlation_id: &str) -> Self {
        Self(derive_id("audit-entry", correlation_id))
    }
}

impl ImportProblemId {
    pub fn new(code: &str, subject: &str, location: &str, detail: &str) -> Self {
        Self(derive_id(
            "import-problem",
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
        let id = PartId::for_ipn(&ipn);
        assert_eq!(id, PartId::for_ipn(&ipn));
        assert_eq!(id.as_str().len(), ID_LENGTH);
        assert!(
            id.as_str()
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch.is_ascii_lowercase())
        );
        // Pinned: the server derives the same value (see catalog_lib.js),
        // and stored records depend on it never changing.
        assert_eq!(id.as_str(), "0670aad03a0e111");
        assert_eq!(
            derive_id("part", "RES-0000-0470"),
            sha256_hex("kicad-parts:part:v1\nRES-0000-0470")[..15]
        );
        assert_ne!(
            PartRevisionId::new(&id, 1).as_str(),
            PartRevisionId::new(&id, 2).as_str()
        );
    }
}

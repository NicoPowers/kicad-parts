//! The Catalog context: the parts a team designs with, how each is
//! numbered, how its record has changed, and which exact CAD it uses.
//!
//! The terms are those of `docs/contexts/catalog/CONTEXT.md`. This crate
//! pins down only what the glossary leaves to the implementation:
//!
//! - A **Managed Part** is identified by a [`ManagedPartId`] derived from its
//!   [`Ipn`], so every import of the same IPN resolves to the same part. The
//!   IPN itself never changes.
//! - A **Part Revision** is an immutable [`PartRevisionSnapshot`]. Its
//!   SHA-256 is taken over the exact canonical JSON text that is stored, so a
//!   reader can verify it. Revisions are numbered from 1 per part; the
//!   **Head** is the newest.
//! - A **Catalog Asset** is one logical symbol, footprint or 3D model,
//!   identified by its kind and library reference (for example the symbol
//!   `g-pas:R_US`). An **Asset Revision** is an immutable
//!   [`AssetRevisionDeclaration`]: the exact [`AssetBlob`] it uses plus its
//!   format and [`LicenseDeclaration`]. Its revision hash covers that whole
//!   declaration; [`Provenance`] is attached to the revision but is not part
//!   of the hash, so re-reading identical content from a moved or edited
//!   library file finds the same revision.
//! - An **Asset Blob** is addressed by the SHA-256 of its bytes. For a
//!   symbol, the blob is the exact byte span of that one symbol inside its
//!   library file, so editing a neighbouring symbol does not revise it.
//! - A **Change Set** groups everything one import or edit published, with
//!   its reason. Supplier part numbers and prices are **Operational Facts**:
//!   they are kept in the Part Revision's Provenance but never in its
//!   snapshot, so they can never create a revision.
//!
//! [`legacy_import`] turns the read-only Legacy Catalog into a
//! [`LegacyImport`] without touching any store; [`pocketbase`] submits it to
//! the protected import route and reads the stored records back.

mod asset;
mod hash;
mod ids;
mod ipn;
pub mod legacy_import;
mod part;
pub mod pocketbase;

pub use asset::{
    AssetBlob, AssetKind, AssetRevisionDeclaration, CatalogAsset, LicenseDeclaration,
    ModelReference, ModelTarget,
};
pub use hash::sha256_hex;
pub use ids::{
    AssetBlobId, AssetRevisionId, AuditEventId, CatalogAssetId, ChangeSetId, ID_LENGTH,
    ImportDiagnosticId, ManagedPartId, PartRevisionId, derive_id,
};
pub use ipn::{Ipn, IpnError};
pub use legacy_import::{
    AssetRevisionDraft, ImportDiagnostic, LegacyCatalog, LegacyImport, PartDraft, Provenance,
    SourceFile,
};
pub use part::{
    ApprovedManufacturerPart, AssetBinding, BindingRole, BindingTarget, PartRevisionSnapshot,
    SnapshotField,
};

/// The schema tag every canonical JSON document in this crate carries.
pub const SCHEMA_PREFIX: &str = "kicad-parts/";

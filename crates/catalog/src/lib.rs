//! The Catalog context: the parts a team designs with, their numbers, how
//! their details changed, and which exact CAD items they use.
//!
//! The terms are those of `docs/contexts/catalog/CONTEXT.md`. This crate
//! pins down only what the glossary leaves to the implementation:
//!
//! - A **part** is identified by a [`PartId`] derived from its [`Ipn`], so
//!   every import of the same IPN finds the same part. The IPN never
//!   changes.
//! - A part's [`PartDetails`] (what it is: its describing fields and
//!   manufacturer parts) are shared by every revision. Each change saves
//!   the next numbered copy, as canonical JSON whose SHA-256 a reader can
//!   check, but adds no revision.
//! - A **revision** of a part saves its [`PartRevision`]: only the exact
//!   CAD it uses (its Symbol and Footprint fields, the CAD item revisions
//!   they bind, and Excluded from BOM), as canonical JSON. Only a change to
//!   CAD adds one. Revisions are numbered from 1 per part. A bad revision
//!   is **archived**, never deleted: an append-only archive entry hides it
//!   from KiCad and the app. The **current revision** is the newest
//!   revision that is not archived.
//! - A **CAD item** is one symbol, footprint or 3D model, identified by its
//!   kind and library reference (for example the symbol `g-res:R_US`). A
//!   revision of a CAD item saves its [`CadItemDetails`]: the exact
//!   [`Blob`] it uses, its format and its [`License`]. CAD item revisions
//!   are numbered from 1 per CAD item, like part revisions, and the
//!   current revision is the newest: content that differs from the current
//!   revision adds a revision, even if an older revision had exactly that
//!   content. A part's details name each CAD item revision by number and
//!   snapshot hash, and a binding keeps its number while its content is the
//!   same: the same content under a newer number is not a change. Where
//!   the revision came from (its source: a file chosen in the app, our
//!   libraries or another installed library) is stored with it but is not
//!   hashed, so reading identical content from a moved or edited library
//!   file adds no revision.
//! - A [`Blob`] is exact bytes, addressed by their SHA-256, so revisions
//!   with the same bytes share one stored blob. A symbol's blob
//!   is the exact bytes of that one symbol in its library file, so editing
//!   a neighbouring symbol does not revise it.
//! - A **Change** groups what one edit, new part, archive, unarchive or
//!   category setup saved, with its reason.
//!
//! [`pocketbase`] submits edits and new parts to the protected routes and
//! reads the stored records back;
//! [`replica`] keeps a local copy of every part's current revision in step
//! with the server; [`compare_details`] and [`compare_revisions`] list what
//! differs between two saved part details or two revisions.

pub mod cad_file;
mod cad_item;
pub mod category;
mod change;
mod compare;
pub mod download;
pub mod guess;
mod hash;
mod ids;
mod ipn;
pub mod library_check;
mod part;
pub mod pocketbase;
pub mod replica;

pub use cad_item::{
    Blob, CadItem, CadItemDetails, CadItemKind, License, ModelReference, ModelTarget,
};
pub use category::{CATEGORY_SCHEMA, CategoryDetails, CategoryRules};
pub use change::ChangeKind;
pub use compare::{
    CATEGORY_LABEL, DetailDifference, DifferenceKind, compare_details, compare_revisions,
    describe_binding,
};
pub use hash::sha256_hex;
pub use ids::{
    BlobId, CadItemId, CadItemRevisionId, ChangeId, ID_LENGTH, PartDetailsId, PartId,
    PartRevisionId, derive_id,
};
pub use ipn::{Ipn, IpnError};
pub use part::{
    CAD_COLUMNS, CadReference, CadReferenceRole, CadReferenceTarget, DetailField, ManufacturerPart,
    PART_DETAILS_SCHEMA, PART_REVISION_SCHEMA, PartDetails, PartRevision,
};

/// The schema tag every canonical JSON document in this crate carries.
pub const SCHEMA_PREFIX: &str = "kicad-parts/";

/// The schema tag of the `source` the team server stores with each saved
/// record: where it came from.
pub const SOURCE_SCHEMA: &str = "kicad-parts/source/1";

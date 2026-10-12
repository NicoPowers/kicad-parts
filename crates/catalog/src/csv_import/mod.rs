//! Turns one file of the read-only CSV catalog (such as `g-res`, that is
//! `database/g-res.csv`) into a [`CsvImport`]: parts with their part
//! details and first revision, the CAD items, CAD item revisions and blobs
//! they use, and every import problem. Nothing here writes anywhere; the same inputs
//! always give the same [`CsvImport`].
//!
//! Every source field ends up in exactly one place (see [`route_column`]
//! and [`route_substitutes_csv_column`]):
//!
//! - the part details: the IPN, every column that describes the part, and
//!   the manufacturer parts (`MPN` and `Manufacturer` of the row, then of
//!   each `substitutes.csv` row);
//! - the part revision: the `Symbol` and `Footprint` columns exactly as
//!   written, and the CAD item revisions they bind;
//! - supplier data, kept in the revision's [`Source`] and never in its
//!   part details (supplier part numbers and prices);
//! - an import problem, for anything the import cannot place, such as a
//!   row that cannot become a part, a substitutes.csv row with another
//!   datasheet, an extra MPN (`MPN2`) whose manufacturer the CSV file does
//!   not name, or MPNs still to be confirmed (`MPN TBC`).
//!
//! Excluded from BOM is set in the part revision only from explicit
//! evidence in the CAD libraries: a local symbol the part uses says
//! `(in_bom no)`, or every footprint choice is a local footprint with
//! `exclude_from_bom` in its `(attr ...)`. If only some footprint choices
//! say so, a `footprint-choices-disagree-on-bom` import problem keeps them
//! and the setting stays unset.
//!
//! The CSV file makes one category (`g-res` makes `res`, see
//! [`CategoryDraft`]) and every part in it is in that category. The IPN
//! prefix is not the category: it only says which number range an IPN came
//! from.

mod files;

pub use files::{CsvRecord, CsvTable, input_exists, parse_csv, read_input};

use crate::cad_item::{
    Blob, CAD_ITEM_DETAILS_SCHEMA, CadItem, CadItemDetails, CadItemKind, License, ModelReference,
    ModelTarget,
};
use crate::category::{CategoryDetails, category_code, category_id};
use crate::hash::sha256_hex;
use crate::ids::{CadItemRevisionId, ImportProblemId, PartId};
use crate::ipn::Ipn;
use crate::part::{
    CAD_COLUMNS, CadReference, CadReferenceRole, CadReferenceTarget, DetailField, ManufacturerPart,
    PART_DETAILS_SCHEMA, PART_REVISION_SCHEMA, PartDetails, PartRevision,
};
use kicad_integration::library_file::{FootprintFile, SymbolLibrary, SymbolLookup};
use kicad_integration::{LibRef, LibraryResolver, ResolveError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The schema tag of [`Source`] and [`CadItemSource`].
pub const SOURCE_SCHEMA: &str = "kicad-parts/source/1";
/// The CSV catalog file that lists extra manufacturer parts per IPN.
pub const SUBSTITUTES_PATH: &str = "database/substitutes.csv";
/// The `source` of everything this module produces.
pub const CSV_CATALOG_SOURCE: &str = "csv-catalog";
/// The status of a 3D model reference whose file is empty. Its import
/// problem is `empty-3d-model-reference`.
pub const EMPTY_3D_MODEL: &str = "empty";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportError(pub String);

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ImportError {}

/// Where a source column's value goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColumnRoute {
    /// The IPN: the part details' `ipn` and the part's identity.
    Ipn,
    /// A field of the part details, under the column's name.
    PartDetails,
    /// `Symbol` or `Footprint`: a CAD field of the part revision, kept
    /// exactly as written, whose references bind CAD item revisions.
    Cad,
    /// The main manufacturer part's MPN, in the part details.
    MainMpn,
    /// The main manufacturer part's manufacturer, in the part details.
    MainManufacturer,
    /// Another approved MPN, such as `MPN2`. A manufacturer part needs a
    /// manufacturer, and the CSV catalog never says that the row's
    /// Manufacturer makes this MPN too, so a value here becomes an
    /// `mpn-without-manufacturer` import problem rather than a guessed
    /// manufacturer part.
    ExtraMpn,
    /// `MPN TBC`: MPNs still to be confirmed, so not approved manufacturer
    /// parts. A value here becomes an `mpn-to-be-confirmed` import problem
    /// for someone to confirm.
    MpnToBeConfirmed,
    /// Any other column whose name mentions an MPN, in any case, such as
    /// `MPN 2`, `Alt MPN` or `mpn2`. The import does not know what it
    /// means, so a value here becomes an `unknown-mpn-column` import
    /// problem rather than a guessed part-details field.
    UnknownMpn,
    /// Supplier data: kept in the source, never in the part details.
    SupplierData,
}

/// The supplier part numbers and prices of the CSV catalog. `LCSC` holds
/// LCSC supplier codes such as `C21189`.
pub const SUPPLIER_DATA_COLUMNS: [&str; 7] = [
    "LCSC",
    "DigiKey_PN",
    "Mouser_PN",
    "DigiKey_Price",
    "Mouser_Price",
    "Price_Range",
    "Price_LastSynced_UTC",
];

/// Routes a part CSV column. Unknown and category-specific columns describe
/// the part: they go into the part details under their exact name.
pub fn route_column(name: &str) -> ColumnRoute {
    match name {
        "IPN" => ColumnRoute::Ipn,
        "MPN" => ColumnRoute::MainMpn,
        "Manufacturer" => ColumnRoute::MainManufacturer,
        "MPN TBC" => ColumnRoute::MpnToBeConfirmed,
        "Symbol" | "Footprint" => ColumnRoute::Cad,
        name if is_extra_mpn_column(name) => ColumnRoute::ExtraMpn,
        name if SUPPLIER_DATA_COLUMNS.contains(&name) => ColumnRoute::SupplierData,
        name if name.to_ascii_lowercase().contains("mpn") => ColumnRoute::UnknownMpn,
        _ => ColumnRoute::PartDetails,
    }
}

/// `MPN` followed by a number, such as `MPN2`.
fn is_extra_mpn_column(name: &str) -> bool {
    name.strip_prefix("MPN").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Where a substitutes.csv column's value goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubstitutesCsvColumn {
    Ipn,
    Mpn,
    Manufacturer,
    /// A part has one datasheet, in its part details. A blank datasheet, or
    /// the part's own, adds nothing; any other becomes an import problem.
    Datasheet,
    SupplierData,
    /// Not a column this import understands: the row becomes an import
    /// problem rather than being lost.
    Unknown,
}

pub fn route_substitutes_csv_column(name: &str) -> SubstitutesCsvColumn {
    match name {
        "IPN" => SubstitutesCsvColumn::Ipn,
        "MPN" => SubstitutesCsvColumn::Mpn,
        "Manufacturer" => SubstitutesCsvColumn::Manufacturer,
        "Datasheet" => SubstitutesCsvColumn::Datasheet,
        "Supplier" | "SupplierPN" => SubstitutesCsvColumn::SupplierData,
        _ => SubstitutesCsvColumn::Unknown,
    }
}

/// A read input, as recorded on the Change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceFile {
    /// `catalog-csv`, `substitutes-csv`, `symbol-library`, `footprint` or
    /// `3d-model`.
    pub role: String,
    pub path: String,
    pub byte_sha256: String,
    pub size: u64,
    /// For CSV files, the header line's exact bytes.
    pub header_raw: Option<String>,
    /// For CSV files, every row's exact bytes in file order. The header line
    /// followed by these rows is the file, byte for byte, so each import run
    /// can record exactly what the CSV said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<String>>,
}

/// One supplier part number or price, kept away from the part details.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SupplierData {
    pub column: String,
    pub value: String,
}

/// Where a part's details and revision came from: one CSV row, exactly.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Source {
    pub schema: String,
    /// Always [`CSV_CATALOG_SOURCE`].
    pub source: String,
    /// The CSV file name without extension, such as `g-res`.
    pub csv_file: String,
    pub path: String,
    pub byte_sha256: String,
    pub source_commit: Option<String>,
    pub row_number: usize,
    pub headers: Vec<String>,
    /// The row as parsed, not padded to the header.
    pub record: Vec<String>,
    /// The row's exact bytes.
    pub raw_record: String,
    pub supplier_data: Vec<SupplierData>,
    /// Every `substitutes.csv` row that added a manufacturer part, in order
    /// (the main manufacturer part comes from the row itself).
    pub substitutes_csv_rows: Vec<SubstitutesCsvRow>,
}

/// One `substitutes.csv` row, exactly, including its datasheet.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubstitutesCsvRow {
    pub path: String,
    pub byte_sha256: String,
    pub row_number: usize,
    pub record: Vec<String>,
    pub raw_record: String,
    pub supplier_data: Vec<SupplierData>,
}

/// Where a CAD item revision came from: a file, or one symbol's bytes in it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CadItemSource {
    pub schema: String,
    pub source: String,
    /// The file the blob was read from.
    pub path: String,
    /// That whole file's SHA-256 (the oracle's `byte_sha256`).
    pub byte_sha256: String,
    pub source_commit: Option<String>,
    /// For a symbol, the blob's byte range in `path`; `None` when the blob
    /// is the whole file.
    pub byte_range: Option<[u64; 2]>,
}

/// A part, and the part details and revision this import would publish
/// for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartDraft {
    pub id: PartId,
    pub ipn: Ipn,
    pub details: PartDetails,
    pub details_json: String,
    pub details_sha256: String,
    pub revision: PartRevision,
    pub revision_json: String,
    pub revision_sha256: String,
    /// The CSV row both came from.
    pub source: Source,
    pub source_json: String,
}

/// A CAD item revision this import would publish, or find already stored
/// as the CAD item's current revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CadItemRevisionDraft {
    /// Derived from the CAD item and `number`.
    pub id: CadItemRevisionId,
    pub cad_item: CadItem,
    /// The number of the CAD item's current revision when the importer read
    /// the team catalog, or 0 if the CAD item is new. If the current
    /// revision differs when the import arrives, the route answers 409.
    pub expected_revision: u32,
    /// `expected_revision` if this content is the current revision's,
    /// otherwise the next number after it.
    pub number: u32,
    pub snapshot: CadItemDetails,
    pub snapshot_json: String,
    pub snapshot_sha256: String,
    pub source: CadItemSource,
    pub source_json: String,
}

/// A problem found while importing, kept rather than dropped.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ImportProblem {
    pub id: ImportProblemId,
    pub code: String,
    /// What it is about: an IPN, a row (`row=N`), or a CAD item's key.
    pub subject: String,
    pub path: String,
    pub location: String,
    pub message: String,
    /// Canonical JSON with whatever the problem keeps, such as a whole
    /// row that could not become a part.
    pub detail_json: String,
}

impl ImportProblem {
    pub fn new(
        code: &str,
        subject: &str,
        path: &str,
        location: &str,
        message: String,
        detail: Value,
    ) -> Self {
        let detail_json = detail.to_string();
        Self {
            id: ImportProblemId::new(code, subject, location, &detail_json),
            code: code.to_owned(),
            subject: subject.to_owned(),
            path: path.to_owned(),
            location: location.to_owned(),
            message,
            detail_json,
        }
    }
}

/// The category a CSV file makes, as this import would send it. The server
/// saves it as revision 1 of a new category; for a category it already has,
/// it takes only the fields from here and keeps the name, IPN prefix and
/// rules it holds, so an import changes a category only when the CSV
/// file's part details columns change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CategoryDraft {
    /// Derived from the code.
    pub id: String,
    pub details: CategoryDetails,
    pub details_json: String,
    pub details_sha256: String,
}

/// Everything one import would publish, ordered deterministically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CsvImport {
    pub csv_file: String,
    /// The category the CSV file makes.
    pub category: CategoryDraft,
    /// The commit the inputs were read at, as the caller recorded it. The
    /// import tooling appends `+dirty` when any input differs from that
    /// commit, so a dirty tree is never recorded as a clean one.
    pub source_commit: Option<String>,
    pub sources: Vec<SourceFile>,
    /// The repository paths the import looked for but did not find, such
    /// as a footprint file a part names, sorted. They are not sent; the
    /// import tooling checks them with `sources` against the commit, so a
    /// deleted or renamed file is seen as an uncommitted change.
    pub missing_inputs: Vec<String>,
    pub blobs: Vec<Blob>,
    pub cad_item_revisions: Vec<CadItemRevisionDraft>,
    pub parts: Vec<PartDraft>,
    pub problems: Vec<ImportProblem>,
    /// Rows in substitutes.csv.
    pub substitutes_csv_row_count: usize,
    /// Of those, the rows naming one of this import's IPNs.
    pub substitutes_csv_rows_in_scope: usize,
}

impl CsvImport {
    /// The CAD items, one per CAD item revision, in order.
    pub fn cad_items(&self) -> impl Iterator<Item = &CadItem> {
        self.cad_item_revisions
            .iter()
            .map(|revision| &revision.cad_item)
    }

    /// A hash of everything the import would publish. Equal inputs give
    /// equal fingerprints.
    pub fn fingerprint(&self) -> String {
        let mut text = String::new();
        let mut push = |value: &str| {
            text.push_str(&value.len().to_string());
            text.push(':');
            text.push_str(value);
        };
        push(&self.csv_file);
        push(&self.category.details_sha256);
        push(self.source_commit.as_deref().unwrap_or(""));
        for source in &self.sources {
            push(&source.path);
            push(&source.byte_sha256);
        }
        for blob in &self.blobs {
            push(&blob.sha256);
        }
        for revision in &self.cad_item_revisions {
            push(&revision.expected_revision.to_string());
            push(&revision.number.to_string());
            push(&revision.snapshot_sha256);
            push(&revision.source_json);
        }
        for part in &self.parts {
            push(&part.details_sha256);
            push(&part.revision_sha256);
            push(&part.source_json);
        }
        for problem in &self.problems {
            push(problem.id.as_str());
        }
        sha256_hex(text)
    }

    /// This import, with its CAD item revisions numbered against `current`,
    /// what the team catalog holds now.
    ///
    /// A CAD item revision whose content equals the CAD item's current
    /// revision keeps its number; any other content is the next number,
    /// even if an older revision had exactly that content.
    ///
    /// A binding (a part's symbol or footprint, or a footprint's 3D model)
    /// keeps the revision it names now while its content is the same (see
    /// [`CadItemDetails::content_sha256`]): the same content under a newer
    /// number is not a change. A binding whose content changed, or that is
    /// new, names the CAD item's current revision.
    ///
    /// An import never replaces a CAD item revision made in the app
    /// (kicad-parts-3b8b): a CAD item in [`CurrentCad::edited_in_app`] is not
    /// sent, and bindings to it name the newest revision an import saved,
    /// what the CSV said when an import last applied it, or the app's
    /// revision when the CSV has caught up with it. If the CSV's file for it
    /// says anything else, each part whose row uses it gets a
    /// [`CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM`] import problem that keeps the
    /// row, so a person sees it.
    ///
    /// A [`CsvImport`] fresh from [`CsvCatalog::import`] is numbered as if
    /// the team catalog were empty, which is the same as
    /// `numbered_against(&CurrentCad::default())`.
    pub fn numbered_against(&self, current: &CurrentCad) -> Self {
        let mut numbered = self.clone();
        // A footprint's snapshot names its 3D models' numbers, so 3D models
        // are numbered first.
        let mut order: Vec<usize> = (0..numbered.cad_item_revisions.len()).collect();
        order.sort_by_key(|&index| {
            numbered.cad_item_revisions[index].cad_item.kind != CadItemKind::Model3d
        });
        // Each CAD item's current revision once this import is published.
        let mut after: BTreeMap<String, Binding> = BTreeMap::new();
        // The CAD items made in the app, which the import does not send, and
        // the CAD items whose CSV file changed while they were made in the
        // app (or whose 3D model's did), by CAD item ID, with their index.
        let mut not_sent: BTreeSet<usize> = BTreeSet::new();
        let mut changed_in_csv: BTreeMap<String, usize> = BTreeMap::new();
        let no_models = BTreeMap::new();
        for index in order {
            let draft = &mut numbered.cad_item_revisions[index];
            let id = draft.cad_item.id.as_str().to_owned();
            let model_changed = draft.snapshot.model_references.iter().any(|model| {
                matches!(
                    &model.target,
                    ModelTarget::CadItemRevision { cad_item, .. } if changed_in_csv.contains_key(cad_item)
                )
            });
            let stored = current.cad_items.get(&id);
            let edited = current.edited_in_app.get(&id);
            let kept = match edited {
                Some(edited) => edited
                    .last_imported
                    .as_ref()
                    .map_or(&no_models, |last| &last.models),
                None => stored.map_or(&no_models, |stored| &stored.models),
            };
            for model in &mut draft.snapshot.model_references {
                if let ModelTarget::CadItemRevision {
                    cad_item,
                    number,
                    snapshot_sha256,
                } = &mut model.target
                {
                    if let Some(bound) = binding_for(kept, &after, cad_item) {
                        *number = bound.number;
                        snapshot_sha256.clone_from(&bound.snapshot_sha256);
                    }
                }
            }
            draft.snapshot_json = draft.snapshot.canonical_json();
            draft.snapshot_sha256 = sha256_hex(&draft.snapshot_json);
            if let Some(edited) = edited {
                // The CSV has not changed when it still says what an import
                // last applied, or when it has caught up with what the app
                // made; bindings then name that revision.
                let content = draft.snapshot.content_sha256();
                let last = edited.last_imported.as_ref().map(|last| &last.binding);
                let (binding, unchanged) = match last {
                    Some(last) if last.content_sha256 == content => (last.clone(), true),
                    _ if edited.current.content_sha256 == content => (edited.current.clone(), true),
                    Some(last) => (last.clone(), false),
                    None => (edited.current.clone(), false),
                };
                if !unchanged || model_changed {
                    changed_in_csv.insert(id.clone(), index);
                }
                after.insert(id, binding);
                not_sent.insert(index);
                continue;
            }
            if model_changed {
                changed_in_csv.insert(id.clone(), index);
            }
            draft.expected_revision = stored.map_or(0, |stored| stored.number);
            draft.number = match stored {
                Some(stored) if stored.snapshot_sha256 == draft.snapshot_sha256 => stored.number,
                _ => draft.expected_revision + 1,
            };
            draft.id = CadItemRevisionId::new(&draft.cad_item.id, draft.number);
            after.insert(
                id,
                Binding {
                    number: draft.number,
                    snapshot_sha256: draft.snapshot_sha256.clone(),
                    content_sha256: draft.snapshot.content_sha256(),
                },
            );
        }
        let no_bindings = BTreeMap::new();
        let mut problems = Vec::new();
        for part in &mut numbered.parts {
            let kept = current.parts.get(part.ipn.as_str()).unwrap_or(&no_bindings);
            for position in 0..part.revision.cad_references.len() {
                let reference = &mut part.revision.cad_references[position];
                let CadReferenceTarget::CadItemRevision {
                    cad_item,
                    number,
                    snapshot_sha256,
                } = &mut reference.target
                else {
                    continue;
                };
                if let Some(bound) = binding_for(kept, &after, cad_item) {
                    *number = bound.number;
                    snapshot_sha256.clone_from(&bound.snapshot_sha256);
                }
                let cad_item = cad_item.clone();
                if let Some(&index) = changed_in_csv.get(&cad_item) {
                    let edited = current
                        .edited_in_app
                        .get(&cad_item)
                        .map(|edited| edited.current.number);
                    // The CSV's own snapshot of the CAD item, as read (and
                    // numbered as if the team catalog were empty), so its
                    // hash changes exactly when the CSV's files do.
                    let csv = &self.cad_item_revisions[index].snapshot_json;
                    problems.push(cad_item_kept_problem(
                        part,
                        &part.revision.cad_references[position],
                        csv,
                        edited,
                    ));
                }
            }
            part.revision_json = part.revision.canonical_json();
            part.revision_sha256 = sha256_hex(&part.revision_json);
        }
        // CAD items not sent take their bytes with them, unless another CAD
        // item revision uses the same bytes.
        let mut position = 0;
        numbered.cad_item_revisions.retain(|_| {
            position += 1;
            !not_sent.contains(&(position - 1))
        });
        let used: BTreeSet<String> = numbered
            .cad_item_revisions
            .iter()
            .map(|draft| draft.snapshot.blob_sha256.clone())
            .collect();
        numbered.blobs.retain(|blob| used.contains(&blob.sha256));
        numbered.problems.extend(problems);
        numbered.problems.sort_by(|a, b| a.id.cmp(&b.id));
        numbered.problems.dedup_by(|a, b| a.id == b.id);
        numbered
    }
}

/// The import problem code of a part whose row uses a symbol, footprint or
/// 3D model whose CSV file changed after an import last applied it, while
/// its current revision was made in the app, so the import kept it.
pub const CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM: &str = "csv-cad-differs-from-edited-cad-item";

/// The [`CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM`] problem of `part`'s use of
/// `reference`. Its detail keeps the row and the CSV's snapshot of the CAD
/// item (`csv_cad_item`), whose SHA-256 a review covers.
fn cad_item_kept_problem(
    part: &PartDraft,
    reference: &CadReference,
    csv_cad_item: &str,
    edited_revision: Option<u32>,
) -> ImportProblem {
    let (kind, column) = match reference.role {
        CadReferenceRole::Symbol => ("symbol", "Symbol"),
        CadReferenceRole::Footprint => ("footprint", "Footprint"),
    };
    let (cad_item, kept) = match &reference.target {
        CadReferenceTarget::CadItemRevision {
            cad_item, number, ..
        } => (cad_item.clone(), *number),
        _ => (String::new(), 0),
    };
    let source = &part.source;
    let edited = edited_revision.map_or_else(String::new, |number| format!(" (revision {number})"));
    ImportProblem::new(
        CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM,
        part.ipn.as_str(),
        &source.path,
        &format!("rows[IPN={}].{column}", part.ipn),
        format!(
            "the CSV catalog's file for the {kind} {} that the row of {} uses changed after an import last applied it, but the {kind} was changed in the app since{edited}; an import never replaces that, so the part keeps revision {kept} of it and the row is kept here",
            reference.reference, part.ipn
        ),
        json!({
            "role": kind,
            "reference": reference.reference,
            "cad_item": cad_item,
            "kept_revision": kept,
            "edited_revision": edited_revision,
            "path": source.path,
            "row_number": source.row_number,
            "raw_record": source.raw_record,
            "csv_cad_item": csv_cad_item,
        }),
    )
}

/// The revision a binding to `cad_item` names: the one in `kept` (what it
/// binds now) if that has the same content as the CAD item's current
/// revision after the import, otherwise that current revision. `None` if
/// the import has no revision of `cad_item`.
fn binding_for<'a>(
    kept: &'a BTreeMap<String, Binding>,
    after: &'a BTreeMap<String, Binding>,
    cad_item: &str,
) -> Option<&'a Binding> {
    let current = after.get(cad_item)?;
    match kept.get(cad_item) {
        Some(kept) if kept.content_sha256 == current.content_sha256 => Some(kept),
        _ => Some(current),
    }
}

/// What the team catalog holds now, for numbering an import.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CurrentCad {
    /// Each CAD item's current revision, by CAD item ID.
    pub cad_items: BTreeMap<String, CurrentCadItemRevision>,
    /// The CAD item revisions each part's current revision binds, by IPN,
    /// then by CAD item ID.
    pub parts: BTreeMap<String, BTreeMap<String, Binding>>,
    /// The CAD items whose current revision was made in the app (by an
    /// edit, not a CSV import), by CAD item ID. An import never replaces
    /// them.
    pub edited_in_app: BTreeMap<String, EditedInApp>,
}

/// A CAD item whose current revision was made in the app.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditedInApp {
    /// Its current revision, made in the app.
    pub current: Binding,
    /// The newest revision a CSV import saved, with a footprint's 3D model
    /// bindings: what the CSV said when an import last applied it. `None`
    /// when the CAD item was made in the app.
    pub last_imported: Option<LastImported>,
}

/// The newest revision of a CAD item that a CSV import saved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LastImported {
    pub binding: Binding,
    /// For a footprint, the 3D model revisions it binds, by CAD item ID.
    pub models: BTreeMap<String, Binding>,
}

/// A CAD item's current revision in the team catalog.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CurrentCadItemRevision {
    pub number: u32,
    pub snapshot_sha256: String,
    /// For a footprint, the 3D model revisions it binds, by CAD item ID.
    pub models: BTreeMap<String, Binding>,
}

/// One bound CAD item revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub number: u32,
    /// The SHA-256 of its snapshot.
    pub snapshot_sha256: String,
    /// See [`CadItemDetails::content_sha256`].
    pub content_sha256: String,
}

/// The CSV catalog under a repository root, opened read-only.
#[derive(Clone, Debug)]
pub struct CsvCatalog {
    root: PathBuf,
    resolver: LibraryResolver,
}

impl CsvCatalog {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            // Project libraries only: references to anything else are
            // external, never resolved against a local KiCad install.
            resolver: LibraryResolver::new(Some(root.to_path_buf()), None),
        }
    }

    /// Plans the import of `database/<csv_file>.csv`, for example `g-res`.
    pub fn import(
        &self,
        csv_file: &str,
        source_commit: Option<String>,
    ) -> Result<CsvImport, ImportError> {
        if !csv_file.starts_with("g-")
            || !csv_file[2..]
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return Err(ImportError(format!(
                "`{csv_file}` is not the name of a CSV catalog file"
            )));
        }
        Planner::new(self, csv_file, source_commit).run()
    }
}

struct Planner<'a> {
    catalog: &'a CsvCatalog,
    csv_file: String,
    source_commit: Option<String>,
    sources: BTreeMap<String, SourceFile>,
    missing_inputs: BTreeSet<String>,
    blobs: BTreeMap<String, Blob>,
    cad_items: BTreeMap<(CadItemKind, String), CadItemOutcome>,
    cad_item_revisions: Vec<CadItemRevisionDraft>,
    problems: BTreeMap<ImportProblemId, ImportProblem>,
    /// Unresolved local references and the IPNs that use them.
    unresolved: BTreeMap<(CadReferenceRole, String, String), BTreeSet<String>>,
}

/// What a referenced CAD item resolved to, cached per kind and key.
#[derive(Clone)]
enum CadItemOutcome {
    Revision {
        cad_item: String,
        number: u32,
        snapshot_sha256: String,
        /// The symbol says `(in_bom no)` or the footprint's `(attr ...)`
        /// has `exclude_from_bom`.
        excluded_from_bom: bool,
    },
    Unresolved(String),
}

impl<'a> Planner<'a> {
    fn new(catalog: &'a CsvCatalog, csv_file: &str, source_commit: Option<String>) -> Self {
        Self {
            catalog,
            csv_file: csv_file.to_owned(),
            source_commit,
            sources: BTreeMap::new(),
            missing_inputs: BTreeSet::new(),
            blobs: BTreeMap::new(),
            cad_items: BTreeMap::new(),
            cad_item_revisions: Vec::new(),
            problems: BTreeMap::new(),
            unresolved: BTreeMap::new(),
        }
    }

    fn root(&self) -> &Path {
        &self.catalog.root
    }

    fn record_problem(&mut self, problem: ImportProblem) {
        self.problems.insert(problem.id.clone(), problem);
    }

    /// Reads a source file once and records it for the Change.
    fn read_source(&mut self, role: &str, path: &str) -> Result<(Vec<u8>, String), ImportError> {
        let bytes = read_input(self.root(), path)?;
        let sha = sha256_hex(&bytes);
        self.sources
            .entry(path.to_owned())
            .or_insert_with(|| SourceFile {
                role: role.to_owned(),
                path: path.to_owned(),
                byte_sha256: sha.clone(),
                size: bytes.len() as u64,
                header_raw: None,
                rows: None,
            });
        Ok((bytes, sha))
    }

    /// Keeps a read CSV file's header line and rows with its source.
    fn record_csv_rows(&mut self, path: &str, table: &CsvTable) {
        let source = self.sources.get_mut(path).expect("read before parsing");
        source.header_raw = Some(table.header_raw.clone());
        source.rows = Some(
            table
                .records
                .iter()
                .map(|record| record.raw.clone())
                .collect(),
        );
    }

    fn run(mut self) -> Result<CsvImport, ImportError> {
        let catalog_path = format!("database/{}.csv", self.csv_file);
        let (bytes, catalog_sha) = self.read_source("catalog-csv", &catalog_path)?;
        let table = parse_csv(&catalog_path, &bytes)?;
        self.record_csv_rows(&catalog_path, &table);
        let code = category_code(&self.csv_file).to_owned();
        let category = CategoryDetails::starting(
            &code,
            table
                .headers
                .iter()
                .filter(|header| route_column(header) == ColumnRoute::PartDetails)
                .cloned()
                .collect(),
        );
        let category_json = category.canonical_json();

        let (substitutes_csv_bytes, substitutes_csv_sha) =
            self.read_source("substitutes-csv", SUBSTITUTES_PATH)?;
        let substitutes_csv = parse_csv(SUBSTITUTES_PATH, &substitutes_csv_bytes)?;
        self.record_csv_rows(SUBSTITUTES_PATH, &substitutes_csv);

        let ipn_column = table.headers.iter().position(|header| header == "IPN");
        let mut parts = Vec::new();
        let mut seen = BTreeSet::new();
        let mut accepted = BTreeSet::new();
        for record in &table.records {
            let row_detail = || {
                json!({
                    "row_number": record.row_number,
                    "headers": table.headers,
                    "record": record.fields,
                    "raw_record": record.raw,
                })
            };
            let subject = format!("row={}", record.row_number);
            let location = format!("rows[{}]", record.row_number);
            if record.fields.len() > table.headers.len() {
                self.record_problem(ImportProblem::new(
                    "row-has-extra-fields",
                    &subject,
                    &catalog_path,
                    &location,
                    format!(
                        "row {} has {} fields for {} headers; the row is kept here instead of becoming a part",
                        record.row_number,
                        record.fields.len(),
                        table.headers.len()
                    ),
                    row_detail(),
                ));
                continue;
            }
            let ipn_text = ipn_column
                .and_then(|column| record.fields.get(column))
                .map(String::as_str)
                .unwrap_or("");
            let ipn = match Ipn::parse(ipn_text) {
                Ok(ipn) => ipn,
                Err(error) => {
                    self.record_problem(ImportProblem::new(
                        "invalid-ipn",
                        &subject,
                        &catalog_path,
                        &location,
                        format!(
                            "row {}: {error}; the row is kept here instead of becoming a part",
                            record.row_number
                        ),
                        row_detail(),
                    ));
                    continue;
                }
            };
            if !seen.insert(ipn.clone()) {
                self.record_problem(ImportProblem::new(
                    "duplicate-ipn",
                    &subject,
                    &catalog_path,
                    &location,
                    format!(
                        "row {} repeats IPN {ipn}; the first row becomes the part and this row is kept here",
                        record.row_number
                    ),
                    row_detail(),
                ));
                continue;
            }
            accepted.insert(ipn.clone());
            parts.push((ipn, record));
        }

        // substitutes.csv rows, grouped by the IPN they serve (file order kept).
        let mut by_ipn: BTreeMap<String, Vec<&CsvRecord>> = BTreeMap::new();
        let substitutes_csv_ipn = substitutes_csv
            .headers
            .iter()
            .position(|header| header == "IPN");
        let unknown_columns: Vec<&String> = substitutes_csv
            .headers
            .iter()
            .filter(|header| route_substitutes_csv_column(header) == SubstitutesCsvColumn::Unknown)
            .collect();
        let mut substitutes_csv_rows_in_scope = 0;
        for record in &substitutes_csv.records {
            let ipn = substitutes_csv_ipn
                .and_then(|column| record.fields.get(column))
                .cloned()
                .unwrap_or_default();
            let in_scope = accepted.iter().any(|accepted| accepted.as_str() == ipn);
            if !in_scope {
                // substitutes.csv rows for other CSV files belong to their imports.
                // A substitutes.csv row of this IPN prefix with no matching part would
                // otherwise vanish, so it is an import problem.
                let prefixes: BTreeSet<&str> = accepted.iter().map(Ipn::prefix).collect();
                let orphan = Ipn::parse(&ipn)
                    .map(|parsed| prefixes.contains(parsed.prefix()))
                    .unwrap_or(false);
                if orphan {
                    self.record_problem(ImportProblem::new(
                        "substitutes-csv-row-without-part",
                        &ipn,
                        SUBSTITUTES_PATH,
                        &format!("rows[{}]", record.row_number),
                        format!(
                            "substitutes.csv row {} names {ipn}, which is not a row of {catalog_path}",
                            record.row_number
                        ),
                        json!({
                            "row_number": record.row_number,
                            "headers": substitutes_csv.headers,
                            "record": record.fields,
                            "raw_record": record.raw,
                        }),
                    ));
                }
                continue;
            }
            substitutes_csv_rows_in_scope += 1;
            if !unknown_columns.is_empty() || record.fields.len() > substitutes_csv.headers.len() {
                self.record_problem(ImportProblem::new(
                    "substitutes-csv-row-has-unknown-columns",
                    &ipn,
                    SUBSTITUTES_PATH,
                    &format!("rows[{}]", record.row_number),
                    format!(
                        "substitutes.csv row {} has columns this import does not understand; it is kept here instead of becoming a manufacturer part",
                        record.row_number
                    ),
                    json!({
                        "row_number": record.row_number,
                        "headers": substitutes_csv.headers,
                        "record": record.fields,
                        "raw_record": record.raw,
                    }),
                ));
                continue;
            }
            by_ipn.entry(ipn).or_default().push(record);
        }

        let mut drafts = Vec::new();
        for (ipn, record) in parts {
            let substitutes_csv_rows_for = by_ipn.remove(ipn.as_str()).unwrap_or_default();
            drafts.push(self.part(
                &catalog_path,
                &catalog_sha,
                &table.headers,
                &ipn,
                record,
                &substitutes_csv.headers,
                &substitutes_csv_sha,
                &substitutes_csv_rows_for,
            )?);
        }

        for ((role, reference, status), ipns) in std::mem::take(&mut self.unresolved) {
            let (kind, column) = match role {
                CadReferenceRole::Symbol => ("symbol", "Symbol"),
                CadReferenceRole::Footprint => ("footprint", "Footprint"),
            };
            let users: Vec<String> = ipns.into_iter().collect();
            self.record_problem(ImportProblem::new(
                &format!("{status}-local-{kind}-reference"),
                &reference,
                &catalog_path,
                &format!("{column}={reference}"),
                format!(
                    "local {kind} reference `{reference}` is {status}; used by {}",
                    users.join(", ")
                ),
                json!({ "reference": reference, "status": status, "used_by": users }),
            ));
        }

        Ok(CsvImport {
            csv_file: self.csv_file,
            category: CategoryDraft {
                id: category_id(&code),
                details_sha256: sha256_hex(&category_json),
                details_json: category_json,
                details: category,
            },
            source_commit: self.source_commit,
            sources: self.sources.into_values().collect(),
            missing_inputs: self.missing_inputs.into_iter().collect(),
            blobs: self.blobs.into_values().collect(),
            cad_item_revisions: self.cad_item_revisions,
            parts: drafts,
            problems: self.problems.into_values().collect(),
            substitutes_csv_row_count: substitutes_csv.records.len(),
            substitutes_csv_rows_in_scope,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn part(
        &mut self,
        catalog_path: &str,
        catalog_sha: &str,
        headers: &[String],
        ipn: &Ipn,
        record: &CsvRecord,
        substitutes_csv_headers: &[String],
        substitutes_csv_sha: &str,
        rows_for_part: &[&CsvRecord],
    ) -> Result<PartDraft, ImportError> {
        let value = |index: usize| record.fields.get(index).cloned().unwrap_or_default();
        let mut fields = Vec::new();
        let mut cad_fields = Vec::new();
        let mut supplier_data = Vec::new();
        let mut mpn = None;
        let mut manufacturer = None;
        let mut unplaced_mpns = Vec::new();
        for (index, header) in headers.iter().enumerate() {
            match route_column(header) {
                ColumnRoute::Ipn => {}
                ColumnRoute::PartDetails => fields.push(DetailField {
                    name: header.clone(),
                    value: value(index),
                }),
                ColumnRoute::Cad => cad_fields.push(DetailField {
                    name: header.clone(),
                    value: value(index),
                }),
                ColumnRoute::MainMpn => mpn = Some(value(index)),
                ColumnRoute::MainManufacturer => manufacturer = Some(value(index)),
                route @ (ColumnRoute::ExtraMpn
                | ColumnRoute::MpnToBeConfirmed
                | ColumnRoute::UnknownMpn) => {
                    if !value(index).is_empty() {
                        unplaced_mpns.push((route, header.clone(), value(index)));
                    }
                }
                ColumnRoute::SupplierData => supplier_data.push(SupplierData {
                    column: header.clone(),
                    value: value(index),
                }),
            }
        }
        // A revision keeps Symbol, then Footprint, whatever the CSV column
        // order, as the server requires.
        cad_fields.sort_by_key(|field| CAD_COLUMNS.iter().position(|name| *name == field.name));
        for (route, column, value) in unplaced_mpns {
            let location = format!("rows[IPN={ipn}].{column}");
            let problem = match route {
                ColumnRoute::ExtraMpn => ImportProblem::new(
                    "mpn-without-manufacturer",
                    ipn.as_str(),
                    catalog_path,
                    &location,
                    format!(
                        "{ipn} has {column} `{value}`, but the CSV file does not say who makes it, so it is kept here instead of becoming a manufacturer part"
                    ),
                    json!({
                        "column": column,
                        "mpn": value,
                        "row_manufacturer": manufacturer.clone().unwrap_or_default(),
                    }),
                ),
                ColumnRoute::MpnToBeConfirmed => ImportProblem::new(
                    "mpn-to-be-confirmed",
                    ipn.as_str(),
                    catalog_path,
                    &location,
                    format!(
                        "{ipn} lists MPNs still to be confirmed in {column} (`{value}`); they are not approved manufacturer parts, so they are kept here"
                    ),
                    json!({ "column": column, "value": value }),
                ),
                _ => ImportProblem::new(
                    "unknown-mpn-column",
                    ipn.as_str(),
                    catalog_path,
                    &location,
                    format!(
                        "{ipn} has `{value}` in {column}, a column this import does not know; it is kept here instead of going into the part details"
                    ),
                    json!({ "column": column, "value": value }),
                ),
            };
            self.record_problem(problem);
        }

        let mut manufacturer_parts = Vec::new();
        match (mpn, manufacturer) {
            (Some(mpn), Some(manufacturer)) if !mpn.is_empty() && !manufacturer.is_empty() => {
                manufacturer_parts.push(ManufacturerPart { manufacturer, mpn });
            }
            (None, None) => {}
            (mpn, manufacturer) => {
                // Half a manufacturer part is not one; keep what
                // the row said.
                self.record_problem(ImportProblem::new(
                    "incomplete-main-manufacturer-part",
                    ipn.as_str(),
                    catalog_path,
                    &format!("rows[IPN={ipn}]"),
                    format!(
                        "{ipn} has no complete main manufacturer and MPN; the values are kept here"
                    ),
                    json!({ "MPN": mpn, "Manufacturer": manufacturer }),
                ));
            }
        }

        // A part has one datasheet: the one in its part details.
        let part_datasheet = fields
            .iter()
            .find(|field| field.name == "Datasheet")
            .map(|field| field.value.clone())
            .unwrap_or_default();
        let mut substitutes_csv_rows = Vec::new();
        for substitutes_row in rows_for_part {
            let column = |route: SubstitutesCsvColumn| {
                substitutes_csv_headers
                    .iter()
                    .position(|header| route_substitutes_csv_column(header) == route)
                    .map(|index| {
                        substitutes_row
                            .fields
                            .get(index)
                            .cloned()
                            .unwrap_or_default()
                    })
            };
            let candidate = ManufacturerPart {
                manufacturer: column(SubstitutesCsvColumn::Manufacturer).unwrap_or_default(),
                mpn: column(SubstitutesCsvColumn::Mpn).unwrap_or_default(),
            };
            let location = format!("rows[{}]", substitutes_row.row_number);
            let detail = json!({
                "row_number": substitutes_row.row_number,
                "headers": substitutes_csv_headers,
                "record": substitutes_row.fields,
                "raw_record": substitutes_row.raw,
            });
            if candidate.mpn.is_empty() || candidate.manufacturer.is_empty() {
                self.record_problem(ImportProblem::new(
                    "incomplete-substitutes-csv-row",
                    ipn.as_str(),
                    SUBSTITUTES_PATH,
                    &location,
                    format!(
                        "substitutes.csv row {} for {ipn} lacks a manufacturer or MPN",
                        substitutes_row.row_number
                    ),
                    detail,
                ));
                continue;
            }
            if manufacturer_parts
                .iter()
                .any(|existing: &ManufacturerPart| {
                    existing.manufacturer == candidate.manufacturer && existing.mpn == candidate.mpn
                })
            {
                self.record_problem(ImportProblem::new(
                    "duplicate-manufacturer-part",
                    ipn.as_str(),
                    SUBSTITUTES_PATH,
                    &location,
                    format!(
                        "substitutes.csv row {} repeats {} {} for {ipn}",
                        substitutes_row.row_number, candidate.manufacturer, candidate.mpn
                    ),
                    detail,
                ));
                continue;
            }
            // Its datasheet is kept in the row's source either way; one that
            // differs from the part's is a problem for someone to resolve.
            let datasheet = column(SubstitutesCsvColumn::Datasheet).unwrap_or_default();
            if !datasheet.is_empty() && datasheet != part_datasheet {
                self.record_problem(ImportProblem::new(
                    "substitutes-csv-datasheet-differs",
                    ipn.as_str(),
                    SUBSTITUTES_PATH,
                    &location,
                    format!(
                        "substitutes.csv row {} for {ipn} names a different datasheet; a part has one datasheet, so the part's is kept and this one is recorded here",
                        substitutes_row.row_number
                    ),
                    json!({
                        "row_number": substitutes_row.row_number,
                        "datasheet": datasheet,
                        "part_datasheet": part_datasheet,
                    }),
                ));
            }
            manufacturer_parts.push(candidate);
            substitutes_csv_rows.push(SubstitutesCsvRow {
                path: SUBSTITUTES_PATH.to_owned(),
                byte_sha256: substitutes_csv_sha.to_owned(),
                row_number: substitutes_row.row_number,
                record: substitutes_row.fields.clone(),
                raw_record: substitutes_row.raw.clone(),
                supplier_data: substitutes_csv_headers
                    .iter()
                    .enumerate()
                    .filter(|(_, header)| {
                        route_substitutes_csv_column(header) == SubstitutesCsvColumn::SupplierData
                    })
                    .map(|(index, header)| SupplierData {
                        column: header.clone(),
                        value: substitutes_row
                            .fields
                            .get(index)
                            .cloned()
                            .unwrap_or_default(),
                    })
                    .collect(),
            });
        }

        let mut cad_references = Vec::new();
        // Excluded from BOM is set only from explicit evidence: a local
        // symbol the part uses says `(in_bom no)`, or every footprint choice
        // is a local footprint with `exclude_from_bom` in its `(attr ...)`.
        // Descriptions and notes are never read for it.
        let mut symbol_marked = false;
        let mut footprint_marks: Vec<(String, bool)> = Vec::new();
        for (role, column) in [
            (CadReferenceRole::Symbol, "Symbol"),
            (CadReferenceRole::Footprint, "Footprint"),
        ] {
            let Some(index) = headers.iter().position(|header| header == column) else {
                continue;
            };
            let field = value(index);
            let references: Vec<&str> = field
                .split(';')
                .map(str::trim)
                .filter(|reference| !reference.is_empty())
                .collect();
            // KiCad gives a part one symbol, and a footprint choice listed
            // twice is a slip. Like a missing symbol, the row still imports
            // with its CAD exactly as the CSV says; a problem says what to
            // fix.
            let location = format!("rows[IPN={ipn}].{column}");
            if role == CadReferenceRole::Symbol && references.len() > 1 {
                self.record_problem(ImportProblem::new(
                    "several-symbols",
                    ipn.as_str(),
                    catalog_path,
                    &location,
                    format!(
                        "{ipn} names {} symbols ({field}), but KiCad gives a part one symbol; fix its Symbol in {catalog_path}",
                        references.len()
                    ),
                    json!({ "column": column, "value": field, "references": references }),
                ));
            }
            let mut repeated: Vec<&str> = Vec::new();
            for (index, reference) in references.iter().enumerate() {
                if references[..index].contains(reference) && !repeated.contains(reference) {
                    repeated.push(reference);
                }
            }
            if role == CadReferenceRole::Footprint && !repeated.is_empty() {
                self.record_problem(ImportProblem::new(
                    "repeated-footprint-choice",
                    ipn.as_str(),
                    catalog_path,
                    &location,
                    format!(
                        "{ipn} lists the footprint choice {} more than once; fix its Footprint in {catalog_path}",
                        repeated.join(", ")
                    ),
                    json!({ "column": column, "value": field, "repeated": repeated }),
                ));
            }
            for (ordinal, reference) in references.into_iter().enumerate() {
                let (target, marked) = self.bind(role, reference, ipn)?;
                match role {
                    CadReferenceRole::Symbol => symbol_marked |= marked,
                    CadReferenceRole::Footprint => {
                        footprint_marks.push((reference.to_owned(), marked));
                    }
                }
                cad_references.push(CadReference {
                    role,
                    ordinal: ordinal as u32,
                    reference: reference.to_owned(),
                    target,
                });
            }
        }
        let marked_choices = footprint_marks.iter().filter(|(_, marked)| *marked).count();
        let all_choices_marked = marked_choices > 0 && marked_choices == footprint_marks.len();
        if !symbol_marked && marked_choices > 0 && !all_choices_marked {
            // Some footprint choices say the part is not in the BOM and
            // others do not: that is unclear, so the setting stays unset.
            self.record_problem(ImportProblem::new(
                "footprint-choices-disagree-on-bom",
                ipn.as_str(),
                catalog_path,
                &format!("rows[IPN={ipn}].Footprint"),
                format!(
                    "only some footprint choices of {ipn} are excluded from the BOM, so Excluded from BOM is left unset"
                ),
                json!({
                    "footprints": footprint_marks
                        .iter()
                        .map(|(reference, marked)| json!({ "reference": reference, "exclude_from_bom": marked }))
                        .collect::<Vec<_>>(),
                }),
            ));
        }
        let excluded_from_bom = symbol_marked || all_choices_marked;

        let details = PartDetails {
            schema: PART_DETAILS_SCHEMA.to_owned(),
            ipn: ipn.as_str().to_owned(),
            category: category_code(&self.csv_file).to_owned(),
            fields,
            manufacturer_parts,
        };
        let revision = PartRevision {
            schema: PART_REVISION_SCHEMA.to_owned(),
            ipn: ipn.as_str().to_owned(),
            cad_fields,
            cad_references,
            excluded_from_bom,
        };
        let source = Source {
            schema: SOURCE_SCHEMA.to_owned(),
            source: CSV_CATALOG_SOURCE.to_owned(),
            csv_file: self.csv_file.clone(),
            path: catalog_path.to_owned(),
            byte_sha256: catalog_sha.to_owned(),
            source_commit: self.source_commit.clone(),
            row_number: record.row_number,
            headers: headers.to_vec(),
            record: record.fields.clone(),
            raw_record: record.raw.clone(),
            supplier_data,
            substitutes_csv_rows,
        };
        let details_json = details.canonical_json();
        let revision_json = revision.canonical_json();
        Ok(PartDraft {
            id: PartId::for_ipn(ipn),
            ipn: ipn.clone(),
            details_sha256: sha256_hex(&details_json),
            details_json,
            details,
            revision_sha256: sha256_hex(&revision_json),
            revision_json,
            revision,
            source_json: serde_json::to_string(&source).expect("source serializes"),
            source,
        })
    }

    /// Resolves one symbol or footprint reference of `ipn`, and says
    /// whether the CAD item revision it binds is marked excluded from the
    /// BOM.
    fn bind(
        &mut self,
        role: CadReferenceRole,
        reference: &str,
        ipn: &Ipn,
    ) -> Result<(CadReferenceTarget, bool), ImportError> {
        // The oracle's rule: `g-*` libraries are the CSV catalog's own;
        // anything else (KiCad's bundled libraries) is external.
        let namespace = reference.split_once(':').map_or(reference, |(lib, _)| lib);
        if !namespace.starts_with("g-") {
            return Ok((CadReferenceTarget::External, false));
        }
        let kind = match role {
            CadReferenceRole::Symbol => CadItemKind::Symbol,
            CadReferenceRole::Footprint => CadItemKind::Footprint,
        };
        let outcome = match self.cad_items.get(&(kind, reference.to_owned())) {
            Some(outcome) => outcome.clone(),
            None => {
                let outcome = match role {
                    CadReferenceRole::Symbol => self.symbol_cad_item(reference)?,
                    CadReferenceRole::Footprint => self.footprint_cad_item(reference)?,
                };
                self.cad_items
                    .insert((kind, reference.to_owned()), outcome.clone());
                outcome
            }
        };
        Ok(match outcome {
            CadItemOutcome::Revision {
                cad_item,
                number,
                snapshot_sha256,
                excluded_from_bom,
            } => (
                CadReferenceTarget::CadItemRevision {
                    cad_item,
                    number,
                    snapshot_sha256,
                },
                excluded_from_bom,
            ),
            CadItemOutcome::Unresolved(status) => {
                self.unresolved
                    .entry((role, reference.to_owned(), status.clone()))
                    .or_default()
                    .insert(ipn.as_str().to_owned());
                (CadReferenceTarget::Unresolved { status }, false)
            }
        })
    }

    fn unresolved_reference(reference: &str, error: &ResolveError) -> String {
        match error {
            ResolveError::Malformed(_) | ResolveError::Unassigned => {
                let member = reference.split_once(':').map(|(_, member)| member);
                if member.is_none_or(str::is_empty) {
                    "invalid".into()
                } else {
                    "unsafe-path".into()
                }
            }
            ResolveError::LibraryNotFound { .. } => "missing-library".into(),
            ResolveError::FootprintNotFound { .. } => "missing-member".into(),
        }
    }

    fn symbol_cad_item(&mut self, reference: &str) -> Result<CadItemOutcome, ImportError> {
        let resolved = match self.catalog.resolver.symbol(reference) {
            Ok(resolved) => resolved,
            Err(error) => {
                if let (ResolveError::LibraryNotFound { .. }, Ok(lib_ref)) =
                    (&error, LibRef::parse(reference))
                {
                    self.missing_inputs
                        .insert(format!("symbols/{}.kicad_sym", lib_ref.library));
                }
                return Ok(CadItemOutcome::Unresolved(Self::unresolved_reference(
                    reference, &error,
                )));
            }
        };
        let path = format!("symbols/{}.kicad_sym", resolved.lib_ref.library);
        let (bytes, file_sha) = self.read_source("symbol-library", &path)?;
        let library = SymbolLibrary::parse(&bytes)
            .map_err(|error| ImportError(format!("cannot read `{path}`: {error}")))?;
        let entry = match library.find(&resolved.lib_ref.item) {
            SymbolLookup::Found(entry) => entry.clone(),
            SymbolLookup::Missing => {
                return Ok(CadItemOutcome::Unresolved("missing-member".into()));
            }
            SymbolLookup::Ambiguous => return Ok(CadItemOutcome::Unresolved("ambiguous".into())),
        };
        let span = bytes[entry.span.clone()].to_vec();
        if let Some(parent) = &entry.extends {
            self.record_problem(ImportProblem::new(
                "symbol-extends-parent",
                reference,
                &path,
                &format!("symbol={}", resolved.lib_ref.item),
                format!("symbol `{reference}` extends `{parent}`, which its CAD item revision does not include"),
                json!({ "reference": reference, "extends": parent }),
            ));
        }
        Ok(self.cad_item_revision(
            CadItemKind::Symbol,
            reference,
            "kicad-symbol-entry",
            library.version.clone(),
            span,
            vec![],
            &path,
            &file_sha,
            Some([entry.span.start as u64, entry.span.end as u64]),
            entry.in_bom == Some(false),
        ))
    }

    /// Notes the footprint file that `field` names (its first choice) as
    /// looked for but not found.
    fn footprint_not_found(&mut self, field: &str) {
        let first = kicad_integration::resolver::footprint_choices(field)
            .into_iter()
            .next()
            .unwrap_or_default();
        if let Ok(LibRef { library, item }) = LibRef::parse(first) {
            self.missing_inputs
                .insert(format!("footprints/{library}.pretty/{item}.kicad_mod"));
        }
    }

    fn footprint_cad_item(&mut self, reference: &str) -> Result<CadItemOutcome, ImportError> {
        let resolved = match self.catalog.resolver.footprint(reference) {
            Ok(resolved) => resolved,
            Err(ResolveError::LibraryNotFound { .. }) => {
                self.footprint_not_found(reference);
                // The oracle checks only for the footprint file itself.
                return Ok(CadItemOutcome::Unresolved("missing-member".into()));
            }
            Err(error) => {
                if let ResolveError::FootprintNotFound { .. } = error {
                    self.footprint_not_found(reference);
                }
                return Ok(CadItemOutcome::Unresolved(Self::unresolved_reference(
                    reference, &error,
                )));
            }
        };
        let LibRef { library, item } = &resolved.lib_ref;
        let path = format!("footprints/{library}.pretty/{item}.kicad_mod");
        let (bytes, file_sha) = self.read_source("footprint", &path)?;
        let footprint = FootprintFile::parse(&bytes)
            .map_err(|error| ImportError(format!("cannot read `{path}`: {error}")))?;
        let mut models = Vec::new();
        for model in footprint.model_references.clone() {
            let classified = classify_model_reference(&model);
            let target = match &classified.candidate {
                Some(candidate) if input_exists(self.root(), candidate)? => {
                    match self.model_cad_item(candidate)? {
                        CadItemOutcome::Revision {
                            cad_item,
                            number,
                            snapshot_sha256,
                            ..
                        } => ModelTarget::CadItemRevision {
                            cad_item,
                            number,
                            snapshot_sha256,
                        },
                        CadItemOutcome::Unresolved(status) => ModelTarget::Unresolved { status },
                    }
                }
                Some(candidate) => {
                    self.missing_inputs.insert(candidate.clone());
                    ModelTarget::Unresolved {
                        status: "missing".into(),
                    }
                }
                None => ModelTarget::Unresolved {
                    status: classified.status.into(),
                },
            };
            if let ModelTarget::Unresolved { status } = &target {
                let message = if status == EMPTY_3D_MODEL {
                    format!(
                        "3D model reference `{model}` names an empty file ({}); the file is kept as a source of this import",
                        classified.scope
                    )
                } else {
                    format!(
                        "3D model reference `{model}` is {status} ({})",
                        classified.scope
                    )
                };
                self.record_problem(ImportProblem::new(
                    &format!("{status}-3d-model-reference"),
                    reference,
                    &path,
                    &format!("model={model}"),
                    message,
                    json!({ "footprint": reference, "reference": model, "scope": classified.scope }),
                ));
            }
            models.push(ModelReference {
                reference: model,
                scope: classified.scope.into(),
                target,
            });
        }
        Ok(self.cad_item_revision(
            CadItemKind::Footprint,
            reference,
            "kicad-footprint",
            footprint.version.clone(),
            bytes,
            models,
            &path,
            &file_sha,
            None,
            footprint.exclude_from_bom,
        ))
    }

    fn model_cad_item(&mut self, path: &str) -> Result<CadItemOutcome, ImportError> {
        if let Some(outcome) = self.cad_items.get(&(CadItemKind::Model3d, path.to_owned())) {
            return Ok(outcome.clone());
        }
        let (bytes, file_sha) = self.read_source("3d-model", path)?;
        if bytes.is_empty() {
            // An empty file is no 3D model, and PocketBase cannot store an
            // empty blob. The file stays a source of the import, and the
            // footprint's reference to it becomes an import problem.
            let outcome = CadItemOutcome::Unresolved(EMPTY_3D_MODEL.into());
            self.cad_items
                .insert((CadItemKind::Model3d, path.to_owned()), outcome.clone());
            return Ok(outcome);
        }
        let outcome = self.cad_item_revision(
            CadItemKind::Model3d,
            path,
            "3d-model",
            None,
            bytes,
            vec![],
            path,
            &file_sha,
            None,
            false,
        );
        self.cad_items
            .insert((CadItemKind::Model3d, path.to_owned()), outcome.clone());
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    fn cad_item_revision(
        &mut self,
        kind: CadItemKind,
        key: &str,
        format: &str,
        format_version: Option<String>,
        bytes: Vec<u8>,
        model_references: Vec<ModelReference>,
        path: &str,
        file_sha: &str,
        byte_range: Option<[u64; 2]>,
        excluded_from_bom: bool,
    ) -> CadItemOutcome {
        let blob = Blob::new(bytes);
        let cad_item = CadItem::new(kind, key);
        let snapshot = CadItemDetails {
            schema: CAD_ITEM_DETAILS_SCHEMA.to_owned(),
            kind,
            key: key.to_owned(),
            format: format.to_owned(),
            format_version,
            blob_sha256: blob.sha256.clone(),
            blob_size: blob.bytes.len() as u64,
            model_references,
            license: License::UnknownNeedsReview,
        };
        let snapshot_json = snapshot.canonical_json();
        let snapshot_sha256 = sha256_hex(&snapshot_json);
        let source = CadItemSource {
            schema: SOURCE_SCHEMA.to_owned(),
            source: CSV_CATALOG_SOURCE.to_owned(),
            path: path.to_owned(),
            byte_sha256: file_sha.to_owned(),
            source_commit: self.source_commit.clone(),
            byte_range,
        };
        // Numbered as if the team catalog were empty;
        // CsvImport::numbered_against renumbers against what is stored.
        let outcome = CadItemOutcome::Revision {
            cad_item: cad_item.id.as_str().to_owned(),
            number: 1,
            snapshot_sha256: snapshot_sha256.clone(),
            excluded_from_bom,
        };
        self.blobs.entry(blob.sha256.clone()).or_insert(blob);
        self.cad_item_revisions.push(CadItemRevisionDraft {
            id: CadItemRevisionId::new(&cad_item.id, 1),
            cad_item,
            expected_revision: 0,
            number: 1,
            snapshot,
            snapshot_json,
            snapshot_sha256,
            source_json: serde_json::to_string(&source).expect("source serializes"),
            source,
        });
        outcome
    }
}

pub(crate) struct ClassifiedModel {
    pub(crate) scope: &'static str,
    pub(crate) status: &'static str,
    /// The repository path to look for, for a local reference.
    pub(crate) candidate: Option<String>,
}

/// The oracle's lexical, host-independent classification of a 3D model
/// reference (`tooling/csv-catalog-fixture`). The team server's
/// catalog_lib.js classifyModelReference reads uploaded footprints the same
/// way.
pub(crate) fn classify_model_reference(reference: &str) -> ClassifiedModel {
    let strip = |variable: &str| {
        reference
            .strip_prefix(variable)
            .and_then(|rest| rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\')))
    };
    let local = |relative: &str| match safe_relative(relative) {
        Some(clean) => ClassifiedModel {
            scope: "local",
            status: "missing",
            candidate: Some(format!("3d-models/{clean}")),
        },
        None => ClassifiedModel {
            scope: "local",
            status: "unsafe-path",
            candidate: None,
        },
    };
    if let Some(rest) = strip("${GITPLM_PARTS}") {
        return match rest
            .strip_prefix("3d-models")
            .and_then(|rest| rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\')))
        {
            Some(model) => local(model),
            None => ClassifiedModel {
                scope: "local",
                status: "unsafe-path",
                candidate: None,
            },
        };
    }
    if let Some(rest) = strip("${GITPLM_3DMODELS}") {
        return local(rest);
    }
    let bytes = reference.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let scope = if reference.starts_with("\\\\") || reference.starts_with("//") {
        "unc-absolute"
    } else if drive && matches!(bytes.get(2), Some(b'/' | b'\\')) {
        "windows-drive-absolute"
    } else if reference.starts_with('/') {
        "posix-absolute"
    } else if reference.starts_with('\\') {
        "windows-rooted"
    } else if drive {
        "windows-drive-relative"
    } else if reference.starts_with("${") {
        "external-variable"
    } else {
        "relative"
    };
    ClassifiedModel {
        scope,
        status: "unresolved",
        candidate: None,
    }
}

/// `relative` as a clean `/`-separated path, or `None` if it could escape.
fn safe_relative(relative: &str) -> Option<String> {
    let bytes = relative.as_bytes();
    if relative.is_empty()
        || relative.starts_with(['/', '\\'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return None;
    }
    let mut parts = Vec::new();
    for component in relative.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return None,
            value if value.contains(':') => return None,
            value => parts.push(value),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests;

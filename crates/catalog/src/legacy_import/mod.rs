//! Turns one grouping of the read-only Legacy Catalog (such as `g-res`,
//! `database/g-res.csv`) into a [`LegacyImport`]: Managed Parts with their
//! first Part Revision snapshot, the Catalog Assets, Asset Revisions and
//! Asset Blobs they use, and every Import Diagnostic. Nothing here writes
//! anywhere; the same inputs always give the same [`LegacyImport`].
//!
//! Every source field ends up in exactly one place (see [`route_column`]):
//!
//! - the Part Revision snapshot (the IPN and every semantic column);
//! - an Approved Manufacturer Part (`MPN`, `Manufacturer`, and the
//!   substitutes' `MPN`, `Manufacturer` and `Datasheet`);
//! - an Operational Fact, kept in the revision's [`Provenance`] and never in
//!   its snapshot (supplier part numbers and prices);
//! - an Import Diagnostic, for a row that cannot become a Managed Part.
//!
//! The legacy grouping (`g-res`) is provenance only. It is neither a Catalog
//! Category nor the IPN Family.

mod sexpr;
mod source;

pub use source::{CsvRecord, CsvTable, input_exists, parse_csv, read_input};

use crate::asset::{
    ASSET_REVISION_SCHEMA, AssetBlob, AssetKind, AssetRevisionDeclaration, CatalogAsset,
    LicenseDeclaration, ModelReference, ModelTarget,
};
use crate::hash::sha256_hex;
use crate::ids::{AssetRevisionId, ImportDiagnosticId, ManagedPartId};
use crate::ipn::Ipn;
use crate::part::{
    ApprovedManufacturerPart, AssetBinding, BindingRole, BindingTarget, PART_REVISION_SCHEMA,
    PartRevisionSnapshot, SnapshotField,
};
use kicad_integration::{LibRef, LibraryResolver, ResolveError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The schema tag of [`Provenance`] and [`AssetProvenance`].
pub const PROVENANCE_SCHEMA: &str = "kicad-parts/provenance/1";
/// The substitutes list of the Legacy Catalog.
pub const SUBSTITUTES_PATH: &str = "database/substitutes.csv";
/// The `source` of everything this module produces.
pub const LEGACY_SOURCE: &str = "legacy-catalog";

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
    /// The IPN: the snapshot's `ipn` and the Managed Part's identity.
    Ipn,
    /// A field of the Part Revision snapshot, under the column's name.
    Snapshot,
    /// The primary Approved Manufacturer Part's MPN.
    PrimaryMpn,
    /// The primary Approved Manufacturer Part's manufacturer.
    PrimaryManufacturer,
    /// An Operational Fact: kept in Provenance, never in the snapshot.
    OperationalFact,
}

/// The supplier part numbers and prices of the Legacy Catalog. `LCSC` holds
/// LCSC supplier codes such as `C21189`.
pub const OPERATIONAL_FACT_COLUMNS: [&str; 7] = [
    "LCSC",
    "DigiKey_PN",
    "Mouser_PN",
    "DigiKey_Price",
    "Mouser_Price",
    "Price_Range",
    "Price_LastSynced_UTC",
];

/// Routes a part CSV column. Unknown and category-specific columns are
/// semantic: they go into the snapshot under their exact name.
pub fn route_column(name: &str) -> ColumnRoute {
    match name {
        "IPN" => ColumnRoute::Ipn,
        "MPN" => ColumnRoute::PrimaryMpn,
        "Manufacturer" => ColumnRoute::PrimaryManufacturer,
        name if OPERATIONAL_FACT_COLUMNS.contains(&name) => ColumnRoute::OperationalFact,
        _ => ColumnRoute::Snapshot,
    }
}

/// Where a substitutes column's value goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubstituteRoute {
    Ipn,
    Mpn,
    Manufacturer,
    Datasheet,
    OperationalFact,
    /// Not a column this import understands: the row becomes an Import
    /// Diagnostic rather than losing it.
    Unknown,
}

pub fn route_substitute_column(name: &str) -> SubstituteRoute {
    match name {
        "IPN" => SubstituteRoute::Ipn,
        "MPN" => SubstituteRoute::Mpn,
        "Manufacturer" => SubstituteRoute::Manufacturer,
        "Datasheet" => SubstituteRoute::Datasheet,
        "Supplier" | "SupplierPN" => SubstituteRoute::OperationalFact,
        _ => SubstituteRoute::Unknown,
    }
}

/// A read input, as recorded on the Change Set.
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
}

/// A supplier part number or price, routed away from the snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationalFact {
    pub column: String,
    pub value: String,
}

/// Where a Part Revision came from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    pub schema: String,
    pub source: String,
    /// The Legacy Catalog grouping, such as `g-res`.
    pub legacy_grouping: String,
    pub path: String,
    pub byte_sha256: String,
    pub source_commit: Option<String>,
    pub row_number: usize,
    pub headers: Vec<String>,
    /// The row as parsed, not padded to the header.
    pub record: Vec<String>,
    /// The row's exact bytes.
    pub raw_record: String,
    pub operational_facts: Vec<OperationalFact>,
    /// One per substitute that became an Approved Manufacturer Part, in
    /// order (the primary comes from the row itself).
    pub substitutes: Vec<SubstituteProvenance>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubstituteProvenance {
    pub path: String,
    pub byte_sha256: String,
    pub row_number: usize,
    pub record: Vec<String>,
    pub raw_record: String,
    pub operational_facts: Vec<OperationalFact>,
}

/// Where an Asset Revision came from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssetProvenance {
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

/// A Managed Part and its first Part Revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartDraft {
    pub id: ManagedPartId,
    pub ipn: Ipn,
    pub snapshot: PartRevisionSnapshot,
    pub snapshot_json: String,
    pub snapshot_sha256: String,
    pub provenance: Provenance,
    pub provenance_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetRevisionDraft {
    pub id: AssetRevisionId,
    pub asset: CatalogAsset,
    pub declaration: AssetRevisionDeclaration,
    pub declaration_json: String,
    pub revision_sha256: String,
    pub provenance: AssetProvenance,
    pub provenance_json: String,
}

/// A problem found while importing, kept rather than dropped.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ImportDiagnostic {
    pub id: ImportDiagnosticId,
    pub code: String,
    /// What it is about: an IPN, a row (`row=N`), or an asset key.
    pub subject: String,
    pub path: String,
    pub location: String,
    pub message: String,
    /// Canonical JSON with whatever the diagnostic keeps, such as a whole
    /// row that could not become a Managed Part.
    pub detail_json: String,
}

impl ImportDiagnostic {
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
            id: ImportDiagnosticId::new(code, subject, location, &detail_json),
            code: code.to_owned(),
            subject: subject.to_owned(),
            path: path.to_owned(),
            location: location.to_owned(),
            message,
            detail_json,
        }
    }
}

/// Everything one import would publish, ordered deterministically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyImport {
    pub legacy_grouping: String,
    pub source_commit: Option<String>,
    pub sources: Vec<SourceFile>,
    pub blobs: Vec<AssetBlob>,
    pub asset_revisions: Vec<AssetRevisionDraft>,
    pub parts: Vec<PartDraft>,
    pub diagnostics: Vec<ImportDiagnostic>,
    /// Rows in the substitutes list.
    pub substitute_rows: usize,
    /// Of those, the rows naming one of this import's IPNs.
    pub substitute_rows_in_scope: usize,
}

impl LegacyImport {
    /// The Catalog Assets, one per asset revision, in order.
    pub fn assets(&self) -> impl Iterator<Item = &CatalogAsset> {
        self.asset_revisions.iter().map(|revision| &revision.asset)
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
        push(&self.legacy_grouping);
        push(self.source_commit.as_deref().unwrap_or(""));
        for source in &self.sources {
            push(&source.path);
            push(&source.byte_sha256);
        }
        for blob in &self.blobs {
            push(&blob.sha256);
        }
        for revision in &self.asset_revisions {
            push(&revision.revision_sha256);
            push(&revision.provenance_json);
        }
        for part in &self.parts {
            push(&part.snapshot_sha256);
            push(&part.provenance_json);
        }
        for diagnostic in &self.diagnostics {
            push(diagnostic.id.as_str());
        }
        sha256_hex(text)
    }
}

/// The Legacy Catalog under a repository root, opened read-only.
#[derive(Clone, Debug)]
pub struct LegacyCatalog {
    root: PathBuf,
    resolver: LibraryResolver,
}

impl LegacyCatalog {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            // Project libraries only: references to anything else are
            // external, never resolved against a local KiCad install.
            resolver: LibraryResolver::new(Some(root.to_path_buf()), None),
        }
    }

    /// Plans the import of `database/<grouping>.csv`, for example `g-res`.
    pub fn import(
        &self,
        grouping: &str,
        source_commit: Option<String>,
    ) -> Result<LegacyImport, ImportError> {
        if !grouping.starts_with("g-")
            || !grouping[2..]
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return Err(ImportError(format!(
                "`{grouping}` is not a legacy grouping"
            )));
        }
        Planner::new(self, grouping, source_commit).run()
    }
}

struct Planner<'a> {
    catalog: &'a LegacyCatalog,
    grouping: String,
    source_commit: Option<String>,
    sources: BTreeMap<String, SourceFile>,
    blobs: BTreeMap<String, AssetBlob>,
    assets: BTreeMap<(AssetKind, String), AssetOutcome>,
    asset_revisions: Vec<AssetRevisionDraft>,
    diagnostics: BTreeMap<ImportDiagnosticId, ImportDiagnostic>,
    /// Unresolved local references and the IPNs that use them.
    unresolved: BTreeMap<(BindingRole, String, String), BTreeSet<String>>,
}

/// What a referenced asset resolved to, cached per kind and key.
#[derive(Clone)]
enum AssetOutcome {
    Revision {
        asset: String,
        revision_sha256: String,
    },
    Unresolved(String),
}

impl<'a> Planner<'a> {
    fn new(catalog: &'a LegacyCatalog, grouping: &str, source_commit: Option<String>) -> Self {
        Self {
            catalog,
            grouping: grouping.to_owned(),
            source_commit,
            sources: BTreeMap::new(),
            blobs: BTreeMap::new(),
            assets: BTreeMap::new(),
            asset_revisions: Vec::new(),
            diagnostics: BTreeMap::new(),
            unresolved: BTreeMap::new(),
        }
    }

    fn root(&self) -> &Path {
        &self.catalog.root
    }

    fn diagnose(&mut self, diagnostic: ImportDiagnostic) {
        self.diagnostics.insert(diagnostic.id.clone(), diagnostic);
    }

    /// Reads a source file once and records it on the Change Set.
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
            });
        Ok((bytes, sha))
    }

    fn run(mut self) -> Result<LegacyImport, ImportError> {
        let catalog_path = format!("database/{}.csv", self.grouping);
        let (bytes, catalog_sha) = self.read_source("catalog-csv", &catalog_path)?;
        let table = parse_csv(&catalog_path, &bytes)?;
        self.sources
            .get_mut(&catalog_path)
            .expect("just read")
            .header_raw = Some(table.header_raw.clone());

        let (substitute_bytes, substitutes_sha) =
            self.read_source("substitutes-csv", SUBSTITUTES_PATH)?;
        let substitutes = parse_csv(SUBSTITUTES_PATH, &substitute_bytes)?;
        self.sources
            .get_mut(SUBSTITUTES_PATH)
            .expect("just read")
            .header_raw = Some(substitutes.header_raw.clone());

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
                self.diagnose(ImportDiagnostic::new(
                    "row-has-extra-fields",
                    &subject,
                    &catalog_path,
                    &location,
                    format!(
                        "row {} has {} fields for {} headers; the row is kept here instead of becoming a Managed Part",
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
                    self.diagnose(ImportDiagnostic::new(
                        "invalid-ipn",
                        &subject,
                        &catalog_path,
                        &location,
                        format!(
                            "row {}: {error}; the row is kept here instead of becoming a Managed Part",
                            record.row_number
                        ),
                        row_detail(),
                    ));
                    continue;
                }
            };
            if !seen.insert(ipn.clone()) {
                self.diagnose(ImportDiagnostic::new(
                    "duplicate-ipn",
                    &subject,
                    &catalog_path,
                    &location,
                    format!(
                        "row {} repeats IPN {ipn}; the first row becomes the Managed Part and this row is kept here",
                        record.row_number
                    ),
                    row_detail(),
                ));
                continue;
            }
            accepted.insert(ipn.clone());
            parts.push((ipn, record));
        }

        // Substitutes, grouped by the IPN they serve (file order kept).
        let mut by_ipn: BTreeMap<String, Vec<&CsvRecord>> = BTreeMap::new();
        let substitute_ipn = substitutes
            .headers
            .iter()
            .position(|header| header == "IPN");
        let unknown_columns: Vec<&String> = substitutes
            .headers
            .iter()
            .filter(|header| route_substitute_column(header) == SubstituteRoute::Unknown)
            .collect();
        let mut substitute_rows_in_scope = 0;
        for record in &substitutes.records {
            let ipn = substitute_ipn
                .and_then(|column| record.fields.get(column))
                .cloned()
                .unwrap_or_default();
            let in_scope = accepted.iter().any(|accepted| accepted.as_str() == ipn);
            if !in_scope {
                // Substitutes of other groupings belong to their own import.
                // A substitute of this IPN Family with no matching row would
                // otherwise vanish, so it is diagnosed.
                let families: BTreeSet<&str> = accepted.iter().map(Ipn::family).collect();
                let orphan = Ipn::parse(&ipn)
                    .map(|parsed| families.contains(parsed.family()))
                    .unwrap_or(false);
                if orphan {
                    self.diagnose(ImportDiagnostic::new(
                        "substitute-without-managed-part",
                        &ipn,
                        SUBSTITUTES_PATH,
                        &format!("rows[{}]", record.row_number),
                        format!(
                            "substitute row {} names {ipn}, which is not a row of {catalog_path}",
                            record.row_number
                        ),
                        json!({
                            "row_number": record.row_number,
                            "headers": substitutes.headers,
                            "record": record.fields,
                            "raw_record": record.raw,
                        }),
                    ));
                }
                continue;
            }
            substitute_rows_in_scope += 1;
            if !unknown_columns.is_empty() || record.fields.len() > substitutes.headers.len() {
                self.diagnose(ImportDiagnostic::new(
                    "substitute-has-unknown-columns",
                    &ipn,
                    SUBSTITUTES_PATH,
                    &format!("rows[{}]", record.row_number),
                    format!(
                        "substitute row {} has columns this import does not understand; it is kept here instead of becoming an Approved Manufacturer Part",
                        record.row_number
                    ),
                    json!({
                        "row_number": record.row_number,
                        "headers": substitutes.headers,
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
            let substitutes_for = by_ipn.remove(ipn.as_str()).unwrap_or_default();
            drafts.push(self.part(
                &catalog_path,
                &catalog_sha,
                &table.headers,
                &ipn,
                record,
                &substitutes.headers,
                &substitutes_sha,
                &substitutes_for,
            )?);
        }

        for ((role, reference, status), ipns) in std::mem::take(&mut self.unresolved) {
            let (kind, column) = match role {
                BindingRole::Symbol => ("symbol", "Symbol"),
                BindingRole::Footprint => ("footprint", "Footprint"),
            };
            let users: Vec<String> = ipns.into_iter().collect();
            self.diagnose(ImportDiagnostic::new(
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

        Ok(LegacyImport {
            legacy_grouping: self.grouping,
            source_commit: self.source_commit,
            sources: self.sources.into_values().collect(),
            blobs: self.blobs.into_values().collect(),
            asset_revisions: self.asset_revisions,
            parts: drafts,
            diagnostics: self.diagnostics.into_values().collect(),
            substitute_rows: substitutes.records.len(),
            substitute_rows_in_scope,
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
        substitute_headers: &[String],
        substitutes_sha: &str,
        substitutes: &[&CsvRecord],
    ) -> Result<PartDraft, ImportError> {
        let value = |index: usize| record.fields.get(index).cloned().unwrap_or_default();
        let mut fields = Vec::new();
        let mut operational_facts = Vec::new();
        let mut mpn = None;
        let mut manufacturer = None;
        for (index, header) in headers.iter().enumerate() {
            match route_column(header) {
                ColumnRoute::Ipn => {}
                ColumnRoute::Snapshot => fields.push(SnapshotField {
                    name: header.clone(),
                    value: value(index),
                }),
                ColumnRoute::PrimaryMpn => mpn = Some(value(index)),
                ColumnRoute::PrimaryManufacturer => manufacturer = Some(value(index)),
                ColumnRoute::OperationalFact => operational_facts.push(OperationalFact {
                    column: header.clone(),
                    value: value(index),
                }),
            }
        }

        let mut approved = Vec::new();
        match (mpn, manufacturer) {
            (Some(mpn), Some(manufacturer)) if !mpn.is_empty() && !manufacturer.is_empty() => {
                approved.push(ApprovedManufacturerPart {
                    manufacturer,
                    mpn,
                    datasheet: None,
                });
            }
            (None, None) => {}
            (mpn, manufacturer) => {
                // Half an Approved Manufacturer Part is not one; keep what
                // the row said.
                self.diagnose(ImportDiagnostic::new(
                    "incomplete-primary-manufacturer-part",
                    ipn.as_str(),
                    catalog_path,
                    &format!("rows[IPN={ipn}]"),
                    format!(
                        "{ipn} has no complete primary manufacturer and MPN; the values are kept here"
                    ),
                    json!({ "MPN": mpn, "Manufacturer": manufacturer }),
                ));
            }
        }

        let mut substitute_provenance = Vec::new();
        for substitute in substitutes {
            let column = |route: SubstituteRoute| {
                substitute_headers
                    .iter()
                    .position(|header| route_substitute_column(header) == route)
                    .map(|index| substitute.fields.get(index).cloned().unwrap_or_default())
            };
            let candidate = ApprovedManufacturerPart {
                manufacturer: column(SubstituteRoute::Manufacturer).unwrap_or_default(),
                mpn: column(SubstituteRoute::Mpn).unwrap_or_default(),
                datasheet: column(SubstituteRoute::Datasheet),
            };
            let location = format!("rows[{}]", substitute.row_number);
            let detail = json!({
                "row_number": substitute.row_number,
                "headers": substitute_headers,
                "record": substitute.fields,
                "raw_record": substitute.raw,
            });
            if candidate.mpn.is_empty() || candidate.manufacturer.is_empty() {
                self.diagnose(ImportDiagnostic::new(
                    "incomplete-substitute",
                    ipn.as_str(),
                    SUBSTITUTES_PATH,
                    &location,
                    format!(
                        "substitute row {} for {ipn} lacks a manufacturer or MPN",
                        substitute.row_number
                    ),
                    detail,
                ));
                continue;
            }
            if approved.iter().any(|existing: &ApprovedManufacturerPart| {
                existing.manufacturer == candidate.manufacturer && existing.mpn == candidate.mpn
            }) {
                self.diagnose(ImportDiagnostic::new(
                    "duplicate-approved-manufacturer-part",
                    ipn.as_str(),
                    SUBSTITUTES_PATH,
                    &location,
                    format!(
                        "substitute row {} repeats {} {} for {ipn}",
                        substitute.row_number, candidate.manufacturer, candidate.mpn
                    ),
                    detail,
                ));
                continue;
            }
            approved.push(candidate);
            substitute_provenance.push(SubstituteProvenance {
                path: SUBSTITUTES_PATH.to_owned(),
                byte_sha256: substitutes_sha.to_owned(),
                row_number: substitute.row_number,
                record: substitute.fields.clone(),
                raw_record: substitute.raw.clone(),
                operational_facts: substitute_headers
                    .iter()
                    .enumerate()
                    .filter(|(_, header)| {
                        route_substitute_column(header) == SubstituteRoute::OperationalFact
                    })
                    .map(|(index, header)| OperationalFact {
                        column: header.clone(),
                        value: substitute.fields.get(index).cloned().unwrap_or_default(),
                    })
                    .collect(),
            });
        }

        let mut bindings = Vec::new();
        for (role, column) in [
            (BindingRole::Symbol, "Symbol"),
            (BindingRole::Footprint, "Footprint"),
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
            for (ordinal, reference) in references.into_iter().enumerate() {
                let target = self.bind(role, reference, ipn)?;
                bindings.push(AssetBinding {
                    role,
                    ordinal: ordinal as u32,
                    reference: reference.to_owned(),
                    target,
                });
            }
        }

        let snapshot = PartRevisionSnapshot {
            schema: PART_REVISION_SCHEMA.to_owned(),
            ipn: ipn.as_str().to_owned(),
            fields,
            approved_manufacturer_parts: approved,
            asset_bindings: bindings,
        };
        let provenance = Provenance {
            schema: PROVENANCE_SCHEMA.to_owned(),
            source: LEGACY_SOURCE.to_owned(),
            legacy_grouping: self.grouping.clone(),
            path: catalog_path.to_owned(),
            byte_sha256: catalog_sha.to_owned(),
            source_commit: self.source_commit.clone(),
            row_number: record.row_number,
            headers: headers.to_vec(),
            record: record.fields.clone(),
            raw_record: record.raw.clone(),
            operational_facts,
            substitutes: substitute_provenance,
        };
        let snapshot_json = snapshot.canonical_json();
        Ok(PartDraft {
            id: ManagedPartId::for_ipn(ipn),
            ipn: ipn.clone(),
            snapshot_sha256: sha256_hex(&snapshot_json),
            snapshot_json,
            provenance_json: serde_json::to_string(&provenance).expect("provenance serializes"),
            snapshot,
            provenance,
        })
    }

    /// Resolves one symbol or footprint reference of `ipn`.
    fn bind(
        &mut self,
        role: BindingRole,
        reference: &str,
        ipn: &Ipn,
    ) -> Result<BindingTarget, ImportError> {
        // The oracle's rule: `g-*` libraries are the Legacy Catalog's own;
        // anything else (KiCad's bundled libraries) is external.
        let namespace = reference.split_once(':').map_or(reference, |(lib, _)| lib);
        if !namespace.starts_with("g-") {
            return Ok(BindingTarget::External);
        }
        let kind = match role {
            BindingRole::Symbol => AssetKind::Symbol,
            BindingRole::Footprint => AssetKind::Footprint,
        };
        let outcome = match self.assets.get(&(kind, reference.to_owned())) {
            Some(outcome) => outcome.clone(),
            None => {
                let outcome = match role {
                    BindingRole::Symbol => self.symbol_asset(reference)?,
                    BindingRole::Footprint => self.footprint_asset(reference)?,
                };
                self.assets
                    .insert((kind, reference.to_owned()), outcome.clone());
                outcome
            }
        };
        Ok(match outcome {
            AssetOutcome::Revision {
                asset,
                revision_sha256,
            } => BindingTarget::AssetRevision {
                asset,
                asset_revision_sha256: revision_sha256,
            },
            AssetOutcome::Unresolved(status) => {
                self.unresolved
                    .entry((role, reference.to_owned(), status.clone()))
                    .or_default()
                    .insert(ipn.as_str().to_owned());
                BindingTarget::Unresolved { status }
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

    fn symbol_asset(&mut self, reference: &str) -> Result<AssetOutcome, ImportError> {
        let resolved = match self.catalog.resolver.symbol(reference) {
            Ok(resolved) => resolved,
            Err(error) => {
                return Ok(AssetOutcome::Unresolved(Self::unresolved_reference(
                    reference, &error,
                )));
            }
        };
        let path = format!("symbols/{}.kicad_sym", resolved.lib_ref.library);
        let (bytes, file_sha) = self.read_source("symbol-library", &path)?;
        let (head, children) = sexpr::root_children(&bytes)
            .map_err(|error| ImportError(format!("cannot read `{path}`: {error}")))?;
        if head != "kicad_symbol_lib" {
            return Err(ImportError(format!(
                "`{path}` is not a KiCad symbol library"
            )));
        }
        let matches: Vec<_> = children
            .iter()
            .filter(|child| {
                child.head == "symbol" && child.first.as_deref() == Some(&resolved.lib_ref.item)
            })
            .collect();
        let child = match matches.as_slice() {
            [] => return Ok(AssetOutcome::Unresolved("missing-member".into())),
            [one] => (*one).clone(),
            _ => return Ok(AssetOutcome::Unresolved("ambiguous".into())),
        };
        let span = bytes[child.span.clone()].to_vec();
        if String::from_utf8_lossy(&span).contains("(extends") {
            self.diagnose(ImportDiagnostic::new(
                "symbol-extends-parent",
                reference,
                &path,
                &format!("symbol={}", resolved.lib_ref.item),
                format!("symbol `{reference}` extends another symbol, which its Asset Revision does not include"),
                json!({ "reference": reference }),
            ));
        }
        Ok(self.asset_revision(
            AssetKind::Symbol,
            reference,
            "kicad-symbol-entry",
            sexpr::file_version(&children),
            span,
            vec![],
            &path,
            &file_sha,
            Some([child.span.start as u64, child.span.end as u64]),
        ))
    }

    fn footprint_asset(&mut self, reference: &str) -> Result<AssetOutcome, ImportError> {
        let resolved = match self.catalog.resolver.footprint(reference) {
            Ok(resolved) => resolved,
            Err(ResolveError::LibraryNotFound { .. }) => {
                // The oracle checks only for the footprint file itself.
                return Ok(AssetOutcome::Unresolved("missing-member".into()));
            }
            Err(error) => {
                return Ok(AssetOutcome::Unresolved(Self::unresolved_reference(
                    reference, &error,
                )));
            }
        };
        let LibRef { library, item } = &resolved.lib_ref;
        let path = format!("footprints/{library}.pretty/{item}.kicad_mod");
        let (bytes, file_sha) = self.read_source("footprint", &path)?;
        let (head, children) = sexpr::root_children(&bytes)
            .map_err(|error| ImportError(format!("cannot read `{path}`: {error}")))?;
        if head != "footprint" && head != "module" {
            return Err(ImportError(format!("`{path}` is not a KiCad footprint")));
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut models = Vec::new();
        for model in sexpr::model_references(&text) {
            let classified = classify_model_reference(&model);
            let target = match &classified.candidate {
                Some(candidate) if input_exists(self.root(), candidate)? => {
                    match self.model_asset(candidate)? {
                        AssetOutcome::Revision {
                            asset,
                            revision_sha256,
                        } => ModelTarget::AssetRevision {
                            asset,
                            asset_revision_sha256: revision_sha256,
                        },
                        AssetOutcome::Unresolved(status) => ModelTarget::Unresolved { status },
                    }
                }
                Some(_) => ModelTarget::Unresolved {
                    status: "missing".into(),
                },
                None => ModelTarget::Unresolved {
                    status: classified.status.into(),
                },
            };
            if let ModelTarget::Unresolved { status } = &target {
                self.diagnose(ImportDiagnostic::new(
                    &format!("{status}-3d-model-reference"),
                    reference,
                    &path,
                    &format!("model={model}"),
                    format!(
                        "3D model reference `{model}` is {status} ({})",
                        classified.scope
                    ),
                    json!({ "footprint": reference, "reference": model, "scope": classified.scope }),
                ));
            }
            models.push(ModelReference {
                reference: model,
                scope: classified.scope.into(),
                target,
            });
        }
        Ok(self.asset_revision(
            AssetKind::Footprint,
            reference,
            "kicad-footprint",
            sexpr::file_version(&children),
            bytes,
            models,
            &path,
            &file_sha,
            None,
        ))
    }

    fn model_asset(&mut self, path: &str) -> Result<AssetOutcome, ImportError> {
        if let Some(outcome) = self.assets.get(&(AssetKind::Model3d, path.to_owned())) {
            return Ok(outcome.clone());
        }
        let (bytes, file_sha) = self.read_source("3d-model", path)?;
        let outcome = self.asset_revision(
            AssetKind::Model3d,
            path,
            "3d-model",
            None,
            bytes,
            vec![],
            path,
            &file_sha,
            None,
        );
        self.assets
            .insert((AssetKind::Model3d, path.to_owned()), outcome.clone());
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    fn asset_revision(
        &mut self,
        kind: AssetKind,
        key: &str,
        format: &str,
        format_version: Option<String>,
        bytes: Vec<u8>,
        model_references: Vec<ModelReference>,
        path: &str,
        file_sha: &str,
        byte_range: Option<[u64; 2]>,
    ) -> AssetOutcome {
        let blob = AssetBlob::new(bytes);
        let asset = CatalogAsset::new(kind, key);
        let declaration = AssetRevisionDeclaration {
            schema: ASSET_REVISION_SCHEMA.to_owned(),
            kind,
            key: key.to_owned(),
            format: format.to_owned(),
            format_version,
            blob_sha256: blob.sha256.clone(),
            blob_size: blob.bytes.len() as u64,
            model_references,
            license: LicenseDeclaration::UndeclaredNeedsReview,
        };
        let declaration_json = declaration.canonical_json();
        let revision_sha256 = sha256_hex(&declaration_json);
        let provenance = AssetProvenance {
            schema: PROVENANCE_SCHEMA.to_owned(),
            source: LEGACY_SOURCE.to_owned(),
            path: path.to_owned(),
            byte_sha256: file_sha.to_owned(),
            source_commit: self.source_commit.clone(),
            byte_range,
        };
        let outcome = AssetOutcome::Revision {
            asset: asset.id.as_str().to_owned(),
            revision_sha256: revision_sha256.clone(),
        };
        self.blobs.entry(blob.sha256.clone()).or_insert(blob);
        self.asset_revisions.push(AssetRevisionDraft {
            id: AssetRevisionId::new(&asset.id, &revision_sha256),
            asset,
            declaration,
            declaration_json,
            revision_sha256,
            provenance_json: serde_json::to_string(&provenance).expect("provenance serializes"),
            provenance,
        });
        outcome
    }
}

struct ClassifiedModel {
    scope: &'static str,
    status: &'static str,
    /// The repository path to look for, for a local reference.
    candidate: Option<String>,
}

/// The oracle's lexical, host-independent classification of a 3D model
/// reference (`tooling/import-fixture`).
fn classify_model_reference(reference: &str) -> ClassifiedModel {
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

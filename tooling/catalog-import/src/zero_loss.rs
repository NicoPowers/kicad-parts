//! The zero-loss report: every source field, substitute, CAD reference and
//! oracle diagnostic of one CSV catalog file, found again in the Catalog
//! records read back from PocketBase.
//!
//! The expected side is always the oracle manifest, never this crate's own
//! reading of the CSV. Each source field must be found in exactly one of
//! these places, with the same value:
//!
//! | destination                       | where it is checked                              |
//! |-----------------------------------|--------------------------------------------------|
//! | `part-details`                    | the current revision's `ipn` or named field     |
//! | `part-details.manufacturer-parts` | the current revision's manufacturer parts         |
//! | `supplier-data`                   | the current revision's source                     |
//! | `import-problem`                  | a stored import problem's detail                  |
//!
//! Manufacturer parts are part of the part details; the report names them
//! separately only to show where each `MPN` and `Manufacturer` went.

use catalog::csv_import::{
    CadItemSource, ColumnRoute, Source, SubstituteRoute, route_column, route_substitute_column,
};
use catalog::pocketbase::{CatalogReadback, StoredImportProblem, StoredPartRevision};
use catalog::{
    CadItemDetails, CadItemId, CadItemRevisionId, CadReferenceRole, CadReferenceTarget, License,
    ModelTarget, PartDetails, sha256_hex,
};
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup};
use legacy_import_fixture::Manifest;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const PART_DETAILS: &str = "part-details";
pub const MANUFACTURER_PARTS: &str = "part-details.manufacturer-parts";
pub const SUPPLIER_DATA: &str = "supplier-data";
pub const IMPORT_PROBLEM: &str = "import-problem";

#[derive(Clone, Debug, Serialize)]
pub struct ZeroLossReport {
    pub csv_file: String,
    pub source_path: String,
    pub oracle_byte_sha256: String,
    /// The CSV's hash as recorded on the Change(s).
    pub change_byte_sha256: Vec<String>,
    /// The hash of the CSV rebuilt from PocketBase alone: the stored header
    /// line plus every row's stored raw bytes in row order.
    pub rebuilt_byte_sha256: String,
    pub rows: usize,
    pub columns: usize,
    pub fields: usize,
    /// Fields per destination; they sum to `fields`.
    pub destinations: BTreeMap<String, usize>,
    /// Where each column went (a column of a row kept as an import problem
    /// goes there instead).
    pub column_routes: Vec<(String, String)>,
    pub parts: usize,
    pub substitutes: SubstitutesAccount,
    pub cad_items: Vec<CadItemAccount>,
    pub model_references: Vec<ModelAccount>,
    pub oracle_diagnostics: Vec<ProblemAccount>,
    /// Stored import problems that no oracle diagnostic or row explains.
    pub other_problems: Vec<String>,
    pub changes: Vec<ChangeAccount>,
    /// Every field, with its oracle and stored values.
    pub field_accounts: Vec<FieldAccount>,
    /// Everything that did not match. Empty means lossless.
    pub problems: Vec<String>,
}

impl ZeroLossReport {
    pub fn is_lossless(&self) -> bool {
        self.problems.is_empty()
    }

    /// The report without the per-field list, for printing.
    pub fn summary(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("the report serializes");
        if let Some(object) = value.as_object_mut() {
            object.remove("field_accounts");
            object.insert("lossless".into(), Value::Bool(self.is_lossless()));
        }
        value
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FieldAccount {
    pub row_number: usize,
    pub ipn: String,
    pub column: String,
    pub destination: String,
    pub oracle: String,
    pub stored: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SubstitutesAccount {
    pub path: String,
    pub oracle_byte_sha256: String,
    pub change_byte_sha256: Vec<String>,
    pub rows: usize,
    pub rows_in_scope: usize,
    pub fields_in_scope: usize,
    /// In-scope substitute fields per destination.
    pub destinations: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CadItemAccount {
    pub kind: String,
    pub reference: String,
    pub oracle_status: String,
    pub oracle_asset_path: Option<String>,
    pub oracle_byte_sha256: Option<String>,
    pub parts: usize,
    pub cad_item: Option<String>,
    pub cad_item_revision: Option<String>,
    pub snapshot_sha256: Option<String>,
    pub blob_sha256: Option<String>,
    pub blob_bytes: Option<usize>,
    pub license: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelAccount {
    pub source_footprint: String,
    pub reference: String,
    pub oracle_scope: String,
    pub oracle_status: String,
    pub stored: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProblemAccount {
    pub code: String,
    pub path: String,
    pub location: String,
    pub stored: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChangeAccount {
    pub id: String,
    pub seq: u64,
    pub kind: String,
    pub reason: String,
    pub source_commit: String,
    pub audit_entries: usize,
}

/// Builds the report for `csv_file` (such as `g-res`). `repository` is
/// read (never written) only to cut each symbol's byte range out of the
/// library file the oracle hashed.
pub fn report(
    oracle: &Manifest,
    csv_file: &str,
    readback: &CatalogReadback,
    repository: &Path,
) -> ZeroLossReport {
    Reporter::new(oracle, csv_file, readback, repository).run()
}

struct Reporter<'a> {
    oracle: &'a Manifest,
    csv_file: String,
    readback: &'a CatalogReadback,
    repository: &'a Path,
    problems: Vec<String>,
}

/// A part's current revision, decoded.
struct CurrentRevision<'a> {
    revision: &'a StoredPartRevision,
    snapshot: PartDetails,
    source: Source,
}

impl<'a> Reporter<'a> {
    fn new(
        oracle: &'a Manifest,
        csv_file: &str,
        readback: &'a CatalogReadback,
        repository: &'a Path,
    ) -> Self {
        Self {
            oracle,
            csv_file: csv_file.to_owned(),
            readback,
            repository,
            problems: Vec::new(),
        }
    }

    fn problem(&mut self, message: String) {
        self.problems.push(message);
    }

    fn current_revisions(&mut self) -> Vec<CurrentRevision<'a>> {
        let mut current_revisions = Vec::new();
        let readback = self.readback;
        for part in &readback.parts {
            let Some(revision) = readback.part_revision(&part.current_revision) else {
                self.problem(format!(
                    "{}: current revision {} is missing",
                    part.ipn, part.current_revision
                ));
                continue;
            };
            if sha256_hex(&revision.snapshot) != revision.snapshot_sha256 {
                self.problem(format!("{}: snapshot does not match its hash", part.ipn));
            }
            if revision.number != part.current_revision_number || revision.part != part.id {
                self.problem(format!(
                    "{}: current revision pointer is inconsistent",
                    part.ipn
                ));
            }
            let snapshot = match serde_json::from_str::<PartDetails>(&revision.snapshot) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.problem(format!("{}: snapshot does not parse: {error}", part.ipn));
                    continue;
                }
            };
            let source = match serde_json::from_str::<Source>(&revision.source) {
                Ok(source) => source,
                Err(error) => {
                    self.problem(format!("{}: source does not parse: {error}", part.ipn));
                    continue;
                }
            };
            if snapshot.ipn != part.ipn {
                self.problem(format!("{}: snapshot names {}", part.ipn, snapshot.ipn));
            }
            current_revisions.push(CurrentRevision {
                revision,
                snapshot,
                source,
            });
        }
        current_revisions
    }

    fn run(mut self) -> ZeroLossReport {
        let oracle = self.oracle;
        let readback = self.readback;
        let table = self.csv_file.trim_start_matches("g-").to_owned();
        let Some(category) = oracle
            .catalog
            .categories
            .iter()
            .find(|category| category.table == table)
        else {
            self.problem(format!("the oracle has no category `{table}`"));
            return self.empty();
        };
        let document = &category.document;
        let source_path = document.path.clone();
        let headers = &document.headers;
        let ipn_column = headers.iter().position(|header| header == "IPN");

        // Changes and the source files they recorded.
        let mut changes = Vec::new();
        let mut csv_hashes = BTreeSet::new();
        let mut substitute_hashes = BTreeSet::new();
        let mut header_raw = None;
        for change in &readback.changes {
            let audit_entries = readback
                .audit_entries
                .iter()
                .filter(|entry| entry.change == change.id)
                .count();
            if audit_entries != 1 {
                self.problem(format!(
                    "Change {} has {audit_entries} audit entries",
                    change.seq
                ));
            }
            // A Change never has zero revisions.
            let revisions = readback
                .part_revisions
                .iter()
                .filter(|revision| revision.change == change.id)
                .count()
                + readback
                    .cad_item_revisions
                    .iter()
                    .filter(|revision| revision.change == change.id)
                    .count();
            if revisions == 0 {
                self.problem(format!("Change {} has no revisions", change.seq));
            }
            if change.reason.trim().is_empty() {
                self.problem(format!("Change {} has no reason", change.seq));
            }
            let sources: Value = serde_json::from_str(&change.sources).unwrap_or(Value::Null);
            for file in sources["files"].as_array().into_iter().flatten() {
                let path = file["path"].as_str().unwrap_or_default();
                let sha = file["byte_sha256"].as_str().unwrap_or_default().to_owned();
                if path == source_path {
                    if sha == document.byte_sha256 {
                        header_raw = file["header_raw"].as_str().map(str::to_owned);
                    }
                    csv_hashes.insert(sha);
                } else if path == oracle.substitutes.path {
                    substitute_hashes.insert(sha);
                }
            }
            changes.push(ChangeAccount {
                id: change.id.clone(),
                seq: change.seq,
                kind: change.kind.clone(),
                reason: change.reason.clone(),
                source_commit: change.source_commit.clone(),
                audit_entries,
            });
        }
        if !csv_hashes.contains(&document.byte_sha256) {
            self.problem(format!(
                "no Change recorded {source_path} with the oracle's byte hash"
            ));
        }

        let current_revisions = self.current_revisions();
        let by_row: BTreeMap<usize, &CurrentRevision> = current_revisions
            .iter()
            .filter(|current_revision| current_revision.source.path == source_path)
            .map(|current_revision| (current_revision.source.row_number, current_revision))
            .collect();
        let problems = &readback.problems;
        let problem_rows: BTreeMap<usize, (&StoredImportProblem, Value)> = problems
            .iter()
            .filter(|problem| problem.path == source_path)
            .filter_map(|problem| {
                let detail: Value = serde_json::from_str(&problem.detail).ok()?;
                let row = detail["row_number"].as_u64()? as usize;
                detail["raw_record"].as_str()?;
                Some((row, (problem, detail)))
            })
            .collect();

        let mut destinations: BTreeMap<String, usize> = BTreeMap::new();
        let mut column_routes: BTreeMap<String, String> = BTreeMap::new();
        let mut field_accounts = Vec::new();
        let mut explained_problems = BTreeSet::new();
        let mut rebuilt = header_raw.clone().unwrap_or_default();
        let mut scope_ipns = BTreeSet::new();
        let mut matched_heads = BTreeSet::new();
        let mut rows_with_main_part: BTreeMap<String, bool> = BTreeMap::new();
        if header_raw.is_none() {
            self.problem(format!("no Change kept the header line of {source_path}"));
        }

        for (index, row) in document.rows.iter().enumerate() {
            let row_number = index + 1;
            let ipn = ipn_column
                .and_then(|column| row.get(column))
                .cloned()
                .unwrap_or_default();
            let mut account =
                |column: &str, destination: &str, oracle: &str, stored: Option<String>| {
                    *destinations.entry(destination.to_owned()).or_default() += 1;
                    column_routes
                        .entry(column.to_owned())
                        .or_insert_with(|| destination.to_owned());
                    field_accounts.push(FieldAccount {
                        row_number,
                        ipn: ipn.clone(),
                        column: column.to_owned(),
                        destination: destination.to_owned(),
                        oracle: oracle.to_owned(),
                        stored,
                    });
                };
            if let Some(current_revision) = by_row.get(&row_number) {
                matched_heads.insert(current_revision.revision.id.clone());
                scope_ipns.insert(current_revision.snapshot.ipn.clone());
                let source = &current_revision.source;
                rebuilt.push_str(&source.raw_record);
                let mut padded = source.record.clone();
                padded.resize(headers.len(), String::new());
                if &source.headers != headers || &padded != row {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): Source does not hold the oracle row"
                    ));
                }
                if source.byte_sha256 != document.byte_sha256 {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): Source names another version of {source_path}"
                    ));
                }
                let snapshot = &current_revision.snapshot;
                // The row made the main manufacturer part only if
                // it has the columns and was not kept as an incomplete one;
                // otherwise the first entry is a substitute.
                let incomplete = problems.iter().find(|problem| {
                    problem.code == "incomplete-main-manufacturer-part"
                        && problem.subject == snapshot.ipn
                });
                let has_columns = headers.iter().any(|header| {
                    matches!(
                        route_column(header),
                        ColumnRoute::MainMpn | ColumnRoute::MainManufacturer
                    )
                });
                let row_made_main = has_columns && incomplete.is_none();
                rows_with_main_part.insert(snapshot.ipn.clone(), row_made_main);
                let main = snapshot
                    .manufacturer_parts
                    .first()
                    .filter(|_| row_made_main);
                let mut snapshot_columns = Vec::new();
                let mut supplier_columns = Vec::new();
                for (column, value) in headers.iter().zip(row) {
                    let route = route_column(column);
                    let (destination, stored) = match route {
                        ColumnRoute::Ipn => (PART_DETAILS, Some(snapshot.ipn.clone())),
                        ColumnRoute::PartDetails => {
                            snapshot_columns.push(column.clone());
                            (PART_DETAILS, snapshot.field(column).map(str::to_owned))
                        }
                        ColumnRoute::MainMpn | ColumnRoute::MainManufacturer => {
                            match (main, incomplete) {
                                (Some(main), _) => (
                                    MANUFACTURER_PARTS,
                                    Some(if route == ColumnRoute::MainMpn {
                                        main.mpn.clone()
                                    } else {
                                        main.manufacturer.clone()
                                    }),
                                ),
                                (None, Some(problem)) => {
                                    explained_problems.insert(problem.id.clone());
                                    let detail: Value =
                                        serde_json::from_str(&problem.detail).unwrap_or_default();
                                    (
                                        IMPORT_PROBLEM,
                                        detail[column.as_str()].as_str().map(str::to_owned),
                                    )
                                }
                                (None, None) => (MANUFACTURER_PARTS, None),
                            }
                        }
                        ColumnRoute::SupplierData => {
                            supplier_columns.push(column.clone());
                            (
                                SUPPLIER_DATA,
                                source
                                    .supplier_data
                                    .iter()
                                    .find(|fact| &fact.column == column)
                                    .map(|fact| fact.value.clone()),
                            )
                        }
                    };
                    if stored.as_deref() != Some(value.as_str()) {
                        self.problems.push(format!(
                            "row {row_number} ({ipn}) field {column}: oracle {value:?}, stored {stored:?} in {destination}"
                        ));
                    }
                    account(column, destination, value, stored);
                }
                // Exactly one place: no extra or repeated snapshot fields or
                // supplier data.
                let stored_fields: Vec<&String> =
                    snapshot.fields.iter().map(|field| &field.name).collect();
                if stored_fields != snapshot_columns.iter().collect::<Vec<_>>() {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): snapshot fields {stored_fields:?} are not the part-details columns {snapshot_columns:?}"
                    ));
                }
                let stored_facts: Vec<&String> = source
                    .supplier_data
                    .iter()
                    .map(|fact| &fact.column)
                    .collect();
                if stored_facts != supplier_columns.iter().collect::<Vec<_>>() {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): supplier data {stored_facts:?} are not {supplier_columns:?}"
                    ));
                }
                for fact_column in &supplier_columns {
                    if let Some(value) = row
                        .get(
                            headers
                                .iter()
                                .position(|h| h == fact_column)
                                .unwrap_or(usize::MAX),
                        )
                        .filter(|value| !value.is_empty())
                    {
                        if current_revision
                            .revision
                            .snapshot
                            .contains(&format!("\"{value}\""))
                        {
                            self.problems.push(format!(
                                "row {row_number} ({ipn}): supplier data {fact_column}={value} also appears in the snapshot"
                            ));
                        }
                    }
                }
            } else if let Some((problem, detail)) = problem_rows.get(&row_number) {
                explained_problems.insert(problem.id.clone());
                rebuilt.push_str(detail["raw_record"].as_str().unwrap_or_default());
                let record: Vec<String> = detail["record"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|value| value.as_str().unwrap_or_default().to_owned())
                    .collect();
                for (position, (column, value)) in headers.iter().zip(row).enumerate() {
                    let stored = Some(record.get(position).cloned().unwrap_or_default());
                    if stored.as_deref() != Some(value.as_str()) {
                        self.problems.push(format!(
                            "row {row_number} field {column}: oracle {value:?}, problem kept {stored:?}"
                        ));
                    }
                    account(column, IMPORT_PROBLEM, value, stored);
                }
            } else {
                self.problems.push(format!(
                    "row {row_number} ({ipn}) is neither a part nor an import problem"
                ));
                for (column, value) in headers.iter().zip(row) {
                    account(column, "lost", value, None);
                }
            }
        }
        for current_revision in &current_revisions {
            if current_revision.source.path == source_path
                && !matched_heads.contains(&current_revision.revision.id)
            {
                self.problems.push(format!(
                    "{} comes from row {}, which the oracle does not have",
                    current_revision.snapshot.ipn, current_revision.source.row_number
                ));
            }
        }
        let rebuilt_byte_sha256 = sha256_hex(rebuilt.as_bytes());
        if rebuilt_byte_sha256 != document.byte_sha256 {
            self.problem(format!(
                "{source_path} rebuilt from PocketBase hashes to {rebuilt_byte_sha256}, not the oracle's {}",
                document.byte_sha256
            ));
        }

        let substitutes = self.substitutes(
            &current_revisions,
            &scope_ipns,
            &substitute_hashes,
            &rows_with_main_part,
            &mut explained_problems,
        );
        let (cad_items, footprint_paths) =
            self.cad_items(&current_revisions, &scope_ipns, &mut explained_problems);
        let model_references = self.models(&footprint_paths, &mut explained_problems);
        let oracle_diagnostics =
            self.oracle_diagnostics(&footprint_paths, &cad_items, &mut explained_problems);
        // Every import problem belongs to the audit entry of the run that
        // found it.
        for problem in problems {
            if !readback
                .audit_entries
                .iter()
                .any(|entry| entry.id == problem.audit_entry)
            {
                self.problems.push(format!(
                    "import problem {} at {} has no audit entry",
                    problem.code, problem.location
                ));
            }
        }
        let other_problems = problems
            .iter()
            .filter(|problem| !explained_problems.contains(&problem.id))
            .map(|problem| format!("{} {} {}", problem.code, problem.path, problem.location))
            .collect();

        let fields = field_accounts.len();
        if fields != document.rows.len() * headers.len() {
            self.problem(format!(
                "{fields} fields accounted for, but the oracle has {}",
                document.rows.len() * headers.len()
            ));
        }
        ZeroLossReport {
            csv_file: self.csv_file.clone(),
            source_path,
            oracle_byte_sha256: document.byte_sha256.clone(),
            change_byte_sha256: csv_hashes.into_iter().collect(),
            rebuilt_byte_sha256,
            rows: document.rows.len(),
            columns: headers.len(),
            fields,
            destinations,
            column_routes: headers
                .iter()
                .map(|header| {
                    (
                        header.clone(),
                        column_routes.get(header).cloned().unwrap_or_default(),
                    )
                })
                .collect(),
            parts: matched_heads.len(),
            substitutes,
            cad_items,
            model_references,
            oracle_diagnostics,
            other_problems,
            changes,
            field_accounts,
            problems: self.problems,
        }
    }

    fn empty(self) -> ZeroLossReport {
        ZeroLossReport {
            csv_file: self.csv_file,
            source_path: String::new(),
            oracle_byte_sha256: String::new(),
            change_byte_sha256: vec![],
            rebuilt_byte_sha256: String::new(),
            rows: 0,
            columns: 0,
            fields: 0,
            destinations: BTreeMap::new(),
            column_routes: vec![],
            parts: 0,
            substitutes: SubstitutesAccount::default(),
            cad_items: vec![],
            model_references: vec![],
            oracle_diagnostics: vec![],
            other_problems: vec![],
            changes: vec![],
            field_accounts: vec![],
            problems: self.problems,
        }
    }

    fn substitutes(
        &mut self,
        current_revisions: &[CurrentRevision],
        scope: &BTreeSet<String>,
        hashes: &BTreeSet<String>,
        rows_with_main_part: &BTreeMap<String, bool>,
        explained: &mut BTreeSet<String>,
    ) -> SubstitutesAccount {
        let oracle = self.oracle;
        let readback = self.readback;
        let document = &oracle.substitutes;
        if !hashes.contains(&document.byte_sha256) {
            self.problem(format!(
                "no Change recorded {} with the oracle's byte hash",
                document.path
            ));
        }
        let ipn_column = document.headers.iter().position(|header| header == "IPN");
        let mut account = SubstitutesAccount {
            path: document.path.clone(),
            oracle_byte_sha256: document.byte_sha256.clone(),
            change_byte_sha256: hashes.iter().cloned().collect(),
            rows: document.rows.len(),
            ..SubstitutesAccount::default()
        };
        for (index, row) in document.rows.iter().enumerate() {
            let row_number = index + 1;
            let ipn = ipn_column
                .and_then(|column| row.get(column))
                .cloned()
                .unwrap_or_default();
            if !scope.contains(&ipn) {
                continue;
            }
            account.rows_in_scope += 1;
            account.fields_in_scope += row.len();
            // Entries come after the main manufacturer part only if the row
            // made one.
            let offset = usize::from(rows_with_main_part.get(&ipn).copied().unwrap_or(false));
            let current = current_revisions
                .iter()
                .find(|current| current.snapshot.ipn == ipn);
            let found = current.and_then(|current| {
                let position = current
                    .source
                    .substitutes
                    .iter()
                    .position(|source| source.row_number == row_number)?;
                let manufacturer_part =
                    current.snapshot.manufacturer_parts.get(position + offset)?;
                Some((
                    current,
                    manufacturer_part,
                    &current.source.substitutes[position],
                ))
            });
            let Some((current, manufacturer_part, source)) = found else {
                // A row that could not become a manufacturer part
                // must be kept whole by an import problem.
                let location = format!("rows[{row_number}]");
                let kept = readback.problems.iter().find(|problem| {
                    let detail: Value = serde_json::from_str(&problem.detail).unwrap_or_default();
                    let record: Vec<String> = detail["record"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|value| value.as_str().unwrap_or_default().to_owned())
                        .collect();
                    let mut padded = record;
                    padded.resize(row.len(), String::new());
                    problem.path == document.path && problem.location == location && &padded == row
                });
                match kept {
                    Some(problem) => {
                        explained.insert(problem.id.clone());
                        *account
                            .destinations
                            .entry(IMPORT_PROBLEM.to_owned())
                            .or_default() += row.len();
                    }
                    None => self.problem(format!(
                        "substitute row {row_number} of {ipn} is neither a manufacturer part nor an import problem"
                    )),
                }
                continue;
            };
            // The row's source keeps the whole row, datasheet included.
            let mut padded = source.record.clone();
            padded.resize(row.len(), String::new());
            if &padded != row {
                self.problem(format!(
                    "substitute row {row_number} of {ipn}: its source does not hold the oracle row"
                ));
            }
            let part_datasheet = current.snapshot.field("Datasheet").unwrap_or_default();
            for (column, value) in document.headers.iter().zip(row) {
                let (destination, stored) = match route_substitute_column(column) {
                    SubstituteRoute::Ipn => (MANUFACTURER_PARTS, Some(ipn.clone())),
                    SubstituteRoute::Mpn => {
                        (MANUFACTURER_PARTS, Some(manufacturer_part.mpn.clone()))
                    }
                    SubstituteRoute::Manufacturer => (
                        MANUFACTURER_PARTS,
                        Some(manufacturer_part.manufacturer.clone()),
                    ),
                    // A part has one datasheet. A blank one adds nothing and
                    // the part's own is already in its part details; any
                    // other must be an import problem.
                    SubstituteRoute::Datasheet if value.is_empty() => {
                        (PART_DETAILS, Some(String::new()))
                    }
                    SubstituteRoute::Datasheet if value == part_datasheet => {
                        (PART_DETAILS, Some(part_datasheet.to_owned()))
                    }
                    SubstituteRoute::Datasheet => {
                        let location = format!("rows[{row_number}]");
                        let kept = readback.problems.iter().find(|problem| {
                            problem.code == "substitute-datasheet-differs"
                                && problem.subject == ipn
                                && problem.location == location
                        });
                        let stored = kept.and_then(|problem| {
                            explained.insert(problem.id.clone());
                            let detail: Value =
                                serde_json::from_str(&problem.detail).unwrap_or_default();
                            detail["datasheet"].as_str().map(str::to_owned)
                        });
                        (IMPORT_PROBLEM, stored)
                    }
                    SubstituteRoute::SupplierData => (
                        SUPPLIER_DATA,
                        source
                            .supplier_data
                            .iter()
                            .find(|fact| &fact.column == column)
                            .map(|fact| fact.value.clone()),
                    ),
                    SubstituteRoute::Unknown => ("lost", None),
                };
                *account
                    .destinations
                    .entry(destination.to_owned())
                    .or_default() += 1;
                if stored.as_deref() != Some(value.as_str()) {
                    self.problem(format!(
                        "substitute row {row_number} field {column}: oracle {value:?}, stored {stored:?} in {destination}"
                    ));
                }
            }
        }
        account
    }

    /// Checks every symbol and footprint reference of the CSV file's rows.
    /// Returns the accounts and the repository paths of resolved
    /// footprints.
    fn cad_items(
        &mut self,
        current_revisions: &[CurrentRevision],
        scope: &BTreeSet<String>,
        explained: &mut BTreeSet<String>,
    ) -> (Vec<CadItemAccount>, BTreeSet<String>) {
        let oracle = self.oracle;
        let mut accounts = Vec::new();
        let readback = self.readback;
        let mut footprint_paths = BTreeSet::new();
        let references = oracle
            .assets
            .symbol_references
            .iter()
            .chain(&oracle.assets.footprint_references);
        for reference in references {
            let users: Vec<&String> = reference
                .source_ipns
                .iter()
                .filter(|ipn| scope.contains(*ipn))
                .collect();
            if users.is_empty() {
                continue;
            }
            let (role, files) = if reference.kind == "symbol" {
                (CadReferenceRole::Symbol, &oracle.assets.symbols)
            } else {
                (CadReferenceRole::Footprint, &oracle.assets.footprints)
            };
            let oracle_file = reference
                .asset_path
                .as_ref()
                .and_then(|path| files.iter().find(|file| &file.path == path));
            let mut account = CadItemAccount {
                kind: reference.kind.clone(),
                reference: reference.reference.clone(),
                oracle_status: reference.status.clone(),
                oracle_asset_path: reference.asset_path.clone(),
                oracle_byte_sha256: oracle_file.map(|file| file.byte_sha256.clone()),
                parts: users.len(),
                cad_item: None,
                cad_item_revision: None,
                snapshot_sha256: None,
                blob_sha256: None,
                blob_bytes: None,
                license: None,
            };
            let mut targets = BTreeSet::new();
            for ipn in &users {
                let Some(current_revision) = current_revisions
                    .iter()
                    .find(|current_revision| &&current_revision.snapshot.ipn == ipn)
                else {
                    self.problem(format!(
                        "{ipn} uses {} but is not a part",
                        reference.reference
                    ));
                    continue;
                };
                let bindings: Vec<_> = current_revision
                    .snapshot
                    .cad_references
                    .iter()
                    .filter(|binding| {
                        binding.role == role && binding.reference == reference.reference
                    })
                    .collect();
                match bindings.as_slice() {
                    [binding] => {
                        targets.insert(serde_json::to_string(&binding.target).unwrap_or_default());
                    }
                    _ => self.problem(format!(
                        "{ipn} has {} bindings for {}",
                        bindings.len(),
                        reference.reference
                    )),
                }
            }
            if targets.len() != 1 {
                self.problem(format!(
                    "{} binds {} different targets across its parts",
                    reference.reference,
                    targets.len()
                ));
                accounts.push(account);
                continue;
            }
            let target: CadReferenceTarget =
                serde_json::from_str(targets.iter().next().expect("one target")).expect("parses");
            match (&target, reference.local, reference.status.as_str()) {
                (CadReferenceTarget::External, false, _) => {}
                (
                    CadReferenceTarget::CadItemRevision {
                        cad_item,
                        snapshot_sha256,
                    },
                    true,
                    "resolved",
                ) => {
                    self.check_cad_item_revision(
                        &mut account,
                        cad_item,
                        snapshot_sha256,
                        oracle_file.map(|file| file.byte_sha256.as_str()),
                    );
                    if role == CadReferenceRole::Footprint {
                        if let Some(path) = &reference.asset_path {
                            footprint_paths.insert(path.clone());
                        }
                    }
                }
                (CadReferenceTarget::Unresolved { status }, true, oracle_status)
                    if status == oracle_status =>
                {
                    let column = if role == CadReferenceRole::Symbol {
                        "Symbol"
                    } else {
                        "Footprint"
                    };
                    let code = format!("{status}-local-{}-reference", reference.kind);
                    let location = format!("{column}={}", reference.reference);
                    match readback
                        .problems
                        .iter()
                        .find(|problem| problem.code == code && problem.location == location)
                    {
                        Some(problem) => {
                            explained.insert(problem.id.clone());
                        }
                        None => self.problem(format!("no import problem {code} for {location}")),
                    }
                }
                (target, local, status) => self.problem(format!(
                    "{} (oracle: local={local}, {status}) is stored as {target:?}",
                    reference.reference
                )),
            }
            accounts.push(account);
        }
        (accounts, footprint_paths)
    }

    fn check_cad_item_revision(
        &mut self,
        account: &mut CadItemAccount,
        cad_item_id: &str,
        snapshot_sha256: &str,
        oracle_file_sha: Option<&str>,
    ) {
        let readback = self.readback;
        let reference = account.reference.clone();
        account.cad_item = Some(cad_item_id.to_owned());
        account.snapshot_sha256 = Some(snapshot_sha256.to_owned());
        let Some(cad_item) = readback.cad_item(cad_item_id) else {
            self.problem(format!("{reference}: CAD item {cad_item_id} is missing"));
            return;
        };
        if cad_item.kind != account.kind || cad_item.key != reference {
            self.problem(format!(
                "{reference}: CAD item is {} {}",
                cad_item.kind, cad_item.key
            ));
        }
        let revision_id =
            CadItemRevisionId::new(&CadItemId::from_stored(cad_item_id), snapshot_sha256);
        account.cad_item_revision = Some(revision_id.to_string());
        let Some(revision) = readback.cad_item_revision(revision_id.as_str()) else {
            self.problem(format!(
                "{reference}: CAD item revision {revision_id} is missing"
            ));
            return;
        };
        if sha256_hex(&revision.snapshot) != snapshot_sha256
            || revision.snapshot_sha256 != snapshot_sha256
        {
            self.problem(format!(
                "{reference}: snapshot does not match its revision hash"
            ));
        }
        let Ok(snapshot) = serde_json::from_str::<CadItemDetails>(&revision.snapshot) else {
            self.problem(format!("{reference}: snapshot does not parse"));
            return;
        };
        account.license = Some(revision.license.clone());
        if snapshot.license != License::UnknownNeedsReview
            || revision.license != snapshot.license.status()
        {
            self.problem(format!("{reference}: license is not explicit"));
        }
        let Some(blob) = readback.blob(&revision.blob) else {
            self.problem(format!("{reference}: blob {} is missing", revision.blob));
            return;
        };
        account.blob_sha256 = Some(blob.sha256.clone());
        account.blob_bytes = Some(blob.bytes.len());
        if blob.sha256 != snapshot.blob_sha256 || sha256_hex(&blob.bytes) != blob.sha256 {
            self.problem(format!(
                "{reference}: stored blob is not the declared bytes"
            ));
        }
        let Ok(source) = serde_json::from_str::<CadItemSource>(&revision.source) else {
            self.problem(format!("{reference}: source does not parse"));
            return;
        };
        if Some(source.path.as_str()) != account.oracle_asset_path.as_deref()
            || Some(source.byte_sha256.as_str()) != oracle_file_sha
        {
            self.problem(format!(
                "{reference}: Source names {} {}, the oracle {:?} {:?}",
                source.path, source.byte_sha256, account.oracle_asset_path, oracle_file_sha
            ));
        }
        match source.byte_range {
            // A whole file: the blob is the file the oracle hashed.
            None => {
                if Some(blob.sha256.as_str()) != oracle_file_sha {
                    self.problem(format!("{reference}: blob is not the oracle's file"));
                }
            }
            // One symbol: the blob is that byte range of the oracle's file.
            Some([start, end]) => {
                let file = std::fs::read(self.repository.join(&source.path)).unwrap_or_default();
                if Some(sha256_hex(&file).as_str()) != oracle_file_sha {
                    self.problem(format!(
                        "{reference}: {} is not the oracle's file",
                        source.path
                    ));
                } else {
                    if file.get(start as usize..end as usize) != Some(blob.bytes.as_slice()) {
                        self.problem(format!(
                            "{reference}: blob is not bytes {start}..{end} of the oracle's file"
                        ));
                    }
                    // The range must be exactly the whole top-level symbol,
                    // as KiCad Integration reads the oracle's file.
                    let item = reference.split_once(':').map_or("", |(_, item)| item);
                    let whole = SymbolLibrary::parse(&file)
                        .ok()
                        .and_then(|library| match library.find(item) {
                            SymbolLookup::Found(entry) => Some(entry.span.clone()),
                            _ => None,
                        });
                    if whole != Some(start as usize..end as usize) {
                        self.problem(format!(
                            "{reference}: bytes {start}..{end} are not the whole symbol `{item}` ({whole:?})"
                        ));
                    }
                }
            }
        }
    }

    fn models(
        &mut self,
        footprint_paths: &BTreeSet<String>,
        explained: &mut BTreeSet<String>,
    ) -> Vec<ModelAccount> {
        let oracle = self.oracle;
        let readback = self.readback;
        let mut accounts = Vec::new();
        for model in &oracle.assets.model_references {
            if !footprint_paths.contains(&model.source_footprint) {
                continue;
            }
            // The footprint's stored snapshot must list the reference.
            let snapshot = readback.cad_item_revisions.iter().find_map(|revision| {
                let source: CadItemSource = serde_json::from_str(&revision.source).ok()?;
                (source.path == model.source_footprint).then_some(())?;
                serde_json::from_str::<CadItemDetails>(&revision.snapshot).ok()
            });
            let stored = snapshot.and_then(|snapshot| {
                snapshot
                    .model_references
                    .into_iter()
                    .find(|stored| stored.reference == model.reference)
            });
            let description = match &stored {
                None => {
                    self.problem(format!(
                        "{} does not record its 3D model reference {}",
                        model.source_footprint, model.reference
                    ));
                    "missing".to_owned()
                }
                Some(stored) => {
                    if stored.scope != model.scope {
                        self.problem(format!(
                            "3D model {}: scope {} but the oracle says {}",
                            model.reference, stored.scope, model.scope
                        ));
                    }
                    match (&stored.target, model.status.as_str()) {
                        (
                            ModelTarget::CadItemRevision {
                                cad_item,
                                snapshot_sha256,
                            },
                            "resolved",
                        ) => {
                            let oracle_sha = model.asset_path.as_ref().and_then(|path| {
                                oracle
                                    .assets
                                    .models_3d
                                    .iter()
                                    .find(|file| &file.path == path)
                                    .map(|file| file.byte_sha256.clone())
                            });
                            let mut account = CadItemAccount {
                                kind: "model-3d".into(),
                                reference: model.asset_path.clone().unwrap_or_default(),
                                oracle_status: model.status.clone(),
                                oracle_asset_path: model.asset_path.clone(),
                                oracle_byte_sha256: oracle_sha.clone(),
                                parts: 0,
                                cad_item: None,
                                cad_item_revision: None,
                                snapshot_sha256: None,
                                blob_sha256: None,
                                blob_bytes: None,
                                license: None,
                            };
                            self.check_cad_item_revision(
                                &mut account,
                                cad_item,
                                snapshot_sha256,
                                oracle_sha.as_deref(),
                            );
                            format!("cad-item-revision {snapshot_sha256}")
                        }
                        (ModelTarget::Unresolved { status }, oracle_status)
                            if status == oracle_status =>
                        {
                            let code = format!("{status}-3d-model-reference");
                            let location = format!("model={}", model.reference);
                            match readback.problems.iter().find(|problem| {
                                problem.code == code
                                    && problem.path == model.source_footprint
                                    && problem.location == location
                            }) {
                                Some(problem) => {
                                    explained.insert(problem.id.clone());
                                }
                                None => self.problem(format!(
                                    "no import problem {code} for {location} in {}",
                                    model.source_footprint
                                )),
                            }
                            format!("unresolved ({status}) + import problem")
                        }
                        (target, status) => {
                            self.problem(format!(
                                "3D model {}: stored {target:?}, oracle {status}",
                                model.reference
                            ));
                            format!("{target:?}")
                        }
                    }
                }
            };
            accounts.push(ModelAccount {
                source_footprint: model.source_footprint.clone(),
                reference: model.reference.clone(),
                oracle_scope: model.scope.clone(),
                oracle_status: model.status.clone(),
                stored: description,
            });
        }
        accounts
    }

    /// Every oracle diagnostic about this CSV file's CAD must be stored as an
    /// import problem with the same code and location (and, for 3D
    /// models, the same footprint path and message).
    fn oracle_diagnostics(
        &mut self,
        footprint_paths: &BTreeSet<String>,
        cad_items: &[CadItemAccount],
        explained: &mut BTreeSet<String>,
    ) -> Vec<ProblemAccount> {
        let references: BTreeSet<String> = cad_items
            .iter()
            .map(|cad_item| {
                let column = if cad_item.kind == "symbol" {
                    "Symbol"
                } else {
                    "Footprint"
                };
                format!("{column}={}", cad_item.reference)
            })
            .collect();
        let oracle = self.oracle;
        let readback = self.readback;
        let mut accounts = Vec::new();
        for problem in &oracle.diagnostics {
            let model = footprint_paths.contains(&problem.path);
            let reference =
                problem.code.ends_with("-reference") && references.contains(&problem.location);
            if !model && !reference {
                continue;
            }
            let stored = readback.problems.iter().find(|stored| {
                stored.code == problem.code
                    && stored.location == problem.location
                    && (!model
                        || (stored.path == problem.path && stored.message == problem.message))
            });
            match stored {
                Some(stored) => {
                    explained.insert(stored.id.clone());
                }
                None => self.problem(format!(
                    "oracle diagnostic {} {} {} has no import problem",
                    problem.code, problem.path, problem.location
                )),
            }
            accounts.push(ProblemAccount {
                code: problem.code.clone(),
                path: problem.path.clone(),
                location: problem.location.clone(),
                stored: stored.map(|stored| stored.id.clone()),
            });
        }
        accounts
    }
}

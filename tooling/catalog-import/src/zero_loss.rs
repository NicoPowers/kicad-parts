//! The zero-loss report: every source field, substitutes.csv row, CAD reference and
//! oracle import problem of one CSV catalog file, found again in the Catalog
//! records read back from PocketBase.
//!
//! The expected side is always the oracle manifest, never this crate's own
//! reading of the CSV. The report rebuilds the CSV file from the latest
//! import run of that file alone (its audit entry records the header line
//! and every row's bytes), so later edits do not matter, and checks that it
//! is the oracle's file byte for byte. Rows are matched by IPN, not by row
//! number. Each source field must then be found in exactly one of these
//! places, with the same value:
//!
//! | destination                       | where it is checked                                         |
//! |-----------------------------------|-------------------------------------------------------------|
//! | `part-details`                    | the part's details after the latest import run: `ipn` or a named field |
//! | `part-details.manufacturer-parts` | those details' manufacturer parts                           |
//! | `part-revision`                   | the part's revision after the latest import run: its Symbol or Footprint field, exactly |
//! | `supplier-data`                   | the latest import run's row (supplier data never makes a revision) |
//! | `import-problem`                  | a stored import problem's detail                            |
//!
//! Manufacturer parts are part of the part details; the report names them
//! separately only to show where each `MPN` and `Manufacturer` went. A part
//! that an earlier import run of the file made, but that the file no longer
//! has, must have a `part-not-in-csv-file` import problem.
//!
//! Where every column of the 21 CSV files goes (`route_column` in
//! `crates/catalog`; the test `every_column_of_every_csv_file_has_one_destination`
//! checks this table against the oracle's headers):
//!
//! - `IPN` and every column not named below: `part-details`.
//! - `Symbol` and `Footprint`: `part-revision`, where they bind CAD item
//!   revisions.
//! - `MPN` and `Manufacturer`: `part-details.manufacturer-parts`, the main
//!   manufacturer part. If either is blank, an
//!   `incomplete-main-manufacturer-part` import problem keeps both.
//! - `MPN2` (any `MPN` followed by a number): `import-problem`
//!   (`mpn-without-manufacturer`). It is another approved MPN, but the CSV
//!   file does not say who makes it, and a manufacturer part needs a
//!   manufacturer. A blank cell adds nothing.
//! - `MPN TBC`: `import-problem` (`mpn-to-be-confirmed`). These MPNs are
//!   still to be confirmed, so they are not approved manufacturer parts.
//! - Any other column whose name mentions MPN in any case (`MPN 2`,
//!   `Alt MPN`, `mpn2`): `import-problem` (`unknown-mpn-column`), never a
//!   guessed part-details field. No CSV file has one today.
//! - `LCSC`, `DigiKey_PN`, `Mouser_PN`, `DigiKey_Price`, `Mouser_Price`,
//!   `Price_Range`, `Price_LastSynced_UTC`: `supplier-data`.
//!
//! `Symbol` and `Footprint` go to `part-revision` in every file but g-pcb,
//! so the table leaves them out. A Symbol that names several symbols, or a
//! Footprint that lists a choice more than once, still goes there exactly as
//! the CSV says (each listed reference is bound), and the row also needs a
//! `several-symbols` or `repeated-footprint-choice` import problem, as a
//! missing symbol needs its own.
//!
//! | CSV file | `part-details` besides `IPN` | manufacturer parts | `import-problem` |
//! |----------|------------------------------|--------------------|------------------|
//! | g-ana | Description, Datasheet, Sim_Library, Sim_Name, Sim_Device, Sim_Pins | MPN, Manufacturer | |
//! | g-art | Description, Comments | | |
//! | g-cap | Description, Value, Voltage, Material, Tolerance, Datasheet | MPN, Manufacturer | |
//! | g-con | Value, Description, Pins, Datasheet | MPN, Manufacturer | |
//! | g-cpd | Description, Datasheet | MPN, Manufacturer | |
//! | g-dio | Value, Description, Current, Voltage, Datasheet | MPN, Manufacturer | |
//! | g-ics | Description, Datasheet | MPN, Manufacturer | |
//! | g-ind | Description, Value, Current, Datasheet | MPN, Manufacturer | |
//! | g-mcu | Description, Datasheet | MPN, Manufacturer | |
//! | g-mec | Description, Type, Datasheet | MPN, Manufacturer | |
//! | g-mpu | Description, Datasheet | MPN, Manufacturer | |
//! | g-opt | Description, Color, I-forward-max, V-forward, Wavelength, Datasheet | MPN, Manufacturer | |
//! | g-osc | Description, Frequency, Stability, Load, Datasheet | MPN, Manufacturer | |
//! | g-pcb | Description, Project link | | |
//! | g-pwr | Description, Datasheet | MPN, Manufacturer | |
//! | g-reg | Description, Voltage, Current, Datasheet | MPN, Manufacturer | |
//! | g-res | Description, Value, Tolerance, Datasheet | MPN, Manufacturer | |
//! | g-rfm | Value, Description, Datasheet | MPN, Manufacturer | MPN2 |
//! | g-rvr | Description, Resistance, Tolerance, Turns, Datasheet | MPN, Manufacturer | MPN TBC |
//! | g-swi | Description, Form, Datasheet | MPN, Manufacturer | |
//! | g-trs | Value, Type, max(Vce), max(Ic), Description, Datasheet | MPN, Manufacturer | |
//!
//! All 21 files also have the 7 supplier data columns.
//!
//! A row whose details or CAD differ from what an edit saved is kept by an
//! import problem for that kind only (`csv-details-differ-from-edited-details`
//! or `csv-cad-differs-from-edited-revision`); so is CAD whose content matches
//! an archived revision (`csv-cad-matches-archived-revision`), and CAD that
//! uses a symbol, footprint or 3D model whose CSV file changed while its
//! current revision was made in the app
//! (`csv-cad-differs-from-edited-cad-item`, one per symbol or footprint):
//! an import never replaces that. Its other columns are checked as usual. A CSV review (someone taking some of the
//! CSV's values, or keeping theirs) does not change this: while the row
//! still differs, each import records the problem again, and the review only
//! stops the app from asking about the same CSV values twice.
//!
//! Excluded from BOM belongs to the revision but is no column: the report
//! checks that every stored part revision (from an import, an edit or a CSV
//! review) has it exactly when a symbol revision it binds says
//! `(in_bom no)`
//! in its stored bytes, or every footprint choice has `exclude_from_bom` in
//! its `(attr ...)`; for a row of the latest run, footprint choices that
//! disagree need a `footprint-choices-disagree-on-bom` import problem.

use catalog::csv_import::{
    CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM, CadItemSource, ColumnRoute, CsvRecord, EMPTY_3D_MODEL,
    SubstitutesCsvColumn, parse_csv, route_column, route_substitutes_csv_column,
};
use catalog::pocketbase::{
    CatalogReadback, ImportRun, StoredAuditEntry, StoredImportProblem, StoredPartDetails,
    StoredPartRevision,
};
use catalog::{
    CadItemDetails, CadItemId, CadItemRevisionId, CadReference, CadReferenceRole,
    CadReferenceTarget, License, ModelTarget, PartDetails, PartRevision, sha256_hex,
};
use csv_catalog_fixture::Manifest;
use kicad_integration::library_file::{
    SymbolLibrary, SymbolLookup, footprint_says_excluded_from_bom, symbol_entries_say_not_in_bom,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const PART_DETAILS: &str = "part-details";
pub const PART_REVISION: &str = "part-revision";
pub const MANUFACTURER_PARTS: &str = "part-details.manufacturer-parts";
pub const SUPPLIER_DATA: &str = "supplier-data";
pub const IMPORT_PROBLEM: &str = "import-problem";

/// The SHA-256 of an empty file.
const EMPTY_FILE_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The audit entry action of a CSV import run.
pub const CSV_IMPORT_ACTION: &str = "catalog.csv-import";

/// Import problems that keep a whole row of the CSV file that could not
/// become a part.
const ROW_PROBLEM_CODES: [&str; 3] = ["row-has-extra-fields", "invalid-ipn", "duplicate-ipn"];

/// Import problems that keep a whole substitutes.csv row that could not
/// become a manufacturer part.
const SUBSTITUTES_ROW_PROBLEM_CODES: [&str; 3] = [
    "incomplete-substitutes-csv-row",
    "duplicate-manufacturer-part",
    "substitutes-csv-row-has-unknown-columns",
];

#[derive(Clone, Debug, Serialize)]
pub struct ZeroLossReport {
    pub csv_file: String,
    pub source_path: String,
    pub oracle_byte_sha256: String,
    /// The latest import run of the CSV file, which the report checks.
    pub import_run: Option<ImportRunAccount>,
    /// The CSV's byte hash as that run recorded it.
    pub recorded_byte_sha256: String,
    /// The hash of the CSV rebuilt from PocketBase alone: the header line
    /// and every row's bytes the latest import run recorded.
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
    /// Parts set Excluded from BOM, because a symbol or footprint they bind
    /// is marked so in its stored bytes.
    pub excluded_from_bom: Vec<String>,
    /// Parts that earlier import runs made from this file but that it no
    /// longer has, each kept by an import problem.
    pub removed_parts: Vec<String>,
    pub substitutes_csv_rows: SubstitutesCsvAccount,
    pub cad_items: Vec<CadItemAccount>,
    pub model_references: Vec<ModelAccount>,
    pub oracle_import_problems: Vec<ProblemAccount>,
    /// Stored import problems that no oracle import problem or row explains.
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
pub struct ImportRunAccount {
    pub audit_entry: String,
    pub csv_import_number: u64,
    /// The Change it made, or empty if it made no revision.
    pub change: String,
    pub source_commit: String,
    pub allowed_older_checkout: bool,
    pub allowed_uncommitted_changes: bool,
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
pub struct SubstitutesCsvAccount {
    pub path: String,
    pub oracle_byte_sha256: String,
    /// The byte hash the latest import run recorded.
    pub recorded_byte_sha256: String,
    pub rows: usize,
    pub rows_in_scope: usize,
    pub fields_in_scope: usize,
    /// Fields of in-scope substitutes.csv rows, per destination.
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
    /// Each stored import problem's detail, parsed once.
    details: Vec<Value>,
    /// The IDs of the import problems the latest import run found. Problems
    /// are shared by content, so a problem an earlier run found explains a
    /// row only if the latest run found it again.
    run_problems: BTreeSet<String>,
    /// Problems that already explain a row; none may explain two.
    row_problems_used: BTreeSet<String>,
    problems: Vec<String>,
}

/// A part's details and revision as the latest import run left them,
/// decoded.
struct RunPart<'a> {
    revision: &'a StoredPartRevision,
    cad: PartRevision,
    details_record: &'a StoredPartDetails,
    details: PartDetails,
}

/// A CSV file rebuilt from what an import run recorded.
#[derive(Default)]
struct RebuiltCsv {
    recorded_byte_sha256: String,
    rebuilt_byte_sha256: String,
    /// The rebuilt file's rows, one per oracle row when it matches.
    records: Vec<CsvRecord>,
}

impl RebuiltCsv {
    /// Row `index`'s field `column`, or empty if the row is shorter.
    fn field(&self, index: usize, column: usize) -> String {
        self.records
            .get(index)
            .and_then(|record| record.fields.get(column))
            .cloned()
            .unwrap_or_default()
    }

    fn raw(&self, index: usize) -> &str {
        self.records
            .get(index)
            .map_or("", |record| record.raw.as_str())
    }
}

/// The per-field ledger of the CSV file's rows.
#[derive(Default)]
struct Fields {
    destinations: BTreeMap<String, usize>,
    column_routes: BTreeMap<String, String>,
    accounts: Vec<FieldAccount>,
}

impl Fields {
    fn account(
        &mut self,
        row_number: usize,
        ipn: &str,
        column: &str,
        destination: &str,
        oracle: &str,
        stored: Option<String>,
    ) {
        *self.destinations.entry(destination.to_owned()).or_default() += 1;
        self.column_routes
            .entry(column.to_owned())
            .or_insert_with(|| destination.to_owned());
        self.accounts.push(FieldAccount {
            row_number,
            ipn: ipn.to_owned(),
            column: column.to_owned(),
            destination: destination.to_owned(),
            oracle: oracle.to_owned(),
            stored,
        });
    }
}

/// Text fields of a JSON list, such as a problem detail's `record`.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| item.as_str().unwrap_or_default().to_owned())
        .collect()
}

fn padded(mut fields: Vec<String>, length: usize) -> Vec<String> {
    if fields.len() < length {
        fields.resize(length, String::new());
    }
    fields
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
            details: readback
                .problems
                .iter()
                .map(|problem| serde_json::from_str(&problem.detail).unwrap_or_default())
                .collect(),
            run_problems: BTreeSet::new(),
            row_problems_used: BTreeSet::new(),
            problems: Vec::new(),
        }
    }

    /// Whether the latest import run found `problem`.
    fn in_run(&self, problem: &StoredImportProblem) -> bool {
        self.run_problems.contains(&problem.id)
    }

    /// Marks `problem` as keeping one row; reports a loss if it already
    /// keeps another.
    fn use_for_row(&mut self, problem: &StoredImportProblem) {
        if !self.row_problems_used.insert(problem.id.clone()) {
            self.problem(format!(
                "import problem {} at {} is counted for more than one row",
                problem.code, problem.location
            ));
        }
    }

    fn problem(&mut self, message: String) {
        self.problems.push(message);
    }

    /// The first import problem the latest run found, with its parsed
    /// detail, that `matches`.
    fn find_problem(
        &self,
        matches: impl Fn(&StoredImportProblem, &Value) -> bool,
    ) -> Option<(&'a StoredImportProblem, Value)> {
        let readback = self.readback;
        readback
            .problems
            .iter()
            .zip(&self.details)
            .filter(|(problem, _)| self.in_run(problem))
            .find(|(problem, detail)| matches(problem, detail))
            .map(|(problem, detail)| (problem, detail.clone()))
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
            return self.empty(Vec::new());
        };
        let document = &category.document;
        let source_path = document.path.clone();
        let headers = &document.headers;
        let ipn_column = headers.iter().position(|header| header == "IPN");

        let changes = self.changes();
        self.check_import_runs_record_their_files();
        self.check_every_revision_excluded_from_bom();
        self.check_current_revisions();
        let Some((entry, run)) = self.latest_import_run() else {
            self.problem(format!(
                "no import run of {source_path} is recorded in an audit entry"
            ));
            return self.empty(changes);
        };
        self.run_problems = run.problems.iter().cloned().collect();
        let import_run = ImportRunAccount {
            audit_entry: entry.id.clone(),
            csv_import_number: entry.csv_import_number,
            change: entry.change.clone(),
            source_commit: entry.source_commit.clone(),
            allowed_older_checkout: entry.allowed_older_checkout,
            allowed_uncommitted_changes: entry.allowed_uncommitted_changes,
        };
        let csv = self.rebuild_csv(&run, &source_path, &document.byte_sha256, &document.rows);
        let run_parts: BTreeMap<&str, (u32, u32)> = run
            .parts
            .iter()
            .map(|part| (part.ipn.as_str(), (part.revision, part.details)))
            .collect();

        let mut fields = Fields::default();
        let mut explained = BTreeSet::new();
        // Parts whose row the latest run made, and the details and revision
        // it left them at, unless an import problem keeps that kind of the
        // row instead.
        let mut details_scope: BTreeMap<String, PartDetails> = BTreeMap::new();
        let mut cad_scope: BTreeMap<String, PartRevision> = BTreeMap::new();
        let mut kept_by_problem: BTreeMap<String, Value> = BTreeMap::new();
        let mut part_rows = BTreeSet::new();
        let mut rows_with_main_part: BTreeMap<String, bool> = BTreeMap::new();

        for (index, row) in document.rows.iter().enumerate() {
            let row_number = index + 1;
            let raw = csv.raw(index).to_owned();
            let ipn = ipn_column
                .and_then(|column| row.get(column))
                .cloned()
                .unwrap_or_default();

            // A row that could not become a part.
            if let Some((problem, detail)) = self.find_problem(|problem, detail| {
                ROW_PROBLEM_CODES.contains(&problem.code.as_str())
                    && problem.path == source_path
                    && detail["row_number"].as_u64() == Some(row_number as u64)
                    && detail["raw_record"].as_str() == Some(raw.as_str())
            }) {
                explained.insert(problem.id.clone());
                self.use_for_row(problem);
                let record = padded(strings(&detail["record"]), headers.len());
                for (position, (column, value)) in headers.iter().zip(row).enumerate() {
                    let stored = record.get(position).cloned();
                    if stored.as_deref() != Some(value.as_str()) {
                        self.problem(format!(
                            "row {row_number} field {column}: oracle {value:?}, problem kept {stored:?}"
                        ));
                    }
                    fields.account(row_number, &ipn, column, IMPORT_PROBLEM, value, stored);
                }
                continue;
            }

            let revision = match run_parts.get(ipn.as_str()) {
                Some(&(number, details)) => self.run_part(&ipn, number, details),
                None => {
                    self.problem(format!(
                        "row {row_number} ({ipn}) is neither a part of the latest import run nor an import problem"
                    ));
                    None
                }
            };
            let Some(revision) = revision else {
                for (column, value) in headers.iter().zip(row) {
                    fields.account(row_number, &ipn, column, "lost", value, None);
                }
                continue;
            };
            part_rows.insert(ipn.clone());

            // Details or CAD that differ from what an edit saved are kept by
            // an import problem instead, each kind on its own.
            let details_kept = self
                .find_problem(|problem, detail| {
                    problem.code == "csv-details-differ-from-edited-details"
                        && problem.subject == ipn
                        && problem.path == source_path
                        && detail["raw_record"].as_str() == Some(raw.as_str())
                        && detail["current_details_number"].as_u64()
                            == Some(u64::from(revision.details_record.number))
                })
                .map(|(problem, detail)| (problem.clone(), detail.clone()));
            let cad_kept = self
                .find_problem(|problem, detail| {
                    (problem.code == "csv-cad-differs-from-edited-revision"
                        || problem.code == "csv-cad-matches-archived-revision")
                        && problem.subject == ipn
                        && problem.path == source_path
                        && detail["raw_record"].as_str() == Some(raw.as_str())
                        && detail["current_revision_number"].as_u64()
                            == Some(u64::from(revision.revision.number))
                })
                .map(|(problem, detail)| (problem.clone(), detail.clone()));
            // A symbol, footprint or 3D model the row uses whose CSV file
            // changed while its current revision was made in the app: the
            // import kept the CAD item as it is (kicad-parts-3b8b). One
            // problem per symbol or footprint.
            let cad_items_kept: Vec<StoredImportProblem> = readback
                .problems
                .iter()
                .zip(&self.details)
                .filter(|(problem, detail)| {
                    self.in_run(problem)
                        && problem.code == CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM
                        && problem.subject == ipn
                        && problem.path == source_path
                        && detail["raw_record"].as_str() == Some(raw.as_str())
                })
                .map(|(problem, _)| problem.clone())
                .collect();
            for problem in details_kept
                .iter()
                .chain(&cad_kept)
                .map(|(problem, _)| problem)
                .chain(&cad_items_kept)
            {
                explained.insert(problem.id.clone());
                self.use_for_row(problem);
            }
            if let Some((_, detail)) = &details_kept {
                kept_by_problem.insert(ipn.clone(), detail.clone());
            }
            let cad_kept = cad_kept.is_some() || !cad_items_kept.is_empty();
            let kept = |route: ColumnRoute| match route {
                ColumnRoute::Cad => cad_kept,
                ColumnRoute::SupplierData => false,
                _ => details_kept.is_some(),
            };

            let details = &revision.details;
            let cad = &revision.cad;
            let has_main_columns = headers.iter().any(|header| {
                matches!(
                    route_column(header),
                    ColumnRoute::MainMpn | ColumnRoute::MainManufacturer
                )
            });
            // The row makes the main manufacturer part only when its MPN and
            // Manufacturer are both there and not blank; otherwise an import
            // problem keeps exactly these values and the first manufacturer
            // part comes from substitutes.csv.
            let main_values: Vec<(&String, &String)> = headers
                .iter()
                .zip(row)
                .filter(|(header, _)| {
                    matches!(
                        route_column(header),
                        ColumnRoute::MainMpn | ColumnRoute::MainManufacturer
                    )
                })
                .collect();
            let row_made_main =
                main_values.len() == 2 && main_values.iter().all(|(_, value)| !value.is_empty());
            let incomplete = if has_main_columns && !row_made_main {
                self.find_problem(|problem, detail| {
                    problem.code == "incomplete-main-manufacturer-part"
                        && problem.subject == ipn
                        && main_values
                            .iter()
                            .all(|(column, value)| detail[column.as_str()].as_str() == Some(value))
                })
            } else {
                None
            };
            rows_with_main_part.insert(ipn.clone(), row_made_main);
            let main = details.manufacturer_parts.first().filter(|_| row_made_main);

            let mut part_detail_columns = Vec::new();
            for (position, (column, value)) in headers.iter().zip(row).enumerate() {
                let route = route_column(column);
                if kept(route) {
                    let stored = Some(csv.field(index, position));
                    fields.account(row_number, &ipn, column, IMPORT_PROBLEM, value, stored);
                    continue;
                }
                let (destination, stored) = match route {
                    ColumnRoute::Ipn => (PART_DETAILS, Some(details.ipn.clone())),
                    ColumnRoute::Cad => (PART_REVISION, cad.field(column).map(str::to_owned)),
                    ColumnRoute::PartDetails => {
                        part_detail_columns.push(column.clone());
                        (PART_DETAILS, details.field(column).map(str::to_owned))
                    }
                    ColumnRoute::MainMpn | ColumnRoute::MainManufacturer => {
                        match (main, &incomplete) {
                            (Some(main), _) => (
                                MANUFACTURER_PARTS,
                                Some(if route == ColumnRoute::MainMpn {
                                    main.mpn.clone()
                                } else {
                                    main.manufacturer.clone()
                                }),
                            ),
                            (None, Some((problem, detail))) => {
                                explained.insert(problem.id.clone());
                                (
                                    IMPORT_PROBLEM,
                                    detail[column.as_str()].as_str().map(str::to_owned),
                                )
                            }
                            (None, None) => (MANUFACTURER_PARTS, None),
                        }
                    }
                    // An extra MPN without a known manufacturer, or MPNs
                    // still to be confirmed: an import problem keeps the
                    // value. A blank cell needs no problem.
                    ColumnRoute::ExtraMpn
                    | ColumnRoute::MpnToBeConfirmed
                    | ColumnRoute::UnknownMpn
                        if value.is_empty() =>
                    {
                        (IMPORT_PROBLEM, Some(String::new()))
                    }
                    ColumnRoute::ExtraMpn
                    | ColumnRoute::MpnToBeConfirmed
                    | ColumnRoute::UnknownMpn => {
                        let (code, key) = match route {
                            ColumnRoute::ExtraMpn => ("mpn-without-manufacturer", "mpn"),
                            ColumnRoute::MpnToBeConfirmed => ("mpn-to-be-confirmed", "value"),
                            _ => ("unknown-mpn-column", "value"),
                        };
                        let kept = self.find_problem(|problem, detail| {
                            problem.code == code
                                && problem.subject == ipn
                                && problem.path == source_path
                                && detail["column"].as_str() == Some(column.as_str())
                        });
                        let stored = kept.and_then(|(problem, detail)| {
                            explained.insert(problem.id.clone());
                            detail[key].as_str().map(str::to_owned)
                        });
                        (IMPORT_PROBLEM, stored)
                    }
                    // Supplier data never makes a revision; the latest run's
                    // row keeps it.
                    ColumnRoute::SupplierData => (SUPPLIER_DATA, Some(csv.field(index, position))),
                };
                if stored.as_deref() != Some(value.as_str()) {
                    self.problem(format!(
                        "row {row_number} ({ipn}) field {column}: oracle {value:?}, stored {stored:?} in {destination}"
                    ));
                }
                fields.account(row_number, &ipn, column, destination, value, stored);
            }
            // Exactly one place: no extra or repeated part-details or CAD
            // fields, and no supplier data in either.
            let stored_fields: Vec<&String> =
                details.fields.iter().map(|field| &field.name).collect();
            if details_kept.is_none()
                && stored_fields != part_detail_columns.iter().collect::<Vec<_>>()
            {
                self.problem(format!(
                    "row {row_number} ({ipn}): part-details fields {stored_fields:?} are not the part-details columns {part_detail_columns:?}"
                ));
            }
            let cad_columns: Vec<&String> = headers
                .iter()
                .filter(|header| route_column(header) == ColumnRoute::Cad)
                .collect();
            let stored_cad: Vec<&String> = cad.cad_fields.iter().map(|field| &field.name).collect();
            if !cad_kept && stored_cad != cad_columns {
                self.problem(format!(
                    "row {row_number} ({ipn}): part-revision fields {stored_cad:?} are not the CAD columns {cad_columns:?}"
                ));
            }
            // A supplier part number may equal a value another column puts
            // in the part details (a Mouser number can be the MPN itself);
            // only a value no such column has would have leaked.
            let detail_values: BTreeSet<&String> = headers
                .iter()
                .zip(row)
                .filter(|(column, _)| route_column(column) != ColumnRoute::SupplierData)
                .map(|(_, value)| value)
                .collect();
            for (column, value) in headers.iter().zip(row) {
                if route_column(column) == ColumnRoute::SupplierData
                    && !value.is_empty()
                    && !detail_values.contains(value)
                    && (revision.revision.snapshot.contains(&format!("\"{value}\""))
                        || revision
                            .details_record
                            .snapshot
                            .contains(&format!("\"{value}\"")))
                {
                    self.problem(format!(
                        "row {row_number} ({ipn}): supplier data {column}={value} also appears in the part details"
                    ));
                }
            }
            let cad_cell = |name: &str| {
                headers
                    .iter()
                    .zip(row)
                    .find(|(column, _)| column.as_str() == name)
                    .map(|(_, value)| value.as_str())
            };
            self.check_cad_lists(
                &ipn,
                cad_cell("Symbol"),
                cad_cell("Footprint"),
                &mut explained,
            );
            if !cad_kept {
                self.check_excluded_from_bom(row_number, &ipn, cad, &mut explained);
                cad_scope.insert(ipn.clone(), revision.cad.clone());
            }
            if details_kept.is_none() {
                details_scope.insert(ipn.clone(), revision.details.clone());
            }
        }

        for part in &run.parts {
            if !part_rows.contains(&part.ipn) {
                self.problem(format!(
                    "{} is a part of the latest import run, but no row of {source_path} makes it",
                    part.ipn
                ));
            }
        }
        let removed_parts = self.removed_parts(&source_path, &part_rows, &mut explained);
        if csv.rebuilt_byte_sha256 != document.byte_sha256 {
            self.problem(format!(
                "{source_path} rebuilt from PocketBase hashes to {}, not the oracle's {}",
                csv.rebuilt_byte_sha256, document.byte_sha256
            ));
        }

        let in_scope: BTreeMap<String, &PartDetails> = details_scope
            .iter()
            .map(|(ipn, details)| (ipn.clone(), details))
            .collect();
        let substitutes_csv_rows = self.substitutes_csv_rows(
            &run,
            &in_scope,
            &kept_by_problem,
            &rows_with_main_part,
            &mut explained,
        );
        let cad_in_scope: BTreeMap<String, &PartRevision> = cad_scope
            .iter()
            .map(|(ipn, revision)| (ipn.clone(), revision))
            .collect();
        let (cad_items, footprint_paths) = self.cad_items(&cad_in_scope, &mut explained);
        let model_references = self.models(&footprint_paths, &mut explained);
        let oracle_import_problems =
            self.oracle_import_problems(&footprint_paths, &cad_items, &mut explained);
        // Every import problem belongs to the audit entry of the run that
        // first found it.
        for problem in &readback.problems {
            if !readback
                .audit_entries
                .iter()
                .any(|entry| entry.id == problem.audit_entry)
            {
                self.problem(format!(
                    "import problem {} at {} has no audit entry",
                    problem.code, problem.location
                ));
            }
        }
        let other_problems = readback
            .problems
            .iter()
            .filter(|problem| !explained.contains(&problem.id))
            .map(|problem| format!("{} {} {}", problem.code, problem.path, problem.location))
            .collect();

        let field_count = fields.accounts.len();
        if field_count != document.rows.len() * headers.len() {
            self.problem(format!(
                "{field_count} fields accounted for, but the oracle has {}",
                document.rows.len() * headers.len()
            ));
        }
        ZeroLossReport {
            csv_file: self.csv_file.clone(),
            source_path,
            oracle_byte_sha256: document.byte_sha256.clone(),
            import_run: Some(import_run),
            recorded_byte_sha256: csv.recorded_byte_sha256,
            rebuilt_byte_sha256: csv.rebuilt_byte_sha256,
            rows: document.rows.len(),
            columns: headers.len(),
            fields: field_count,
            destinations: fields.destinations,
            column_routes: headers
                .iter()
                .map(|header| {
                    (
                        header.clone(),
                        fields
                            .column_routes
                            .get(header)
                            .cloned()
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            parts: part_rows.len(),
            excluded_from_bom: cad_scope
                .iter()
                .filter(|(_, revision)| revision.excluded_from_bom)
                .map(|(ipn, _)| ipn.clone())
                .collect(),
            removed_parts,
            substitutes_csv_rows,
            cad_items,
            model_references,
            oracle_import_problems,
            other_problems,
            changes,
            field_accounts: fields.accounts,
            problems: self.problems,
        }
    }

    fn empty(self, changes: Vec<ChangeAccount>) -> ZeroLossReport {
        ZeroLossReport {
            csv_file: self.csv_file,
            source_path: String::new(),
            oracle_byte_sha256: String::new(),
            import_run: None,
            recorded_byte_sha256: String::new(),
            rebuilt_byte_sha256: String::new(),
            rows: 0,
            columns: 0,
            fields: 0,
            destinations: BTreeMap::new(),
            column_routes: vec![],
            parts: 0,
            excluded_from_bom: vec![],
            removed_parts: vec![],
            substitutes_csv_rows: SubstitutesCsvAccount::default(),
            cad_items: vec![],
            model_references: vec![],
            oracle_import_problems: vec![],
            other_problems: vec![],
            changes,
            field_accounts: vec![],
            problems: self.problems,
        }
    }

    /// Checks every Change: one audit entry, at least one revision, saved
    /// part details, archive entry or CSV review, and a reason.
    fn changes(&mut self) -> Vec<ChangeAccount> {
        let readback = self.readback;
        let mut changes = Vec::new();
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
            let revisions = readback
                .part_revisions
                .iter()
                .filter(|revision| revision.change == change.id)
                .count()
                + readback
                    .cad_item_revisions
                    .iter()
                    .filter(|revision| revision.change == change.id)
                    .count()
                + readback
                    .part_details
                    .iter()
                    .filter(|details| details.change == change.id)
                    .count()
                + readback
                    .archive_entries
                    .iter()
                    .filter(|entry| entry.change == change.id)
                    .count()
                + readback
                    .csv_reviews
                    .iter()
                    .filter(|review| review.change == change.id)
                    .count();
            if revisions == 0 {
                self.problem(format!(
                    "Change {} has no revisions, no part details, no archive entry and no CSV review",
                    change.seq
                ));
            }
            if change.reason.trim().is_empty() {
                self.problem(format!("Change {} has no reason", change.seq));
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
        changes
    }

    /// A row whose Symbol names several symbols, or whose Footprint lists
    /// a choice more than once, needs a several-symbols or
    /// repeated-footprint-choice import problem (the row still imports
    /// with its CAD as the CSV says).
    fn check_cad_lists(
        &mut self,
        ipn: &str,
        symbol: Option<&str>,
        footprint: Option<&str>,
        explained: &mut BTreeSet<String>,
    ) {
        let named = |cell: Option<&str>| -> Vec<String> {
            cell.unwrap_or_default()
                .split(';')
                .map(str::trim)
                .filter(|reference| !reference.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let symbols = named(symbol);
        let footprints = named(footprint);
        let repeated = footprints
            .iter()
            .enumerate()
            .any(|(index, reference)| footprints[..index].contains(reference));
        for (code, column, needed) in [
            ("several-symbols", "Symbol", symbols.len() > 1),
            ("repeated-footprint-choice", "Footprint", repeated),
        ] {
            if !needed {
                continue;
            }
            let location = format!("rows[IPN={ipn}].{column}");
            let found = self
                .find_problem(|problem, _| {
                    problem.code == code && problem.subject == ipn && problem.location == location
                })
                .map(|(problem, _)| problem.id.clone());
            match found {
                Some(id) => {
                    explained.insert(id);
                }
                None => self.problem(format!("no import problem {code} for {ipn}")),
            }
        }
    }

    /// A part is Excluded from BOM exactly when a symbol revision it binds
    /// says `(in_bom no)` in its stored bytes, or every footprint choice is a
    /// footprint revision whose `(attr ...)` has `exclude_from_bom`. If only
    /// some footprint choices say so, the setting must be unset and the
    /// latest run must have found a `footprint-choices-disagree-on-bom`
    /// import problem.
    fn check_excluded_from_bom(
        &mut self,
        row_number: usize,
        ipn: &str,
        revision: &PartRevision,
        explained: &mut BTreeSet<String>,
    ) {
        let (symbol_marked, footprints) = bom_marks(self.readback, revision);
        let all_footprints = !footprints.is_empty() && !footprints.contains(&false);
        let expected = symbol_marked || all_footprints;
        if expected != revision.excluded_from_bom {
            self.problem(format!(
                "row {row_number} ({ipn}): Excluded from BOM is {}, but its symbols and footprints say {expected}",
                revision.excluded_from_bom
            ));
        }
        if !symbol_marked && footprints.contains(&true) && !all_footprints {
            match self.find_problem(|problem, _| {
                problem.code == "footprint-choices-disagree-on-bom" && problem.subject == ipn
            }) {
                Some((problem, _)) => {
                    explained.insert(problem.id.clone());
                }
                None => self.problem(format!(
                    "row {row_number} ({ipn}): only some footprint choices are excluded from the BOM, and no import problem says so"
                )),
            }
        }
    }

    /// Every stored part revision, whoever added it, is Excluded from BOM
    /// exactly when the CAD it binds says so (kicad-parts-ccc0).
    fn check_every_revision_excluded_from_bom(&mut self) {
        let readback = self.readback;
        for stored in &readback.part_revisions {
            let Ok(revision) = serde_json::from_str::<PartRevision>(&stored.snapshot) else {
                let ipn = readback
                    .parts
                    .iter()
                    .find(|part| part.id == stored.part)
                    .map_or(stored.part.as_str(), |part| part.ipn.as_str());
                self.problem(format!(
                    "{ipn} revision {}: its stored snapshot is not a part revision, so its Excluded from BOM cannot be checked",
                    stored.number
                ));
                continue;
            };
            let (symbol_marked, footprints) = bom_marks(readback, &revision);
            let expected =
                symbol_marked || (!footprints.is_empty() && !footprints.contains(&false));
            if expected != revision.excluded_from_bom {
                self.problem(format!(
                    "{} revision {}: Excluded from BOM is {}, but its symbols and footprints say {expected}",
                    revision.ipn, stored.number, revision.excluded_from_bom
                ));
            }
        }
    }

    /// Every CSV import run, including one that made no Change, records the
    /// files it read with their byte hashes.
    fn check_import_runs_record_their_files(&mut self) {
        let readback = self.readback;
        for entry in &readback.audit_entries {
            if entry.action != CSV_IMPORT_ACTION {
                continue;
            }
            let Some(run) = entry
                .import_run()
                .filter(|run| !run.files.is_empty() && run.csv_file == entry.csv_file)
            else {
                self.problem(format!(
                    "the audit entry of import run {} does not record the files it read",
                    entry.correlation_id
                ));
                continue;
            };
            if entry.csv_import_number == 0 {
                self.problem(format!(
                    "the audit entry of import run {} has no import number",
                    entry.correlation_id
                ));
            }
            // The short list importers read must name the same files.
            let mut files: Vec<(String, String)> = run
                .files
                .iter()
                .map(|file| (file.path.clone(), file.byte_sha256.clone()))
                .collect();
            files.sort();
            let listed: Option<Vec<(String, String)>> =
                serde_json::from_str(&entry.file_hashes).ok();
            if listed.as_ref() != Some(&files) {
                self.problem(format!(
                    "the file hashes of import run {} do not match the files it records",
                    entry.correlation_id
                ));
            }
        }
    }

    /// The current revision and current details of every part are its own,
    /// match their hashes and hold that part's revision and details.
    fn check_current_revisions(&mut self) {
        let readback = self.readback;
        for part in &readback.parts {
            match readback.part_details(&part.current_details) {
                None => self.problem(format!(
                    "{}: current details {} are missing",
                    part.ipn, part.current_details
                )),
                Some(details) => {
                    if sha256_hex(&details.snapshot) != details.snapshot_sha256 {
                        self.problem(format!("{}: details do not match their hash", part.ipn));
                    }
                    if details.number != part.current_details_number || details.part != part.id {
                        self.problem(format!(
                            "{}: current details pointer is inconsistent",
                            part.ipn
                        ));
                    }
                    match serde_json::from_str::<PartDetails>(&details.snapshot) {
                        Ok(parsed) if parsed.ipn == part.ipn => {}
                        Ok(parsed) => {
                            self.problem(format!("{}: details name {}", part.ipn, parsed.ipn))
                        }
                        Err(error) => {
                            self.problem(format!("{}: details do not parse: {error}", part.ipn))
                        }
                    }
                }
            }
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
            // The current revision is the newest one that is not archived.
            let archived = readback.archived(&part.id);
            let newest = readback
                .part_revisions
                .iter()
                .filter(|other| other.part == part.id && !archived.contains(&other.number))
                .map(|other| other.number)
                .max();
            if newest != Some(revision.number) {
                self.problem(format!(
                    "{}: the current revision is {}, not the newest revision that is not archived ({newest:?})",
                    part.ipn, revision.number
                ));
            }
            match serde_json::from_str::<PartRevision>(&revision.snapshot) {
                Ok(details) if details.ipn == part.ipn => {
                    // The relation lists exactly the CAD item revisions the
                    // revision names, by CAD item and number.
                    let named: Vec<String> = details
                        .cad_references
                        .iter()
                        .filter_map(|reference| match &reference.target {
                            CadReferenceTarget::CadItemRevision {
                                cad_item, number, ..
                            } => Some(
                                CadItemRevisionId::new(&CadItemId::from_stored(cad_item), *number)
                                    .to_string(),
                            ),
                            _ => None,
                        })
                        .collect();
                    let (mut named, mut related) = (named, revision.cad_item_revisions.clone());
                    named.sort();
                    named.dedup();
                    related.sort();
                    related.dedup();
                    if named != related {
                        self.problem(format!(
                            "{}: the current revision's CAD item revisions are not the ones it names",
                            part.ipn
                        ));
                    }
                }
                Ok(details) => {
                    self.problem(format!("{}: snapshot names {}", part.ipn, details.ipn));
                }
                Err(error) => {
                    self.problem(format!("{}: snapshot does not parse: {error}", part.ipn));
                }
            }
        }
    }

    /// The latest import run of this CSV file and what it recorded.
    fn latest_import_run(&self) -> Option<(&'a StoredAuditEntry, ImportRun)> {
        let readback = self.readback;
        readback
            .audit_entries
            .iter()
            .filter(|entry| {
                entry.action == CSV_IMPORT_ACTION
                    && entry.csv_file == self.csv_file
                    && entry.csv_import_number > 0
            })
            .max_by_key(|entry| entry.csv_import_number)
            .and_then(|entry| Some((entry, entry.import_run()?)))
    }

    /// Part `ipn`'s revision `number` and details `details_number`, checked
    /// against their hashes.
    fn run_part(&mut self, ipn: &str, number: u32, details_number: u32) -> Option<RunPart<'a>> {
        let readback = self.readback;
        let Some(part) = readback.part(ipn) else {
            self.problem(format!("{ipn} is in the latest import run but not a part"));
            return None;
        };
        let Some(revision) = readback
            .part_revisions
            .iter()
            .find(|revision| revision.part == part.id && revision.number == number)
        else {
            self.problem(format!(
                "{ipn}: revision {number}, which the latest import run left it at, is missing"
            ));
            return None;
        };
        let Some(details_record) = readback
            .part_details
            .iter()
            .find(|details| details.part == part.id && details.number == details_number)
        else {
            self.problem(format!(
                "{ipn}: details {details_number}, which the latest import run left it at, are missing"
            ));
            return None;
        };
        if sha256_hex(&revision.snapshot) != revision.snapshot_sha256 {
            self.problem(format!("{ipn}: revision {number} does not match its hash"));
        }
        if sha256_hex(&details_record.snapshot) != details_record.snapshot_sha256 {
            self.problem(format!(
                "{ipn}: details {details_number} do not match their hash"
            ));
        }
        let cad = match serde_json::from_str::<PartRevision>(&revision.snapshot) {
            Ok(cad) if cad.ipn == ipn => cad,
            Ok(cad) => {
                self.problem(format!("{ipn}: revision {number} names {}", cad.ipn));
                return None;
            }
            Err(error) => {
                self.problem(format!("{ipn}: revision {number} does not parse: {error}"));
                return None;
            }
        };
        let details = match serde_json::from_str::<PartDetails>(&details_record.snapshot) {
            Ok(details) if details.ipn == ipn => details,
            Ok(details) => {
                self.problem(format!(
                    "{ipn}: details {details_number} name {}",
                    details.ipn
                ));
                return None;
            }
            Err(error) => {
                self.problem(format!(
                    "{ipn}: details {details_number} do not parse: {error}"
                ));
                return None;
            }
        };
        Some(RunPart {
            revision,
            cad,
            details_record,
            details,
        })
    }

    /// Rebuilds `path` from the header line and rows `run` recorded, and
    /// checks it against the oracle's byte hash and rows.
    fn rebuild_csv(
        &mut self,
        run: &ImportRun,
        path: &str,
        oracle_byte_sha256: &str,
        oracle_rows: &[Vec<String>],
    ) -> RebuiltCsv {
        let Some(file) = run.file(path) else {
            self.problem(format!("the latest import run did not record {path}"));
            return RebuiltCsv::default();
        };
        if file.byte_sha256 != oracle_byte_sha256 {
            self.problem(format!(
                "the latest import run read {path} with byte hash {}, not the oracle's {oracle_byte_sha256}",
                file.byte_sha256
            ));
        }
        let (Some(header), Some(rows)) = (&file.header_raw, &file.rows) else {
            self.problem(format!(
                "the latest import run did not keep the header line and rows of {path}"
            ));
            return RebuiltCsv {
                recorded_byte_sha256: file.byte_sha256.clone(),
                ..RebuiltCsv::default()
            };
        };
        let text: String = std::iter::once(header.as_str())
            .chain(rows.iter().map(String::as_str))
            .collect();
        let rebuilt_byte_sha256 = sha256_hex(text.as_bytes());
        let records = match parse_csv(path, text.as_bytes()) {
            Ok(table) => table.records,
            Err(error) => {
                self.problem(format!(
                    "{path} rebuilt from PocketBase does not parse: {error}"
                ));
                Vec::new()
            }
        };
        if rows.len() != records.len() {
            self.problem(format!(
                "the latest import run recorded {} rows of {path}, but the file it rebuilds has {}",
                rows.len(),
                records.len()
            ));
        }
        if records.len() != oracle_rows.len() {
            self.problem(format!(
                "{path} rebuilt from PocketBase has {} rows, the oracle {}",
                records.len(),
                oracle_rows.len()
            ));
        }
        for (index, (record, oracle_row)) in records.iter().zip(oracle_rows).enumerate() {
            if &padded(record.fields.clone(), oracle_row.len()) != oracle_row {
                self.problem(format!(
                    "row {} of {path} rebuilt from PocketBase is not the oracle's row",
                    index + 1
                ));
            }
            if rows.get(index) != Some(&record.raw) {
                self.problem(format!(
                    "row {} of {path}: the bytes the import run recorded are not one whole row",
                    index + 1
                ));
            }
        }
        RebuiltCsv {
            recorded_byte_sha256: file.byte_sha256.clone(),
            rebuilt_byte_sha256,
            records,
        }
    }

    /// Parts that an earlier import run made from `path`, but that no row of
    /// it makes now. Each must be kept by a `part-not-in-csv-file` import
    /// problem; nothing is ever deleted.
    fn removed_parts(
        &mut self,
        path: &str,
        part_rows: &BTreeSet<String>,
        explained: &mut BTreeSet<String>,
    ) -> Vec<String> {
        let readback = self.readback;
        let earlier: BTreeSet<String> = readback
            .audit_entries
            .iter()
            .filter(|entry| entry.action == CSV_IMPORT_ACTION && entry.csv_file == self.csv_file)
            .filter_map(StoredAuditEntry::import_run)
            .flat_map(|run| run.parts.into_iter().map(|part| part.ipn))
            .filter(|ipn| !part_rows.contains(ipn))
            .collect();
        let mut removed = Vec::new();
        for ipn in earlier {
            let kept = readback.problems.iter().find(|problem| {
                self.in_run(problem)
                    && problem.code == "part-not-in-csv-file"
                    && problem.subject == ipn
                    && problem.path == path
            });
            match kept {
                Some(problem) => {
                    explained.insert(problem.id.clone());
                    removed.push(ipn);
                }
                None => self.problem(format!(
                    "{ipn} was a part of {path} at an earlier import, is not in it now, and no import problem says so"
                )),
            }
        }
        removed
    }

    fn substitutes_csv_rows(
        &mut self,
        run: &ImportRun,
        scope: &BTreeMap<String, &PartDetails>,
        kept_by_problem: &BTreeMap<String, Value>,
        rows_with_main_part: &BTreeMap<String, bool>,
        explained: &mut BTreeSet<String>,
    ) -> SubstitutesCsvAccount {
        let oracle = self.oracle;
        let document = &oracle.substitutes;
        let csv = self.rebuild_csv(run, &document.path, &document.byte_sha256, &document.rows);
        if csv.rebuilt_byte_sha256 != document.byte_sha256 {
            self.problem(format!(
                "{} rebuilt from PocketBase hashes to {}, not the oracle's {}",
                document.path, csv.rebuilt_byte_sha256, document.byte_sha256
            ));
        }
        let ipn_column = document.headers.iter().position(|header| header == "IPN");
        let mut account = SubstitutesCsvAccount {
            path: document.path.clone(),
            oracle_byte_sha256: document.byte_sha256.clone(),
            recorded_byte_sha256: csv.recorded_byte_sha256.clone(),
            rows: document.rows.len(),
            ..SubstitutesCsvAccount::default()
        };
        // How many of each part's substitutes.csv rows became manufacturer
        // parts so far, in file order.
        let mut accepted: BTreeMap<String, usize> = BTreeMap::new();
        for (index, row) in document.rows.iter().enumerate() {
            let row_number = index + 1;
            let raw = csv.raw(index).to_owned();
            let ipn = ipn_column
                .and_then(|column| row.get(column))
                .cloned()
                .unwrap_or_default();
            if !scope.contains_key(&ipn) && !kept_by_problem.contains_key(&ipn) {
                continue;
            }
            account.rows_in_scope += 1;
            account.fields_in_scope += row.len();
            let location = format!("rows[{row_number}]");

            // A row that could not become a manufacturer part is kept whole
            // by an import problem.
            if let Some((problem, detail)) = self.find_problem(|problem, detail| {
                SUBSTITUTES_ROW_PROBLEM_CODES.contains(&problem.code.as_str())
                    && problem.path == document.path
                    && problem.location == location
                    && detail["raw_record"].as_str() == Some(raw.as_str())
            }) {
                if &padded(strings(&detail["record"]), row.len()) != row {
                    self.problem(format!(
                        "substitutes.csv row {row_number} of {ipn}: its import problem does not hold the oracle row"
                    ));
                }
                explained.insert(problem.id.clone());
                self.use_for_row(problem);
                *account
                    .destinations
                    .entry(IMPORT_PROBLEM.to_owned())
                    .or_default() += row.len();
                continue;
            }
            // The part's own row is kept by an import problem, together with
            // its substitutes.csv rows.
            if let Some(detail) = kept_by_problem.get(&ipn) {
                let kept = detail["substitutes_csv_rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|kept| kept.as_str() == Some(raw.as_str()));
                if kept {
                    *account
                        .destinations
                        .entry(IMPORT_PROBLEM.to_owned())
                        .or_default() += row.len();
                } else {
                    self.problem(format!(
                        "substitutes.csv row {row_number} of {ipn} is neither in the import problem that keeps its part's row nor an import problem itself"
                    ));
                }
                continue;
            }

            let details = scope[&ipn];
            // Entries come after the main manufacturer part only if the row
            // made one.
            let offset = usize::from(rows_with_main_part.get(&ipn).copied().unwrap_or(false));
            let position = accepted.entry(ipn.clone()).or_default();
            let Some(manufacturer_part) = details.manufacturer_parts.get(offset + *position) else {
                self.problem(format!(
                    "substitutes.csv row {row_number} of {ipn} is neither a manufacturer part nor an import problem"
                ));
                continue;
            };
            *position += 1;
            let part_datasheet = details.field("Datasheet").unwrap_or_default();
            for (column_index, (column, value)) in document.headers.iter().zip(row).enumerate() {
                let (destination, stored) = match route_substitutes_csv_column(column) {
                    SubstitutesCsvColumn::Ipn => (MANUFACTURER_PARTS, Some(ipn.clone())),
                    SubstitutesCsvColumn::Mpn => {
                        (MANUFACTURER_PARTS, Some(manufacturer_part.mpn.clone()))
                    }
                    SubstitutesCsvColumn::Manufacturer => (
                        MANUFACTURER_PARTS,
                        Some(manufacturer_part.manufacturer.clone()),
                    ),
                    // A part has one datasheet. A blank one adds nothing and
                    // the part's own is already in its part details; any
                    // other must be an import problem.
                    SubstitutesCsvColumn::Datasheet if value.is_empty() => {
                        (PART_DETAILS, Some(String::new()))
                    }
                    SubstitutesCsvColumn::Datasheet if value == part_datasheet => {
                        (PART_DETAILS, Some(part_datasheet.to_owned()))
                    }
                    SubstitutesCsvColumn::Datasheet => {
                        let kept = self.find_problem(|problem, _| {
                            problem.code == "substitutes-csv-datasheet-differs"
                                && problem.subject == ipn
                                && problem.location == location
                        });
                        let stored = kept.and_then(|(problem, detail)| {
                            explained.insert(problem.id.clone());
                            detail["datasheet"].as_str().map(str::to_owned)
                        });
                        (IMPORT_PROBLEM, stored)
                    }
                    // Supplier data never makes a revision; the latest run's
                    // row keeps it.
                    SubstitutesCsvColumn::SupplierData => {
                        (SUPPLIER_DATA, Some(csv.field(index, column_index)))
                    }
                    SubstitutesCsvColumn::Unknown => ("lost", None),
                };
                *account
                    .destinations
                    .entry(destination.to_owned())
                    .or_default() += 1;
                if stored.as_deref() != Some(value.as_str()) {
                    self.problem(format!(
                        "substitutes.csv row {row_number} field {column}: oracle {value:?}, stored {stored:?} in {destination}"
                    ));
                }
            }
        }
        // No manufacturer part without a row that made it.
        for (ipn, details) in scope {
            let offset = usize::from(rows_with_main_part.get(ipn).copied().unwrap_or(false));
            let expected = offset + accepted.get(ipn).copied().unwrap_or(0);
            if details.manufacturer_parts.len() != expected {
                self.problem(format!(
                    "{ipn} has {} manufacturer parts, but its row and substitutes.csv rows make {expected}",
                    details.manufacturer_parts.len()
                ));
            }
        }
        account
    }

    /// Checks every symbol and footprint reference of the parts in `scope`.
    /// Returns the accounts and, for each resolved footprint's repository
    /// path, the CAD item revision the parts bind.
    fn cad_items(
        &mut self,
        scope: &BTreeMap<String, &PartRevision>,
        explained: &mut BTreeSet<String>,
    ) -> (Vec<CadItemAccount>, BTreeMap<String, String>) {
        let oracle = self.oracle;
        let mut accounts = Vec::new();
        let readback = self.readback;
        let mut footprint_paths = BTreeMap::new();
        let references = oracle
            .assets
            .symbol_references
            .iter()
            .chain(&oracle.assets.footprint_references);
        for reference in references {
            let users: Vec<&String> = reference
                .source_ipns
                .iter()
                .filter(|ipn| scope.contains_key(*ipn))
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
                let bindings: Vec<_> = scope[*ipn]
                    .cad_references
                    .iter()
                    .filter(|binding| {
                        binding.role == role && binding.reference == reference.reference
                    })
                    .collect();
                // One binding for each time the part's field lists the
                // reference: a repeated footprint choice (kept with a
                // repeated-footprint-choice problem) binds each time.
                let column = if role == CadReferenceRole::Symbol {
                    "Symbol"
                } else {
                    "Footprint"
                };
                let listed = scope[*ipn]
                    .field(column)
                    .unwrap_or_default()
                    .split(';')
                    .map(str::trim)
                    .filter(|named| *named == reference.reference)
                    .count()
                    .max(1);
                match bindings.as_slice() {
                    found if found.len() == listed => {
                        for binding in found {
                            targets
                                .insert(serde_json::to_string(&binding.target).unwrap_or_default());
                        }
                    }
                    _ => self.problem(format!(
                        "{ipn} has {} bindings for {}",
                        bindings.len(),
                        reference.reference
                    )),
                }
            }
            // Parts may bind different numbers of one CAD item while the
            // content is the same (the same content under a newer number is
            // not a change), so targets are compared by content.
            let mut targets: Vec<CadReferenceTarget> = targets
                .iter()
                .map(|target| serde_json::from_str(target).expect("parses"))
                .collect();
            targets.sort_by_key(|target| match target {
                CadReferenceTarget::CadItemRevision { number, .. } => *number,
                _ => 0,
            });
            let contents: BTreeSet<String> = targets
                .iter()
                .map(|target| match target {
                    CadReferenceTarget::CadItemRevision {
                        cad_item,
                        number,
                        snapshot_sha256,
                    } => {
                        let id = CadItemRevisionId::new(&CadItemId::from_stored(cad_item), *number);
                        // The record's content, only if it is the revision
                        // the part names; otherwise the named hash, which
                        // then differs from its peers.
                        let content = readback
                            .cad_item_revision(id.as_str())
                            .filter(|revision| &revision.snapshot_sha256 == snapshot_sha256)
                            .and_then(|revision| {
                                serde_json::from_str::<CadItemDetails>(&revision.snapshot).ok()
                            })
                            .map_or_else(
                                || snapshot_sha256.clone(),
                                |details| details.content_sha256(),
                            );
                        format!("{cad_item} {content}")
                    }
                    other => serde_json::to_string(other).unwrap_or_default(),
                })
                .collect();
            if contents.len() != 1 {
                self.problem(format!(
                    "{} binds {} different targets across its parts",
                    reference.reference,
                    contents.len()
                ));
                accounts.push(account);
                continue;
            }
            let target = targets.last().expect("one target").clone();
            match (&target, reference.local, reference.status.as_str()) {
                (CadReferenceTarget::External, false, _) => {}
                (
                    CadReferenceTarget::CadItemRevision {
                        cad_item,
                        number,
                        snapshot_sha256,
                    },
                    true,
                    "resolved",
                ) => {
                    // Every other bound revision must be a revision with
                    // this content too; the account shows the highest.
                    for other in &targets[..targets.len() - 1] {
                        if let CadReferenceTarget::CadItemRevision {
                            number: other_number,
                            snapshot_sha256: other_sha256,
                            ..
                        } = other
                        {
                            self.check_cad_item_revision(
                                &mut account,
                                cad_item,
                                *other_number,
                                other_sha256,
                                oracle_file.map(|file| file.byte_sha256.as_str()),
                            );
                        }
                    }
                    self.check_cad_item_revision(
                        &mut account,
                        cad_item,
                        *number,
                        snapshot_sha256,
                        oracle_file.map(|file| file.byte_sha256.as_str()),
                    );
                    if role == CadReferenceRole::Footprint {
                        if let Some(path) = &reference.asset_path {
                            let revision =
                                CadItemRevisionId::new(&CadItemId::from_stored(cad_item), *number);
                            footprint_paths.insert(path.clone(), revision.to_string());
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
                    match readback.problems.iter().find(|problem| {
                        self.in_run(problem) && problem.code == code && problem.location == location
                    }) {
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
        number: u32,
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
        let revision_id = CadItemRevisionId::new(&CadItemId::from_stored(cad_item_id), number);
        account.cad_item_revision = Some(revision_id.to_string());
        let Some(revision) = readback.cad_item_revision(revision_id.as_str()) else {
            self.problem(format!(
                "{reference}: CAD item revision {number} ({revision_id}) is missing"
            ));
            return;
        };
        if revision.cad_item != cad_item_id || revision.number != number {
            self.problem(format!(
                "{reference}: record {revision_id} is not revision {number} of its CAD item"
            ));
        }
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
        if Some(source.path.as_str()) != account.oracle_asset_path.as_deref() {
            self.problem(format!(
                "{reference}: Source names {}, the oracle {:?}",
                source.path, account.oracle_asset_path
            ));
        }
        match source.byte_range {
            // A whole file: the blob is the file the oracle hashed.
            None => {
                if Some(source.byte_sha256.as_str()) != oracle_file_sha {
                    self.problem(format!(
                        "{reference}: Source names file hash {}, the oracle {oracle_file_sha:?}",
                        source.byte_sha256
                    ));
                }
                if Some(blob.sha256.as_str()) != oracle_file_sha {
                    self.problem(format!("{reference}: blob is not the oracle's file"));
                }
            }
            // One symbol: the blob is exactly that whole top-level symbol in
            // the oracle's file today, as KiCad Integration reads it. The
            // stored range and file hash may name an older version of the
            // library: a symbol keeps its revision when only a neighbouring
            // symbol changes.
            Some([start, end]) => {
                let file = std::fs::read(self.repository.join(&source.path)).unwrap_or_default();
                let file_sha256 = sha256_hex(&file);
                if Some(file_sha256.as_str()) != oracle_file_sha {
                    self.problem(format!(
                        "{reference}: {} is not the oracle's file",
                        source.path
                    ));
                } else {
                    let item = reference.split_once(':').map_or("", |(_, item)| item);
                    let whole = SymbolLibrary::parse(&file)
                        .ok()
                        .and_then(|library| match library.find(item) {
                            SymbolLookup::Found(entry) => Some(entry.span.clone()),
                            _ => None,
                        });
                    let bytes = whole.clone().and_then(|span| file.get(span));
                    if bytes != Some(blob.bytes.as_slice()) {
                        self.problem(format!(
                            "{reference}: blob is not the whole symbol `{item}` of the oracle's file ({whole:?})"
                        ));
                    }
                    // When the source names this very file, its range must
                    // be exactly that symbol too.
                    if source.byte_sha256 == file_sha256
                        && whole != Some(start as usize..end as usize)
                    {
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
        footprint_paths: &BTreeMap<String, String>,
        explained: &mut BTreeSet<String>,
    ) -> Vec<ModelAccount> {
        let oracle = self.oracle;
        let readback = self.readback;
        let mut accounts = Vec::new();
        for model in &oracle.assets.model_references {
            let Some(bound) = footprint_paths.get(&model.source_footprint) else {
                continue;
            };
            // The snapshot of the footprint revision the parts bind must list
            // the reference.
            let snapshot = readback.cad_item_revision(bound).and_then(|revision| {
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
                    let oracle_sha = model.asset_path.as_ref().and_then(|path| {
                        oracle
                            .assets
                            .models_3d
                            .iter()
                            .find(|file| &file.path == path)
                            .map(|file| file.byte_sha256.clone())
                    });
                    match (&stored.target, model.status.as_str()) {
                        (
                            ModelTarget::CadItemRevision {
                                cad_item,
                                number,
                                snapshot_sha256,
                            },
                            "resolved",
                        ) => {
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
                                *number,
                                snapshot_sha256,
                                oracle_sha.as_deref(),
                            );
                            format!("cad-item-revision {number} ({snapshot_sha256})")
                        }
                        // The oracle finds the file, but it is empty: the
                        // Catalog keeps it as an import problem, not a blob.
                        (ModelTarget::Unresolved { status }, "resolved")
                            if status == EMPTY_3D_MODEL
                                && oracle_sha.as_deref() == Some(EMPTY_FILE_SHA256) =>
                        {
                            self.expect_model_problem(status, model, explained);
                            "empty file + import problem".to_owned()
                        }
                        (ModelTarget::Unresolved { status }, oracle_status)
                            if status == oracle_status =>
                        {
                            self.expect_model_problem(status, model, explained);
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

    /// The latest import run must have found a `<status>-3d-model-reference`
    /// import problem for `model`.
    fn expect_model_problem(
        &mut self,
        status: &str,
        model: &csv_catalog_fixture::ModelReference,
        explained: &mut BTreeSet<String>,
    ) {
        let readback = self.readback;
        let code = format!("{status}-3d-model-reference");
        let location = format!("model={}", model.reference);
        match readback.problems.iter().find(|problem| {
            self.in_run(problem)
                && problem.code == code
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
    }

    /// Every oracle import problem about this CSV file's CAD must be stored as an
    /// import problem with the same code and location (and, for 3D
    /// models, the same footprint path and message).
    fn oracle_import_problems(
        &mut self,
        footprint_paths: &BTreeMap<String, String>,
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
        for problem in &oracle.import_problems {
            let model = footprint_paths.contains_key(&problem.path);
            let reference =
                problem.code.ends_with("-reference") && references.contains(&problem.location);
            if !model && !reference {
                continue;
            }
            let stored = readback.problems.iter().find(|stored| {
                self.in_run(stored)
                    && stored.code == problem.code
                    && stored.location == problem.location
                    && (!model
                        || (stored.path == problem.path && stored.message == problem.message))
            });
            match stored {
                Some(stored) => {
                    explained.insert(stored.id.clone());
                }
                None => self.problem(format!(
                    "oracle import problem {} {} {} is not stored",
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

/// Whether a symbol the revision binds says `(in_bom no)` in its stored
/// bytes, and for each footprint choice whether its `(attr ...)` has
/// `exclude_from_bom`. A reference not bound to a stored CAD item revision
/// gives no evidence (false).
fn bom_marks(readback: &CatalogReadback, revision: &PartRevision) -> (bool, Vec<bool>) {
    let marked = |reference: &CadReference| {
        let CadReferenceTarget::CadItemRevision {
            cad_item, number, ..
        } = &reference.target
        else {
            return false;
        };
        let id = CadItemRevisionId::new(&CadItemId::from_stored(cad_item), *number);
        let Some(blob) = readback
            .cad_item_revision(id.as_str())
            .and_then(|revision| readback.blob(&revision.blob))
        else {
            return false;
        };
        match reference.role {
            CadReferenceRole::Symbol => symbol_entries_say_not_in_bom(&blob.bytes),
            CadReferenceRole::Footprint => footprint_says_excluded_from_bom(&blob.bytes),
        }
    };
    let of_role = |role: CadReferenceRole| -> Vec<bool> {
        revision
            .cad_references
            .iter()
            .filter(|reference| reference.role == role)
            .map(marked)
            .collect()
    };
    (
        of_role(CadReferenceRole::Symbol).contains(&true),
        of_role(CadReferenceRole::Footprint),
    )
}

//! The zero-loss report: every source field, substitute, CAD reference and
//! oracle diagnostic of one legacy grouping, found again in the Catalog
//! records read back from PocketBase.
//!
//! The expected side is always the oracle manifest, never this crate's own
//! reading of the CSV. Each source field must be found in exactly one of
//! four places, with the same value:
//!
//! | destination                  | where it is checked                         |
//! |------------------------------|---------------------------------------------|
//! | `part-revision-snapshot`     | the head snapshot's `ipn` or named field    |
//! | `approved-manufacturer-part` | the head snapshot's primary or a substitute |
//! | `operational-fact`           | the head revision's Provenance              |
//! | `import-diagnostic`          | a stored Import Diagnostic's detail         |

use catalog::legacy_import::{
    AssetProvenance, ColumnRoute, Provenance, SubstituteRoute, route_column,
    route_substitute_column,
};
use catalog::pocketbase::{CatalogReadback, StoredImportDiagnostic, StoredPartRevision};
use catalog::{
    AssetRevisionDeclaration, AssetRevisionId, BindingRole, BindingTarget, CatalogAssetId,
    LicenseDeclaration, ModelTarget, PartRevisionSnapshot, sha256_hex,
};
use legacy_import_fixture::Manifest;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const SNAPSHOT: &str = "part-revision-snapshot";
pub const APPROVED_MANUFACTURER_PART: &str = "approved-manufacturer-part";
pub const OPERATIONAL_FACT: &str = "operational-fact";
pub const IMPORT_DIAGNOSTIC: &str = "import-diagnostic";

#[derive(Clone, Debug, Serialize)]
pub struct ZeroLossReport {
    pub legacy_grouping: String,
    pub source_path: String,
    pub oracle_byte_sha256: String,
    /// The CSV's hash as recorded on the Change Set(s).
    pub change_set_byte_sha256: Vec<String>,
    /// The hash of the CSV rebuilt from PocketBase alone: the stored header
    /// line plus every row's stored raw bytes in row order.
    pub rebuilt_byte_sha256: String,
    pub rows: usize,
    pub columns: usize,
    pub fields: usize,
    /// Fields per destination; they sum to `fields`.
    pub destinations: BTreeMap<String, usize>,
    /// Where each column went (a column of a row kept as an Import
    /// Diagnostic goes there instead).
    pub column_routes: Vec<(String, String)>,
    pub managed_parts: usize,
    pub substitutes: SubstitutesAccount,
    pub assets: Vec<AssetAccount>,
    pub model_references: Vec<ModelAccount>,
    pub oracle_diagnostics: Vec<DiagnosticAccount>,
    /// Stored diagnostics no oracle diagnostic or row explains.
    pub other_diagnostics: Vec<String>,
    pub change_sets: Vec<ChangeSetAccount>,
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
    pub change_set_byte_sha256: Vec<String>,
    pub rows: usize,
    pub rows_in_scope: usize,
    pub fields_in_scope: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssetAccount {
    pub kind: String,
    pub reference: String,
    pub oracle_status: String,
    pub oracle_asset_path: Option<String>,
    pub oracle_byte_sha256: Option<String>,
    pub parts: usize,
    pub asset: Option<String>,
    pub asset_revision: Option<String>,
    pub revision_sha256: Option<String>,
    pub blob_sha256: Option<String>,
    pub blob_bytes: Option<usize>,
    pub license_status: Option<String>,
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
pub struct DiagnosticAccount {
    pub code: String,
    pub path: String,
    pub location: String,
    pub stored: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChangeSetAccount {
    pub id: String,
    pub seq: u64,
    pub kind: String,
    pub reason: String,
    pub source_commit: String,
    pub audit_events: usize,
}

/// Builds the report for `grouping` (such as `g-res`). `repository` is
/// read (never written) only to cut each symbol's byte range out of the
/// library file the oracle hashed.
pub fn report(
    oracle: &Manifest,
    grouping: &str,
    readback: &CatalogReadback,
    repository: &Path,
) -> ZeroLossReport {
    Reporter::new(oracle, grouping, readback, repository).run()
}

struct Reporter<'a> {
    oracle: &'a Manifest,
    grouping: String,
    readback: &'a CatalogReadback,
    repository: &'a Path,
    problems: Vec<String>,
}

/// A part's head revision, decoded.
struct Head<'a> {
    revision: &'a StoredPartRevision,
    snapshot: PartRevisionSnapshot,
    provenance: Provenance,
}

impl<'a> Reporter<'a> {
    fn new(
        oracle: &'a Manifest,
        grouping: &str,
        readback: &'a CatalogReadback,
        repository: &'a Path,
    ) -> Self {
        Self {
            oracle,
            grouping: grouping.to_owned(),
            readback,
            repository,
            problems: Vec::new(),
        }
    }

    fn problem(&mut self, message: String) {
        self.problems.push(message);
    }

    fn heads(&mut self) -> Vec<Head<'a>> {
        let mut heads = Vec::new();
        let readback = self.readback;
        for part in &readback.managed_parts {
            let Some(revision) = readback.part_revision(&part.head) else {
                self.problem(format!("{}: head {} is missing", part.ipn, part.head));
                continue;
            };
            if sha256_hex(&revision.snapshot) != revision.snapshot_sha256 {
                self.problem(format!("{}: snapshot does not match its hash", part.ipn));
            }
            if revision.number != part.head_number || revision.managed_part != part.id {
                self.problem(format!("{}: head pointer is inconsistent", part.ipn));
            }
            let snapshot = match serde_json::from_str::<PartRevisionSnapshot>(&revision.snapshot) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.problem(format!("{}: snapshot does not parse: {error}", part.ipn));
                    continue;
                }
            };
            let provenance = match serde_json::from_str::<Provenance>(&revision.provenance) {
                Ok(provenance) => provenance,
                Err(error) => {
                    self.problem(format!("{}: provenance does not parse: {error}", part.ipn));
                    continue;
                }
            };
            if snapshot.ipn != part.ipn {
                self.problem(format!("{}: snapshot names {}", part.ipn, snapshot.ipn));
            }
            heads.push(Head {
                revision,
                snapshot,
                provenance,
            });
        }
        heads
    }

    fn run(mut self) -> ZeroLossReport {
        let oracle = self.oracle;
        let readback = self.readback;
        let table = self.grouping.trim_start_matches("g-").to_owned();
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

        // Change Sets and the source files they recorded.
        let mut change_sets = Vec::new();
        let mut csv_hashes = BTreeSet::new();
        let mut substitute_hashes = BTreeSet::new();
        let mut header_raw = None;
        for change_set in &readback.change_sets {
            let audit_events = readback
                .audit_events
                .iter()
                .filter(|event| event.change_set == change_set.id)
                .count();
            if audit_events != 1 {
                self.problem(format!(
                    "Change Set {} has {audit_events} Audit Events",
                    change_set.seq
                ));
            }
            if change_set.reason.trim().is_empty() {
                self.problem(format!("Change Set {} has no reason", change_set.seq));
            }
            let sources: Value = serde_json::from_str(&change_set.sources).unwrap_or(Value::Null);
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
            change_sets.push(ChangeSetAccount {
                id: change_set.id.clone(),
                seq: change_set.seq,
                kind: change_set.kind.clone(),
                reason: change_set.reason.clone(),
                source_commit: change_set.source_commit.clone(),
                audit_events,
            });
        }
        if !csv_hashes.contains(&document.byte_sha256) {
            self.problem(format!(
                "no Change Set recorded {source_path} with the oracle's byte hash"
            ));
        }

        let heads = self.heads();
        let by_row: BTreeMap<usize, &Head> = heads
            .iter()
            .filter(|head| head.provenance.path == source_path)
            .map(|head| (head.provenance.row_number, head))
            .collect();
        let diagnostics = &readback.diagnostics;
        let diagnostic_rows: BTreeMap<usize, (&StoredImportDiagnostic, Value)> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.path == source_path)
            .filter_map(|diagnostic| {
                let detail: Value = serde_json::from_str(&diagnostic.detail).ok()?;
                let row = detail["row_number"].as_u64()? as usize;
                detail["raw_record"].as_str()?;
                Some((row, (diagnostic, detail)))
            })
            .collect();

        let mut destinations: BTreeMap<String, usize> = BTreeMap::new();
        let mut column_routes: BTreeMap<String, String> = BTreeMap::new();
        let mut field_accounts = Vec::new();
        let mut explained_diagnostics = BTreeSet::new();
        let mut rebuilt = header_raw.clone().unwrap_or_default();
        let mut scope_ipns = BTreeSet::new();
        let mut matched_heads = BTreeSet::new();
        if header_raw.is_none() {
            self.problem(format!(
                "no Change Set kept the header line of {source_path}"
            ));
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
            if let Some(head) = by_row.get(&row_number) {
                matched_heads.insert(head.revision.id.clone());
                scope_ipns.insert(head.snapshot.ipn.clone());
                let provenance = &head.provenance;
                rebuilt.push_str(&provenance.raw_record);
                let mut padded = provenance.record.clone();
                padded.resize(headers.len(), String::new());
                if &provenance.headers != headers || &padded != row {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): Provenance does not hold the oracle row"
                    ));
                }
                if provenance.byte_sha256 != document.byte_sha256 {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): Provenance names another version of {source_path}"
                    ));
                }
                let snapshot = &head.snapshot;
                let primary = snapshot.approved_manufacturer_parts.first();
                let incomplete = diagnostics.iter().find(|diagnostic| {
                    diagnostic.code == "incomplete-primary-manufacturer-part"
                        && diagnostic.subject == snapshot.ipn
                });
                let mut snapshot_columns = Vec::new();
                let mut fact_columns = Vec::new();
                for (column, value) in headers.iter().zip(row) {
                    let route = route_column(column);
                    let (destination, stored) = match route {
                        ColumnRoute::Ipn => (SNAPSHOT, Some(snapshot.ipn.clone())),
                        ColumnRoute::Snapshot => {
                            snapshot_columns.push(column.clone());
                            (SNAPSHOT, snapshot.field(column).map(str::to_owned))
                        }
                        ColumnRoute::PrimaryMpn | ColumnRoute::PrimaryManufacturer => {
                            match (primary, incomplete) {
                                (Some(primary), _) => (
                                    APPROVED_MANUFACTURER_PART,
                                    Some(if route == ColumnRoute::PrimaryMpn {
                                        primary.mpn.clone()
                                    } else {
                                        primary.manufacturer.clone()
                                    }),
                                ),
                                (None, Some(diagnostic)) => {
                                    explained_diagnostics.insert(diagnostic.id.clone());
                                    let detail: Value = serde_json::from_str(&diagnostic.detail)
                                        .unwrap_or_default();
                                    (
                                        IMPORT_DIAGNOSTIC,
                                        detail[column.as_str()].as_str().map(str::to_owned),
                                    )
                                }
                                (None, None) => (APPROVED_MANUFACTURER_PART, None),
                            }
                        }
                        ColumnRoute::OperationalFact => {
                            fact_columns.push(column.clone());
                            (
                                OPERATIONAL_FACT,
                                provenance
                                    .operational_facts
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
                // Operational Facts.
                let stored_fields: Vec<&String> =
                    snapshot.fields.iter().map(|field| &field.name).collect();
                if stored_fields != snapshot_columns.iter().collect::<Vec<_>>() {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): snapshot fields {stored_fields:?} are not the semantic columns {snapshot_columns:?}"
                    ));
                }
                let stored_facts: Vec<&String> = provenance
                    .operational_facts
                    .iter()
                    .map(|fact| &fact.column)
                    .collect();
                if stored_facts != fact_columns.iter().collect::<Vec<_>>() {
                    self.problems.push(format!(
                        "row {row_number} ({ipn}): Operational Facts {stored_facts:?} are not {fact_columns:?}"
                    ));
                }
                for fact_column in &fact_columns {
                    if let Some(value) = row
                        .get(
                            headers
                                .iter()
                                .position(|h| h == fact_column)
                                .unwrap_or(usize::MAX),
                        )
                        .filter(|value| !value.is_empty())
                    {
                        if head.revision.snapshot.contains(&format!("\"{value}\"")) {
                            self.problems.push(format!(
                                "row {row_number} ({ipn}): Operational Fact {fact_column}={value} also appears in the snapshot"
                            ));
                        }
                    }
                }
            } else if let Some((diagnostic, detail)) = diagnostic_rows.get(&row_number) {
                explained_diagnostics.insert(diagnostic.id.clone());
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
                            "row {row_number} field {column}: oracle {value:?}, diagnostic kept {stored:?}"
                        ));
                    }
                    account(column, IMPORT_DIAGNOSTIC, value, stored);
                }
            } else {
                self.problems.push(format!(
                    "row {row_number} ({ipn}) is neither a Managed Part nor an Import Diagnostic"
                ));
                for (column, value) in headers.iter().zip(row) {
                    account(column, "lost", value, None);
                }
            }
        }
        for head in &heads {
            if head.provenance.path == source_path && !matched_heads.contains(&head.revision.id) {
                self.problems.push(format!(
                    "{} comes from row {}, which the oracle does not have",
                    head.snapshot.ipn, head.provenance.row_number
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

        let substitutes = self.substitutes(&heads, &scope_ipns, &substitute_hashes);
        let (assets, footprint_paths) =
            self.assets(&heads, &scope_ipns, &mut explained_diagnostics);
        let model_references = self.models(&footprint_paths, &mut explained_diagnostics);
        let oracle_diagnostics =
            self.oracle_diagnostics(&footprint_paths, &assets, &mut explained_diagnostics);
        let other_diagnostics = diagnostics
            .iter()
            .filter(|diagnostic| !explained_diagnostics.contains(&diagnostic.id))
            .map(|diagnostic| {
                format!(
                    "{} {} {}",
                    diagnostic.code, diagnostic.path, diagnostic.location
                )
            })
            .collect();

        let fields = field_accounts.len();
        if fields != document.rows.len() * headers.len() {
            self.problem(format!(
                "{fields} fields accounted for, but the oracle has {}",
                document.rows.len() * headers.len()
            ));
        }
        ZeroLossReport {
            legacy_grouping: self.grouping.clone(),
            source_path,
            oracle_byte_sha256: document.byte_sha256.clone(),
            change_set_byte_sha256: csv_hashes.into_iter().collect(),
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
            managed_parts: matched_heads.len(),
            substitutes,
            assets,
            model_references,
            oracle_diagnostics,
            other_diagnostics,
            change_sets,
            field_accounts,
            problems: self.problems,
        }
    }

    fn empty(self) -> ZeroLossReport {
        ZeroLossReport {
            legacy_grouping: self.grouping,
            source_path: String::new(),
            oracle_byte_sha256: String::new(),
            change_set_byte_sha256: vec![],
            rebuilt_byte_sha256: String::new(),
            rows: 0,
            columns: 0,
            fields: 0,
            destinations: BTreeMap::new(),
            column_routes: vec![],
            managed_parts: 0,
            substitutes: SubstitutesAccount::default(),
            assets: vec![],
            model_references: vec![],
            oracle_diagnostics: vec![],
            other_diagnostics: vec![],
            change_sets: vec![],
            field_accounts: vec![],
            problems: self.problems,
        }
    }

    fn substitutes(
        &mut self,
        heads: &[Head],
        scope: &BTreeSet<String>,
        hashes: &BTreeSet<String>,
    ) -> SubstitutesAccount {
        let oracle = self.oracle;
        let document = &oracle.substitutes;
        if !hashes.contains(&document.byte_sha256) {
            self.problem(format!(
                "no Change Set recorded {} with the oracle's byte hash",
                document.path
            ));
        }
        let ipn_column = document.headers.iter().position(|header| header == "IPN");
        let mut account = SubstitutesAccount {
            path: document.path.clone(),
            oracle_byte_sha256: document.byte_sha256.clone(),
            change_set_byte_sha256: hashes.iter().cloned().collect(),
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
            let head = heads.iter().find(|head| head.snapshot.ipn == ipn);
            let found = head.and_then(|head| {
                let position = head
                    .provenance
                    .substitutes
                    .iter()
                    .position(|source| source.row_number == row_number)?;
                // The primary is the first Approved Manufacturer Part.
                let approved = head
                    .snapshot
                    .approved_manufacturer_parts
                    .get(position + 1)?;
                Some((approved, &head.provenance.substitutes[position]))
            });
            let Some((approved, source)) = found else {
                self.problem(format!(
                    "substitute row {row_number} of {ipn} is not an Approved Manufacturer Part"
                ));
                continue;
            };
            for (column, value) in document.headers.iter().zip(row) {
                let stored = match route_substitute_column(column) {
                    SubstituteRoute::Ipn => Some(ipn.clone()),
                    SubstituteRoute::Mpn => Some(approved.mpn.clone()),
                    SubstituteRoute::Manufacturer => Some(approved.manufacturer.clone()),
                    SubstituteRoute::Datasheet => approved.datasheet.clone(),
                    SubstituteRoute::OperationalFact => source
                        .operational_facts
                        .iter()
                        .find(|fact| &fact.column == column)
                        .map(|fact| fact.value.clone()),
                    SubstituteRoute::Unknown => None,
                };
                if stored.as_deref() != Some(value.as_str()) {
                    self.problem(format!(
                        "substitute row {row_number} field {column}: oracle {value:?}, stored {stored:?}"
                    ));
                }
            }
        }
        account
    }

    /// Checks every symbol and footprint reference of the grouping's rows.
    /// Returns the accounts and the repository paths of resolved
    /// footprints.
    fn assets(
        &mut self,
        heads: &[Head],
        scope: &BTreeSet<String>,
        explained: &mut BTreeSet<String>,
    ) -> (Vec<AssetAccount>, BTreeSet<String>) {
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
                (BindingRole::Symbol, &oracle.assets.symbols)
            } else {
                (BindingRole::Footprint, &oracle.assets.footprints)
            };
            let oracle_file = reference
                .asset_path
                .as_ref()
                .and_then(|path| files.iter().find(|file| &file.path == path));
            let mut account = AssetAccount {
                kind: reference.kind.clone(),
                reference: reference.reference.clone(),
                oracle_status: reference.status.clone(),
                oracle_asset_path: reference.asset_path.clone(),
                oracle_byte_sha256: oracle_file.map(|file| file.byte_sha256.clone()),
                parts: users.len(),
                asset: None,
                asset_revision: None,
                revision_sha256: None,
                blob_sha256: None,
                blob_bytes: None,
                license_status: None,
            };
            let mut targets = BTreeSet::new();
            for ipn in &users {
                let Some(head) = heads.iter().find(|head| &&head.snapshot.ipn == ipn) else {
                    self.problem(format!(
                        "{ipn} uses {} but is not a Managed Part",
                        reference.reference
                    ));
                    continue;
                };
                let bindings: Vec<_> = head
                    .snapshot
                    .asset_bindings
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
            let target: BindingTarget =
                serde_json::from_str(targets.iter().next().expect("one target")).expect("parses");
            match (&target, reference.local, reference.status.as_str()) {
                (BindingTarget::External, false, _) => {}
                (
                    BindingTarget::AssetRevision {
                        asset,
                        asset_revision_sha256,
                    },
                    true,
                    "resolved",
                ) => {
                    self.check_asset_revision(
                        &mut account,
                        asset,
                        asset_revision_sha256,
                        oracle_file.map(|file| file.byte_sha256.as_str()),
                    );
                    if role == BindingRole::Footprint {
                        if let Some(path) = &reference.asset_path {
                            footprint_paths.insert(path.clone());
                        }
                    }
                }
                (BindingTarget::Unresolved { status }, true, oracle_status)
                    if status == oracle_status =>
                {
                    let column = if role == BindingRole::Symbol {
                        "Symbol"
                    } else {
                        "Footprint"
                    };
                    let code = format!("{status}-local-{}-reference", reference.kind);
                    let location = format!("{column}={}", reference.reference);
                    match readback.diagnostics.iter().find(|diagnostic| {
                        diagnostic.code == code && diagnostic.location == location
                    }) {
                        Some(diagnostic) => {
                            explained.insert(diagnostic.id.clone());
                        }
                        None => self.problem(format!("no Import Diagnostic {code} for {location}")),
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

    fn check_asset_revision(
        &mut self,
        account: &mut AssetAccount,
        asset_id: &str,
        revision_sha256: &str,
        oracle_file_sha: Option<&str>,
    ) {
        let readback = self.readback;
        let reference = account.reference.clone();
        account.asset = Some(asset_id.to_owned());
        account.revision_sha256 = Some(revision_sha256.to_owned());
        let Some(asset) = readback.asset(asset_id) else {
            self.problem(format!("{reference}: Catalog Asset {asset_id} is missing"));
            return;
        };
        if asset.kind != account.kind || asset.key != reference {
            self.problem(format!(
                "{reference}: Catalog Asset is {} {}",
                asset.kind, asset.key
            ));
        }
        let revision_id =
            AssetRevisionId::new(&CatalogAssetId::from_stored(asset_id), revision_sha256);
        account.asset_revision = Some(revision_id.to_string());
        let Some(revision) = readback.asset_revision(revision_id.as_str()) else {
            self.problem(format!(
                "{reference}: Asset Revision {revision_id} is missing"
            ));
            return;
        };
        if sha256_hex(&revision.declaration) != revision_sha256
            || revision.revision_sha256 != revision_sha256
        {
            self.problem(format!(
                "{reference}: declaration does not match its revision hash"
            ));
        }
        let Ok(declaration) =
            serde_json::from_str::<AssetRevisionDeclaration>(&revision.declaration)
        else {
            self.problem(format!("{reference}: declaration does not parse"));
            return;
        };
        account.license_status = Some(revision.license_status.clone());
        if declaration.license != LicenseDeclaration::UndeclaredNeedsReview
            || revision.license_status != declaration.license.status()
        {
            self.problem(format!("{reference}: License Declaration is not explicit"));
        }
        let Some(blob) = readback.blob(&revision.blob) else {
            self.problem(format!(
                "{reference}: Asset Blob {} is missing",
                revision.blob
            ));
            return;
        };
        account.blob_sha256 = Some(blob.sha256.clone());
        account.blob_bytes = Some(blob.bytes.len());
        if blob.sha256 != declaration.blob_sha256 || sha256_hex(&blob.bytes) != blob.sha256 {
            self.problem(format!(
                "{reference}: stored blob is not the declared bytes"
            ));
        }
        let Ok(provenance) = serde_json::from_str::<AssetProvenance>(&revision.provenance) else {
            self.problem(format!("{reference}: provenance does not parse"));
            return;
        };
        if Some(provenance.path.as_str()) != account.oracle_asset_path.as_deref()
            || Some(provenance.byte_sha256.as_str()) != oracle_file_sha
        {
            self.problem(format!(
                "{reference}: Provenance names {} {}, the oracle {:?} {:?}",
                provenance.path, provenance.byte_sha256, account.oracle_asset_path, oracle_file_sha
            ));
        }
        match provenance.byte_range {
            // A whole file: the blob is the file the oracle hashed.
            None => {
                if Some(blob.sha256.as_str()) != oracle_file_sha {
                    self.problem(format!("{reference}: blob is not the oracle's file"));
                }
            }
            // One symbol: the blob is that byte range of the oracle's file.
            Some([start, end]) => {
                let file =
                    std::fs::read(self.repository.join(&provenance.path)).unwrap_or_default();
                if Some(sha256_hex(&file).as_str()) != oracle_file_sha {
                    self.problem(format!(
                        "{reference}: {} is not the oracle's file",
                        provenance.path
                    ));
                } else if file.get(start as usize..end as usize) != Some(blob.bytes.as_slice()) {
                    self.problem(format!(
                        "{reference}: blob is not bytes {start}..{end} of the oracle's file"
                    ));
                } else {
                    let item = reference.split_once(':').map_or("", |(_, item)| item);
                    if !blob
                        .bytes
                        .starts_with(format!("(symbol \"{item}\"").as_bytes())
                    {
                        self.problem(format!("{reference}: blob is not the symbol `{item}`"));
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
            // The footprint's stored declaration must list the reference.
            let declaration = readback.asset_revisions.iter().find_map(|revision| {
                let provenance: AssetProvenance =
                    serde_json::from_str(&revision.provenance).ok()?;
                (provenance.path == model.source_footprint).then_some(())?;
                serde_json::from_str::<AssetRevisionDeclaration>(&revision.declaration).ok()
            });
            let stored = declaration.and_then(|declaration| {
                declaration
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
                            ModelTarget::AssetRevision {
                                asset,
                                asset_revision_sha256,
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
                            let mut account = AssetAccount {
                                kind: "model-3d".into(),
                                reference: model.asset_path.clone().unwrap_or_default(),
                                oracle_status: model.status.clone(),
                                oracle_asset_path: model.asset_path.clone(),
                                oracle_byte_sha256: oracle_sha.clone(),
                                parts: 0,
                                asset: None,
                                asset_revision: None,
                                revision_sha256: None,
                                blob_sha256: None,
                                blob_bytes: None,
                                license_status: None,
                            };
                            self.check_asset_revision(
                                &mut account,
                                asset,
                                asset_revision_sha256,
                                oracle_sha.as_deref(),
                            );
                            format!("asset-revision {asset_revision_sha256}")
                        }
                        (ModelTarget::Unresolved { status }, oracle_status)
                            if status == oracle_status =>
                        {
                            let code = format!("{status}-3d-model-reference");
                            let location = format!("model={}", model.reference);
                            match readback.diagnostics.iter().find(|diagnostic| {
                                diagnostic.code == code
                                    && diagnostic.path == model.source_footprint
                                    && diagnostic.location == location
                            }) {
                                Some(diagnostic) => {
                                    explained.insert(diagnostic.id.clone());
                                }
                                None => self.problem(format!(
                                    "no Import Diagnostic {code} for {location} in {}",
                                    model.source_footprint
                                )),
                            }
                            format!("unresolved ({status}) + import diagnostic")
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

    /// Every oracle diagnostic about this grouping's CAD must be stored as an
    /// Import Diagnostic with the same code and location (and, for 3D
    /// models, the same footprint path and message).
    fn oracle_diagnostics(
        &mut self,
        footprint_paths: &BTreeSet<String>,
        assets: &[AssetAccount],
        explained: &mut BTreeSet<String>,
    ) -> Vec<DiagnosticAccount> {
        let references: BTreeSet<String> = assets
            .iter()
            .map(|asset| {
                let column = if asset.kind == "symbol" {
                    "Symbol"
                } else {
                    "Footprint"
                };
                format!("{column}={}", asset.reference)
            })
            .collect();
        let oracle = self.oracle;
        let readback = self.readback;
        let mut accounts = Vec::new();
        for diagnostic in &oracle.diagnostics {
            let model = footprint_paths.contains(&diagnostic.path);
            let reference = diagnostic.code.ends_with("-reference")
                && references.contains(&diagnostic.location);
            if !model && !reference {
                continue;
            }
            let stored = readback.diagnostics.iter().find(|stored| {
                stored.code == diagnostic.code
                    && stored.location == diagnostic.location
                    && (!model
                        || (stored.path == diagnostic.path && stored.message == diagnostic.message))
            });
            match stored {
                Some(stored) => {
                    explained.insert(stored.id.clone());
                }
                None => self.problem(format!(
                    "oracle diagnostic {} {} {} has no Import Diagnostic",
                    diagnostic.code, diagnostic.path, diagnostic.location
                )),
            }
            accounts.push(DiagnosticAccount {
                code: diagnostic.code.clone(),
                path: diagnostic.path.clone(),
                location: diagnostic.location.clone(),
                stored: stored.map(|stored| stored.id.clone()),
            });
        }
        accounts
    }
}

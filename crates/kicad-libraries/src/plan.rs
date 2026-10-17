//! What the KiCad libraries hold for one state of the Catalog, worked out
//! from its records alone: no file is read and no blob is needed. The
//! result depends only on the records, in a fixed order, so the same
//! Catalog always gives the same libraries.

use crate::names::{file_stem, revision_name, windows_refuses};
use catalog::pocketbase::{CatalogReadback, StoredBlob, StoredCadItemRevision, StoredPart};
use catalog::{
    CadItemDetails, CadItemKind, CadReference, CadReferenceRole, CadReferenceTarget,
    CategoryDetails, ModelTarget, PartDetails, PartRevision,
};
use kicad_integration::library_file::escape_text;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The group in KiCad's part chooser for revisions that are older than
/// their part's current revision and not archived.
pub const OLDER_GROUP: &str = "Older revisions";
/// The group for archived revisions. KiCad can hide only a whole library
/// from its chooser, and a revision's link must keep working when it is
/// archived, so archived revisions stay in the database library under
/// this name.
pub const ARCHIVED_GROUP: &str = "Archived revisions";
const OLDER_TABLE: &str = "older_revisions";
const ARCHIVED_TABLE: &str = "archived_revisions";

/// The columns every table has, in order. `Entry` is the key KiCad links
/// to; the others become symbol fields, except Symbol, Footprint and
/// Exclude from BOM, which KiCad reads as the symbol's own settings.
pub const FIXED_COLUMNS: [&str; 11] = [
    "Entry",
    "IPN",
    "Revision",
    "Symbol",
    "Footprint",
    "Value",
    "Manufacturer",
    "MPN",
    "Description",
    "Datasheet",
    EXCLUDE_FROM_BOM,
];
pub const EXCLUDE_FROM_BOM: &str = "Exclude from BOM";

/// Category fields shown on the schematic when a part is placed, as
/// `database/#gplm.kicad_dbl` shows them. Other category fields are only
/// listed in the chooser.
const SHOWN_ON_ADD: [(&str, &str); 2] = [("reg", "Voltage"), ("reg", "Current")];

/// Everything to write.
#[derive(Debug, Default)]
pub struct Plan<'a> {
    /// By nickname.
    pub symbol_libraries: Vec<SymbolLibraryPlan<'a>>,
    /// By nickname.
    pub footprint_libraries: Vec<FootprintLibraryPlan>,
    /// Footprint and 3D model files, by path.
    pub files: Vec<FilePlan<'a>>,
    /// Database tables: every category by code, then older and archived
    /// revisions.
    pub tables: Vec<TablePlan>,
    /// What could not be made, in plain sentences.
    pub problems: Vec<String>,
}

#[derive(Debug)]
pub struct SymbolLibraryPlan<'a> {
    pub nickname: String,
    /// Relative to the libraries folder, with `/`.
    pub path: String,
    /// By CAD item name, then revision number.
    pub entries: Vec<SymbolEntryPlan<'a>>,
}

#[derive(Debug)]
pub struct SymbolEntryPlan<'a> {
    /// The name in the library, such as `R_US_v2`.
    pub name: String,
    /// The CAD item's own name, which the stored bytes use.
    pub original: String,
    pub blob: &'a StoredBlob,
    /// The KiCad file version the bytes came from.
    pub version: Option<String>,
    /// Such as `symbol g-res:R_US revision 2`, for problems.
    pub label: String,
}

#[derive(Debug)]
pub struct FootprintLibraryPlan {
    pub nickname: String,
    /// The `.pretty` folder, relative to the libraries folder, with `/`.
    pub path: String,
}

#[derive(Debug)]
pub struct FilePlan<'a> {
    /// Relative to the libraries folder, with `/`.
    pub path: String,
    pub blob: &'a StoredBlob,
    pub kind: FileKind,
    /// Such as `footprint g-Resistor_SMD:R_0603_1608Metric revision 1`.
    pub label: String,
}

#[derive(Debug)]
pub enum FileKind {
    /// The stored bytes, unchanged.
    Model,
    /// The stored bytes with 3D model links pointed at the files written
    /// beside the footprint library.
    Footprint {
        /// For each `(model ...)` in file order, the new path as written
        /// between the quotes, or `None` to keep it.
        links: Vec<Option<String>>,
        /// Each `(model ...)` path as the stored bytes write it.
        references: Vec<String>,
    },
}

/// One table of the database, and one group in KiCad's chooser when it is
/// listed in the `.kicad_dbl`.
#[derive(Debug, Eq, PartialEq)]
pub struct TablePlan {
    pub group: Group,
    /// The SQLite table name: lower-case letters, digits and `_` only.
    pub table: String,
    /// The group KiCad shows, such as `Resistors`.
    pub name: String,
    /// Listed in the `.kicad_dbl`. A category table with no current
    /// revision stays in the database but is not listed, so KiCad shows no
    /// empty group; the older and archived tables are always listed, so a
    /// revision moving into them never needs a table KiCad has not loaded.
    pub listed: bool,
    /// Category fields after the fixed columns.
    pub extra: Vec<ExtraField>,
    /// By entry name.
    pub rows: Vec<Row>,
}

/// Which revisions a table holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Group {
    /// The current revisions of one category's parts.
    Category,
    Older,
    Archived,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraField {
    /// The column, named like the Catalog's field.
    pub column: String,
    /// The KiCad field name.
    pub field: String,
    pub shown_on_add: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    /// One value per fixed column (Exclude from BOM as `0` or `1`), then
    /// one per extra field.
    pub values: Vec<String>,
}

impl Row {
    pub fn entry(&self) -> &str {
        &self.values[0]
    }
}

/// Where a revision's row goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Current,
    Older,
    Archived,
}

/// The libraries for `catalog`, written to `folder`. The folder is only
/// used to point footprints at their 3D models.
pub fn plan<'a>(catalog: &'a CatalogReadback, folder: &Path) -> Plan<'a> {
    let mut plan = Plan::default();
    let records = Records::new(catalog);
    let mut paths = UsedPaths::default();
    plan_symbols(&records, &mut plan, &mut paths);
    plan_footprints(&records, folder, &mut plan, &mut paths);
    plan_tables(&records, &mut plan);
    plan
}

/// The records `plan` reads, looked up by ID.
struct Records<'a> {
    catalog: &'a CatalogReadback,
    blobs: BTreeMap<&'a str, &'a StoredBlob>,
    /// CAD item revisions by (CAD item, number).
    cad_revisions: BTreeMap<(&'a str, u32), &'a StoredCadItemRevision>,
}

impl<'a> Records<'a> {
    fn new(catalog: &'a CatalogReadback) -> Self {
        Self {
            catalog,
            blobs: catalog
                .blobs
                .iter()
                .map(|blob| (blob.id.as_str(), blob))
                .collect(),
            cad_revisions: catalog
                .cad_item_revisions
                .iter()
                .map(|revision| ((revision.cad_item.as_str(), revision.number), revision))
                .collect(),
        }
    }

    /// The CAD items of `kind`, by key, each with its revisions by number
    /// and its key split into library and name.
    fn cad_items(&self, kind: CadItemKind) -> Vec<CadItemRevisions<'a>> {
        let mut items: Vec<CadItemRevisions<'a>> = self
            .catalog
            .cad_items
            .iter()
            .filter(|item| item.kind == kind.as_str())
            .map(|item| {
                let mut revisions: Vec<&StoredCadItemRevision> = self
                    .catalog
                    .cad_item_revisions
                    .iter()
                    .filter(|revision| revision.cad_item == item.id)
                    .collect();
                revisions.sort_by_key(|revision| revision.number);
                CadItemRevisions {
                    key: &item.key,
                    revisions,
                }
            })
            .collect();
        items.sort_by(|a, b| a.key.cmp(b.key));
        items
    }
}

struct CadItemRevisions<'a> {
    key: &'a str,
    revisions: Vec<&'a StoredCadItemRevision>,
}

/// Relative paths written so far, compared as Windows compares them
/// (ignoring case), so two files never land on one.
#[derive(Default)]
struct UsedPaths(BTreeSet<String>);

impl UsedPaths {
    fn take(&mut self, path: &str) -> bool {
        self.0.insert(path.to_lowercase())
    }
}

/// A CAD item revision's details, or why they cannot be used.
fn details_of(revision: &StoredCadItemRevision, label: &str) -> Result<CadItemDetails, String> {
    serde_json::from_str(&revision.snapshot)
        .map_err(|_| format!("The {label} could not be read, so it was left out."))
}

fn plan_symbols<'a>(records: &Records<'a>, plan: &mut Plan<'a>, paths: &mut UsedPaths) {
    let mut libraries: BTreeMap<String, SymbolLibraryPlan<'a>> = BTreeMap::new();
    for item in records.cad_items(CadItemKind::Symbol) {
        let Some((library, name)) = item.key.split_once(':') else {
            plan.problems.push(format!(
                "The symbol {} has no library name, so it was left out.",
                item.key
            ));
            continue;
        };
        if !libraries.contains_key(library) {
            let path = format!("symbols/{}.kicad_sym", file_stem(library));
            if !paths.take(&path) {
                plan.problems.push(format!(
                    "The symbol library {library} would use the same file as another library, so it was left out."
                ));
                continue;
            }
            libraries.insert(
                library.to_owned(),
                SymbolLibraryPlan {
                    nickname: library.to_owned(),
                    path,
                    entries: Vec::new(),
                },
            );
        }
        let Some(target) = libraries.get_mut(library) else {
            continue;
        };
        for revision in &item.revisions {
            let label = format!("symbol {} revision {}", item.key, revision.number);
            let details = match details_of(revision, &label) {
                Ok(details) => details,
                Err(problem) => {
                    plan.problems.push(problem);
                    continue;
                }
            };
            let Some(blob) = records.blobs.get(revision.blob.as_str()) else {
                plan.problems.push(format!(
                    "The team catalog has no file for the {label}, so it was left out."
                ));
                continue;
            };
            if details.format != "kicad-symbol-entry" {
                plan.problems.push(format!(
                    "The {label} is not a KiCad symbol, so it was left out."
                ));
                continue;
            }
            target.entries.push(SymbolEntryPlan {
                name: revision_name(name, revision.number),
                original: name.to_owned(),
                blob,
                version: details.format_version.clone(),
                label,
            });
        }
    }
    plan.symbol_libraries = libraries.into_values().collect();
}

fn plan_footprints<'a>(
    records: &Records<'a>,
    folder: &Path,
    plan: &mut Plan<'a>,
    paths: &mut UsedPaths,
) {
    let mut libraries: BTreeMap<String, FootprintLibraryPlan> = BTreeMap::new();
    let mut files = Vec::new();
    for item in records.cad_items(CadItemKind::Footprint) {
        let Some((library, name)) = item.key.split_once(':') else {
            plan.problems.push(format!(
                "The footprint {} has no library name, so it was left out.",
                item.key
            ));
            continue;
        };
        let pretty = format!("footprints/{}.pretty", file_stem(library));
        let shapes = format!("footprints/{}.3dshapes", file_stem(library));
        if !libraries.contains_key(library) {
            if !paths.take(&pretty) || !paths.take(&shapes) {
                plan.problems.push(format!(
                    "The footprint library {library} would use the same folder as another library, so it was left out."
                ));
                continue;
            }
            libraries.insert(
                library.to_owned(),
                FootprintLibraryPlan {
                    nickname: library.to_owned(),
                    path: pretty.clone(),
                },
            );
        }
        for revision in &item.revisions {
            let label = format!("footprint {} revision {}", item.key, revision.number);
            let details = match details_of(revision, &label) {
                Ok(details) => details,
                Err(problem) => {
                    plan.problems.push(problem);
                    continue;
                }
            };
            let Some(blob) = records.blobs.get(revision.blob.as_str()) else {
                plan.problems.push(format!(
                    "The team catalog has no file for the {label}, so it was left out."
                ));
                continue;
            };
            let revision_name = revision_name(name, revision.number);
            let file_name = format!("{revision_name}.kicad_mod");
            if let Some(why) = windows_refuses(&file_name) {
                plan.problems.push(format!(
                    "The {label} was left out: Windows refuses the file name {file_name} because {why}."
                ));
                continue;
            }
            let path = format!("{pretty}/{file_name}");
            if !paths.take(&path) {
                plan.problems.push(format!(
                    "The {label} would use the same file as another footprint, so it was left out."
                ));
                continue;
            }
            let mut links = Vec::new();
            let mut models = 0;
            for reference in &details.model_references {
                let ModelTarget::CadItemRevision {
                    cad_item, number, ..
                } = &reference.target
                else {
                    links.push(None);
                    continue;
                };
                let model = model_file(
                    records,
                    (cad_item, *number),
                    &revision_name,
                    models,
                    &shapes,
                    &label,
                );
                match model {
                    Ok((model_path, model_blob)) if paths.take(&model_path) => {
                        models += 1;
                        links.push(Some(escape_text(&absolute(folder, &model_path))));
                        files.push(FilePlan {
                            path: model_path,
                            blob: model_blob,
                            kind: FileKind::Model,
                            label: format!("3D model of the {label}"),
                        });
                    }
                    Ok(_) => {
                        plan.problems.push(format!(
                            "A 3D model of the {label} would use the same file as another, so its link was kept."
                        ));
                        links.push(None);
                    }
                    Err(problem) => {
                        plan.problems.push(problem);
                        links.push(None);
                    }
                }
            }
            files.push(FilePlan {
                path,
                blob,
                kind: FileKind::Footprint {
                    links,
                    references: details
                        .model_references
                        .iter()
                        .map(|reference| reference.reference.clone())
                        .collect(),
                },
                label,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    plan.footprint_libraries = libraries.into_values().collect();
    plan.files = files;
}

/// The path and blob of the 3D model revision `target` that the footprint
/// revision `footprint` uses as its model number `index` (from 0): named
/// like the footprint revision, with `_2`, `_3` and so on after the first.
fn model_file<'a>(
    records: &Records<'a>,
    target: (&str, u32),
    footprint: &str,
    index: usize,
    shapes: &str,
    label: &str,
) -> Result<(String, &'a StoredBlob), String> {
    let missing =
        || format!("A 3D model of the {label} is not in the team catalog, so its link was kept.");
    let revision = records.cad_revisions.get(&target).ok_or_else(missing)?;
    let blob = records
        .blobs
        .get(revision.blob.as_str())
        .ok_or_else(missing)?;
    let item = records
        .catalog
        .cad_item(&revision.cad_item)
        .ok_or_else(missing)?;
    let extension = item
        .key
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| {
            !extension.is_empty() && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .ok_or_else(|| {
            format!("A 3D model of the {label} has no file type in its name, so its link was kept.")
        })?;
    let stem = if index == 0 {
        footprint.to_owned()
    } else {
        format!("{footprint}_{}", index + 1)
    };
    Ok((format!("{shapes}/{}.{extension}", file_stem(&stem)), blob))
}

/// `relative` inside `folder`, written with `/` as KiCad accepts on every
/// system, without a `\\?\` prefix.
pub fn absolute(folder: &Path, relative: &str) -> String {
    let folder = folder.to_string_lossy().replace('\\', "/");
    let folder = folder.strip_prefix("//?/").unwrap_or(&folder);
    format!("{}/{relative}", folder.trim_end_matches('/'))
}

/// A category's table name: `category_` and its code, with every character
/// other than a lower-case letter or digit written as `_` and its hex code.
fn category_table(code: &str) -> String {
    let mut table = String::from("category_");
    for byte in code.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            table.push(byte as char);
        } else {
            table.push_str(&format!("_{byte:02x}"));
        }
    }
    table
}

/// A part's current revision: its newest revision that is not archived.
fn current_number(numbers: &[u32], archived: &BTreeSet<u32>) -> Option<u32> {
    numbers
        .iter()
        .copied()
        .filter(|number| !archived.contains(number))
        .max()
}

fn plan_tables(records: &Records<'_>, plan: &mut Plan<'_>) {
    let catalog = records.catalog;
    // The current revision of each category, by code.
    let mut categories: BTreeMap<String, CategoryDetails> = BTreeMap::new();
    let mut revisions: Vec<_> = catalog.category_revisions.iter().collect();
    revisions.sort_by(|a, b| a.category.cmp(&b.category).then(a.number.cmp(&b.number)));
    for revision in revisions {
        if let Ok(details) = serde_json::from_str::<CategoryDetails>(&revision.snapshot) {
            categories.insert(details.code.clone(), details);
        }
    }
    let mut parts: Vec<&StoredPart> = catalog.parts.iter().collect();
    parts.sort_by(|a, b| a.ipn.cmp(&b.ipn));
    // Rows by table, and the categories parts name but the Catalog lacks.
    let mut rows: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for part in parts {
        let Some(details) = catalog
            .part_details(&part.current_details)
            .and_then(|stored| serde_json::from_str::<PartDetails>(&stored.snapshot).ok())
        else {
            plan.problems.push(format!(
                "The details of {} could not be read, so it was left out.",
                part.ipn
            ));
            continue;
        };
        if !categories.contains_key(&details.category) {
            plan.problems.push(format!(
                "{} is in the category {}, which the team catalog does not have; it is listed under that code.",
                part.ipn, details.category
            ));
            categories.insert(
                details.category.clone(),
                CategoryDetails::starting(&details.category, Vec::new()),
            );
        }
        let category = &categories[&details.category];
        let mut part_revisions: Vec<(u32, PartRevision)> = Vec::new();
        for stored in catalog.part_revisions.iter().filter(|r| r.part == part.id) {
            match serde_json::from_str::<PartRevision>(&stored.snapshot) {
                Ok(revision) => part_revisions.push((stored.number, revision)),
                Err(_) => plan.problems.push(format!(
                    "Revision {} of {} could not be read, so it was left out.",
                    stored.number, part.ipn
                )),
            }
        }
        part_revisions.sort_by_key(|(number, _)| *number);
        let archived = catalog.archived(&part.id);
        let numbers: Vec<u32> = part_revisions.iter().map(|(number, _)| *number).collect();
        let current = current_number(&numbers, &archived);
        for (number, revision) in &part_revisions {
            let Some(symbol) = revision
                .cad_references
                .iter()
                .find(|reference| reference.role == CadReferenceRole::Symbol)
            else {
                // A part without a symbol, such as a bare board, cannot be
                // placed in a schematic.
                continue;
            };
            let state = if archived.contains(number) {
                State::Archived
            } else if Some(*number) == current {
                State::Current
            } else {
                State::Older
            };
            let table = match state {
                State::Current => category_table(&category.code),
                State::Older => OLDER_TABLE.to_owned(),
                State::Archived => ARCHIVED_TABLE.to_owned(),
            };
            let mut footprints: Vec<&CadReference> = revision
                .cad_references
                .iter()
                .filter(|reference| reference.role == CadReferenceRole::Footprint)
                .collect();
            footprints.sort_by_key(|reference| reference.ordinal);
            let footprints: Vec<String> = footprints
                .into_iter()
                .map(|reference| link(records, reference, &mut plan.problems))
                .collect();
            let manufacturer = details.manufacturer_parts.first();
            let mut values = vec![
                revision_name(&part.ipn, *number),
                part.ipn.clone(),
                number.to_string(),
                link(records, symbol, &mut plan.problems),
                footprints.join(";"),
                value_of(category, &details),
                manufacturer
                    .map(|m| m.manufacturer.clone())
                    .unwrap_or_default(),
                manufacturer.map(|m| m.mpn.clone()).unwrap_or_default(),
                details.field("Description").unwrap_or_default().to_owned(),
                details.field("Datasheet").unwrap_or_default().to_owned(),
                if revision.excluded_from_bom { "1" } else { "0" }.to_owned(),
            ];
            if state == State::Current {
                for field in extra_fields(category) {
                    values.push(details.field(&field.column).unwrap_or_default().to_owned());
                }
            }
            rows.entry(table).or_default().push(Row { values });
        }
    }
    let names = group_names(&categories);
    for (code, category) in &categories {
        let table = category_table(code);
        let mut table_rows = rows.remove(&table).unwrap_or_default();
        table_rows.sort_by(|a, b| a.entry().cmp(b.entry()));
        plan.tables.push(TablePlan {
            group: Group::Category,
            name: names[code].clone(),
            listed: !table_rows.is_empty(),
            table,
            extra: extra_fields(category),
            rows: table_rows,
        });
    }
    for (group, table, name) in [
        (Group::Older, OLDER_TABLE, OLDER_GROUP),
        (Group::Archived, ARCHIVED_TABLE, ARCHIVED_GROUP),
    ] {
        let mut table_rows = rows.remove(table).unwrap_or_default();
        table_rows.sort_by(|a, b| a.entry().cmp(b.entry()));
        plan.tables.push(TablePlan {
            group,
            table: table.to_owned(),
            name: name.to_owned(),
            listed: true,
            extra: Vec::new(),
            rows: table_rows,
        });
    }
}

/// What a symbol or footprint reference becomes in a row: an exact CAD
/// item revision becomes its revision name in its library, such as
/// `g-res:R_US_v2`; a KiCad library item, or a reference that did not
/// resolve, stays as written, so KiCad finds it in its own libraries.
fn link(records: &Records<'_>, reference: &CadReference, problems: &mut Vec<String>) -> String {
    let CadReferenceTarget::CadItemRevision {
        cad_item, number, ..
    } = &reference.target
    else {
        return reference.reference.clone();
    };
    match records
        .catalog
        .cad_item(cad_item)
        .and_then(|item| item.key.split_once(':'))
    {
        Some((library, name)) => format!("{library}:{}", revision_name(name, *number)),
        None => {
            problems.push(format!(
                "{} names a CAD item the team catalog does not have; it is linked as written.",
                reference.reference
            ));
            reference.reference.clone()
        }
    }
}

/// The Value field: the category's value field (Resistance for variable
/// resistors, Color for optoelectronics, as `#gplm.kicad_dbl` maps them,
/// and Value otherwise), else the main MPN, else the IPN.
fn value_of(category: &CategoryDetails, details: &PartDetails) -> String {
    let field = match category.code.as_str() {
        "rvr" => "Resistance",
        "opt" => "Color",
        _ => "Value",
    };
    [
        details.field(field).unwrap_or_default(),
        details
            .manufacturer_parts
            .first()
            .map(|part| part.mpn.as_str())
            .unwrap_or_default(),
    ]
    .into_iter()
    .find(|value| !value.trim().is_empty())
    .unwrap_or(&details.ipn)
    .to_owned()
}

/// A category's fields that are not fixed columns, in order. SQLite
/// compares column names ignoring case, so a field whose name matches an
/// earlier column that way is left out.
pub fn extra_fields(category: &CategoryDetails) -> Vec<ExtraField> {
    let mut taken: BTreeSet<String> = FIXED_COLUMNS
        .iter()
        .map(|column| column.to_lowercase())
        .collect();
    category
        .fields
        .iter()
        .filter(|field| !field.trim().is_empty() && taken.insert(field.to_lowercase()))
        .map(|field| ExtraField {
            column: field.clone(),
            field: match field.strip_prefix("Sim_") {
                // KiCad's simulator fields are named Sim.Library and so on.
                Some(rest) => format!("Sim.{rest}"),
                None => field.clone(),
            },
            shown_on_add: SHOWN_ON_ADD.contains(&(category.code.as_str(), field.as_str())),
        })
        .collect()
}

/// The group each category shows as in KiCad's chooser: its name, without
/// `/` (KiCad's separator), with its code added when two groups would
/// otherwise share a name.
fn group_names(categories: &BTreeMap<String, CategoryDetails>) -> BTreeMap<String, String> {
    let plain = |category: &CategoryDetails| {
        let name = category.name.replace('/', "-").trim().to_owned();
        if name.is_empty() {
            category.code.clone()
        } else {
            name
        }
    };
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for name in [OLDER_GROUP, ARCHIVED_GROUP] {
        *counts.entry(name.to_lowercase()).or_default() += 1;
    }
    for category in categories.values() {
        *counts.entry(plain(category).to_lowercase()).or_default() += 1;
    }
    categories
        .iter()
        .map(|(code, category)| {
            let name = plain(category);
            let name = if counts[&name.to_lowercase()] > 1 {
                format!("{name} ({code})")
            } else {
                name
            };
            (code.clone(), name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_tables_use_only_safe_characters() {
        assert_eq!(category_table("res"), "category_res");
        assert_eq!(category_table("Res"), "category__52es");
        assert_eq!(category_table("my.cat-1_x"), "category_my_2ecat_2d1_5fx");
        assert_ne!(category_table("a_2e"), category_table("a.")); // _ is escaped too
    }

    #[test]
    fn the_current_revision_is_the_newest_not_archived() {
        let archived: BTreeSet<u32> = [3].into();
        assert_eq!(current_number(&[1, 2, 3], &archived), Some(2));
        assert_eq!(current_number(&[1], &BTreeSet::new()), Some(1));
        assert_eq!(current_number(&[], &BTreeSet::new()), None);
    }

    #[test]
    fn links_never_keep_a_verbatim_prefix() {
        assert_eq!(
            absolute(
                Path::new(r"\\?\C:\Data\kicad-libraries\"),
                "footprints/x.step"
            ),
            "C:/Data/kicad-libraries/footprints/x.step"
        );
    }
}

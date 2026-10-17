//! The generated KiCad libraries compared with the CSV catalog's own KiCad
//! library: `database/parts.sqlite` and `database/#gplm.kicad_dbl`, which
//! gitplm built from the CSV files. The team never used that library in
//! KiCad, but it is the best record of what the CSV catalog meant KiCad to
//! get. So every row of `parts.sqlite` must be the generated entry
//! `<IPN>_v1`, with the same values, and with its Symbol and Footprint
//! pointing at generated items that are exactly the CSV catalog's own.
//!
//! Every difference is listed field by field. Each one that is meant is
//! matched by one rule in [`intended::INTENDED`], which says why; one that
//! no rule matches fails the proof. [`load`] reads both libraries from
//! their files.
//!
//! What is compared:
//!
//! - **Groups**: every table of `parts.sqlite` is a category table of the
//!   generated database, and every group the `.kicad_dbl` files list.
//! - **Rows**: every row is the entry `<IPN>_v1` in its category's table,
//!   and each column of `parts.sqlite` has the same value in the generated
//!   table's column of the same name. Columns the generated table adds are
//!   listed once per table.
//! - **Symbol and Footprint**: each `;`-separated choice. A symbol or
//!   footprint from the CSV catalog's own libraries must point at a
//!   generated revision of it, such as `MA-res-20241209:R_US_v1`, whose
//!   bytes are the CSV catalog's own: the symbol renamed back, or the
//!   footprint with its 3D model links put back. Any other choice, such as
//!   one of KiCad's own symbols, must be written exactly as the CSV says.
//! - **KiCad fields**: the fields KiCad shows on a placed part, worked out
//!   through each `.kicad_dbl`'s field list. This is where the CSV
//!   catalog's library shows an oscillator's Frequency as its Value.
//!
//! Field visibility (shown on the schematic, shown in the chooser) is not
//! compared: the generated library chose its own (kicad-parts-f397).

pub mod intended;
pub mod load;

use catalog::csv_import::{ColumnRoute, route_column};
use kicad_integration::library_file::{rename_symbol_entry, replace_model_references};
use serde::Serialize;
use std::collections::BTreeMap;

/// One row: each column's value, with SQLite's NULL as empty text.
pub type Row = BTreeMap<String, String>;

/// One table of a database library, and the group KiCad shows for it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// Its columns, in order.
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    /// The `.kicad_dbl` lists it; `None` when it does not.
    pub group: Option<Group>,
}

/// How a `.kicad_dbl` shows a table in KiCad.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    /// The column KiCad finds an entry by: `IPN` in the CSV catalog's,
    /// `Entry` in the generated one.
    pub key: String,
    /// (column, KiCad field name), in order. A later field with the same
    /// name replaces an earlier one, as in KiCad.
    pub fields: Vec<(String, String)>,
}

/// A database library: its tables by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatabaseLibrary {
    pub tables: BTreeMap<String, Table>,
}

/// The CSV catalog's side: its KiCad library, and the bytes of each symbol
/// and footprint its rows take from its own libraries, by the reference
/// rows use (such as `g-res:R_US`).
#[derive(Clone, Debug, Default)]
pub struct CsvSide {
    pub library: DatabaseLibrary,
    pub symbols: BTreeMap<String, Vec<u8>>,
    pub footprints: BTreeMap<String, Vec<u8>>,
}

/// The generated side: the database library, and every generated symbol
/// and footprint by its link (such as `MA-res-20241209:R_US_v1`), with its
/// bytes as written.
#[derive(Clone, Debug, Default)]
pub struct GeneratedSide {
    pub library: DatabaseLibrary,
    pub symbols: BTreeMap<String, Vec<u8>>,
    pub footprints: BTreeMap<String, Vec<u8>>,
}

/// One difference between the two libraries.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Difference {
    /// A generated group with no group in the CSV catalog's `.kicad_dbl`.
    GroupAdded {
        table: String,
        rows: usize,
        /// The `parts.sqlite` table it holds, if any, and that table's rows.
        csv_table: Option<String>,
        csv_rows: usize,
        /// That table has a Symbol column.
        csv_has_symbols: bool,
    },
    /// A table of `parts.sqlite`, or a group of the CSV catalog's
    /// `.kicad_dbl`, that the generated library lacks.
    GroupMissing { table: String },
    /// A row of `parts.sqlite` with no generated entry.
    EntryMissing {
        table: String,
        ipn: String,
        /// The row's Symbol, empty when it has none.
        symbol: String,
    },
    /// A column of `parts.sqlite` that the generated table lacks.
    ColumnLeftOut {
        table: String,
        ipn: String,
        column: String,
        value: String,
    },
    /// A column the generated table adds.
    ColumnAdded { table: String, column: String },
    /// A column with another value.
    ValueDiffers {
        table: String,
        ipn: String,
        column: String,
        csv: String,
        generated: String,
        /// The `parts.sqlite` row, for the rules that compare with it.
        #[serde(skip)]
        row: Row,
    },
    /// A Symbol or Footprint choice that does not point at the matching
    /// generated item.
    LinkDiffers {
        table: String,
        ipn: String,
        column: String,
        csv: String,
        generated: String,
        why: String,
    },
    /// A field KiCad shows on a placed part, with another value or only on
    /// one side. `None` is a field the library does not give.
    FieldDiffers {
        table: String,
        ipn: String,
        field: String,
        csv: Option<String>,
        generated: Option<String>,
        /// The `parts.sqlite` row, for the rules that compare with it.
        #[serde(skip)]
        row: Row,
    },
}

impl Difference {
    /// The place, such as `res RES-0000-0470 Value`, for people.
    pub fn place(&self) -> String {
        match self {
            Self::GroupAdded { table, .. } | Self::GroupMissing { table } => table.clone(),
            Self::EntryMissing { table, ipn, .. } => format!("{table} {ipn}"),
            Self::ColumnAdded { table, column } => format!("{table} column {column}"),
            Self::ColumnLeftOut {
                table, ipn, column, ..
            }
            | Self::ValueDiffers {
                table, ipn, column, ..
            }
            | Self::LinkDiffers {
                table, ipn, column, ..
            } => format!("{table} {ipn} {column}"),
            Self::FieldDiffers {
                table, ipn, field, ..
            } => format!("{table} {ipn} field {field}"),
        }
    }

    /// What differs, in a sentence.
    pub fn describe(&self) -> String {
        let place = self.place();
        match self {
            Self::GroupAdded { rows, .. } => {
                format!(
                    "{place}: a group the CSV catalog's library does not list ({rows} entries)."
                )
            }
            Self::GroupMissing { .. } => format!("{place}: no such group is generated."),
            Self::EntryMissing { .. } => format!("{place}: no entry is generated."),
            Self::ColumnLeftOut { value, .. } => {
                format!("{place}: not in the generated library (the CSV says {value:?}).")
            }
            Self::ColumnAdded { .. } => format!("{place}: only in the generated library."),
            Self::ValueDiffers { csv, generated, .. } => {
                format!("{place}: the CSV says {csv:?}, the generated library {generated:?}.")
            }
            Self::LinkDiffers {
                csv,
                generated,
                why,
                ..
            } => {
                format!("{place}: the CSV says {csv:?}, the generated library {generated:?}: {why}")
            }
            Self::FieldDiffers { csv, generated, .. } => format!(
                "{place}: KiCad shows {} from the CSV catalog's library and {} from the generated one.",
                shown(csv),
                shown(generated)
            ),
        }
    }
}

fn shown(value: &Option<String>) -> String {
    match value {
        Some(value) => format!("{value:?}"),
        None => "no such field".to_owned(),
    }
}

/// A difference and the reason it is meant, if a rule explains it.
#[derive(Clone, Debug, Serialize)]
pub struct Found {
    pub difference: Difference,
    /// The rule that explains it, by name, and its reason.
    pub rule: Option<&'static str>,
    pub reason: Option<&'static str>,
}

/// The whole comparison.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Comparison {
    /// Rows of `parts.sqlite`, and how many have their generated entry.
    pub csv_rows: usize,
    pub entries_found: usize,
    /// Groups each `.kicad_dbl` lists.
    pub csv_groups: usize,
    pub generated_groups: usize,
    /// Every difference, in a fixed order.
    pub differences: Vec<Found>,
}

impl Comparison {
    /// The differences no rule explains. Each fails the proof.
    pub fn unexplained(&self) -> impl Iterator<Item = &Found> {
        self.differences.iter().filter(|found| found.rule.is_none())
    }
}

/// The generated table of the category a `parts.sqlite` table holds, such
/// as `category_res` for `res` (see `crates/kicad-libraries/src/plan.rs`).
pub fn category_table(csv_table: &str) -> String {
    format!("category_{csv_table}")
}

/// The generated entry of the first revision of `ipn`.
pub fn first_entry(ipn: &str) -> String {
    format!("{ipn}_v1")
}

/// Compares the two sides and explains each difference it can.
pub fn compare(csv: &CsvSide, generated: &GeneratedSide) -> Comparison {
    let mut comparison = Comparison::default();
    let mut differences = Vec::new();
    let csv_tables = &csv.library.tables;
    let generated_tables = &generated.library.tables;
    comparison.csv_groups = csv_tables.values().filter(|t| t.group.is_some()).count();
    comparison.generated_groups = generated_tables
        .values()
        .filter(|t| t.group.is_some())
        .count();

    for (name, table) in csv_tables {
        let Some(mine) = generated_tables.get(&category_table(name)) else {
            differences.push(Difference::GroupMissing {
                table: name.clone(),
            });
            continue;
        };
        if table.group.is_some() && mine.group.is_none() {
            differences.push(Difference::GroupMissing {
                table: category_table(name),
            });
        }
        for column in &mine.columns {
            if !table.columns.contains(column) {
                differences.push(Difference::ColumnAdded {
                    table: name.clone(),
                    column: column.clone(),
                });
            }
        }
        let entries: BTreeMap<&str, &Row> = mine
            .rows
            .iter()
            .filter_map(|row| row.get("Entry").map(|entry| (entry.as_str(), row)))
            .collect();
        for row in &table.rows {
            comparison.csv_rows += 1;
            let ipn = row.get("IPN").cloned().unwrap_or_default();
            let Some(entry) = entries.get(first_entry(&ipn).as_str()) else {
                differences.push(Difference::EntryMissing {
                    table: name.clone(),
                    ipn,
                    symbol: row.get("Symbol").cloned().unwrap_or_default(),
                });
                continue;
            };
            comparison.entries_found += 1;
            compare_row(name, &ipn, row, entry, csv, generated, &mut differences);
            if let (Some(csv_group), Some(group)) = (&table.group, &mine.group) {
                compare_fields(name, &ipn, row, entry, csv_group, group, &mut differences);
            }
        }
    }
    for (name, table) in generated_tables {
        let csv_table = name
            .strip_prefix("category_")
            .filter(|code| csv_tables.contains_key(*code));
        let listed_by_csv = csv_table
            .and_then(|code| csv_tables[code].group.as_ref())
            .is_some();
        if table.group.is_some() && !listed_by_csv {
            differences.push(Difference::GroupAdded {
                table: name.clone(),
                rows: table.rows.len(),
                csv_table: csv_table.map(str::to_owned),
                csv_rows: csv_table.map_or(0, |code| csv_tables[code].rows.len()),
                csv_has_symbols: csv_table
                    .is_some_and(|code| csv_tables[code].columns.iter().any(|c| c == "Symbol")),
            });
        }
    }
    comparison.differences = differences
        .into_iter()
        .map(|difference| match intended::explain(&difference) {
            Some(rule) => Found {
                difference,
                rule: Some(rule.name),
                reason: Some(rule.reason),
            },
            None => Found {
                difference,
                rule: None,
                reason: None,
            },
        })
        .collect();
    comparison
}

/// Compares one row's columns, and its Symbol and Footprint choices.
fn compare_row(
    table: &str,
    ipn: &str,
    row: &Row,
    entry: &Row,
    csv: &CsvSide,
    generated: &GeneratedSide,
    differences: &mut Vec<Difference>,
) {
    for (column, value) in row {
        if column == "IPN" && entry.get("IPN") == Some(value) {
            continue;
        }
        let Some(mine) = entry.get(column) else {
            differences.push(Difference::ColumnLeftOut {
                table: table.to_owned(),
                ipn: ipn.to_owned(),
                column: column.clone(),
                value: value.clone(),
            });
            continue;
        };
        if route_column(column) == ColumnRoute::Cad {
            if let Some(why) = link_problem(column, value, mine, csv, generated) {
                differences.push(Difference::LinkDiffers {
                    table: table.to_owned(),
                    ipn: ipn.to_owned(),
                    column: column.clone(),
                    csv: value.clone(),
                    generated: mine.clone(),
                    why,
                });
            }
        } else if mine != value {
            differences.push(Difference::ValueDiffers {
                table: table.to_owned(),
                ipn: ipn.to_owned(),
                column: column.clone(),
                csv: value.clone(),
                generated: mine.clone(),
                row: row.clone(),
            });
        }
    }
}

/// Why the generated Symbol or Footprint `mine` does not match the CSV's
/// `theirs`, choice by choice, or `None` when it does.
fn link_problem(
    column: &str,
    theirs: &str,
    mine: &str,
    csv: &CsvSide,
    generated: &GeneratedSide,
) -> Option<String> {
    let split = |text: &str| -> Vec<String> {
        text.split(';')
            .map(str::trim)
            .filter(|choice| !choice.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (theirs, mine) = (split(theirs), split(mine));
    if theirs.len() != mine.len() {
        return Some(format!(
            "the CSV has {} choices and the generated library {}.",
            theirs.len(),
            mine.len()
        ));
    }
    let symbol = column == "Symbol";
    for (their, my) in theirs.iter().zip(&mine) {
        let source = if symbol {
            csv.symbols.get(their)
        } else {
            csv.footprints.get(their)
        };
        let Some(source) = source else {
            // Not one of the CSV catalog's own: KiCad finds it in its own
            // libraries, so it must be written as the CSV writes it.
            if their != my {
                return Some(format!("{their} must stay as the CSV writes it."));
            }
            continue;
        };
        let why = if symbol {
            same_symbol(their, my, source, generated)
        } else {
            same_footprint(their, my, source, generated)
        };
        if let Err(why) = why {
            return Some(why);
        }
    }
    None
}

/// The item name after the colon of a link or reference.
fn item_name(link: &str) -> &str {
    link.split_once(':').map_or(link, |(_, name)| name)
}

/// Whether `link` names a revision of `reference`: the same name with
/// `_v` and a number after it, such as `R_US_v2` for `g-res:R_US`.
fn is_revision_of(link: &str, reference: &str) -> bool {
    item_name(link)
        .strip_prefix(item_name(reference))
        .and_then(|rest| rest.strip_prefix("_v"))
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

/// The generated symbol `link` is the CSV catalog's symbol `reference`,
/// whose bytes are `source`, renamed.
fn same_symbol(
    reference: &str,
    link: &str,
    source: &[u8],
    generated: &GeneratedSide,
) -> Result<(), String> {
    if !is_revision_of(link, reference) {
        return Err(format!("{link} is not named as a revision of {reference}."));
    }
    let bytes = generated
        .symbols
        .get(link)
        .ok_or_else(|| format!("{link} is not in the generated symbol libraries."))?;
    let renamed = rename_symbol_entry(bytes, item_name(link), item_name(reference))
        .map_err(|error| format!("{link} could not be read: {error}"))?;
    if renamed != source {
        return Err(format!(
            "{link} is not the CSV catalog's symbol {reference}, byte for byte."
        ));
    }
    Ok(())
}

/// The generated footprint `link` is the CSV catalog's footprint
/// `reference`, whose bytes are `source`, with only its 3D model links
/// changed.
fn same_footprint(
    reference: &str,
    link: &str,
    source: &[u8],
    generated: &GeneratedSide,
) -> Result<(), String> {
    if !is_revision_of(link, reference) {
        return Err(format!("{link} is not named as a revision of {reference}."));
    }
    let bytes = generated
        .footprints
        .get(link)
        .ok_or_else(|| format!("{link} is not in the generated footprint libraries."))?;
    let (Ok(text), Ok(source)) = (std::str::from_utf8(bytes), std::str::from_utf8(source)) else {
        return Err(format!("{link} or {reference} is not UTF-8 text."));
    };
    let originals: Vec<Option<String>> = kicad_integration::library_file::model_references(source)
        .into_iter()
        .map(Some)
        .collect();
    let (restored, links) = replace_model_references(text, &originals);
    if links.len() != originals.len() || restored != source {
        return Err(format!(
            "{link} is not the CSV catalog's footprint {reference} with only its 3D model links changed."
        ));
    }
    Ok(())
}

/// What KiCad shows on a placed part from `row` through `group`'s field
/// list: each field name with its value, a later field of the same name
/// replacing an earlier one. A field whose column the table lacks gives
/// nothing.
fn kicad_fields(row: &Row, group: &Group) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for (column, name) in &group.fields {
        if let Some(value) = row.get(column) {
            fields.insert(name.clone(), value.clone());
        }
    }
    fields
}

/// Compares what KiCad shows on a placed part from each library. An empty
/// field and a missing one look the same in KiCad, so they count as the
/// same.
fn compare_fields(
    table: &str,
    ipn: &str,
    row: &Row,
    entry: &Row,
    csv_group: &Group,
    group: &Group,
    differences: &mut Vec<Difference>,
) {
    let theirs = kicad_fields(row, csv_group);
    let mine = kicad_fields(entry, group);
    let mut names: Vec<&String> = theirs.keys().chain(mine.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let (their, my) = (theirs.get(name), mine.get(name));
        let blank = |value: Option<&String>| value.is_none_or(|value| value.is_empty());
        if their == my || (blank(their) && blank(my)) {
            continue;
        }
        differences.push(Difference::FieldDiffers {
            table: table.to_owned(),
            ipn: ipn.to_owned(),
            field: name.clone(),
            csv: their.cloned(),
            generated: my.cloned(),
            row: row.clone(),
        });
    }
}

#[cfg(test)]
mod tests;

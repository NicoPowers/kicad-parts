//! The generated KiCad libraries compared with the CSV catalog's own KiCad
//! library: `database/parts.sqlite` and `database/#gplm.kicad_dbl`, which
//! gitplm built from the CSV files. The team never used that library in
//! KiCad, but it is the best record of what the CSV catalog meant KiCad to
//! get. So every CSV row must be the generated entry `<IPN>_v1`, with the
//! same values, and with its Symbol and Footprint pointing at generated
//! items that are exactly the CSV catalog's own.
//!
//! `parts.sqlite` is a frozen copy: nothing can build it again. It was
//! made from the CSV files one way (one table per file, every value as
//! text, rows in file order), so the comparison builds the same tables from
//! today's CSV files and compares the generated library with those. It
//! then compares those tables with the frozen copy: each place where the
//! frozen copy is older than the CSV files must be one the CSV catalog
//! fixture records, or the proof fails. Today they are the same.
//!
//! Every difference is listed field by field. Each one that is meant is
//! matched by one rule in [`intended::INTENDED`], which says why; one that
//! no rule matches fails the proof. [`load`] reads both libraries from
//! their files.
//!
//! What is compared:
//!
//! - **Groups**: every CSV file's table is a category table of the
//!   generated database, and every group each `.kicad_dbl` lists. Each
//!   group's key, Symbol and Footprint columns and Exclude from BOM
//!   setting are compared too.
//! - **Rows**: every row is the entry `<IPN>_v1` in its category's table,
//!   and each column has the same value in the generated table's column of
//!   the same name. Every generated entry must be one of those: a stray
//!   one, in any table, is a difference. Each entry's Revision must be the
//!   number in its name, and its Exclude from BOM must be what the CSV
//!   catalog's symbol and footprints say. Columns the generated table adds
//!   are listed once per table.
//! - **Symbol and Footprint**: each `;`-separated choice. A symbol or
//!   footprint from the CSV catalog's own libraries must point at a
//!   generated revision of it, such as `MA-res-20241209:R_US_v1`, whose
//!   bytes are the CSV catalog's own: the symbol renamed back, or the
//!   footprint with its 3D model links put back. Each 3D model link the
//!   generated footprint has must lead, from the footprint's folder, to a
//!   file with the bytes of the CSV catalog's model; a link that never led
//!   to a file must stay as written. Any other choice, such as one of
//!   KiCad's own symbols, must be written exactly as the CSV says.
//! - **KiCad fields**: the fields KiCad shows on a placed part, worked out
//!   through each `.kicad_dbl`'s field list. This is where the CSV
//!   catalog's library shows an oscillator's Frequency as its Value.
//!
//! Field visibility (shown on the schematic, shown in the chooser) is not
//! compared: the generated library chose its own (kicad-parts-f397).

pub mod intended;
pub mod load;

use catalog::csv_import::{ColumnRoute, route_column};
use kicad_integration::library_file::{
    footprint_says_excluded_from_bom, model_references, rename_symbol_entry,
    replace_model_references, symbol_entries_say_not_in_bom,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

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
    /// The columns KiCad reads the symbol and the footprints from.
    pub symbols: String,
    pub footprints: String,
    /// The column KiCad reads Exclude from BOM from, if any.
    pub exclude_from_bom: Option<String>,
    /// (column, KiCad field name), in order. A later field with the same
    /// name replaces an earlier one, as in KiCad.
    pub fields: Vec<(String, String)>,
}

impl Group {
    /// How the CSV catalog's own `.kicad_dbl` lays out every group it
    /// lists, for groups it does not list.
    fn csv_catalog_layout() -> Self {
        Self {
            key: "IPN".to_owned(),
            symbols: "Symbol".to_owned(),
            footprints: "Footprint".to_owned(),
            exclude_from_bom: None,
            fields: Vec::new(),
        }
    }
}

/// A database library: its tables by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatabaseLibrary {
    pub tables: BTreeMap<String, Table>,
}

/// A footprint of the CSV catalog's own libraries, and its 3D model links
/// in file order.
#[derive(Clone, Debug, Default)]
pub struct CsvFootprint {
    pub bytes: Vec<u8>,
    pub models: Vec<ExpectedModel>,
}

/// A 3D model link of a CSV catalog footprint, and what the generated copy
/// of it must hold.
#[derive(Clone, Debug, Default)]
pub struct ExpectedModel {
    /// As the CSV catalog's footprint writes it.
    pub link: String,
    /// The SHA-256 of the model file the link leads to in the CSV catalog,
    /// as the CSV catalog fixture records it. `None` when it leads to no
    /// file, or to an empty one: then the generated footprint keeps the
    /// link as written.
    pub sha256: Option<String>,
}

/// What the CSV catalog fixture records about `parts.sqlite` being older
/// than today's CSV files.
#[derive(Clone, Debug, Default)]
pub struct FixtureNotes {
    /// Cells that differ, by (table, IPN, column).
    pub cells: BTreeSet<(String, String, String)>,
    /// Tables whose rows, columns or very existence differ, so every place
    /// in them may differ.
    pub tables: BTreeSet<String>,
}

/// The CSV catalog's side.
#[derive(Clone, Debug, Default)]
pub struct CsvSide {
    /// Its KiCad library as built from today's CSV files, with the groups
    /// of `#gplm.kicad_dbl`.
    pub library: DatabaseLibrary,
    /// `parts.sqlite` as frozen, by table.
    pub frozen: BTreeMap<String, Table>,
    pub notes: FixtureNotes,
    /// The bytes of each symbol and footprint its rows take from its own
    /// libraries, by the reference rows use (such as `g-res:R_US`).
    pub symbols: BTreeMap<String, Vec<u8>>,
    pub footprints: BTreeMap<String, CsvFootprint>,
}

/// A generated footprint file.
#[derive(Clone, Debug, Default)]
pub struct GeneratedFootprint {
    pub bytes: Vec<u8>,
    /// Its library's folder, from the team folder, such as
    /// `footprints/MA-Resistor_SMD.pretty`.
    pub folder: String,
}

/// The generated side.
#[derive(Clone, Debug, Default)]
pub struct GeneratedSide {
    pub library: DatabaseLibrary,
    /// Every generated symbol by its link (such as
    /// `MA-res-20241209:R_US_v1`), with its bytes as written.
    pub symbols: BTreeMap<String, Vec<u8>>,
    /// Every generated footprint by its link.
    pub footprints: BTreeMap<String, GeneratedFootprint>,
    /// The SHA-256 of every file in the team folder, by path from it.
    pub files: BTreeMap<String, String>,
}

/// One difference between the two libraries.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Difference {
    /// `parts.sqlite` differs from today's CSV files at `place`.
    PartsSqliteIsOlder {
        place: String,
        frozen: Option<String>,
        csv: Option<String>,
        /// The CSV catalog fixture records it.
        recorded: bool,
    },
    /// A generated group with no group in the CSV catalog's `.kicad_dbl`.
    GroupAdded {
        table: String,
        rows: usize,
        /// The CSV file's table it holds, if any, and that table's rows.
        csv_table: Option<String>,
        csv_rows: usize,
        /// That table has a Symbol column.
        csv_has_symbols: bool,
    },
    /// A CSV file's table, or a group of the CSV catalog's `.kicad_dbl`,
    /// that the generated library lacks.
    GroupMissing { table: String },
    /// A group setting of the `.kicad_dbl`, such as its key column.
    LayoutDiffers {
        table: String,
        setting: String,
        csv: String,
        generated: String,
    },
    /// A CSV row with no generated entry.
    EntryMissing {
        table: String,
        ipn: String,
        /// The row's Symbol, empty when it has none.
        symbol: String,
    },
    /// A generated entry that is no CSV row's.
    EntryAdded { table: String, entry: String },
    /// A column of the CSV file that the generated table lacks.
    ColumnLeftOut {
        table: String,
        ipn: String,
        column: String,
        value: String,
    },
    /// A column the generated table adds.
    ColumnAdded {
        table: String,
        column: String,
        /// Every generated row leaves it empty.
        empty: bool,
    },
    /// A column with another value.
    ValueDiffers {
        table: String,
        ipn: String,
        column: String,
        csv: String,
        generated: String,
        /// The CSV row, for the rules that compare with it.
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
        /// The generated entry, such as `RES-0000-0470_v1`.
        entry: String,
        field: String,
        csv: Option<String>,
        generated: Option<String>,
        /// The CSV row, for the rules that compare with it.
        #[serde(skip)]
        row: Row,
    },
}

impl Difference {
    /// The place, such as `res RES-0000-0470 Value`, for people.
    pub fn place(&self) -> String {
        match self {
            Self::PartsSqliteIsOlder { place, .. } => place.clone(),
            Self::GroupAdded { table, .. } | Self::GroupMissing { table } => table.clone(),
            Self::LayoutDiffers { table, setting, .. } => format!("{table} {setting}"),
            Self::EntryMissing { table, ipn, .. } => format!("{table} {ipn}"),
            Self::EntryAdded { table, entry } => format!("{table} {entry}"),
            Self::ColumnAdded { table, column, .. } => format!("{table} column {column}"),
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
            Self::PartsSqliteIsOlder { frozen, csv, .. } => format!(
                "{place}: parts.sqlite has {}, today's CSV file {}.",
                shown(frozen),
                shown(csv)
            ),
            Self::GroupAdded { rows, .. } => {
                format!(
                    "{place}: a group the CSV catalog's library does not list ({rows} entries)."
                )
            }
            Self::GroupMissing { .. } => format!("{place}: no such group is generated."),
            Self::LayoutDiffers { csv, generated, .. } => format!(
                "{place}: the CSV catalog's library says {csv:?}, the generated one {generated:?}."
            ),
            Self::EntryMissing { .. } => format!("{place}: no entry is generated."),
            Self::EntryAdded { .. } => {
                format!("{place}: an entry that no CSV row makes.")
            }
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
    /// CSV rows, and how many have their generated entry.
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

/// The generated table of the category a CSV file makes, such as
/// `category_res` for `res` (see `crates/kicad-libraries/src/plan.rs`).
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
    let mut differences = compare_frozen(csv);
    let csv_tables = &csv.library.tables;
    let generated_tables = &generated.library.tables;
    comparison.csv_groups = csv_tables.values().filter(|t| t.group.is_some()).count();
    comparison.generated_groups = generated_tables
        .values()
        .filter(|t| t.group.is_some())
        .count();
    // The generated entries a CSV row makes, by table.
    let mut matched: BTreeSet<(String, String)> = BTreeSet::new();

    for (name, table) in csv_tables {
        let mine_name = category_table(name);
        let Some(mine) = generated_tables.get(&mine_name) else {
            differences.push(Difference::GroupMissing {
                table: name.clone(),
            });
            continue;
        };
        if table.group.is_some() && mine.group.is_none() {
            differences.push(Difference::GroupMissing {
                table: mine_name.clone(),
            });
        }
        for column in &mine.columns {
            if !table.columns.contains(column) {
                differences.push(Difference::ColumnAdded {
                    table: name.clone(),
                    column: column.clone(),
                    empty: mine
                        .rows
                        .iter()
                        .all(|row| row.get(column).is_none_or(|value| value.is_empty())),
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
            let entry_name = first_entry(&ipn);
            let Some(entry) = entries.get(entry_name.as_str()) else {
                differences.push(Difference::EntryMissing {
                    table: name.clone(),
                    ipn,
                    symbol: row.get("Symbol").cloned().unwrap_or_default(),
                });
                continue;
            };
            matched.insert((mine_name.clone(), entry_name.clone()));
            comparison.entries_found += 1;
            compare_row(name, &ipn, row, entry, csv, generated, &mut differences);
            if let (Some(csv_group), Some(group)) = (&table.group, &mine.group) {
                compare_fields(
                    name,
                    &ipn,
                    &entry_name,
                    row,
                    entry,
                    csv_group,
                    group,
                    &mut differences,
                );
            }
        }
    }
    for (name, table) in generated_tables {
        for entry in table.rows.iter().filter_map(|row| row.get("Entry")) {
            if !matched.contains(&(name.clone(), entry.clone())) {
                differences.push(Difference::EntryAdded {
                    table: name.clone(),
                    entry: entry.clone(),
                });
            }
        }
        let csv_table = name
            .strip_prefix("category_")
            .filter(|code| csv_tables.contains_key(*code));
        let csv_group = csv_table.and_then(|code| csv_tables[code].group.as_ref());
        if let Some(group) = &table.group {
            compare_layout(
                name,
                csv_group.unwrap_or(&Group::csv_catalog_layout()),
                group,
                &mut differences,
            );
        }
        if table.group.is_some() && csv_group.is_none() {
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

/// Where the frozen `parts.sqlite` differs from the tables built from
/// today's CSV files: tables, columns, rows (by IPN) and cells.
fn compare_frozen(csv: &CsvSide) -> Vec<Difference> {
    let notes = &csv.notes;
    let mut differences = Vec::new();
    let mut older = |place: String, frozen: Option<&str>, now: Option<&str>, recorded: bool| {
        differences.push(Difference::PartsSqliteIsOlder {
            place,
            frozen: frozen.map(str::to_owned),
            csv: now.map(str::to_owned),
            recorded,
        });
    };
    let names: BTreeSet<&String> = csv.frozen.keys().chain(csv.library.tables.keys()).collect();
    for name in names {
        let recorded_table = notes.tables.contains(name.as_str());
        let (Some(frozen), Some(now)) = (csv.frozen.get(name), csv.library.tables.get(name)) else {
            older(
                format!("table {name}"),
                csv.frozen.contains_key(name).then_some("the table"),
                csv.library.tables.contains_key(name).then_some("the table"),
                recorded_table,
            );
            continue;
        };
        if frozen.columns != now.columns {
            older(
                format!("{name} columns"),
                Some(&frozen.columns.join(", ")),
                Some(&now.columns.join(", ")),
                recorded_table,
            );
        }
        let by_ipn = |rows: &[Row]| -> BTreeMap<String, Row> {
            rows.iter()
                .map(|row| (row.get("IPN").cloned().unwrap_or_default(), row.clone()))
                .collect()
        };
        let (frozen_rows, now_rows) = (by_ipn(&frozen.rows), by_ipn(&now.rows));
        let ipns: BTreeSet<&String> = frozen_rows.keys().chain(now_rows.keys()).collect();
        for ipn in ipns {
            let (Some(frozen_row), Some(now_row)) = (frozen_rows.get(ipn), now_rows.get(ipn))
            else {
                older(
                    format!("{name} {ipn}"),
                    frozen_rows.contains_key(ipn).then_some("the row"),
                    now_rows.contains_key(ipn).then_some("the row"),
                    recorded_table,
                );
                continue;
            };
            for (column, value) in frozen_row {
                if let Some(now_value) = now_row.get(column).filter(|now| *now != value) {
                    let recorded = recorded_table
                        || notes
                            .cells
                            .contains(&(name.clone(), ipn.clone(), column.clone()));
                    older(
                        format!("{name} {ipn} {column}"),
                        Some(value),
                        Some(now_value),
                        recorded,
                    );
                }
            }
        }
    }
    differences
}

/// Compares a group's settings: the key, the Symbol and Footprint columns
/// and the Exclude from BOM column.
fn compare_layout(table: &str, csv: &Group, mine: &Group, differences: &mut Vec<Difference>) {
    let settings = [
        ("key", Some(&csv.key), Some(&mine.key)),
        ("symbols", Some(&csv.symbols), Some(&mine.symbols)),
        ("footprints", Some(&csv.footprints), Some(&mine.footprints)),
        (
            "exclude_from_bom",
            csv.exclude_from_bom.as_ref(),
            mine.exclude_from_bom.as_ref(),
        ),
    ];
    for (setting, theirs, my) in settings {
        if theirs != my {
            differences.push(Difference::LayoutDiffers {
                table: table.to_owned(),
                setting: setting.to_owned(),
                csv: theirs.cloned().unwrap_or_default(),
                generated: my.cloned().unwrap_or_default(),
            });
        }
    }
}

/// Compares one row's columns, its Symbol and Footprint choices, and the
/// entry's own Revision and Exclude from BOM.
fn compare_row(
    table: &str,
    ipn: &str,
    row: &Row,
    entry: &Row,
    csv: &CsvSide,
    generated: &GeneratedSide,
    differences: &mut Vec<Difference>,
) {
    let mut differs = |column: &str, theirs: String, mine: Option<&String>| {
        if mine != Some(&theirs) {
            differences.push(Difference::ValueDiffers {
                table: table.to_owned(),
                ipn: ipn.to_owned(),
                column: column.to_owned(),
                csv: theirs,
                generated: mine.cloned().unwrap_or_default(),
                row: row.clone(),
            });
        }
    };
    // Entry `<IPN>_v<N>` is revision N.
    let number = entry
        .get("Entry")
        .and_then(|name| name.strip_prefix(ipn))
        .and_then(|rest| rest.strip_prefix("_v"))
        .unwrap_or_default()
        .to_owned();
    differs("Revision", number, entry.get("Revision"));
    let excluded = excluded_from_bom(row, csv);
    differs(
        "Exclude from BOM",
        if excluded { "1" } else { "0" }.to_owned(),
        entry.get("Exclude from BOM"),
    );
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

/// The choices of a Symbol or Footprint cell, split at `;`.
fn choices(cell: &str) -> Vec<&str> {
    cell.split(';')
        .map(str::trim)
        .filter(|choice| !choice.is_empty())
        .collect()
}

/// Whether the part of `row` is Excluded from BOM, worked out as the team
/// server does from the CSV catalog's own CAD: a symbol marked
/// `(in_bom no)`, or every footprint choice marked `exclude_from_bom`. A
/// symbol or footprint of KiCad's own gives no mark.
fn excluded_from_bom(row: &Row, csv: &CsvSide) -> bool {
    let cell = |column: &str| row.get(column).map_or("", String::as_str);
    let symbol = choices(cell("Symbol")).iter().any(|choice| {
        csv.symbols
            .get(*choice)
            .is_some_and(|bytes| symbol_entries_say_not_in_bom(bytes))
    });
    let footprints = choices(cell("Footprint"));
    let every_footprint = !footprints.is_empty()
        && footprints.iter().all(|choice| {
            csv.footprints
                .get(*choice)
                .is_some_and(|footprint| footprint_says_excluded_from_bom(&footprint.bytes))
        });
    symbol || every_footprint
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
    let (theirs, mine) = (choices(theirs), choices(mine));
    if theirs.len() != mine.len() {
        return Some(format!(
            "the CSV has {} choices and the generated library {}.",
            theirs.len(),
            mine.len()
        ));
    }
    let symbol = column == "Symbol";
    for (their, my) in theirs.into_iter().zip(mine) {
        let why = if symbol {
            match csv.symbols.get(their) {
                Some(source) => same_symbol(their, my, source, generated),
                None => as_written(their, my),
            }
        } else {
            match csv.footprints.get(their) {
                Some(source) => same_footprint(their, my, source, generated),
                None => as_written(their, my),
            }
        };
        if let Err(why) = why {
            return Some(why);
        }
    }
    None
}

/// A choice that is not one of the CSV catalog's own: KiCad finds it in
/// its own libraries, so it must be written as the CSV writes it.
fn as_written(theirs: &str, mine: &str) -> Result<(), String> {
    if theirs == mine {
        Ok(())
    } else {
        Err(format!("{theirs} must stay as the CSV writes it."))
    }
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
/// `reference`, `source`, with only its 3D model links changed, and each
/// changed link leads to a copy of the CSV catalog's model file.
fn same_footprint(
    reference: &str,
    link: &str,
    source: &CsvFootprint,
    generated: &GeneratedSide,
) -> Result<(), String> {
    if !is_revision_of(link, reference) {
        return Err(format!("{link} is not named as a revision of {reference}."));
    }
    let footprint = generated
        .footprints
        .get(link)
        .ok_or_else(|| format!("{link} is not in the generated footprint libraries."))?;
    let (Ok(text), Ok(source_text)) = (
        std::str::from_utf8(&footprint.bytes),
        std::str::from_utf8(&source.bytes),
    ) else {
        return Err(format!("{link} or {reference} is not UTF-8 text."));
    };
    let originals: Vec<Option<String>> = source
        .models
        .iter()
        .map(|model| Some(model.link.clone()))
        .collect();
    let (restored, links) = replace_model_references(text, &originals);
    if links.len() != originals.len() || restored != source_text {
        return Err(format!(
            "{link} is not the CSV catalog's footprint {reference} with only its 3D model links changed."
        ));
    }
    for (expected, written) in source.models.iter().zip(model_references(text)) {
        same_model(link, expected, &written, &footprint.folder, generated)?;
    }
    Ok(())
}

/// The generated footprint `link`, in `folder`, writes the 3D model link
/// `written` where the CSV catalog's footprint writes `expected.link`.
fn same_model(
    link: &str,
    expected: &ExpectedModel,
    written: &str,
    folder: &str,
    generated: &GeneratedSide,
) -> Result<(), String> {
    let Some(sha256) = &expected.sha256 else {
        // It never led to a model file, so it must stay as written.
        return if written == expected.link {
            Ok(())
        } else {
            Err(format!(
                "{link} changed the 3D model link {} to {written}, but that link never led to a file.",
                expected.link
            ))
        };
    };
    let path = resolve(folder, written).ok_or_else(|| {
        format!(
            "{link} links its 3D model to {written}, which does not lead into the libraries folder."
        )
    })?;
    match generated.files.get(&path) {
        None => Err(format!(
            "{link} links its 3D model to {written}, but there is no such file."
        )),
        Some(found) if found != sha256 => Err(format!(
            "{link} links its 3D model to {written}, but that file is not the CSV catalog's model {}.",
            expected.link
        )),
        Some(_) => Ok(()),
    }
}

/// `relative` from `folder` (both with `/`, from the team folder), or
/// `None` when it leaves the team folder or is not relative.
fn resolve(folder: &str, relative: &str) -> Option<String> {
    if relative.starts_with('/') || relative.contains(':') || relative.contains('$') {
        return None;
    }
    let mut parts: Vec<&str> = folder.split('/').filter(|part| !part.is_empty()).collect();
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
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
#[allow(clippy::too_many_arguments)]
fn compare_fields(
    table: &str,
    ipn: &str,
    entry_name: &str,
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
            entry: entry_name.to_owned(),
            field: name.clone(),
            csv: their.cloned(),
            generated: my.cloned(),
            row: row.clone(),
        });
    }
}

#[cfg(test)]
mod tests;

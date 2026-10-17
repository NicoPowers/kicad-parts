//! The database library: the SQLite database KiCad reads through the
//! SQLite3 ODBC driver, and the `.kicad_dbl` file that tells KiCad how.
//!
//! KiCad may be reading the database at any moment, so it is changed in
//! place, in one transaction, and only when something differs. A query
//! sees one table as it was before the update or after it, never half
//! changed. KiCad's background refresh reads one table at a time, outside
//! a transaction (`backgroundRefreshWorker` in KiCad's
//! `eeschema/sch_io/database/sch_io_database.cpp`), so during one refresh a
//! row that moves between groups can show in both or in neither. A placed
//! part's link still resolves, because KiCad's `LoadSymbol` then looks in
//! every table. The database uses write-ahead logging, so readers never
//! block the update and the update never blocks them.
//!
//! Tables and columns are only ever added, because a running KiCad keeps
//! the `.kicad_dbl` it loaded and would fail on a table or column it
//! expects that is gone. A row is only deleted when its entry moves to
//! another table: an entry missing from the Catalog's latest reading keeps
//! its row, because a design may link to it.

use crate::plan::{EXCLUDE_FROM_BOM, FIXED_COLUMNS, Naming, TablePlan};
use rusqlite::{Connection, TransactionBehavior, params_from_iter};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

/// How long the update waits for a reader that holds a lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);
/// How often KiCad reads the database again, in seconds, so new entries
/// show up without restarting it.
pub const REFRESH_SECONDS: u32 = 10;

/// The `.kicad_dbl` for `tables`: each becomes a group, keyed by entry
/// name alone (`globally_unique_keys`), so a placed part's link never
/// names its group and keeps working when its row moves.
pub fn dbl_json(tables: &[TablePlan], naming: &Naming) -> String {
    let fixed_field = |column: &str, shown_on_add: bool, in_chooser: bool| {
        json!({
            "column": column,
            "name": column,
            "visible_on_add": shown_on_add,
            "visible_in_chooser": in_chooser,
            "show_name": false,
        })
    };
    let libraries: Vec<Value> = tables
        .iter()
        .map(|table| {
            let mut fields = vec![
                fixed_field("IPN", false, true),
                fixed_field("Revision", false, true),
                fixed_field("Value", true, false),
                fixed_field("Manufacturer", false, true),
                fixed_field("MPN", false, true),
                fixed_field("Description", false, true),
                fixed_field("Datasheet", false, true),
            ];
            for extra in &table.extra {
                fields.push(json!({
                    "column": extra.column,
                    "name": extra.field,
                    "visible_on_add": extra.shown_on_add,
                    "visible_in_chooser": true,
                    "show_name": false,
                }));
            }
            json!({
                "name": table.name,
                "table": table.table,
                "key": "Entry",
                "symbols": "Symbol",
                "footprints": "Footprint",
                "fields": fields,
                "properties": { "exclude_from_bom": EXCLUDE_FROM_BOM },
            })
        })
        .collect();
    let dbl = json!({
        "meta": { "version": 1 },
        "name": naming.database_nickname(),
        "description": "Parts from the team catalog, one entry per revision. Made by the KiCad Parts app, which replaces any change made here.",
        "globally_unique_keys": true,
        "source": {
            "type": "odbc",
            "dsn": "",
            "username": "",
            "password": "",
            "timeout_seconds": 10,
            // The driver's name exactly as Windows lists it. The shorter
            // `{SQLite3}` of the CSV catalog's .kicad_dbl names no
            // installed driver, so KiCad could not load it.
            "connection_string": format!(
                "DRIVER={{{}}};DATABASE=${{CWD}}/{}",
                crate::odbc::DRIVER,
                naming.database_file()
            ),
        },
        "cache": { "max_size": 256, "max_age": REFRESH_SECONDS },
        "libraries": libraries,
    });
    let mut text = serde_json::to_string_pretty(&dbl).expect("JSON values always serialize");
    text.push('\n');
    text
}

/// A column in SQL, in double quotes.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// What a table holds now: its columns in order and its rows by entry.
struct Existing {
    columns: Vec<String>,
    rows: BTreeMap<String, Vec<String>>,
}

/// What [`sync`] did.
#[derive(Debug, Default)]
pub struct Synced {
    pub changed: bool,
    /// Entries kept although the plan has them in no table.
    pub kept: Vec<String>,
}

/// Brings the database at `path` to `tables`, in one transaction that
/// reads the tables as well as changing them, so nothing changes between
/// the reading and the writing. Creates the database the first time.
pub fn sync(path: &Path, tables: &[TablePlan]) -> Result<Synced, String> {
    let fail = |error: rusqlite::Error| format!("{error}");
    let mut connection = Connection::open(path).map_err(fail)?;
    connection.busy_timeout(BUSY_TIMEOUT).map_err(fail)?;
    let mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(fail)?;
    if !mode.eq_ignore_ascii_case("wal") {
        // Only works while nobody else has the database open; a database
        // KiCad already reads keeps its mode, which is still safe.
        let _ = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0));
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(fail)?;
    let existing = read_tables(&transaction, tables).map_err(fail)?;
    let (statements, kept) = changes(tables, &existing);
    if statements.is_empty() {
        // Nothing to write: end the transaction without touching the file.
        transaction.rollback().map_err(fail)?;
        return Ok(Synced {
            changed: false,
            kept,
        });
    }
    for (sql, values) in &statements {
        transaction
            .execute(sql, params_from_iter(values.iter()))
            .map_err(fail)?;
    }
    transaction.commit().map_err(fail)?;
    Ok(Synced {
        changed: true,
        kept,
    })
}

fn read_tables(
    connection: &Connection,
    tables: &[TablePlan],
) -> rusqlite::Result<BTreeMap<String, Existing>> {
    let mut found = BTreeMap::new();
    for table in tables {
        let exists: bool = connection.query_row(
            "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [&table.table],
            |row| row.get(0),
        )?;
        if !exists {
            continue;
        }
        let mut statement =
            connection.prepare(&format!("PRAGMA table_info({})", quoted(&table.table)))?;
        let columns: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<_>>()?;
        let list: Vec<String> = columns.iter().map(|column| quoted(column)).collect();
        let mut statement = connection.prepare(&format!(
            "SELECT {} FROM {}",
            list.join(", "),
            quoted(&table.table)
        ))?;
        let width = columns.len();
        let rows = statement
            .query_map([], |row| {
                (0..width)
                    .map(|index| {
                        Ok(match row.get_ref(index)? {
                            rusqlite::types::ValueRef::Null => String::new(),
                            rusqlite::types::ValueRef::Integer(number) => number.to_string(),
                            rusqlite::types::ValueRef::Real(number) => number.to_string(),
                            rusqlite::types::ValueRef::Text(text)
                            | rusqlite::types::ValueRef::Blob(text) => {
                                String::from_utf8_lossy(text).into_owned()
                            }
                        })
                    })
                    .collect::<rusqlite::Result<Vec<String>>>()
            })?
            .collect::<rusqlite::Result<Vec<Vec<String>>>>()?;
        found.insert(
            table.table.clone(),
            Existing {
                columns,
                rows: rows
                    .into_iter()
                    .map(|row| (row.first().cloned().unwrap_or_default(), row))
                    .collect(),
            },
        );
    }
    Ok(found)
}

/// One SQL statement and its values.
type Statement = (String, Vec<rusqlite::types::Value>);

/// The column definition: Exclude from BOM is a number, as KiCad needs.
fn definition(column: &str) -> String {
    if column == "Entry" {
        format!("{} TEXT PRIMARY KEY NOT NULL", quoted(column))
    } else if column == EXCLUDE_FROM_BOM {
        format!("{} INTEGER NOT NULL DEFAULT 0", quoted(column))
    } else {
        format!("{} TEXT NOT NULL DEFAULT ''", quoted(column))
    }
}

fn sql_value(column: &str, value: &str) -> rusqlite::types::Value {
    if column == EXCLUDE_FROM_BOM {
        rusqlite::types::Value::Integer(i64::from(value == "1"))
    } else {
        rusqlite::types::Value::Text(value.to_owned())
    }
}

/// The statements that bring `existing` to `tables`, in a fixed order:
/// tables in plan order, and in each, new columns, then rows that moved
/// away, then changed and new rows by entry. Also the entries kept because
/// the plan has them in no table.
fn changes(
    tables: &[TablePlan],
    existing: &BTreeMap<String, Existing>,
) -> (Vec<Statement>, Vec<String>) {
    let planned: BTreeSet<&str> = tables
        .iter()
        .flat_map(|table| table.rows.iter().map(|row| row.entry()))
        .collect();
    let mut kept = Vec::new();
    let mut statements = Vec::new();
    for table in tables {
        let wanted: Vec<String> = FIXED_COLUMNS
            .iter()
            .map(|column| (*column).to_owned())
            .chain(table.extra.iter().map(|extra| extra.column.clone()))
            .collect();
        let name = quoted(&table.table);
        let columns: Vec<String> = match existing.get(&table.table) {
            None => {
                let definitions: Vec<String> =
                    wanted.iter().map(|column| definition(column)).collect();
                statements.push((
                    format!("CREATE TABLE {name} ({})", definitions.join(", ")),
                    Vec::new(),
                ));
                wanted.clone()
            }
            Some(found) => {
                let have: BTreeSet<String> =
                    found.columns.iter().map(|c| c.to_lowercase()).collect();
                let mut columns = found.columns.clone();
                for column in &wanted {
                    if !have.contains(&column.to_lowercase()) {
                        statements.push((
                            format!("ALTER TABLE {name} ADD COLUMN {}", definition(column)),
                            Vec::new(),
                        ));
                        columns.push(column.clone());
                    }
                }
                columns
            }
        };
        // Each row's value for every column the table has; a column no
        // longer wanted is kept, and left blank.
        let position: BTreeMap<String, usize> = wanted
            .iter()
            .enumerate()
            .map(|(index, column)| (column.to_lowercase(), index))
            .collect();
        let full = |values: &[String]| -> Vec<String> {
            columns
                .iter()
                .map(|column| match position.get(&column.to_lowercase()) {
                    Some(&index) => values.get(index).cloned().unwrap_or_default(),
                    None if column == EXCLUDE_FROM_BOM => "0".to_owned(),
                    None => String::new(),
                })
                .collect()
        };
        let empty = BTreeMap::new();
        let old_rows = existing
            .get(&table.table)
            .map(|found| &found.rows)
            .unwrap_or(&empty);
        let new_rows: BTreeMap<&str, Vec<String>> = table
            .rows
            .iter()
            .map(|row| (row.entry(), full(&row.values)))
            .collect();
        for entry in old_rows.keys() {
            if new_rows.contains_key(entry.as_str()) {
                continue;
            }
            if !planned.contains(entry.as_str()) {
                // Missing from this reading of the Catalog, such as a part
                // whose details could not be read: a design may link to
                // it, so its row stays as it was.
                kept.push(entry.clone());
            } else {
                statements.push((
                    format!("DELETE FROM {name} WHERE \"Entry\" = ?1"),
                    vec![rusqlite::types::Value::Text(entry.clone())],
                ));
            }
        }
        let list: Vec<String> = columns.iter().map(|column| quoted(column)).collect();
        for (entry, values) in &new_rows {
            match old_rows.get(*entry) {
                Some(old) if old == values => {}
                Some(_) => {
                    let sets: Vec<String> = columns
                        .iter()
                        .enumerate()
                        .skip(1)
                        .map(|(index, column)| format!("{} = ?{}", quoted(column), index + 1))
                        .collect();
                    statements.push((
                        format!("UPDATE {name} SET {} WHERE \"Entry\" = ?1", sets.join(", ")),
                        columns
                            .iter()
                            .zip(values)
                            .map(|(column, value)| sql_value(column, value))
                            .collect(),
                    ));
                }
                None => {
                    let marks: Vec<String> = (1..=columns.len())
                        .map(|index| format!("?{index}"))
                        .collect();
                    statements.push((
                        format!(
                            "INSERT INTO {name} ({}) VALUES ({})",
                            list.join(", "),
                            marks.join(", ")
                        ),
                        columns
                            .iter()
                            .zip(values)
                            .map(|(column, value)| sql_value(column, value))
                            .collect(),
                    ));
                }
            }
        }
    }
    kept.sort();
    (statements, kept)
}

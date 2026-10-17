//! Reads the two sides of the comparison from their files. The CSV
//! catalog's side is only read: `parts.sqlite` comes from the CSV catalog
//! fixture's copy of it (which `csv-catalog-fixture check` proves is the
//! file), and `#gplm.kicad_dbl`, the symbols and the footprints are read
//! after checking their bytes are the ones the fixture recorded.

use super::{CsvSide, DatabaseLibrary, GeneratedSide, Group, Row, Table};
use catalog::sha256_hex;
use csv_catalog_fixture::Manifest;
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup};
use kicad_libraries::{Library, LibraryKind, Naming};
use std::collections::BTreeMap;
use std::path::Path;

/// The CSV catalog's KiCad library and the symbols and footprints its
/// rows take from its own libraries.
pub fn csv_side(repository: &Path, oracle: &Manifest) -> Result<CsvSide, String> {
    let dbl = read_checked(
        repository,
        &oracle.descriptor.path,
        &oracle.descriptor.byte_sha256,
    )?;
    let groups = groups(&dbl, &oracle.descriptor.path)?;
    let mut library = DatabaseLibrary::default();
    for table in &oracle.sqlite_projection.tables {
        let rows = table
            .rows
            .iter()
            .map(|row| {
                table
                    .headers
                    .iter()
                    .cloned()
                    .zip(row.iter().map(|cell| cell.clone().unwrap_or_default()))
                    .collect()
            })
            .collect();
        library.tables.insert(
            table.name.clone(),
            Table {
                columns: table.headers.clone(),
                rows,
                group: groups.get(&table.name).cloned(),
            },
        );
    }

    let mut side = CsvSide {
        library,
        ..CsvSide::default()
    };
    let hash_of = |path: &str| -> Option<&str> {
        oracle
            .assets
            .symbols
            .iter()
            .chain(&oracle.assets.footprints)
            .find(|file| file.path == path)
            .map(|file| file.byte_sha256.as_str())
    };
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for reference in oracle
        .assets
        .symbol_references
        .iter()
        .chain(&oracle.assets.footprint_references)
        .filter(|reference| reference.local && reference.status == "resolved")
    {
        let Some(path) = &reference.asset_path else {
            continue;
        };
        if !files.contains_key(path) {
            let hash = hash_of(path)
                .ok_or_else(|| format!("The CSV catalog fixture does not list {path}."))?;
            files.insert(path.clone(), read_checked(repository, path, hash)?);
        }
        let bytes = &files[path];
        if reference.kind == "symbol" {
            let name = reference
                .reference
                .split_once(':')
                .map_or(reference.reference.as_str(), |(_, name)| name);
            let parsed = SymbolLibrary::parse(bytes)
                .map_err(|error| format!("{path} could not be read: {error}"))?;
            let SymbolLookup::Found(entry) = parsed.find(name) else {
                return Err(format!("{path} has no single symbol {name}."));
            };
            side.symbols.insert(
                reference.reference.clone(),
                bytes[entry.span.clone()].to_vec(),
            );
        } else {
            side.footprints
                .insert(reference.reference.clone(), bytes.clone());
        }
    }
    Ok(side)
}

/// The generated KiCad libraries in `folder`, the team's folder, named by
/// `naming`; `libraries` are the symbol and footprint libraries in it.
pub fn generated_side(
    folder: &Path,
    naming: &Naming,
    libraries: &[Library],
) -> Result<GeneratedSide, String> {
    let dbl_path = folder.join(naming.dbl_file());
    let dbl = std::fs::read(&dbl_path)
        .map_err(|error| format!("{} could not be read: {error}", dbl_path.display()))?;
    let groups = groups(&dbl, &naming.dbl_file())?;
    let mut side = GeneratedSide::default();
    for (name, (columns, rows)) in read_database(&folder.join(naming.database_file()))? {
        side.library.tables.insert(
            name.clone(),
            Table {
                columns,
                rows,
                group: groups.get(&name).cloned(),
            },
        );
    }
    for library in libraries {
        let path = Path::new(&library.path);
        match library.kind {
            LibraryKind::Symbols => {
                let bytes = std::fs::read(path)
                    .map_err(|error| format!("{} could not be read: {error}", library.path))?;
                let parsed = SymbolLibrary::parse(&bytes)
                    .map_err(|error| format!("{} could not be read: {error}", library.path))?;
                for entry in parsed.entries {
                    side.symbols.insert(
                        format!("{}:{}", library.nickname, entry.name),
                        bytes[entry.span].to_vec(),
                    );
                }
            }
            LibraryKind::Footprints => {
                let entries = std::fs::read_dir(path)
                    .map_err(|error| format!("{} could not be read: {error}", library.path))?;
                for entry in entries.flatten() {
                    let file = entry.path();
                    if file
                        .extension()
                        .is_some_and(|extension| extension == "kicad_mod")
                    {
                        let stem = file.file_stem().unwrap_or_default().to_string_lossy();
                        let bytes = std::fs::read(&file).map_err(|error| {
                            format!("{} could not be read: {error}", file.display())
                        })?;
                        side.footprints
                            .insert(format!("{}:{stem}", library.nickname), bytes);
                    }
                }
            }
            LibraryKind::Parts => {}
        }
    }
    Ok(side)
}

/// `path` under `repository`, refused unless its bytes have the SHA-256
/// the CSV catalog fixture recorded.
fn read_checked(repository: &Path, path: &str, sha256: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(repository.join(path))
        .map_err(|error| format!("{path} could not be read: {error}"))?;
    if sha256_hex(&bytes) != sha256 {
        return Err(format!(
            "{path} is not the file the CSV catalog fixture recorded. Run `cargo run -p csv-catalog-fixture -- check .` to see what changed."
        ));
    }
    Ok(bytes)
}

/// Each table a `.kicad_dbl` lists, with its key and field list.
fn groups(dbl: &[u8], name: &str) -> Result<BTreeMap<String, Group>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(dbl).map_err(|error| format!("{name} is not JSON: {error}"))?;
    let text = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_owned();
    let mut groups = BTreeMap::new();
    for library in value["libraries"].as_array().into_iter().flatten() {
        let fields = library["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|field| (text(&field["column"]), text(&field["name"])))
            .collect();
        groups.insert(
            text(&library["table"]),
            Group {
                key: text(&library["key"]),
                fields,
            },
        );
    }
    Ok(groups)
}

/// Tables by name, each with its columns and rows.
type Tables = BTreeMap<String, (Vec<String>, Vec<Row>)>;

/// Every table of the SQLite database at `path`, every value as text and
/// NULL as empty text. The database is opened as usual, not read-only, so
/// SQLite tidies its own side files when it closes; nothing is written.
fn read_database(path: &Path) -> Result<Tables, String> {
    let failed = |error: rusqlite::Error| format!("{} could not be read: {error}", path.display());
    let connection = rusqlite::Connection::open(path).map_err(failed)?;
    let names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .map_err(failed)?
        .query_map([], |row| row.get(0))
        .map_err(failed)?
        .collect::<Result<_, _>>()
        .map_err(failed)?;
    let mut tables = Tables::new();
    for name in names {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
            .map_err(failed)?;
        let columns: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let rows = statement
            .query_map([], |row| {
                let mut values = Row::new();
                for (index, column) in columns.iter().enumerate() {
                    let text = match row.get::<_, rusqlite::types::Value>(index)? {
                        rusqlite::types::Value::Null => String::new(),
                        rusqlite::types::Value::Integer(number) => number.to_string(),
                        rusqlite::types::Value::Real(number) => number.to_string(),
                        rusqlite::types::Value::Text(text) => text,
                        rusqlite::types::Value::Blob(bytes) => {
                            String::from_utf8_lossy(&bytes).into_owned()
                        }
                    };
                    values.insert(column.clone(), text);
                }
                Ok(values)
            })
            .map_err(failed)?
            .collect::<Result<Vec<Row>, _>>()
            .map_err(failed)?;
        tables.insert(name, (columns, rows));
    }
    Ok(tables)
}

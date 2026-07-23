use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug)]
pub enum FixtureError {
    Io(std::io::Error),
    Csv(csv::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Csv(error) => write!(f, "CSV error: {error}"),
            Self::Json(error) => write!(f, "JSON error: {error}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for FixtureError {}
impl From<std::io::Error> for FixtureError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<csv::Error> for FixtureError {
    fn from(value: csv::Error) -> Self {
        Self::Csv(value)
    }
}
impl From<serde_json::Error> for FixtureError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub normalization: Normalization,
    pub catalog: CatalogManifest,
    pub substitutes: CsvDocument,
    pub sqlite_projection: SqliteManifest,
    pub descriptor: DescriptorManifest,
    pub assets: AssetManifest,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Normalization {
    pub csv_semantic: String,
    pub kicad_semantic: String,
    pub step_semantic: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SqliteManifest {
    pub path: String,
    pub byte_sha256: String,
    pub semantic_sha256: String,
    pub bytes: u64,
    pub table_count: usize,
    pub row_count: usize,
    pub tables: Vec<SqliteTable>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SqliteTable {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CatalogManifest {
    pub category_count: usize,
    pub row_count: usize,
    pub generated_sqlite_tables: Vec<String>,
    pub update_script_tables: Vec<String>,
    pub categories: Vec<Category>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Category {
    pub table: String,
    #[serde(flatten)]
    pub document: CsvDocument,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CsvDocument {
    pub path: String,
    pub byte_sha256: String,
    pub semantic_sha256: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DescriptorManifest {
    pub path: String,
    pub byte_sha256: String,
    pub semantic_sha256: String,
    pub entry_count: usize,
    pub tables: Vec<String>,
    pub csv_tables_not_exposed: Vec<String>,
    pub descriptor_tables_without_csv: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssetManifest {
    pub symbols: Vec<Asset>,
    pub footprints: Vec<Asset>,
    pub models_3d: Vec<Asset>,
    pub symbol_references: Vec<CadReference>,
    pub footprint_references: Vec<CadReference>,
    pub model_references: Vec<ModelReference>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct Asset {
    pub path: String,
    pub byte_sha256: String,
    pub semantic_sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct CadReference {
    pub kind: String,
    pub reference: String,
    pub source_ipns: Vec<String>,
    pub local: bool,
    pub status: String,
    pub asset_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct ModelReference {
    pub source_footprint: String,
    pub reference: String,
    pub scope: String,
    pub status: String,
    pub asset_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub path: String,
    pub location: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Difference {
    pub path: String,
    pub expected: String,
    pub actual: String,
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: expected {}, found {}",
            self.path, self.expected, self.actual
        )
    }
}

pub fn build_manifest(root: &Path) -> Result<Manifest, FixtureError> {
    let database = root.join("database");
    let mut csv_paths = read_dir_files(root, &database, |path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("g-") && n.ends_with(".csv"))
    })?;
    csv_paths.sort();

    let mut categories = Vec::new();
    for path in csv_paths {
        let table = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .trim_start_matches("g-")
            .to_owned();
        categories.push(Category {
            table,
            document: read_csv_document(root, &path)?,
        });
    }
    let generated_sqlite_tables: Vec<_> = categories.iter().map(|c| c.table.clone()).collect();
    let row_count = categories.iter().map(|c| c.document.rows.len()).sum();
    let substitutes = read_csv_document(root, &database.join("substitutes.csv"))?;
    let sqlite_projection = read_sqlite(root, &database.join("parts.sqlite"))?;
    let update_script_tables = parse_update_tables(&read_checked_to_string(
        root,
        &database.join("update_db.sh"),
    )?);
    let descriptor = read_descriptor(root, &generated_sqlite_tables)?;

    let mut diagnostics = Vec::new();
    compare_projection(&categories, &sqlite_projection, &mut diagnostics);
    if generated_sqlite_tables != update_script_tables {
        let generated: BTreeSet<_> = generated_sqlite_tables.iter().cloned().collect();
        let scripted: BTreeSet<_> = update_script_tables.iter().cloned().collect();
        for table in generated.difference(&scripted) {
            diagnostics.push(Diagnostic {
                code: "update-script-omits-csv-table".into(),
                path: "database/update_db.sh".into(),
                location: format!("table[{table}]"),
                message: format!("CSV table `{table}` is generated by catalog discovery but omitted by the legacy shell script"),
            });
        }
    }
    let assets = read_assets(root, &categories, &mut diagnostics)?;
    diagnostics.sort();

    Ok(Manifest {
        schema_version: SCHEMA_VERSION,
        normalization: Normalization {
            csv_semantic: "RFC 4180 records; UTF-8 text values, header order, row order, unknown columns, and blanks preserved; serialization/line-ending differences ignored".into(),
            kicad_semantic: "UTF-8 KiCad s-expression lexical tokens; whitespace outside quoted strings ignored; token boundaries and quoted bytes preserved".into(),
            step_semantic: "UTF-8/ASCII text with BOM removed, CRLF/CR mapped to LF, trailing horizontal whitespace removed, and terminal blank lines ignored".into(),
        },
        catalog: CatalogManifest { category_count: categories.len(), row_count, generated_sqlite_tables, update_script_tables, categories },
        substitutes,
        sqlite_projection,
        descriptor,
        assets,
        diagnostics,
    })
}

pub fn compare(expected: &Manifest, actual: &Manifest) -> Vec<Difference> {
    let expected = serde_json::to_value(expected).expect("manifest serialization");
    let actual = serde_json::to_value(actual).expect("manifest serialization");
    let mut differences = Vec::new();
    compare_value("$", &expected, &actual, &mut differences);
    for difference in &mut differences {
        difference.path = describe_catalog_field_path(&difference.path, &expected, &actual);
    }
    differences
}

pub fn manifest_json(manifest: &Manifest) -> Result<String, FixtureError> {
    let mut output = serde_json::to_string_pretty(manifest)?;
    output.push('\n');
    Ok(output)
}

pub fn load_manifest(path: &Path) -> Result<Manifest, FixtureError> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn compare_value(path: &str, expected: &Value, actual: &Value, out: &mut Vec<Difference>) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            let keys: BTreeSet<_> = e.keys().chain(a.keys()).collect();
            for key in keys {
                let next = format!("{path}.{key}");
                match (e.get(key), a.get(key)) {
                    (Some(ev), Some(av)) => compare_value(&next, ev, av, out),
                    (Some(ev), None) => out.push(diff(next, ev, &Value::Null)),
                    (None, Some(av)) => out.push(diff(next, &Value::Null, av)),
                    (None, None) => unreachable!(),
                }
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            for index in 0..e.len().max(a.len()) {
                let next = format!("{path}[{index}]");
                match (e.get(index), a.get(index)) {
                    (Some(ev), Some(av)) => compare_value(&next, ev, av, out),
                    (Some(ev), None) => out.push(diff(next, ev, &Value::Null)),
                    (None, Some(av)) => out.push(diff(next, &Value::Null, av)),
                    (None, None) => unreachable!(),
                }
            }
        }
        _ if expected != actual => out.push(diff(path.to_owned(), expected, actual)),
        _ => {}
    }
}

fn diff(path: String, expected: &Value, actual: &Value) -> Difference {
    Difference {
        path,
        expected: render_value(expected),
        actual: render_value(actual),
    }
}

fn render_value(value: &Value) -> String {
    let rendered = serde_json::to_string(value).expect("JSON value serialization");
    if rendered.len() > 120 {
        format!("{}…", &rendered[..119])
    } else {
        rendered
    }
}

fn describe_catalog_field_path(path: &str, expected: &Value, actual: &Value) -> String {
    const PREFIX: &str = "$.catalog.categories[";
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return path.to_owned();
    };
    let Some((category_text, rest)) = rest.split_once("]") else {
        return path.to_owned();
    };
    let Ok(category_index) = category_text.parse::<usize>() else {
        return path.to_owned();
    };
    let Some(rest) = rest.strip_prefix(".rows[") else {
        return path.to_owned();
    };
    let Some((row_text, rest)) = rest.split_once("]") else {
        return path.to_owned();
    };
    let Ok(row_index) = row_text.parse::<usize>() else {
        return path.to_owned();
    };
    let Some(rest) = rest.strip_prefix('[') else {
        return path.to_owned();
    };
    let Some(field_text) = rest.strip_suffix(']') else {
        return path.to_owned();
    };
    let Ok(field_index) = field_text.parse::<usize>() else {
        return path.to_owned();
    };
    let category = expected
        .pointer(&format!("/catalog/categories/{category_index}"))
        .or_else(|| actual.pointer(&format!("/catalog/categories/{category_index}")));
    let Some(category) = category else {
        return path.to_owned();
    };
    let table = category.get("table").and_then(Value::as_str).unwrap_or("?");
    let header = category
        .get("headers")
        .and_then(Value::as_array)
        .and_then(|headers| headers.get(field_index))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let ipn_index = category
        .get("headers")
        .and_then(Value::as_array)
        .and_then(|headers| {
            headers
                .iter()
                .position(|value| value.as_str() == Some("IPN"))
        });
    let row_key = ipn_index
        .and_then(|index| {
            category
                .get("rows")
                .and_then(Value::as_array)
                .and_then(|rows| rows.get(row_index))
                .and_then(Value::as_array)
                .and_then(|row| row.get(index))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty())
        .map_or_else(
            || format!("row={row_index}"),
            |value| format!("IPN={value}"),
        );
    format!("catalog.categories[{table}].rows[{row_key}].fields[{header}]")
}

fn read_csv_document(root: &Path, path: &Path) -> Result<CsvDocument, FixtureError> {
    let relative_path = stable_path(root, path)?;
    let bytes = read_checked(root, path)?;
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(bytes.as_slice());
    let headers: Vec<String> = reader.headers()?.iter().map(str::to_owned).collect();
    let rows: Vec<Vec<String>> = reader
        .records()
        .enumerate()
        .map(|(index, record)| {
            record.and_then(|record| {
                if record.len() > headers.len() {
                    return Err(csv::Error::from(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "{} row {} has {} fields for {} headers",
                            relative_path,
                            index + 1,
                            record.len(),
                            headers.len()
                        ),
                    )));
                }
                let mut values: Vec<String> = record.iter().map(str::to_owned).collect();
                values.resize(headers.len(), String::new());
                Ok(values)
            })
        })
        .collect::<Result<_, _>>()?;
    let semantic = csv_semantic_bytes(&headers, &rows);
    Ok(CsvDocument {
        path: relative_path,
        byte_sha256: sha256(&bytes),
        semantic_sha256: sha256(&semantic),
        headers,
        rows,
    })
}

fn read_sqlite(root: &Path, path: &Path) -> Result<SqliteManifest, FixtureError> {
    use rusqlite::{Connection, OpenFlags};

    let relative_path = stable_path(root, path)?;
    let bytes = read_checked(root, path)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(sqlite_error)?;
    let mut table_names = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(sqlite_error)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)?;
    table_names.sort();

    let mut tables = Vec::new();
    for name in table_names {
        let quoted = quote_sqlite_identifier(&name);
        let headers = connection
            .prepare(&format!("PRAGMA table_info({quoted})"))
            .map_err(sqlite_error)?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {quoted} ORDER BY rowid"))
            .map_err(sqlite_error)?;
        let column_count = headers.len();
        let rows = statement
            .query_map([], |row| {
                (0..column_count)
                    .map(|index| row.get::<_, Option<String>>(index))
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        tables.push(SqliteTable {
            name,
            headers,
            rows,
        });
    }
    let row_count = tables.iter().map(|table| table.rows.len()).sum();
    let semantic = sqlite_semantic_bytes(&tables);
    Ok(SqliteManifest {
        path: relative_path,
        byte_sha256: sha256(&bytes),
        semantic_sha256: sha256(&semantic),
        bytes: bytes.len() as u64,
        table_count: tables.len(),
        row_count,
        tables,
    })
}

fn sqlite_error(error: rusqlite::Error) -> FixtureError {
    FixtureError::Invalid(format!("SQLite fixture error: {error}"))
}

fn quote_sqlite_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn compare_projection(
    categories: &[Category],
    sqlite: &SqliteManifest,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let csv_by_table: BTreeMap<_, _> = categories
        .iter()
        .map(|category| (category.table.as_str(), category))
        .collect();
    let sqlite_by_table: BTreeMap<_, _> = sqlite
        .tables
        .iter()
        .map(|table| (table.name.as_str(), table))
        .collect();
    for table in csv_by_table
        .keys()
        .filter(|table| !sqlite_by_table.contains_key(**table))
    {
        diagnostics.push(Diagnostic {
            code: "sqlite-missing-csv-table".into(),
            path: sqlite.path.clone(),
            location: format!("table[{table}]"),
            message: format!("generated SQLite artifact is missing CSV category table `{table}`"),
        });
    }
    for table in sqlite_by_table
        .keys()
        .filter(|table| !csv_by_table.contains_key(**table))
    {
        diagnostics.push(Diagnostic {
            code: "sqlite-table-without-csv".into(),
            path: sqlite.path.clone(),
            location: format!("table[{table}]"),
            message: format!(
                "generated SQLite artifact contains table `{table}` without a current CSV category"
            ),
        });
    }
    for (name, category) in csv_by_table {
        let Some(table) = sqlite_by_table.get(name) else {
            continue;
        };
        if category.document.headers != table.headers {
            diagnostics.push(Diagnostic {
                code: "sqlite-table-schema-differs-from-csv".into(),
                path: sqlite.path.clone(),
                location: format!("table[{name}]"),
                message: format!("generated SQLite table `{name}` does not preserve the current CSV headers exactly"),
            });
            continue;
        }
        if category.document.rows.len() != table.rows.len() {
            diagnostics.push(Diagnostic {
                code: "sqlite-table-row-count-differs-from-csv".into(),
                path: sqlite.path.clone(),
                location: format!("table[{name}]"),
                message: format!(
                    "generated SQLite table `{name}` has {} rows; CSV has {}",
                    table.rows.len(),
                    category.document.rows.len()
                ),
            });
        }
        let ipn_index = category
            .document
            .headers
            .iter()
            .position(|header| header == "IPN");
        for (row_index, (csv_row, sqlite_row)) in
            category.document.rows.iter().zip(&table.rows).enumerate()
        {
            let row_key = ipn_index
                .and_then(|index| csv_row.get(index))
                .filter(|value| !value.is_empty())
                .map_or_else(
                    || format!("row={row_index}"),
                    |value| format!("IPN={value}"),
                );
            for (field_index, (csv_value, sqlite_value)) in
                csv_row.iter().zip(sqlite_row).enumerate()
            {
                if sqlite_value.as_ref() != Some(csv_value) {
                    let header = &category.document.headers[field_index];
                    let mismatch = match sqlite_value {
                        None if csv_value.is_empty() => {
                            "SQLite NULL does not match CSV empty string".to_owned()
                        }
                        None => format!("SQLite NULL does not match CSV value `{csv_value}`"),
                        Some(value) => {
                            format!("SQLite value `{value}` does not match CSV value `{csv_value}`")
                        }
                    };
                    diagnostics.push(Diagnostic {
                        code: "sqlite-cell-differs-from-csv".into(),
                        path: sqlite.path.clone(),
                        location: format!("table[{name}].rows[{row_key}].fields[{header}]"),
                        message: mismatch,
                    });
                }
            }
        }
    }
}

fn read_descriptor(root: &Path, csv_tables: &[String]) -> Result<DescriptorManifest, FixtureError> {
    let path = root.join("database/#gplm.kicad_dbl");
    let bytes = read_checked(root, &path)?;
    let value: Value = serde_json::from_slice(&bytes)?;
    let libraries = value
        .get("libraries")
        .and_then(Value::as_array)
        .ok_or_else(|| FixtureError::Invalid("descriptor `.libraries` must be an array".into()))?;
    let mut tables = Vec::new();
    for (index, library) in libraries.iter().enumerate() {
        let object = library.as_object().ok_or_else(|| {
            FixtureError::Invalid(format!(
                "descriptor `.libraries[{index}]` must be an object"
            ))
        })?;
        for property in ["name", "table", "key", "symbols", "footprints"] {
            if !object.get(property).is_some_and(Value::is_string) {
                return Err(FixtureError::Invalid(format!(
                    "descriptor `.libraries[{index}].{property}` must be a string"
                )));
            }
        }
        let fields = object
            .get("fields")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                FixtureError::Invalid(format!(
                    "descriptor `.libraries[{index}].fields` must be an array"
                ))
            })?;
        for (field_index, field) in fields.iter().enumerate() {
            let field = field.as_object().ok_or_else(|| {
                FixtureError::Invalid(format!(
                    "descriptor `.libraries[{index}].fields[{field_index}]` must be an object"
                ))
            })?;
            for property in ["column", "name"] {
                if !field.get(property).is_some_and(Value::is_string) {
                    return Err(FixtureError::Invalid(format!(
                        "descriptor `.libraries[{index}].fields[{field_index}].{property}` must be a string"
                    )));
                }
            }
            for property in ["visible_on_add", "visible_in_chooser", "show_name"] {
                if !field.get(property).is_some_and(Value::is_boolean) {
                    return Err(FixtureError::Invalid(format!(
                        "descriptor `.libraries[{index}].fields[{field_index}].{property}` must be a boolean"
                    )));
                }
            }
        }
        let table = object["table"].as_str().expect("validated table string");
        tables.push(table.to_owned());
    }
    let unique_tables: BTreeSet<_> = tables.iter().collect();
    if unique_tables.len() != tables.len() {
        return Err(FixtureError::Invalid(
            "descriptor library table names must be unique".into(),
        ));
    }
    let semantic = canonical_json(&value);
    let csv_set: BTreeSet<_> = csv_tables.iter().cloned().collect();
    let descriptor_set: BTreeSet<_> = tables.iter().cloned().collect();
    Ok(DescriptorManifest {
        path: stable_path(root, &path)?,
        byte_sha256: sha256(&bytes),
        semantic_sha256: sha256(semantic.as_bytes()),
        entry_count: tables.len(),
        tables,
        csv_tables_not_exposed: csv_set.difference(&descriptor_set).cloned().collect(),
        descriptor_tables_without_csv: descriptor_set.difference(&csv_set).cloned().collect(),
    })
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => serde_json::to_string(v).expect("string serialization"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut pairs: Vec<_> = values.iter().collect();
            pairs.sort_by_key(|(key, _)| *key);
            format!(
                "{{{}}}",
                pairs
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("key serialization"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn read_assets(
    root: &Path,
    categories: &[Category],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<AssetManifest, FixtureError> {
    let symbols = asset_files(root, &root.join("symbols"), "kicad_sym")?;
    let footprints = asset_files(root, &root.join("footprints"), "kicad_mod")?;
    let models_3d = asset_files(root, &root.join("3d-models"), "*")?;
    let symbol_references = catalog_references(root, categories, "Symbol", "symbol", diagnostics)?;
    let footprint_references =
        catalog_references(root, categories, "Footprint", "footprint", diagnostics)?;
    let model_references = model_references(root, &footprints, diagnostics)?;
    Ok(AssetManifest {
        symbols,
        footprints,
        models_3d,
        symbol_references,
        footprint_references,
        model_references,
    })
}

fn asset_files(root: &Path, directory: &Path, extension: &str) -> Result<Vec<Asset>, FixtureError> {
    let mut paths = Vec::new();
    collect_files(root, directory, &mut paths)?;
    if extension != "*" {
        paths.retain(|path| path.extension().and_then(|e| e.to_str()) == Some(extension));
    }
    let mut stable_paths = paths
        .into_iter()
        .map(|path| stable_path(root, &path).map(|stable| (stable, path)))
        .collect::<Result<Vec<_>, _>>()?;
    stable_paths.sort_by(|left, right| left.0.cmp(&right.0));
    stable_paths
        .into_iter()
        .map(|(stable, path)| {
            let bytes = read_checked(root, &path)?;
            let semantic = if extension == "kicad_sym" || extension == "kicad_mod" {
                normalize_sexpr(&bytes)
            } else {
                normalize_text(&bytes)
            };
            Ok(Asset {
                path: stable,
                byte_sha256: sha256(&bytes),
                semantic_sha256: sha256(&semantic),
                bytes: bytes.len() as u64,
            })
        })
        .collect()
}

fn catalog_references(
    root: &Path,
    categories: &[Category],
    column: &str,
    kind: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<CadReference>, FixtureError> {
    let mut uses: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for category in categories {
        let Some(column_index) = category.document.headers.iter().position(|h| h == column) else {
            continue;
        };
        let ipn_index = category.document.headers.iter().position(|h| h == "IPN");
        for row in &category.document.rows {
            let ipn = ipn_index
                .and_then(|i| row.get(i))
                .cloned()
                .unwrap_or_default();
            for reference in row
                .get(column_index)
                .into_iter()
                .flat_map(|value| value.split(';'))
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                uses.entry(reference.to_owned())
                    .or_default()
                    .insert(ipn.clone());
            }
        }
    }
    let mut result = Vec::new();
    for (reference, source_ipns) in uses {
        let (namespace, member) = reference
            .split_once(':')
            .unwrap_or((reference.as_str(), ""));
        let local = namespace.starts_with("g-");
        let (status, asset_path) = if !local {
            ("external".to_owned(), None)
        } else if member.is_empty() {
            ("invalid".to_owned(), None)
        } else if kind == "symbol" {
            resolve_symbol(root, namespace, member)?
        } else {
            resolve_footprint(root, namespace, member)?
        };
        if local && status != "resolved" {
            diagnostics.push(Diagnostic {
                code: format!("{status}-local-{kind}-reference"),
                path: "database/g-*.csv".into(),
                location: format!("{column}={reference}"),
                message: format!(
                    "local {kind} reference `{reference}` is {status}; used by {}",
                    source_ipns.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            });
        }
        result.push(CadReference {
            kind: kind.into(),
            reference,
            source_ipns: source_ipns.into_iter().collect(),
            local,
            status,
            asset_path,
        });
    }
    Ok(result)
}

fn resolve_symbol(
    root: &Path,
    namespace: &str,
    member: &str,
) -> Result<(String, Option<String>), FixtureError> {
    if !safe_reference_component(namespace) || !safe_reference_component(member) {
        return Ok(("unsafe-path".into(), None));
    }
    let path = root.join("symbols").join(format!("{namespace}.kicad_sym"));
    if !checked_file_exists(root, &path)? {
        return Ok(("missing-library".into(), None));
    }
    let content = read_checked_to_string(root, &path)?;
    let needle = format!("(symbol \"{member}\"");
    match content.matches(&needle).count() {
        0 => Ok(("missing-member".into(), Some(stable_path(root, &path)?))),
        1 => Ok(("resolved".into(), Some(stable_path(root, &path)?))),
        _ => Ok(("ambiguous".into(), Some(stable_path(root, &path)?))),
    }
}

fn resolve_footprint(
    root: &Path,
    namespace: &str,
    member: &str,
) -> Result<(String, Option<String>), FixtureError> {
    if !safe_reference_component(namespace) || !safe_reference_component(member) {
        return Ok(("unsafe-path".into(), None));
    }
    let path = root
        .join("footprints")
        .join(format!("{namespace}.pretty"))
        .join(format!("{member}.kicad_mod"));
    if checked_file_exists(root, &path)? {
        Ok(("resolved".into(), Some(stable_path(root, &path)?)))
    } else {
        Ok(("missing-member".into(), None))
    }
}

fn model_references(
    root: &Path,
    footprints: &[Asset],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<ModelReference>, FixtureError> {
    let mut refs = Vec::new();
    for asset in footprints {
        let path = safe_join(root, &asset.path).ok_or_else(|| {
            FixtureError::Invalid(format!("manifest asset path is unsafe: {}", asset.path))
        })?;
        let content = read_checked_to_string(root, &path)?;
        for reference in extract_model_paths(&content) {
            let classified = classify_model_reference(root, &reference);
            let (status, asset_path) = match classified.candidate {
                Some(candidate) if checked_file_exists(root, &candidate)? => {
                    ("resolved", Some(stable_path(root, &candidate)?))
                }
                Some(_) => ("missing", None),
                None => (classified.status, None),
            };
            if status != "resolved" {
                diagnostics.push(Diagnostic {
                    code: format!("{status}-3d-model-reference"),
                    path: asset.path.clone(),
                    location: format!("model={reference}"),
                    message: format!(
                        "3D model reference `{reference}` is {status} ({})",
                        classified.scope
                    ),
                });
            }
            refs.push(ModelReference {
                source_footprint: asset.path.clone(),
                reference,
                scope: classified.scope.into(),
                status: status.into(),
                asset_path,
            });
        }
    }
    refs.sort();
    refs.dedup();
    Ok(refs)
}

struct ClassifiedModelReference {
    scope: &'static str,
    status: &'static str,
    candidate: Option<PathBuf>,
}

fn classify_model_reference(root: &Path, reference: &str) -> ClassifiedModelReference {
    if let Some(rest) = strip_variable_path(reference, "${GITPLM_PARTS}") {
        return match strip_path_prefix(rest, "3d-models") {
            Some(model) => local_model_reference(&root.join("3d-models"), model),
            None => unsafe_local_model_reference(),
        };
    }
    if let Some(rest) = strip_variable_path(reference, "${GITPLM_3DMODELS}") {
        return local_model_reference(&root.join("3d-models"), rest);
    }
    let scope = if is_unc_path(reference) {
        "unc-absolute"
    } else if is_windows_drive_absolute(reference) {
        "windows-drive-absolute"
    } else if reference.starts_with('/') {
        "posix-absolute"
    } else if reference.starts_with('\\') {
        "windows-rooted"
    } else if is_windows_drive_relative(reference) {
        "windows-drive-relative"
    } else if reference.starts_with("${") {
        "external-variable"
    } else {
        "relative"
    };
    ClassifiedModelReference {
        scope,
        status: "unresolved",
        candidate: None,
    }
}

fn strip_path_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\')))
}

fn strip_variable_path<'a>(reference: &'a str, variable: &str) -> Option<&'a str> {
    reference
        .strip_prefix(variable)
        .and_then(|rest| rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\')))
}

fn local_model_reference(base: &Path, relative: &str) -> ClassifiedModelReference {
    match safe_join(base, relative) {
        Some(candidate) => ClassifiedModelReference {
            scope: "local",
            status: "missing",
            candidate: Some(candidate),
        },
        None => unsafe_local_model_reference(),
    }
}

fn unsafe_local_model_reference() -> ClassifiedModelReference {
    ClassifiedModelReference {
        scope: "local",
        status: "unsafe-path",
        candidate: None,
    }
}

fn is_unc_path(value: &str) -> bool {
    value.starts_with("\\\\") || value.starts_with("//")
}

fn is_windows_drive_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn is_windows_drive_relative(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn extract_model_paths(content: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for suffix in content.split("(model").skip(1) {
        let value = suffix.trim_start();
        if let Some(value) = value.strip_prefix('"') {
            let mut escaped = false;
            let mut output = String::new();
            for ch in value.chars() {
                if !escaped && ch == '"' {
                    break;
                }
                escaped = !escaped && ch == '\\';
                output.push(ch);
                if ch != '\\' {
                    escaped = false;
                }
            }
            paths.push(output);
        }
    }
    paths
}

fn normalize_sexpr(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    let mut token = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let flush = |out: &mut Vec<u8>, token: &mut Vec<u8>| {
        if !token.is_empty() {
            out.extend_from_slice(&(token.len() as u64).to_be_bytes());
            out.append(token);
        }
    };
    for ch in text.trim_start_matches('\u{feff}').chars() {
        if quoted {
            token.extend_from_slice(ch.to_string().as_bytes());
            if ch == '"' && !escaped {
                quoted = false;
                flush(&mut out, &mut token);
            }
            escaped = ch == '\\' && !escaped;
            if ch != '\\' {
                escaped = false;
            }
        } else if ch == '"' {
            flush(&mut out, &mut token);
            quoted = true;
            token.push(b'"');
        } else if ch == '(' || ch == ')' {
            flush(&mut out, &mut token);
            out.push(ch as u8);
        } else if ch.is_whitespace() {
            flush(&mut out, &mut token);
        } else {
            token.extend_from_slice(ch.to_string().as_bytes());
        }
    }
    flush(&mut out, &mut token);
    out
}

fn normalize_text(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let normalized = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut lines: Vec<_> = normalized
        .lines()
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n").into_bytes()
}

fn parse_update_tables(script: &str) -> Vec<String> {
    script
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("GPLMLIBS=\"")
                .and_then(|v| v.strip_suffix('"'))
        })
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn framed_strings<'a>(values: impl Iterator<Item = &'a String>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes
}

fn csv_semantic_bytes(headers: &[String], rows: &[Vec<String>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(headers.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&(rows.len() as u64).to_be_bytes());
    bytes.extend(framed_strings(headers.iter().chain(rows.iter().flatten())));
    bytes
}

fn sqlite_semantic_bytes(tables: &[SqliteTable]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(tables.len() as u64).to_be_bytes());
    for table in tables {
        bytes.extend(framed_strings(std::iter::once(&table.name)));
        bytes.extend_from_slice(&(table.headers.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&(table.rows.len() as u64).to_be_bytes());
        bytes.extend(framed_strings(table.headers.iter()));
        for cell in table.rows.iter().flatten() {
            match cell {
                None => bytes.push(0),
                Some(value) => {
                    bytes.push(1);
                    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
                    bytes.extend_from_slice(value.as_bytes());
                }
            }
        }
    }
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn stable_path(root: &Path, path: &Path) -> Result<String, FixtureError> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| FixtureError::Invalid("path escapes repository root".into()))
}

fn read_dir_files(
    root: &Path,
    directory: &Path,
    include: impl Fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, FixtureError> {
    ensure_directory(root, directory)?;
    let entries = collect_entry_results(fs::read_dir(directory)?)?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        reject_link(root, &path, &metadata)?;
        if include(&path) {
            if !metadata.is_file() {
                return Err(FixtureError::Invalid(format!(
                    "regular fixture file expected at `{}`",
                    stable_path(root, &path)?
                )));
            }
            paths.push(path);
        }
    }
    Ok(paths)
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), FixtureError> {
    ensure_directory(root, directory)?;
    let entries = collect_entry_results(fs::read_dir(directory)?)?;
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        reject_link(root, &path, &metadata)?;
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        } else {
            return Err(FixtureError::Invalid(format!(
                "unsupported fixture entry type at `{}`",
                stable_path(root, &path)?
            )));
        }
    }
    Ok(())
}

fn collect_entry_results<T, E>(entries: impl Iterator<Item = Result<T, E>>) -> Result<Vec<T>, E> {
    entries.collect()
}

fn ensure_directory(root: &Path, path: &Path) -> Result<(), FixtureError> {
    let metadata = fs::symlink_metadata(path)?;
    reject_link(root, path, &metadata)?;
    if !metadata.is_dir() {
        return Err(FixtureError::Invalid(format!(
            "fixture directory expected at `{}`",
            stable_path(root, path)?
        )));
    }
    Ok(())
}

fn checked_file_exists(root: &Path, path: &Path) -> Result<bool, FixtureError> {
    Ok(checked_exact_file(root, path)?.is_some())
}

fn read_checked(root: &Path, path: &Path) -> Result<Vec<u8>, FixtureError> {
    let exact = checked_exact_file(root, path)?.ok_or_else(|| {
        FixtureError::Invalid(format!(
            "fixture file missing at `{}`",
            stable_path(root, path).unwrap_or_else(|_| "<outside-root>".into())
        ))
    })?;
    Ok(fs::read(exact)?)
}

fn checked_exact_file(root: &Path, path: &Path) -> Result<Option<PathBuf>, FixtureError> {
    use std::path::Component;

    let relative = path
        .strip_prefix(root)
        .map_err(|_| FixtureError::Invalid("path escapes repository root".into()))?;
    let components: Vec<_> = relative.components().collect();
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(FixtureError::Invalid(
                "unsafe fixture path component".into(),
            ));
        };
        let entries = collect_entry_results(fs::read_dir(&current)?)?;
        let Some(entry) = entries.into_iter().find(|entry| entry.file_name() == *name) else {
            return Ok(None);
        };
        current = entry.path();
        let metadata = fs::symlink_metadata(&current)?;
        reject_link(root, &current, &metadata)?;
        if index + 1 == components.len() {
            return Ok(metadata.is_file().then_some(current));
        }
        if !metadata.is_dir() {
            return Ok(None);
        }
    }
    Ok(None)
}

fn read_checked_to_string(root: &Path, path: &Path) -> Result<String, FixtureError> {
    String::from_utf8(read_checked(root, path)?).map_err(|_| {
        FixtureError::Invalid(format!(
            "fixture text is not UTF-8 at `{}`",
            stable_path(root, path).unwrap_or_else(|_| "<outside-root>".into())
        ))
    })
}

fn reject_link(root: &Path, path: &Path, metadata: &fs::Metadata) -> Result<(), FixtureError> {
    if is_link_or_reparse(metadata) {
        return Err(FixtureError::Invalid(format!(
            "fixture links are forbidden at `{}`",
            stable_path(root, path)?
        )));
    }
    Ok(())
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn safe_reference_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !is_windows_drive_relative(value)
}

fn safe_join(base: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty()
        || is_unc_path(relative)
        || is_windows_drive_relative(relative)
        || relative.starts_with(['/', '\\'])
    {
        return None;
    }
    let mut joined = base.to_path_buf();
    for component in relative.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return None,
            value if value.contains(':') => return None,
            value => joined.push(value),
        }
    }
    Some(joined)
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[test]
    fn model_reference_classification_is_host_independent() {
        let root = Path::new("fixture-root");
        let cases = [
            ("/home/user/model.step", "posix-absolute", "unresolved"),
            (
                "C:/Models/model.step",
                "windows-drive-absolute",
                "unresolved",
            ),
            (
                "D:\\Models\\model.step",
                "windows-drive-absolute",
                "unresolved",
            ),
            (
                "\\\\server\\share\\model.step",
                "unc-absolute",
                "unresolved",
            ),
            ("//server/share/model.step", "unc-absolute", "unresolved"),
            ("\\Models\\model.step", "windows-rooted", "unresolved"),
            ("C:model.step", "windows-drive-relative", "unresolved"),
            (
                "${KICAD9_3DMODEL_DIR}/model.step",
                "external-variable",
                "unresolved",
            ),
            ("models/model.step", "relative", "unresolved"),
            ("${GITPLM_PARTS}/3d-models/model.step", "local", "missing"),
            ("${GITPLM_PARTS}\\3d-models\\model.step", "local", "missing"),
            ("${GITPLM_PARTS}/symbols/model.step", "local", "unsafe-path"),
        ];
        for (reference, scope, status) in cases {
            let classified = classify_model_reference(root, reference);
            assert_eq!(classified.scope, scope, "{reference}");
            assert_eq!(classified.status, status, "{reference}");
        }
    }

    #[test]
    fn safe_join_rejects_escape_and_absolute_forms() {
        let root = Path::new("fixture-root");
        for rejected in [
            "../sentinel",
            "models/../../sentinel",
            "/absolute/model.step",
            "C:/absolute/model.step",
            "C:\\absolute\\model.step",
            "\\\\server\\share\\model.step",
            "C:drive-relative.step",
        ] {
            assert!(safe_join(root, rejected).is_none(), "{rejected}");
        }
        assert_eq!(
            safe_join(root, "3d-models/model.step"),
            Some(root.join("3d-models/model.step"))
        );
    }

    #[test]
    fn directory_entry_errors_are_not_discarded() {
        let entries: Vec<Result<u8, &str>> = vec![Ok(1), Err("entry failed"), Ok(2)];
        assert_eq!(
            collect_entry_results(entries.into_iter()),
            Err("entry failed")
        );
    }

    #[test]
    fn matching_non_file_entry_fails_closed() {
        let repository = tempfile::tempdir().unwrap();
        let database = repository.path().join("database");
        fs::create_dir(&database).unwrap();
        fs::create_dir(database.join("g-not-a-file.csv")).unwrap();
        let error = read_dir_files(repository.path(), &database, |path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("g-") && name.ends_with(".csv"))
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("regular fixture file expected"), "{error}");
        assert!(error.contains("database/g-not-a-file.csv"), "{error}");
    }

    #[test]
    fn exact_file_lookup_has_platform_independent_case_semantics() {
        let repository = tempfile::tempdir().unwrap();
        let symbols = repository.path().join("symbols");
        fs::create_dir(&symbols).unwrap();
        fs::write(symbols.join("g-Case.kicad_sym"), "fixture").unwrap();
        assert!(
            checked_exact_file(repository.path(), &symbols.join("g-Case.kicad_sym"))
                .unwrap()
                .is_some()
        );
        assert!(
            checked_exact_file(repository.path(), &symbols.join("g-case.kicad_sym"))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn recursive_discovery_rejects_link_before_external_target_is_read() {
        use std::os::unix::fs::symlink;

        let repository = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let assets = repository.path().join("symbols");
        fs::create_dir(&assets).unwrap();
        let sentinel = external.path().join("sentinel.kicad_sym");
        fs::write(&sentinel, b"outside repository").unwrap();
        symlink(&sentinel, assets.join("g-escape.kicad_sym")).unwrap();

        let error = asset_files(repository.path(), &assets, "kicad_sym")
            .unwrap_err()
            .to_string();
        assert!(error.contains("fixture links are forbidden"), "{error}");
        assert!(error.contains("symbols/g-escape.kicad_sym"), "{error}");
        assert!(
            !error.contains(&external.path().display().to_string()),
            "{error}"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"outside repository");
    }
}

//! Reads KiCad's library tables: the person's `sym-lib-table` and
//! `fp-lib-table` in KiCad's settings folder, which list the libraries they
//! added to KiCad, each by a nickname. Only ever reads; never writes.
//!
//! A table is an s-expression such as
//!
//! ```text
//! (sym_lib_table
//!   (version 7)
//!   (lib (name "Device") (type "KiCad") (uri "${KICAD10_SYMBOL_DIR}/Device.kicad_sym")
//!        (options "") (descr "Resistors, capacitors...") (disabled) (hidden))
//!   (lib (name "KiCad") (type "Table") (uri "${KICAD10_TEMPLATE_DIR}/sym-lib-table")))
//! ```
//!
//! A row of type `Table` (KiCad 10) names another table, whose rows are
//! read as if they were in this one. Paths use variables such as
//! `${KICAD10_SYMBOL_DIR}`, expanded by [`PathVariables`] from the process
//! environment, KiCad's `kicad_common.json` and KiCad's own defaults, in
//! that order, as KiCad does.
//!
//! The desktop app reads the person's own tables at run time; tests pass a
//! folder of fixture tables instead, so they never read a developer's
//! KiCad settings.

use crate::library_file::{LibraryFileError, Sexp};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The largest table file read. KiCad's own template tables are under
/// 50 KB.
pub const MAX_TABLE_BYTES: u64 = 4 * 1024 * 1024;

/// How deep `Table` rows may nest.
pub const MAX_TABLE_NESTING: usize = 8;

/// Which table: symbols or footprints.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TableKind {
    Symbol,
    Footprint,
}

impl TableKind {
    /// The table's file name in KiCad's settings folder.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Symbol => "sym-lib-table",
            Self::Footprint => "fp-lib-table",
        }
    }

    /// The table's root list head.
    fn head(self) -> &'static str {
        match self {
            Self::Symbol => "sym_lib_table",
            Self::Footprint => "fp_lib_table",
        }
    }
}

/// One `(lib ...)` row, exactly as written (paths not expanded).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibTableRow {
    /// The nickname references use, such as `Device` in `Device:R`.
    pub name: String,
    /// `KiCad`, `Table`, `Database`, `Legacy`, `Eagle`...
    pub kind: String,
    pub uri: String,
    pub options: String,
    pub descr: String,
    pub disabled: bool,
    /// KiCad keeps the library but does not offer it in its choosers.
    pub hidden: bool,
}

impl LibTableRow {
    /// A KiCad-format library (`.kicad_sym`, `.kicad_symdir` or `.pretty`).
    pub fn is_kicad(&self) -> bool {
        self.kind.eq_ignore_ascii_case("KiCad")
    }

    /// A row naming another table (KiCad 10).
    pub fn is_table(&self) -> bool {
        self.kind.eq_ignore_ascii_case("Table")
    }
}

/// A parsed table file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibTable {
    pub version: Option<String>,
    pub rows: Vec<LibTableRow>,
}

impl LibTable {
    /// Reads a table of `kind` from its bytes.
    pub fn parse(bytes: &[u8], kind: TableKind) -> Result<Self, LibraryFileError> {
        let tree = Sexp::parse(bytes)?;
        if tree.head() != Some(kind.head()) {
            return Err(LibraryFileError(format!(
                "a {} starts with `{}`, not `{}`",
                kind.file_name(),
                kind.head(),
                tree.head().unwrap_or_default()
            )));
        }
        let version = tree
            .children("version")
            .next()
            .and_then(|version| version.word_at(1))
            .map(str::to_owned);
        let rows = tree
            .children("lib")
            .map(|lib| {
                let text = |head: &str| {
                    lib.children(head)
                        .next()
                        .and_then(|item| item.word_at(1))
                        .unwrap_or_default()
                        .to_owned()
                };
                let flag = |head: &str| {
                    lib.children(head).next().is_some()
                        || lib.items().iter().any(|item| item.word() == Some(head))
                };
                LibTableRow {
                    name: text("name"),
                    kind: text("type"),
                    uri: text("uri"),
                    options: text("options"),
                    descr: text("descr"),
                    disabled: flag("disabled"),
                    hidden: flag("hidden"),
                }
            })
            .filter(|row| !row.name.is_empty())
            .collect();
        Ok(Self { version, rows })
    }

    /// The table as KiCad 10 writes a table of `kind` (version 7), one row
    /// per line. KiCad's Manage Libraries dialog takes this text pasted in.
    pub fn format(&self, kind: TableKind) -> String {
        use crate::library_file::escape_text;
        let mut text = format!("({}\n  (version 7)\n", kind.head());
        for row in &self.rows {
            text.push_str(&format!(
                "  (lib (name \"{}\")(type \"{}\")(uri \"{}\")(options \"{}\")(descr \"{}\")",
                escape_text(&row.name),
                escape_text(&row.kind),
                escape_text(&row.uri),
                escape_text(&row.options),
                escape_text(&row.descr)
            ));
            if row.disabled {
                text.push_str("(disabled)");
            }
            if row.hidden {
                text.push_str("(hidden)");
            }
            text.push_str(")\n");
        }
        text.push_str(")\n");
        text
    }
}

/// Path variables, such as `KICAD10_SYMBOL_DIR`, and their values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PathVariables {
    values: BTreeMap<String, String>,
}

/// Where [`PathVariables::for_kicad`] takes values from. The desktop app
/// fills it from the computer; tests fill it by hand.
#[derive(Clone, Debug, Default)]
pub struct VariableSources {
    /// The process environment; wins over everything else, as in KiCad.
    pub environment: BTreeMap<String, String>,
    /// KiCad's settings folder, holding `kicad_common.json`.
    pub config_dir: Option<PathBuf>,
    /// KiCad's installed data folder (holding `symbols/`, `footprints/`).
    pub share_dir: Option<PathBuf>,
    /// The person's Documents folder, for `KICAD<n>_3RD_PARTY`.
    pub documents_dir: Option<PathBuf>,
    /// KiCad's major version, such as 10.
    pub major: u32,
}

impl PathVariables {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self { values }
    }

    /// KiCad's variables: the process environment first, then the
    /// variables set in KiCad's `kicad_common.json`, then KiCad's own
    /// defaults (`KICAD<n>_SYMBOL_DIR` and the like). Also returns why
    /// `kicad_common.json` could not be read, if it exists but could not.
    pub fn for_kicad(sources: &VariableSources) -> (Self, Option<String>) {
        let mut values = BTreeMap::new();
        let major = sources.major;
        if let Some(share) = &sources.share_dir {
            for (name, folder) in [
                ("SYMBOL_DIR", "symbols"),
                ("FOOTPRINT_DIR", "footprints"),
                ("3DMODEL_DIR", "3dmodels"),
                ("TEMPLATE_DIR", "template"),
            ] {
                values.insert(
                    format!("KICAD{major}_{name}"),
                    share.join(folder).to_string_lossy().into_owned(),
                );
            }
        }
        if let Some(documents) = &sources.documents_dir {
            let user = documents.join("KiCad").join(format!("{major}.0"));
            values.insert(
                format!("KICAD{major}_3RD_PARTY"),
                user.join("3rdparty").to_string_lossy().into_owned(),
            );
            values.insert(
                "KICAD_USER_TEMPLATE_DIR".to_owned(),
                user.join("template").to_string_lossy().into_owned(),
            );
        }
        let mut problem = None;
        if let Some(config) = &sources.config_dir {
            match read_common_variables(&config.join("kicad_common.json")) {
                Ok(set) => values.extend(set),
                Err(why) => problem = Some(why),
            }
        }
        values.extend(
            sources
                .environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        (Self { values }, problem)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// `text` with every `${NAME}` and `$(NAME)` replaced by its value; a
    /// value may use other variables. Refuses with the first name that has
    /// no value.
    pub fn expand(&self, text: &str) -> Result<String, MissingVariable> {
        self.expand_nested(text, 0)
    }

    fn expand_nested(&self, text: &str, depth: usize) -> Result<String, MissingVariable> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find('$') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let close = match after.chars().next() {
                Some('{') => '}',
                Some('(') => ')',
                _ => {
                    out.push('$');
                    rest = after;
                    continue;
                }
            };
            let Some(end) = after.find(close) else {
                out.push_str(&rest[at..]);
                rest = "";
                break;
            };
            let name = &after[1..end];
            let Some(value) = self.values.get(name) else {
                return Err(MissingVariable(name.to_owned()));
            };
            if depth < 8 && value.contains('$') {
                out.push_str(&self.expand_nested(value, depth + 1)?);
            } else {
                out.push_str(value);
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

/// A path variable with no value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingVariable(pub String);

impl fmt::Display for MissingVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the path variable ${{{}}} is not set", self.0)
    }
}

/// The variables set in a `kicad_common.json`
/// (`{"environment": {"vars": {"NAME": "value"}}}`); none when the file is
/// missing.
fn read_common_variables(file: &Path) -> Result<BTreeMap<String, String>, String> {
    let bytes = match read_limited(file) {
        Ok(bytes) => bytes,
        Err(ReadError::Missing) => return Ok(BTreeMap::new()),
        Err(ReadError::Other(why)) => return Err(why),
    };
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} could not be read: {error}", file.display()))?;
    let vars = json
        .get("environment")
        .and_then(|environment| environment.get("vars"))
        .and_then(serde_json::Value::as_object);
    Ok(vars
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
        .collect())
}

/// A library a table lists, with its path expanded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableLibrary {
    pub row: LibTableRow,
    /// The library's file or folder, or why it could not be worked out.
    pub path: Result<PathBuf, String>,
    /// The table file that lists it.
    pub table: PathBuf,
}

/// What [`read_tables`] found.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableReading {
    /// Every enabled library row, in table order, nested tables in place of
    /// their row. A nickname listed twice is kept only the first time, as
    /// KiCad finds the first.
    pub libraries: Vec<TableLibrary>,
    /// Tables that could not be read, in plain words.
    pub problems: Vec<String>,
    /// Whether the top table file exists.
    pub found: bool,
}

/// Reads the person's table of `kind` in KiCad's settings folder
/// `config_dir` (such as `%APPDATA%\kicad\10.0`), following `Table` rows.
pub fn read_tables(config_dir: &Path, kind: TableKind, vars: &PathVariables) -> TableReading {
    let mut reading = TableReading::default();
    let top = config_dir.join(kind.file_name());
    reading.found = top.is_file();
    if reading.found {
        let mut chain = Vec::new();
        let mut nicknames = BTreeSet::new();
        read_table_file(
            &top,
            kind,
            vars,
            0,
            &mut chain,
            &mut nicknames,
            &mut reading,
        );
    }
    reading
}

fn read_table_file(
    file: &Path,
    kind: TableKind,
    vars: &PathVariables,
    depth: usize,
    chain: &mut Vec<PathBuf>,
    nicknames: &mut BTreeSet<String>,
    reading: &mut TableReading,
) {
    // Only a table already on the way here is a loop; one reached again
    // through another branch is read again (its nicknames are kept once).
    let key = normalized(file);
    if chain.contains(&key) {
        reading.problems.push(format!(
            "{} names itself through its nested tables, so it was read once.",
            file.display()
        ));
        return;
    }
    chain.push(key);
    read_table_rows(file, kind, vars, depth, chain, nicknames, reading);
    chain.pop();
}

fn read_table_rows(
    file: &Path,
    kind: TableKind,
    vars: &PathVariables,
    depth: usize,
    chain: &mut Vec<PathBuf>,
    nicknames: &mut BTreeSet<String>,
    reading: &mut TableReading,
) {
    let bytes = match read_limited(file) {
        Ok(bytes) => bytes,
        Err(ReadError::Missing) => {
            reading.problems.push(format!(
                "The library table {} was not found.",
                file.display()
            ));
            return;
        }
        Err(ReadError::Other(why)) => {
            reading.problems.push(why);
            return;
        }
    };
    let table = match LibTable::parse(&bytes, kind) {
        Ok(table) => table,
        Err(error) => {
            reading.problems.push(format!(
                "The library table {} could not be read: {error}.",
                file.display()
            ));
            return;
        }
    };
    let folder = file.parent().unwrap_or(Path::new(""));
    for row in table.rows {
        if row.disabled {
            continue;
        }
        let path = vars
            .expand(&row.uri)
            .map(|expanded| {
                let path = PathBuf::from(expanded);
                if path.is_absolute() {
                    path
                } else {
                    folder.join(path)
                }
            })
            .map_err(|missing| format!("{missing}"));
        if row.is_table() {
            match path {
                Ok(nested) if depth + 1 < MAX_TABLE_NESTING => {
                    read_table_file(&nested, kind, vars, depth + 1, chain, nicknames, reading)
                }
                Ok(nested) => reading.problems.push(format!(
                    "{} is nested more than {MAX_TABLE_NESTING} tables deep, so it was not read.",
                    nested.display()
                )),
                Err(why) => reading.problems.push(format!(
                    "The nested table {} in {} could not be found: {why}.",
                    row.name,
                    file.display()
                )),
            }
            continue;
        }
        if !nicknames.insert(row.name.clone()) {
            continue;
        }
        reading.libraries.push(TableLibrary {
            row,
            path,
            table: file.to_path_buf(),
        });
    }
}

enum ReadError {
    Missing,
    Other(String),
}

/// A file's bytes, refusing one over [`MAX_TABLE_BYTES`] or a link.
fn read_limited(file: &Path) -> Result<Vec<u8>, ReadError> {
    let metadata = match fs::symlink_metadata(file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ReadError::Missing);
        }
        Err(error) => {
            return Err(ReadError::Other(format!(
                "{} could not be read: {error}.",
                file.display()
            )));
        }
    };
    if !metadata.is_file() {
        return Err(ReadError::Other(format!(
            "{} is not a file (links are not followed).",
            file.display()
        )));
    }
    if metadata.len() > MAX_TABLE_BYTES {
        return Err(ReadError::Other(format!(
            "{} is larger than {} MB, so it was not read.",
            file.display(),
            MAX_TABLE_BYTES / (1024 * 1024)
        )));
    }
    fs::read(file).map_err(|error| {
        ReadError::Other(format!("{} could not be read: {error}.", file.display()))
    })
}

/// KiCad's settings folder for KiCad `major`, as KiCad finds it:
/// `KICAD_CONFIG_HOME` when set, else `%APPDATA%\kicad` on Windows,
/// `~/Library/Preferences/kicad` on macOS and `$XDG_CONFIG_HOME/kicad`
/// (or `~/.config/kicad`) elsewhere; then `<major>.0`. `environment` is the
/// process environment. `None` when none of those is set.
pub fn user_config_dir(major: u32, environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    let get = |name: &str| {
        environment
            .get(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = get("KICAD_CONFIG_HOME").or_else(|| {
        if cfg!(windows) {
            get("APPDATA").map(|appdata| appdata.join("kicad"))
        } else if cfg!(target_os = "macos") {
            get("HOME").map(|home| home.join("Library").join("Preferences").join("kicad"))
        } else {
            get("XDG_CONFIG_HOME")
                .or_else(|| get("HOME").map(|home| home.join(".config")))
                .map(|config| config.join("kicad"))
        }
    })?;
    Some(base.join(format!("{major}.0")))
}

/// Whether `path` is `folder` or inside it, comparing names as written
/// after removing `.` and `..`; case-insensitively on Windows, whose file
/// names are.
pub fn is_inside(path: &Path, folder: &Path) -> bool {
    let path = normalized(path);
    let folder = normalized(folder);
    path.starts_with(&folder)
}

/// `path` with `.` and `..` removed and, on Windows, lowercased with `/`
/// as `\`, for comparing paths.
fn normalized(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let text = if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text.into_owned()
    };
    let mut normal = PathBuf::new();
    for component in Path::new(&text).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_formatted_table_reads_back_the_same() {
        let table = LibTable {
            version: Some("7".into()),
            rows: vec![
                LibTableRow {
                    name: "team-parts".into(),
                    kind: "Database".into(),
                    uri: "C:/Users/A \"B\"/team-parts.kicad_dbl".into(),
                    descr: "Parts".into(),
                    ..LibTableRow::default()
                },
                LibTableRow {
                    name: "g-res".into(),
                    kind: "KiCad".into(),
                    uri: "C:\\x\\g-res.kicad_sym".into(),
                    hidden: true,
                    ..LibTableRow::default()
                },
            ],
        };
        let text = table.format(TableKind::Symbol);
        assert!(
            text.starts_with(
                "(sym_lib_table\n  (version 7)\n  (lib (name \"team-parts\")(type \"Database\")"
            ),
            "{text}"
        );
        assert!(text.contains("(descr \"\")(hidden))\n)\n"), "{text}");
        assert_eq!(
            LibTable::parse(text.as_bytes(), TableKind::Symbol).unwrap(),
            table
        );
        let footprints = table.format(TableKind::Footprint);
        assert!(footprints.starts_with("(fp_lib_table\n"));
    }

    #[test]
    fn reads_rows_and_flags() {
        let table = r#"(sym_lib_table
  (version 7)
  (lib (name "Device")(type "KiCad")(uri "${KICAD10_SYMBOL_DIR}/Device.kicad_sym")(options "")(descr "Resistors"))
  (lib (name Old)(type Legacy)(uri x.lib)(options "")(descr "")(disabled))
  (lib (name "Quiet")(type "KiCad")(uri "q.kicad_sym")(options "")(descr "")(hidden))
)"#;
        let table = LibTable::parse(table.as_bytes(), TableKind::Symbol).unwrap();
        assert_eq!(table.version.as_deref(), Some("7"));
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[0].name, "Device");
        assert!(table.rows[0].is_kicad());
        assert_eq!(table.rows[0].descr, "Resistors");
        assert_eq!(table.rows[1].kind, "Legacy");
        assert!(table.rows[1].disabled);
        assert!(table.rows[2].hidden && !table.rows[2].disabled);
        assert!(LibTable::parse(b"(fp_lib_table)", TableKind::Symbol).is_err());
        assert!(LibTable::parse(b"(sym_lib_table (lib", TableKind::Symbol).is_err());
    }

    #[test]
    fn expands_variables_in_kicads_order() {
        let config = tempfile::tempdir().unwrap();
        write(
            &config.path().join("kicad_common.json"),
            r#"{"environment": {"show_warning_dialog": false, "vars": {"MY_LIBS": "${HOME_LIBS}/kicad", "BOTH": "from settings", "NULLED": null}}}"#,
        );
        let sources = VariableSources {
            environment: [
                ("HOME_LIBS".to_owned(), "D:/libs".to_owned()),
                ("BOTH".to_owned(), "from the environment".to_owned()),
            ]
            .into(),
            config_dir: Some(config.path().to_path_buf()),
            share_dir: Some(PathBuf::from("C:/KiCad/share/kicad")),
            documents_dir: Some(PathBuf::from("C:/Users/a/Documents")),
            major: 10,
        };
        let (vars, problem) = PathVariables::for_kicad(&sources);
        assert_eq!(problem, None);
        assert_eq!(
            vars.expand("${MY_LIBS}/a.kicad_sym").unwrap(),
            "D:/libs/kicad/a.kicad_sym"
        );
        assert_eq!(vars.get("BOTH"), Some("from the environment"));
        assert_eq!(vars.get("NULLED"), None);
        let symbols = vars
            .expand("$(KICAD10_SYMBOL_DIR)/Device.kicad_sym")
            .unwrap();
        assert!(symbols.starts_with("C:/KiCad/share/kicad"), "{symbols}");
        assert!(symbols.ends_with("Device.kicad_sym"), "{symbols}");
        assert!(vars.get("KICAD10_3RD_PARTY").unwrap().contains("10.0"));
        assert_eq!(
            vars.expand("${NOPE}/x"),
            Err(MissingVariable("NOPE".to_owned()))
        );
        assert_eq!(vars.expand("a $ b ${unclosed").unwrap(), "a $ b ${unclosed");
        // A broken kicad_common.json is reported, and the rest still works.
        write(&config.path().join("kicad_common.json"), "{not json");
        let (vars, problem) = PathVariables::for_kicad(&sources);
        assert!(problem.unwrap().contains("kicad_common.json"));
        assert_eq!(vars.get("HOME_LIBS"), Some("D:/libs"));
    }

    #[test]
    fn follows_nested_tables_and_keeps_the_first_nickname() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let shared = root.path().join("shared");
        write(
            &config.join("fp-lib-table"),
            r#"(fp_lib_table (version 7)
  (lib (name "Mine")(type "KiCad")(uri "${MINE}/g-SOT.pretty")(options "")(descr "My SOT footprints"))
  (lib (name "Team")(type "Table")(uri "${SHARED}/fp-lib-table")(options "")(descr ""))
  (lib (name "Gone")(type "KiCad")(uri "${UNSET}/x.pretty")(options "")(descr ""))
  (lib (name "Off")(type "KiCad")(uri "off.pretty")(options "")(descr "")(disabled))
  (lib (name "Lost")(type "Table")(uri "${SHARED}/missing-table")(options "")(descr "")))"#,
        );
        write(
            &shared.join("fp-lib-table"),
            r#"(fp_lib_table (version 7)
  (lib (name "Mine")(type "KiCad")(uri "elsewhere.pretty")(options "")(descr ""))
  (lib (name "Shared")(type "KiCad")(uri "shared.pretty")(options "")(descr ""))
  (lib (name "Loop")(type "Table")(uri "${SHARED}/fp-lib-table")(options "")(descr "")))"#,
        );
        let vars = PathVariables::new(
            [
                ("MINE".to_owned(), "D:/mine".to_owned()),
                ("SHARED".to_owned(), shared.to_string_lossy().into_owned()),
            ]
            .into(),
        );
        let reading = read_tables(&config, TableKind::Footprint, &vars);
        assert!(reading.found);
        let names: Vec<&str> = reading
            .libraries
            .iter()
            .map(|library| library.row.name.as_str())
            .collect();
        // A nickname differing from its file name is kept as written.
        assert_eq!(names, ["Mine", "Shared", "Gone"]);
        assert_eq!(
            reading.libraries[0].path,
            Ok(PathBuf::from("D:/mine/g-SOT.pretty"))
        );
        // A relative path is relative to its table's folder.
        assert_eq!(reading.libraries[1].path, Ok(shared.join("shared.pretty")));
        assert_eq!(reading.libraries[1].table, shared.join("fp-lib-table"));
        assert_eq!(
            reading.libraries[2].path,
            Err("the path variable ${UNSET} is not set".to_owned())
        );
        assert_eq!(reading.problems.len(), 2, "{:?}", reading.problems);
        assert!(reading.problems.iter().any(|p| p.contains("names itself")));
        assert!(reading.problems.iter().any(|p| p.contains("was not found")));
        // No table at all is no problem.
        let empty = read_tables(&root.path().join("none"), TableKind::Symbol, &vars);
        assert_eq!(empty, TableReading::default());
    }

    #[test]
    fn a_table_reached_twice_is_not_a_loop() {
        // Two branches include the same shared table; neither names itself.
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let row = |name: &str, kind: &str, uri: &str| {
            format!(
                "(lib (name \"{name}\")(type \"{kind}\")(uri \"{uri}\")(options \"\")(descr \"\"))"
            )
        };
        let table = |rows: &[String]| format!("(sym_lib_table (version 7) {})", rows.join(" "));
        let shared = root.path().join("shared-table");
        // Quoted strings treat a backslash as an escape, as KiCad does.
        let uri = |path: &Path| path.to_string_lossy().replace(char::from(92), "/");
        write(
            &config.join("sym-lib-table"),
            &table(&[
                row("A", "Table", &uri(&root.path().join("a-table"))),
                row("B", "Table", &uri(&root.path().join("b-table"))),
            ]),
        );
        for branch in ["a-table", "b-table"] {
            write(
                &root.path().join(branch),
                &table(&[row("Shared", "Table", &uri(&shared))]),
            );
        }
        write(&shared, &table(&[row("Lib", "KiCad", "lib.kicad_sym")]));
        let reading = read_tables(&config, TableKind::Symbol, &PathVariables::default());
        assert!(reading.problems.is_empty(), "{:?}", reading.problems);
        assert_eq!(reading.libraries.len(), 1);
    }

    #[test]
    fn finds_kicads_settings_folder() {
        let env = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect()
        };
        assert_eq!(
            user_config_dir(10, &env(&[("KICAD_CONFIG_HOME", "/k"), ("APPDATA", "/a")])),
            Some(PathBuf::from("/k").join("10.0"))
        );
        if cfg!(windows) {
            assert_eq!(
                user_config_dir(10, &env(&[("APPDATA", "C:/Users/a/AppData/Roaming")])),
                Some(PathBuf::from("C:/Users/a/AppData/Roaming/kicad/10.0"))
            );
        }
        assert_eq!(user_config_dir(10, &env(&[])), None);
    }

    #[test]
    fn compares_paths_by_name() {
        assert!(is_inside(
            Path::new("C:/KiCad/share/symbols/../symbols/Device.kicad_sym"),
            Path::new("C:/KiCad/share/symbols")
        ));
        assert!(!is_inside(
            Path::new("C:/KiCad/share/symbols2/x"),
            Path::new("C:/KiCad/share/symbols")
        ));
        if cfg!(windows) {
            assert!(is_inside(
                Path::new("c:\\kicad\\SHARE\\symbols\\x"),
                Path::new("C:/KiCad/share")
            ));
        }
    }
}

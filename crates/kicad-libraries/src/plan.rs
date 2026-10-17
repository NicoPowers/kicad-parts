//! What the KiCad libraries hold for one state of the Catalog, worked out
//! from its records alone: no file is read and no blob is needed. The
//! result depends only on the records, in a fixed order, so the same
//! Catalog always gives the same libraries, wherever they are written.

use crate::names::{file_stem, revision_name, windows_refuses};
use catalog::pocketbase::{
    CatalogReadback, StoredBlob, StoredCadItem, StoredCadItemRevision, StoredPart,
};
use catalog::{
    CadItemDetails, CadItemKind, CadReference, CadReferenceRole, CadReferenceTarget,
    CategoryDetails, ModelTarget, PartDetails, PartRevision,
};
use kicad_integration::library_file::escape_text;
use std::collections::{BTreeMap, BTreeSet};

/// How one team's generated libraries are named: every nickname starts
/// with the team's code and a dash, such as `MA-Parts` and
/// `MA-Resistor_SMD`. Each team on a computer gets its own names, and a
/// generated library never clashes with the editable `g-` libraries of the
/// CSV catalog (ADR 0011) or the person's hand-made `MA_` ones. A
/// footprint library's nickname is saved in every board that uses it, so
/// a team's code never changes. This is the one place the names are made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Naming {
    code: String,
}

/// Why `code` is not a team code, or `None` if it is one: two or three
/// capital letters or digits, starting with a letter, such as `MA`.
pub fn team_code_problem(code: &str) -> Option<&'static str> {
    let bytes = code.as_bytes();
    let valid = (2..=3).contains(&bytes.len())
        && bytes[0].is_ascii_uppercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
    (!valid).then_some(
        "A team code is 2 or 3 capital letters or digits, starting with a letter, such as MA.",
    )
}

impl Naming {
    /// The names of the team with `code`, or why it is not a team code.
    pub fn new(code: &str) -> Result<Self, String> {
        match team_code_problem(code) {
            Some(problem) => Err(problem.to_owned()),
            None => Ok(Self {
                code: code.to_owned(),
            }),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    /// The database library's nickname, such as `MA-Parts`. A placed
    /// part's link is this nickname and its entry name.
    pub fn database_nickname(&self) -> String {
        format!("{}-Parts", self.code)
    }

    /// The database library's `.kicad_dbl` file, such as `MA-Parts.kicad_dbl`.
    pub fn dbl_file(&self) -> String {
        format!("{}.kicad_dbl", self.database_nickname())
    }

    /// Its SQLite database, such as `MA-Parts.sqlite`.
    pub fn database_file(&self) -> String {
        format!("{}.sqlite", self.database_nickname())
    }

    /// The nicknames a CAD library's generated library may take, in order:
    /// the code, a dash and its name without `g-`, such as `MA-res`; then
    /// the code, a dash and its whole name. `suffix`, such as a symbol file
    /// version, goes at the end.
    fn nickname_choices(&self, library: &str, suffix: &str) -> Vec<String> {
        let code = &self.code;
        let short = library.strip_prefix("g-").unwrap_or(library);
        let mut choices = vec![format!("{code}-{short}{suffix}")];
        if short != library {
            choices.push(format!("{code}-{library}{suffix}"));
        }
        choices
    }
}

/// The newest symbol library file version KiCad 10.0 reads:
/// `SEXPR_SYMBOL_LIB_FILE_VERSION` in KiCad's `eeschema/sch_file_versions.h`.
/// KiCad refuses a whole library file with a newer version.
pub const NEWEST_SYMBOL_VERSION: &str = "20251024";
/// The version a stored symbol that names none is read as.
const DEFAULT_SYMBOL_VERSION: &str = "20241209";

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
/// The place of the Symbol column in a row.
pub const SYMBOL_COLUMN: usize = 3;

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

/// One symbol library file. KiCad reads every symbol in a file by the
/// file's version, and older versions read some text differently (before
/// 20250318, `~` means empty text), so each file holds the symbols of one
/// CAD library saved in one version.
#[derive(Debug)]
pub struct SymbolLibraryPlan<'a> {
    /// Such as `team-res-20241209`. Only database rows name it, never a
    /// design.
    pub nickname: String,
    /// Relative to the libraries folder, with `/`.
    pub path: String,
    pub version: String,
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
    /// Such as `symbol g-res:R_US revision 2`, for problems.
    pub label: String,
}

#[derive(Debug)]
pub struct FootprintLibraryPlan {
    /// Such as `team-Resistor_SMD`. Boards save it, so it never changes.
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
    /// beside the footprint library, by a path relative to it.
    Footprint {
        /// For each `(model ...)` in file order, the new path as written
        /// between the quotes, or `None` to keep it.
        links: Vec<Option<String>>,
        /// Each `(model ...)` path as the stored bytes write it.
        references: Vec<String>,
    },
}

/// One table of the database, and one group in KiCad's chooser. Every
/// table is listed in the `.kicad_dbl`, even with no rows: a running KiCad
/// looks only in the tables it loaded, so a row moving into a table it has
/// not loaded would not be found until KiCad restarts.
#[derive(Debug, Eq, PartialEq)]
pub struct TablePlan {
    pub group: Group,
    /// The SQLite table name: lower-case letters, digits and `_` only.
    pub table: String,
    /// The group KiCad shows, such as `Resistors`.
    pub name: String,
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

/// The libraries for `catalog`, named by `naming`.
pub fn plan<'a>(catalog: &'a CatalogReadback, naming: &Naming) -> Plan<'a> {
    let mut plan = Plan::default();
    let records = Records::new(catalog, naming);
    let mut links = Links::default();
    plan_symbols(&records, &mut plan, &mut links);
    plan_footprints(&records, &mut plan, &mut links);
    plan_tables(&records, &links, &mut plan);
    plan
}

/// The records `plan` reads, looked up by ID.
struct Records<'a> {
    catalog: &'a CatalogReadback,
    naming: Naming,
    blobs: BTreeMap<&'a str, &'a StoredBlob>,
    /// CAD item revisions by (CAD item, number).
    cad_revisions: BTreeMap<(&'a str, u32), &'a StoredCadItemRevision>,
    /// Each Change's `seq`, by ID.
    seqs: BTreeMap<&'a str, u64>,
}

impl<'a> Records<'a> {
    fn new(catalog: &'a CatalogReadback, naming: &Naming) -> Self {
        Self {
            catalog,
            naming: naming.clone(),
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
            seqs: catalog
                .changes
                .iter()
                .map(|change| (change.id.as_str(), change.seq))
                .collect(),
        }
    }

    /// When the CAD item was first saved, as the `seq` of its first
    /// Change; unknown ones come last.
    fn first_seq(&self, item: &StoredCadItem) -> u64 {
        self.seqs
            .get(item.first_change.as_str())
            .copied()
            .unwrap_or(u64::MAX)
    }

    /// The CAD items of `kind`, oldest first (then by key), each with its
    /// revisions by number and its key split into library and name. An
    /// older item keeps a file name a newer one would share.
    fn cad_items(
        &self,
        kind: CadItemKind,
        problems: &mut Vec<String>,
    ) -> Vec<CadItemRevisions<'a>> {
        let mut items: Vec<CadItemRevisions<'a>> = Vec::new();
        for item in self
            .catalog
            .cad_items
            .iter()
            .filter(|item| item.kind == kind.as_str())
        {
            let Some((library, name)) = item.key.split_once(':') else {
                problems.push(format!(
                    "The {} {} has no library name, so it was left out.",
                    kind.as_str(),
                    item.key
                ));
                continue;
            };
            let mut revisions: Vec<&StoredCadItemRevision> = self
                .catalog
                .cad_item_revisions
                .iter()
                .filter(|revision| revision.cad_item == item.id)
                .collect();
            revisions.sort_by_key(|revision| revision.number);
            items.push(CadItemRevisions {
                id: &item.id,
                key: &item.key,
                library,
                name,
                first_seq: self.first_seq(item),
                revisions,
            });
        }
        items.sort_by(|a, b| a.first_seq.cmp(&b.first_seq).then(a.key.cmp(b.key)));
        items
    }
}

struct CadItemRevisions<'a> {
    id: &'a str,
    key: &'a str,
    library: &'a str,
    name: &'a str,
    first_seq: u64,
    revisions: Vec<&'a StoredCadItemRevision>,
}

/// What a CAD item revision becomes in a row, by (CAD item, number).
#[derive(Default)]
struct Links {
    symbols: BTreeMap<(String, u32), Linked>,
    footprints: BTreeMap<(String, u32), Linked>,
}

struct Linked {
    /// Such as `team-res-20241209:R_US_v2`.
    link: String,
    /// Why the revision was left out of the libraries, if it was.
    left_out: Option<String>,
}

/// Relative paths written so far, compared as Windows compares them
/// (ignoring case), so two files never land on one.
#[derive(Default)]
struct UsedPaths(BTreeSet<String>);

impl UsedPaths {
    /// Takes every path in `paths` if none is taken yet.
    fn take(&mut self, paths: &[&str]) -> bool {
        let lower: Vec<String> = paths.iter().map(|path| path.to_lowercase()).collect();
        if lower.iter().any(|path| self.0.contains(path)) {
            return false;
        }
        self.0.extend(lower);
        true
    }
}

/// A CAD item revision's details, or why they cannot be used.
fn details_of(revision: &StoredCadItemRevision, label: &str) -> Result<CadItemDetails, String> {
    serde_json::from_str(&revision.snapshot)
        .map_err(|_| format!("The {label} could not be read, so it was left out."))
}

/// Whether `version` is a KiCad file version (a date such as `20241209`)
/// that KiCad 10 reads.
fn readable_symbol_version(version: &str) -> Result<(), String> {
    if version.len() != 8 || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        Err(format!(
            "its KiCad file version {version} is not one KiCad writes"
        ))
    } else if version > NEWEST_SYMBOL_VERSION {
        Err(format!(
            "it was saved by a KiCad newer than 10 (file version {version}), which KiCad 10 cannot read"
        ))
    } else {
        Ok(())
    }
}

/// A symbol revision, and its file version and stored bytes or why it is
/// left out.
type ReadSymbol<'r, 'a> = (
    &'r CadItemRevisions<'a>,
    &'a StoredCadItemRevision,
    Result<(String, &'a StoredBlob), String>,
);

fn plan_symbols<'a>(records: &Records<'a>, plan: &mut Plan<'a>, links: &mut Links) {
    let items = records.cad_items(CadItemKind::Symbol, &mut plan.problems);
    let mut read: Vec<ReadSymbol<'_, 'a>> = Vec::new();
    for item in &items {
        for revision in &item.revisions {
            let label = format!("symbol {} revision {}", item.key, revision.number);
            let usable = details_of(revision, &label).and_then(|details| {
                let blob = records.blobs.get(revision.blob.as_str()).ok_or_else(|| {
                    format!("The team catalog has no file for the {label}, so it was left out.")
                })?;
                if details.format != "kicad-symbol-entry" {
                    return Err(format!(
                        "The {label} is not a KiCad symbol, so it was left out."
                    ));
                }
                let version = details
                    .format_version
                    .unwrap_or_else(|| DEFAULT_SYMBOL_VERSION.to_owned());
                readable_symbol_version(&version)
                    .map_err(|why| format!("The {label} was left out, because {why}."))?;
                Ok((version, *blob))
            });
            read.push((item, revision, usable));
        }
    }
    // One file per CAD library and version, the oldest library first.
    let mut units: Vec<(u64, &str, String)> = read
        .iter()
        .filter_map(|(item, _, usable)| {
            usable
                .as_ref()
                .ok()
                .map(|(version, _)| (item.first_seq, item.library, version.clone()))
        })
        .collect();
    units.sort();
    units.dedup_by(|a, b| a.1 == b.1 && a.2 == b.2);
    let mut paths = UsedPaths::default();
    let mut libraries: BTreeMap<(String, String), SymbolLibraryPlan<'a>> = BTreeMap::new();
    for (_, library, version) in units {
        let key = (library.to_owned(), version.clone());
        if libraries.contains_key(&key) {
            continue;
        }
        let chosen = records
            .naming
            .nickname_choices(library, &format!("-{version}"))
            .into_iter()
            .map(|nickname| {
                let path = format!("symbols/{}.kicad_sym", file_stem(&nickname));
                (nickname, path)
            })
            .find(|(_, path)| paths.take(&[path]));
        match chosen {
            Some((nickname, path)) => {
                libraries.insert(
                    key,
                    SymbolLibraryPlan {
                        nickname,
                        path,
                        version,
                        entries: Vec::new(),
                    },
                );
            }
            None => plan.problems.push(format!(
                "The symbol library {library} (file version {version}) would use the same file as another library, so its symbols were left out."
            )),
        }
    }
    for (item, revision, usable) in read {
        let name = revision_name(item.name, revision.number);
        let label = format!("symbol {} revision {}", item.key, revision.number);
        let linked = match usable {
            Ok((version, blob)) => match libraries.get_mut(&(item.library.to_owned(), version)) {
                Some(library) => {
                    library.entries.push(SymbolEntryPlan {
                        name: name.clone(),
                        original: item.name.to_owned(),
                        blob,
                        label,
                    });
                    Linked {
                        link: format!("{}:{name}", library.nickname),
                        left_out: None,
                    }
                }
                None => Linked {
                    link: format!(
                        "{}:{name}",
                        records.naming.nickname_choices(item.library, "")[0]
                    ),
                    left_out: Some(
                        "Its library would use the same file as another library.".to_owned(),
                    ),
                },
            },
            Err(problem) => {
                plan.problems.push(problem.clone());
                Linked {
                    link: format!(
                        "{}:{name}",
                        records.naming.nickname_choices(item.library, "")[0]
                    ),
                    left_out: Some(problem),
                }
            }
        };
        links
            .symbols
            .insert((item.id.to_owned(), revision.number), linked);
    }
    let mut libraries: Vec<SymbolLibraryPlan<'a>> = libraries.into_values().collect();
    for library in &mut libraries {
        library.entries.sort_by(|a, b| {
            a.original
                .cmp(&b.original)
                .then(a.name.len().cmp(&b.name.len()))
                .then(a.name.cmp(&b.name))
        });
    }
    libraries.sort_by(|a, b| a.nickname.cmp(&b.nickname));
    plan.symbol_libraries = libraries;
}

fn plan_footprints<'a>(records: &Records<'a>, plan: &mut Plan<'a>, links: &mut Links) {
    let items = records.cad_items(CadItemKind::Footprint, &mut plan.problems);
    let mut paths = UsedPaths::default();
    // Each CAD library's nickname and folders, the oldest library first.
    let mut libraries: BTreeMap<&str, Option<(String, String)>> = BTreeMap::new();
    for item in &items {
        if libraries.contains_key(item.library) {
            continue;
        }
        let chosen = records
            .naming
            .nickname_choices(item.library, "")
            .into_iter()
            .find(|nickname| {
                let stem = file_stem(nickname);
                paths.take(&[
                    &format!("footprints/{stem}.pretty"),
                    &format!("footprints/{stem}.3dshapes"),
                ])
            })
            .map(|nickname| {
                let stem = file_stem(&nickname);
                (nickname, stem)
            });
        if chosen.is_none() {
            plan.problems.push(format!(
                "The footprint library {} would use the same folder as another library, so its footprints were left out.",
                item.library
            ));
        }
        libraries.insert(item.library, chosen);
    }
    let mut files = Vec::new();
    for item in &items {
        let library = libraries.get(item.library).cloned().flatten();
        for revision in &item.revisions {
            let label = format!("footprint {} revision {}", item.key, revision.number);
            let name = revision_name(item.name, revision.number);
            let nickname = library
                .as_ref()
                .map(|(nickname, _)| nickname.clone())
                .unwrap_or_else(|| records.naming.nickname_choices(item.library, "")[0].clone());
            let linked = match footprint_file(
                records,
                item,
                revision,
                &name,
                library.as_ref(),
                &label,
                &mut paths,
            ) {
                Ok((footprint, models)) => {
                    files.push(footprint);
                    files.extend(models.into_iter().filter_map(|model| match model {
                        Ok(model) => Some(model),
                        Err(problem) => {
                            plan.problems.push(problem);
                            None
                        }
                    }));
                    None
                }
                Err(problem) => {
                    plan.problems.push(problem.clone());
                    Some(problem)
                }
            };
            links.footprints.insert(
                (item.id.to_owned(), revision.number),
                Linked {
                    link: format!("{nickname}:{name}"),
                    left_out: linked,
                },
            );
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut libraries: Vec<FootprintLibraryPlan> = libraries
        .into_values()
        .flatten()
        .map(|(nickname, stem)| FootprintLibraryPlan {
            nickname,
            path: format!("footprints/{stem}.pretty"),
        })
        .collect();
    libraries.sort_by(|a, b| a.nickname.cmp(&b.nickname));
    plan.footprint_libraries = libraries;
    plan.files = files;
}

/// A footprint revision's file and its 3D model files, or why it is left
/// out. A model that cannot be written gives a problem and keeps its link.
#[allow(clippy::type_complexity)]
fn footprint_file<'a>(
    records: &Records<'a>,
    item: &CadItemRevisions<'a>,
    revision: &StoredCadItemRevision,
    name: &str,
    library: Option<&(String, String)>,
    label: &str,
    paths: &mut UsedPaths,
) -> Result<(FilePlan<'a>, Vec<Result<FilePlan<'a>, String>>), String> {
    let details = details_of(revision, label)?;
    let blob = records.blobs.get(revision.blob.as_str()).ok_or_else(|| {
        format!("The team catalog has no file for the {label}, so it was left out.")
    })?;
    let Some((_, stem)) = library else {
        return Err(format!(
            "The {label} was left out, because its library would use the same folder as another library."
        ));
    };
    let file_name = format!("{name}.kicad_mod");
    if let Some(why) = windows_refuses(&file_name) {
        return Err(format!(
            "The {label} was left out: Windows refuses the file name {file_name} because {why}."
        ));
    }
    let path = format!("footprints/{stem}.pretty/{file_name}");
    if !paths.take(&[&path]) {
        return Err(format!(
            "The {label} was left out, because an older footprint of {} has the same name apart from upper and lower case, and Windows sees one file.",
            item.library
        ));
    }
    let shapes = format!("{stem}.3dshapes");
    let mut links = Vec::new();
    let mut models = Vec::new();
    let mut written = 0;
    for reference in &details.model_references {
        let ModelTarget::CadItemRevision {
            cad_item, number, ..
        } = &reference.target
        else {
            links.push(None);
            continue;
        };
        match model_file(records, (cad_item, *number), name, written, label) {
            Ok((file, model_blob)) => {
                let model_path = format!("footprints/{shapes}/{file}");
                if paths.take(&[&model_path]) {
                    written += 1;
                    // Relative to the footprint library's folder, which
                    // KiCad resolves model links against.
                    links.push(Some(escape_text(&format!("../{shapes}/{file}"))));
                    models.push(Ok(FilePlan {
                        path: model_path,
                        blob: model_blob,
                        kind: FileKind::Model,
                        label: format!("3D model of the {label}"),
                    }));
                } else {
                    links.push(None);
                    models.push(Err(format!(
                        "A 3D model of the {label} would use the same file as another, so its link was kept."
                    )));
                }
            }
            Err(problem) => {
                links.push(None);
                models.push(Err(problem));
            }
        }
    }
    Ok((
        FilePlan {
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
            label: label.to_owned(),
        },
        models,
    ))
}

/// The file name and blob of the 3D model revision `target` that the
/// footprint revision `footprint` uses as its model number `index` (from
/// 0): named like the footprint revision, with `_2`, `_3` and so on after
/// the first.
fn model_file<'a>(
    records: &Records<'a>,
    target: (&str, u32),
    footprint: &str,
    index: usize,
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
    Ok((format!("{}.{extension}", file_stem(&stem)), blob))
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

fn plan_tables(records: &Records<'_>, links: &Links, plan: &mut Plan<'_>) {
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
                "The details of {} could not be read, so its rows were not changed.",
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
        let (extra, _) = extra_fields(category);
        let mut part_revisions: Vec<(u32, PartRevision)> = Vec::new();
        for stored in catalog.part_revisions.iter().filter(|r| r.part == part.id) {
            match serde_json::from_str::<PartRevision>(&stored.snapshot) {
                Ok(revision) => part_revisions.push((stored.number, revision)),
                Err(_) => plan.problems.push(format!(
                    "Revision {} of {} could not be read, so its row was not changed.",
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
            let entry = revision_name(&part.ipn, *number);
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
                .map(|reference| {
                    link(
                        records,
                        &links.footprints,
                        reference,
                        &entry,
                        &mut plan.problems,
                    )
                })
                .collect();
            let manufacturer = details.manufacturer_parts.first();
            let mut values = vec![
                entry.clone(),
                part.ipn.clone(),
                number.to_string(),
                link(records, &links.symbols, symbol, &entry, &mut plan.problems),
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
                for field in &extra {
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
        let (extra, refused) = extra_fields(category);
        for field in refused {
            plan.problems.push(format!(
                "The field {field} of {} was left out of KiCad, because KiCad cannot read a field name with a double quote in it.",
                names[code]
            ));
        }
        plan.tables.push(TablePlan {
            group: Group::Category,
            name: names[code].clone(),
            table,
            extra,
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
            extra: Vec::new(),
            rows: table_rows,
        });
    }
}

/// What a symbol or footprint reference becomes in the row of `entry`: an
/// exact CAD item revision becomes its revision name in its generated
/// library, such as `team-res-20241209:R_US_v2`; a KiCad library item, or
/// a reference that did not resolve, stays as written, so KiCad finds it
/// in its own libraries. A revision left out of the libraries keeps the
/// link it would have, so KiCad says plainly that it is missing, and the
/// problem names the entry.
fn link(
    records: &Records<'_>,
    linked: &BTreeMap<(String, u32), Linked>,
    reference: &CadReference,
    entry: &str,
    problems: &mut Vec<String>,
) -> String {
    let CadReferenceTarget::CadItemRevision {
        cad_item, number, ..
    } = &reference.target
    else {
        return reference.reference.clone();
    };
    if let Some(found) = linked.get(&(cad_item.clone(), *number)) {
        if let Some(why) = &found.left_out {
            problems.push(format!(
                "{entry} uses {} revision {number}, which is not in the KiCad libraries, so KiCad shows it as missing. {why}",
                reference.reference
            ));
        }
        return found.link.clone();
    }
    if records.catalog.cad_item(cad_item).is_none() {
        problems.push(format!(
            "{} names a CAD item the team catalog does not have; it is linked as written.",
            reference.reference
        ));
    }
    reference.reference.clone()
}

/// The categories whose value, the text a schematic shows beside the
/// symbol, is a field other than `Value`, as `database/#gplm.kicad_dbl`
/// maps them: a crystal shows its frequency, not its MPN.
const VALUE_FIELDS: [(&str, &str); 3] = [
    ("rvr", "Resistance"),
    ("opt", "Color"),
    ("osc", "Frequency"),
];

/// The Value field: the category's value field (see [`VALUE_FIELDS`], and
/// Value otherwise), else the main MPN, else the IPN.
fn value_of(category: &CategoryDetails, details: &PartDetails) -> String {
    let field = VALUE_FIELDS
        .iter()
        .find(|(code, _)| *code == category.code)
        .map_or("Value", |(_, field)| field);
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

/// A category's fields that are not fixed columns, in order, and the names
/// of the fields left out because KiCad cannot read them. SQLite compares
/// column names ignoring case, so a field whose name matches an earlier
/// column that way is left out quietly. KiCad puts column names in double
/// quotes without escaping them (`common/database/database_connection.cpp`),
/// so a name with a double quote would stop the whole library loading.
pub fn extra_fields(category: &CategoryDetails) -> (Vec<ExtraField>, Vec<String>) {
    let mut taken: BTreeSet<String> = FIXED_COLUMNS
        .iter()
        .map(|column| column.to_lowercase())
        .collect();
    let mut refused = Vec::new();
    let mut fields = Vec::new();
    for field in &category.fields {
        if field.trim().is_empty() || !taken.insert(field.to_lowercase()) {
            continue;
        }
        if field.contains('"') {
            refused.push(field.clone());
            continue;
        }
        fields.push(ExtraField {
            column: field.clone(),
            field: match field.strip_prefix("Sim_") {
                // KiCad's simulator fields are named Sim.Library and so on.
                Some(rest) => format!("Sim.{rest}"),
                None => field.clone(),
            },
            shown_on_add: SHOWN_ON_ADD.contains(&(category.code.as_str(), field.as_str())),
        });
    }
    (fields, refused)
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

    /// What a schematic shows beside the symbol is what the CSV catalog's
    /// own KiCad library showed: a crystal's frequency, a trimmer's
    /// resistance, an LED's colour, or else the Value field; and when that
    /// is blank, the MPN, then the IPN, so it is never empty.
    #[test]
    fn the_value_follows_the_csv_catalogs_own_kicad_library() {
        let details = |category: &str, fields: &[(&str, &str)], mpn: &str| PartDetails {
            schema: String::new(),
            ipn: "OSC-0001-0256".to_owned(),
            category: category.to_owned(),
            fields: fields
                .iter()
                .map(|(name, value)| catalog::DetailField {
                    name: (*name).to_owned(),
                    value: (*value).to_owned(),
                })
                .collect(),
            manufacturer_parts: [mpn]
                .iter()
                .filter(|mpn| !mpn.is_empty())
                .map(|mpn| catalog::ManufacturerPart {
                    manufacturer: "TXC".to_owned(),
                    mpn: (*mpn).to_owned(),
                })
                .collect(),
        };
        let value = |code: &str, fields: &[(&str, &str)], mpn: &str| {
            value_of(
                &CategoryDetails::starting(code, Vec::new()),
                &details(code, fields, mpn),
            )
        };
        assert_eq!(value("osc", &[("Frequency", "25MHz")], "8Y-25"), "25MHz");
        assert_eq!(value("rvr", &[("Resistance", "10k")], "3314J"), "10k");
        assert_eq!(value("opt", &[("Color", "Red")], "LED1"), "Red");
        assert_eq!(value("res", &[("Value", "47")], "RC0603"), "47");
        assert_eq!(value("osc", &[("Frequency", "")], "D2SX"), "D2SX");
        assert_eq!(value("ics", &[], ""), "OSC-0001-0256");
    }

    #[test]
    fn nicknames_start_with_the_team_code() {
        let naming = Naming::new("MA").unwrap();
        assert_eq!(
            naming.nickname_choices("g-res", "-20241209"),
            ["MA-res-20241209", "MA-g-res-20241209"]
        );
        assert_eq!(
            naming.nickname_choices("g-Resistor_SMD", ""),
            ["MA-Resistor_SMD", "MA-g-Resistor_SMD"]
        );
        assert_eq!(naming.nickname_choices("mine", ""), ["MA-mine"]);
        assert_eq!(naming.database_nickname(), "MA-Parts");
        assert_eq!(naming.dbl_file(), "MA-Parts.kicad_dbl");
        assert_eq!(naming.database_file(), "MA-Parts.sqlite");
    }

    #[test]
    fn team_codes_are_two_or_three_capitals_or_digits() {
        for code in ["MA", "SL", "A1", "XYZ", "B52"] {
            assert_eq!(team_code_problem(code), None, "{code}");
            assert!(Naming::new(code).is_ok());
        }
        for code in ["", "M", "ma", "Ma", "1A", "ABCD", "M-", "MA ", "ÄB"] {
            assert!(team_code_problem(code).is_some(), "{code:?}");
            assert!(Naming::new(code).is_err());
        }
    }

    #[test]
    fn only_symbol_versions_kicad_10_reads_are_kept() {
        assert!(readable_symbol_version("20241209").is_ok());
        assert!(readable_symbol_version(NEWEST_SYMBOL_VERSION).is_ok());
        assert!(
            readable_symbol_version("20260101")
                .unwrap_err()
                .contains("newer than 10")
        );
        assert!(readable_symbol_version("2024").is_err());
    }
}

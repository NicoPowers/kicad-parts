//! The library check (kicad-parts-da65): compares the CSV catalog's own
//! libraries (`symbols/g-*.kicad_sym`, `footprints/g-*.pretty`,
//! `3d-models/**`) with the Catalog, and says what is in them but not in the
//! Catalog yet (made by hand in KiCad), which symbols name a footprint
//! nobody has, and which 3D model links work only on one computer or name a
//! missing file.
//!
//! [`check`] is one pure function over plain inputs, so the CSV import and
//! the desktop app say the same thing; [`read`] gathers those inputs from a
//! CSV catalog folder, the team catalog's import state and KiCad's own
//! libraries.
//!
//! A 3D model link is judged like this (`${GITPLM_PARTS}/3d-models` is our
//! library):
//!
//! - in our library, in `g-<footprint library>.3dshapes/` and named like
//!   the footprint: fine;
//! - in our library under another name or folder (a shared model): worth a
//!   look;
//! - in our library, but the file is missing: a problem;
//! - KiCad's own models (`${KICAD<n>_3DMODEL_DIR}/...`) and models embedded
//!   in the footprint (`kicad-embed://...`): fine;
//! - anywhere else (an absolute or relative path, any other variable such
//!   as `${GITPLM_3DMODELS}` or `${KIPRJMOD}`): a problem, because it only
//!   works on one computer.
//!
//! A problem is an error (to fix) only for a footprint that is new or
//! changed since the last import: with bytes that are not those of its
//! newest imported revision, or not in the Catalog while a row of a CSV
//! file imported before uses it. The same problem on a footprint the
//! Catalog has unchanged is clean-up, so today's CSV catalog still imports.
//! A CSV file never imported has no last import to compare with: its first
//! import takes the footprints it uses as they are only while their bytes
//! are those of today's CSV catalog ([`todays_footprints`]), so their old
//! problems are clean-up and today's CSV files import one by one; a
//! footprint drawn or changed since is judged like any new one, so a new or
//! renamed CSV file cannot bring a bad link in. On a footprint that no part uses and the Catalog does not
//! have, a problem is a warning until something brings the footprint in.

mod baseline;
pub mod read;

pub use baseline::todays_footprints;

#[cfg(test)]
mod tests;

use crate::cad_item::CadItemKind;
use crate::csv_import::classify_model_reference;
use crate::ids::CadItemId;
use std::collections::{BTreeMap, BTreeSet};

/// How much a finding matters.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// Must be fixed: it stops an import of the footprint, and Add part or
    /// Edit part with it.
    Error,
    /// Someone should look at it.
    Warning,
    /// Probably fine, but worth a look.
    SoftWarning,
    /// An old problem of something the Catalog already has unchanged.
    CleanUp,
}

/// What a finding is about.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FindingKind {
    /// A symbol, footprint or 3D model in our libraries that no part uses
    /// and the Catalog does not have.
    NotInCatalog,
    /// A CSV row names a symbol or footprint its library does not have,
    /// and the library has one with a similar name.
    SimilarName,
    /// A symbol's own Footprint names a footprint that neither our
    /// libraries nor KiCad's have.
    SymbolFootprint,
    /// A footprint links a 3D model of our library made for another
    /// footprint.
    SharedModel,
    /// A footprint links a 3D model of our library whose file is missing.
    MissingModel,
    /// A footprint links a 3D model outside our library.
    ModelOutside,
    /// A library file could not be read.
    Unreadable,
}

impl FindingKind {
    /// A short name for tests and machine output, such as `missing-model`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotInCatalog => "not-in-catalog",
            Self::SimilarName => "similar-name",
            Self::SymbolFootprint => "symbol-footprint",
            Self::SharedModel => "shared-model",
            Self::MissingModel => "missing-model",
            Self::ModelOutside => "model-outside",
            Self::Unreadable => "unreadable",
        }
    }
}

/// The groups people see the findings in, in this order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Group {
    ToFix,
    NotInCatalog,
    WorthALook,
    CleanUp,
}

impl Group {
    pub const ALL: [Group; 4] = [
        Group::ToFix,
        Group::NotInCatalog,
        Group::WorthALook,
        Group::CleanUp,
    ];

    /// The group's heading, such as `To fix`.
    pub fn title(self) -> &'static str {
        match self {
            Self::ToFix => "To fix",
            Self::NotInCatalog => "Not in the Catalog yet",
            Self::WorthALook => "Worth a look",
            Self::CleanUp => "Clean-up",
        }
    }
}

/// One thing the library check found.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Finding {
    pub severity: Severity,
    pub kind: FindingKind,
    /// What it is about: `LIBRARY:NAME` for a symbol or footprint, the
    /// repository path for a 3D model, such as
    /// `3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step`.
    pub item: String,
    /// The repository file it is in, such as `symbols/g-ics.kicad_sym`,
    /// `footprints/g-Crystal.pretty/Abracon_ABS07L.kicad_mod` or
    /// `database/g-ics.csv`.
    pub file: String,
    /// What is wrong and what to do, in plain words.
    pub message: String,
}

impl Finding {
    pub fn group(&self) -> Group {
        match (self.severity, self.kind) {
            (Severity::Error, _) => Group::ToFix,
            (Severity::CleanUp, _) => Group::CleanUp,
            (_, FindingKind::NotInCatalog) => Group::NotInCatalog,
            _ => Group::WorthALook,
        }
    }

    /// The library part of [`Finding::item`]: `g-ics` for `g-ics:IC_TP4333`,
    /// the folder for a 3D model, such as `g-Diode_SMD.3dshapes`.
    pub fn library(&self) -> &str {
        match self.item.split_once(':') {
            Some((library, _)) => library,
            None => self
                .item
                .rsplit_once('/')
                .map(|(folder, _)| folder.rsplit('/').next().unwrap_or(folder))
                .unwrap_or_default(),
        }
    }

    /// The name part of [`Finding::item`]: `IC_TP4333`, or a 3D model's
    /// file name.
    pub fn name(&self) -> &str {
        match self.item.split_once(':') {
            Some((_, name)) => name,
            None => self.item.rsplit('/').next().unwrap_or(&self.item),
        }
    }
}

/// Everything the library check found, in a fixed order: by group, then
/// file, item and message.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryCheck {
    pub findings: Vec<Finding>,
}

impl LibraryCheck {
    /// How many findings are in `group`.
    pub fn count(&self, group: Group) -> usize {
        self.in_group(group).count()
    }

    /// The findings in `group`, in order.
    pub fn in_group(&self, group: Group) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(move |finding| finding.group() == group)
    }

    /// The errors about one of `files` (repository paths), such as the
    /// files one import reads: these stop it.
    pub fn errors_in<'a>(&'a self, files: &BTreeSet<&str>) -> Vec<&'a Finding> {
        self.in_group(Group::ToFix)
            .filter(|finding| files.contains(finding.file.as_str()))
            .collect()
    }

    /// Why an import that reads `files` must stop before it sends
    /// anything, in plain words, or `None` when no error is in them. Like
    /// the commit-first rule, only the files the import uses count, so
    /// other work in the libraries never blocks it.
    pub fn import_refusal(&self, files: &BTreeSet<&str>) -> Option<String> {
        let errors = self.errors_in(files);
        let (found, them) = match errors.len() {
            0 => return None,
            1 => ("a problem".to_owned(), "it"),
            count => (format!("{count} problems"), "them"),
        };
        let mut text = format!(
            "The library check found {found} in files this import uses, so nothing was sent. Fix {them} in KiCad, commit, and import again:"
        );
        for error in errors {
            text.push_str(&format!("\n- {}: {}", error.file, error.message));
        }
        Some(text)
    }

    /// One line saying how many findings each group has, such as
    /// `Library check: 1 to fix, 22 not in the Catalog yet, 2 worth a look,
    /// 36 to clean up.`; `Library check: nothing found.` when there are
    /// none.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            (Group::ToFix, "to fix"),
            (Group::NotInCatalog, "not in the Catalog yet"),
            (Group::WorthALook, "worth a look"),
            (Group::CleanUp, "to clean up"),
        ]
        .into_iter()
        .filter_map(|(group, words)| {
            let count = self.count(group);
            (count > 0).then(|| format!("{count} {words}"))
        })
        .collect();
        if parts.is_empty() {
            "Library check: nothing found.".to_owned()
        } else {
            format!("Library check: {}.", parts.join(", "))
        }
    }

    /// The check as the CSV import prints it: the summary, then what to
    /// fix, what is worth a look, the list of what is not in the Catalog
    /// yet, and how many old problems are left to clean up.
    pub fn report(&self) -> String {
        let mut lines = vec![self.summary()];
        for group in [Group::ToFix, Group::WorthALook] {
            if self.count(group) > 0 {
                lines.push(format!("{}:", group.title()));
                for finding in self.in_group(group) {
                    lines.push(format!("  {}: {}", finding.file, finding.message));
                }
            }
        }
        if self.count(Group::NotInCatalog) > 0 {
            lines.push(format!("{}:", Group::NotInCatalog.title()));
            for finding in self.in_group(Group::NotInCatalog) {
                lines.push(format!("  {} ({})", finding.item, finding.file));
            }
        }
        let clean_up = self.count(Group::CleanUp);
        if clean_up > 0 {
            lines.push(format!(
                "Clean-up: {clean_up} old {} that {} there before this import; the desktop app lists them.",
                if clean_up == 1 { "problem" } else { "problems" },
                if clean_up == 1 { "was" } else { "were" }
            ));
        }
        lines.join("\n")
    }
}

/// The symbols, footprints and 3D models of the CSV catalog's own (`g-`)
/// libraries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Libraries {
    pub symbols: Vec<LibrarySymbol>,
    pub footprints: Vec<LibraryFootprint>,
    /// Every file under `3d-models/`, as a repository path such as
    /// `3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step`.
    pub models: BTreeSet<String>,
    /// Library files that could not be read, with why.
    pub unreadable: Vec<(String, String)>,
}

/// One symbol of a `symbols/g-*.kicad_sym` library.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibrarySymbol {
    /// Such as `g-ics`.
    pub library: String,
    pub name: String,
    /// The symbol's own Footprint property, blank if it has none.
    pub footprint: String,
}

impl LibrarySymbol {
    pub fn reference(&self) -> String {
        format!("{}:{}", self.library, self.name)
    }

    pub fn file(&self) -> String {
        format!("symbols/{}.kicad_sym", self.library)
    }
}

/// One footprint of a `footprints/g-*.pretty` library.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryFootprint {
    /// Such as `g-Crystal`.
    pub library: String,
    /// The file's name without `.kicad_mod`.
    pub name: String,
    /// The SHA-256 of the file's bytes.
    pub sha256: String,
    /// Every 3D model link, exactly as written, in file order.
    pub models: Vec<String>,
}

impl LibraryFootprint {
    pub fn reference(&self) -> String {
        format!("{}:{}", self.library, self.name)
    }

    pub fn file(&self) -> String {
        format!("footprints/{}.pretty/{}.kicad_mod", self.library, self.name)
    }
}

/// A symbol or footprint a CSV row names.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CsvReference {
    /// The CSV file, such as `database/g-ics.csv`.
    pub file: String,
    /// [`CadItemKind::Symbol`] or [`CadItemKind::Footprint`].
    pub kind: CadItemKind,
    /// `LIBRARY:NAME`, one footprint choice each.
    pub reference: String,
}

/// What the Catalog has, as the library check needs it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KnownCad {
    /// For each CAD item the Catalog has, by CAD item ID, the SHA-256 of
    /// the bytes of its current revision and of its newest imported one.
    pub items: BTreeMap<String, BTreeSet<String>>,
    /// The CSV files imported at least once, such as `database/g-res.csv`.
    /// A file not imported yet has no last import to compare with.
    pub imported_csv_files: BTreeSet<String>,
}

impl KnownCad {
    /// What the import state read from the team catalog says.
    pub fn from_state(state: &crate::pocketbase::ImportState) -> Self {
        Self {
            items: state.cad_item_bytes.clone(),
            imported_csv_files: state
                .latest_imports
                .iter()
                .map(|run| format!("database/{}.csv", run.csv_file))
                .collect(),
        }
    }

    fn has(&self, kind: CadItemKind, key: &str) -> bool {
        self.items.contains_key(CadItemId::new(kind, key).as_str())
    }

    fn bytes_of(&self, kind: CadItemKind, key: &str) -> Option<&BTreeSet<String>> {
        self.items.get(CadItemId::new(kind, key).as_str())
    }
}

/// Everything [`check`] compares.
#[derive(Clone, Copy, Debug)]
pub struct CheckInputs<'a> {
    pub libraries: &'a Libraries,
    pub known: &'a KnownCad,
    pub csv: &'a [CsvReference],
    /// KiCad's own footprints (`LIBRARY:NAME`) that symbols name, when
    /// KiCad's libraries were found; `None` leaves symbols naming a
    /// footprint outside our libraries unchecked.
    pub kicad_footprints: Option<&'a BTreeSet<String>>,
    /// Each footprint file of today's CSV catalog and the SHA-256 of its
    /// bytes ([`todays_footprints`]).
    pub todays_footprints: &'a BTreeMap<String, String>,
}

/// Compares the CSV catalog's own libraries with the Catalog. The same
/// inputs always give the same findings, in the same order.
pub fn check(inputs: CheckInputs<'_>) -> LibraryCheck {
    let CheckInputs {
        libraries,
        known,
        csv,
        kicad_footprints,
        todays_footprints,
    } = inputs;
    let mut findings = Vec::new();
    // Each symbol and footprint the CSV files name, with the files.
    let mut used: BTreeMap<(CadItemKind, &str), BTreeSet<&str>> = BTreeMap::new();
    for reference in csv {
        used.entry((reference.kind, reference.reference.as_str()))
            .or_default()
            .insert(reference.file.as_str());
    }
    let footprint_names: BTreeSet<String> = libraries
        .footprints
        .iter()
        .map(LibraryFootprint::reference)
        .collect();

    // Symbols nobody uses yet, and symbols naming a footprint nobody has.
    for symbol in &libraries.symbols {
        let reference = symbol.reference();
        if !used.contains_key(&(CadItemKind::Symbol, reference.as_str()))
            && !known.has(CadItemKind::Symbol, &reference)
        {
            findings.push(not_in_catalog(&reference, symbol.file(), "part"));
        }
        let footprint = symbol.footprint.trim();
        if let Some((library, _)) = footprint.split_once(':') {
            let ours = library.starts_with("g-");
            let missing = if ours {
                !footprint_names.contains(footprint)
            } else {
                kicad_footprints.is_some_and(|kicad| !kicad.contains(footprint))
            };
            if missing {
                findings.push(Finding {
                    severity: Severity::Warning,
                    kind: FindingKind::SymbolFootprint,
                    item: reference.clone(),
                    file: symbol.file(),
                    message: format!(
                        "The symbol's own Footprint, {footprint}, is not in our libraries or KiCad's. Fix it in KiCad's Symbol Editor, or leave it blank."
                    ),
                });
            }
        }
    }

    // Footprints nobody uses yet, and their 3D model links.
    let mut linked_models: BTreeSet<String> = BTreeSet::new();
    for footprint in &libraries.footprints {
        let reference = footprint.reference();
        let files = used.get(&(CadItemKind::Footprint, reference.as_str()));
        let in_csv = files.is_some();
        let imported_file_uses_it = files.is_some_and(|files| {
            files
                .iter()
                .any(|file| known.imported_csv_files.contains(*file))
        });
        // Still as today's CSV catalog had it, byte for byte.
        let as_today = todays_footprints.get(&footprint.file()) == Some(&footprint.sha256);
        let state = match known.bytes_of(CadItemKind::Footprint, &reference) {
            Some(bytes) if bytes.contains(&footprint.sha256) => FootprintState::Unchanged,
            Some(_) => FootprintState::Changed,
            None if imported_file_uses_it => FootprintState::New,
            None if in_csv && as_today => FootprintState::BeforeFirstImport,
            None if in_csv => FootprintState::New,
            None => FootprintState::Unused,
        };
        if !in_csv && !known.has(CadItemKind::Footprint, &reference) {
            findings.push(not_in_catalog(&reference, footprint.file(), "part"));
        }
        for link in &footprint.models {
            let judged = judge_model_link(&footprint.library, &footprint.name, link);
            if let Some(path) = judged.path() {
                linked_models.insert(path.to_owned());
            }
            if let Some(finding) = model_finding(footprint, link, &judged, libraries, state) {
                findings.push(finding);
            }
        }
    }

    // 3D models no footprint links.
    for model in &libraries.models {
        if !linked_models.contains(model) && !known.has(CadItemKind::Model3d, model) {
            findings.push(not_in_catalog(model, model.clone(), "footprint"));
        }
    }

    // A CSV row naming something its library lacks, when a similar name is
    // there.
    let symbol_names: BTreeSet<String> = libraries
        .symbols
        .iter()
        .map(LibrarySymbol::reference)
        .collect();
    let mut suggested: BTreeSet<(&str, &str)> = BTreeSet::new();
    for reference in csv {
        let (names, library_file) = match reference.kind {
            CadItemKind::Symbol => (&symbol_names, "symbols/{library}.kicad_sym"),
            CadItemKind::Footprint => (&footprint_names, "footprints/{library}.pretty"),
            CadItemKind::Model3d => continue,
        };
        let Some((library, name)) = reference.reference.split_once(':') else {
            continue;
        };
        if !library.starts_with("g-") || names.contains(&reference.reference) {
            continue;
        }
        if !suggested.insert((reference.file.as_str(), reference.reference.as_str())) {
            continue;
        }
        let candidates = names
            .iter()
            .filter_map(|known| known.split_once(':'))
            .filter(|(known_library, _)| *known_library == library)
            .map(|(_, known_name)| known_name);
        if let Some(similar) = similar_name(name, candidates) {
            let kind = match reference.kind {
                CadItemKind::Symbol => "symbol",
                _ => "footprint",
            };
            findings.push(Finding {
                severity: Severity::Warning,
                kind: FindingKind::SimilarName,
                item: reference.reference.clone(),
                file: reference.file.clone(),
                message: format!(
                    "{} names the {kind} {}, which {} does not have. Did you mean {library}:{similar}?",
                    reference.file,
                    reference.reference,
                    library_file.replace("{library}", library)
                ),
            });
        }
    }

    for (file, why) in &libraries.unreadable {
        findings.push(Finding {
            severity: Severity::Warning,
            kind: FindingKind::Unreadable,
            item: file.clone(),
            file: file.clone(),
            message: format!("The file could not be read: {why}"),
        });
    }

    findings.sort_by(|a, b| {
        (a.group(), &a.file, &a.item, a.kind, &a.message).cmp(&(
            b.group(),
            &b.file,
            &b.item,
            b.kind,
            &b.message,
        ))
    });
    findings.dedup();
    LibraryCheck { findings }
}

/// Where a footprint stands against the last import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FootprintState {
    /// Not in the Catalog, only CSV files never imported use it, and its
    /// bytes are still today's CSV catalog's: their first import takes it
    /// as it is.
    BeforeFirstImport,
    /// The Catalog has these bytes.
    Unchanged,
    /// The Catalog has the footprint with other bytes.
    Changed,
    /// Not in the Catalog, and a CSV row uses it: a row of a file imported
    /// before, or of a file never imported when the footprint is not as
    /// today's CSV catalog had it. The next import brings it in.
    New,
    /// Not in the Catalog and no part uses it.
    Unused,
}

fn not_in_catalog(item: &str, file: String, user: &str) -> Finding {
    Finding {
        severity: Severity::Warning,
        kind: FindingKind::NotInCatalog,
        item: item.to_owned(),
        file,
        message: format!("No {user} uses it, and the Catalog does not have it yet."),
    }
}

/// What one 3D model link of a footprint is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelLink {
    /// In our library, in the footprint's folder and named like it.
    Own { path: String },
    /// In our library, but named or placed for another footprint.
    Shared { path: String },
    /// One of KiCad's own models.
    Kicad,
    /// Embedded in the footprint file.
    Embedded,
    /// Outside our library: it only works on one computer.
    Outside,
}

impl ModelLink {
    /// The repository path of a link into our library.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Own { path } | Self::Shared { path } => Some(path),
            _ => None,
        }
    }
}

/// Judges one 3D model link of the footprint `name` in `library` (such as
/// `g-Crystal`). Uses the CSV catalog fixture's classification of model
/// references; only a `${GITPLM_PARTS}/3d-models/...` path counts as our
/// library.
pub fn judge_model_link(library: &str, name: &str, link: &str) -> ModelLink {
    if link.starts_with("kicad-embed://") {
        return ModelLink::Embedded;
    }
    if is_kicad_model_link(link) {
        return ModelLink::Kicad;
    }
    let ours = link
        .strip_prefix("${GITPLM_PARTS}")
        .is_some_and(|rest| rest.starts_with(['/', '\\']));
    let classified = classify_model_reference(link);
    match classified.candidate {
        Some(path) if ours => {
            let folder = format!("3d-models/{library}.3dshapes/");
            let own = path.strip_prefix(&folder).is_some_and(|file| {
                !file.contains('/') && file.rsplit_once('.').is_some_and(|(stem, _)| stem == name)
            });
            if own {
                ModelLink::Own { path }
            } else {
                ModelLink::Shared { path }
            }
        }
        _ => ModelLink::Outside,
    }
}

/// `${KICAD<n>_3DMODEL_DIR}/...`, KiCad's own 3D models.
fn is_kicad_model_link(link: &str) -> bool {
    let Some(rest) = link.strip_prefix("${KICAD") else {
        return false;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    // A `..` part would climb out of KiCad's folder.
    digits > 0
        && rest[digits..]
            .strip_prefix("_3DMODEL_DIR}")
            .is_some_and(|rest| {
                rest.starts_with(['/', '\\']) && !rest.split(['/', '\\']).any(|part| part == "..")
            })
}

/// The link a footprint's own 3D model should have, such as
/// `${GITPLM_PARTS}/3d-models/g-Crystal.3dshapes/Abracon_ABS07L.step`.
pub fn own_model_link(library: &str, name: &str) -> String {
    format!("${{GITPLM_PARTS}}/3d-models/{library}.3dshapes/{name}.step")
}

fn model_finding(
    footprint: &LibraryFootprint,
    link: &str,
    judged: &ModelLink,
    libraries: &Libraries,
    state: FootprintState,
) -> Option<Finding> {
    let (kind, problem) = match judged {
        ModelLink::Own { path } | ModelLink::Shared { path }
            if !libraries.models.contains(path) =>
        {
            (
                FindingKind::MissingModel,
                format!(
                    "The footprint links the 3D model {path}, which is not in our library. Add the file, or fix the link in KiCad's Footprint Editor."
                ),
            )
        }
        ModelLink::Shared { path } => {
            return Some(Finding {
                severity: Severity::SoftWarning,
                kind: FindingKind::SharedModel,
                item: footprint.reference(),
                file: footprint.file(),
                message: format!(
                    "The footprint uses the 3D model {path}, which is named for another footprint. That is fine if they share it; check that it fits."
                ),
            });
        }
        ModelLink::Outside => (
            FindingKind::ModelOutside,
            format!(
                "The footprint links the 3D model {link}, which is outside our library, so it only works on one computer. Put the file in 3d-models/{}.3dshapes/ and link it as {}.",
                footprint.library,
                own_model_link(&footprint.library, &footprint.name)
            ),
        ),
        _ => return None,
    };
    let (severity, message) = match state {
        FootprintState::Changed => (
            Severity::Error,
            format!("{problem} The footprint changed since the last import."),
        ),
        FootprintState::New => (
            Severity::Error,
            format!("{problem} A CSV row uses the footprint, so the next import would add it."),
        ),
        FootprintState::Unused => (
            Severity::Warning,
            format!("{problem} Fix it before a part uses this footprint."),
        ),
        FootprintState::Unchanged => (
            Severity::CleanUp,
            format!("{problem} The Catalog already has the footprint like this."),
        ),
        FootprintState::BeforeFirstImport => (
            Severity::CleanUp,
            format!("{problem} It was like this before the first import."),
        ),
    };
    Some(Finding {
        severity,
        kind,
        item: footprint.reference(),
        file: footprint.file(),
        message,
    })
}

/// The 3D model links of a footprint someone chose in the app (Add part,
/// Edit part) that would stop it being saved, each in plain words: a link
/// outside our library, or one into our library whose 3D model is neither
/// in the Catalog nor sent with it (`available` says which keys, such as
/// `3d-models/g-Crystal.3dshapes/X.step`, are).
pub fn chosen_footprint_problems(
    library: &str,
    name: &str,
    links: &[String],
    available: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    for link in links {
        let problem = match judge_model_link(library, name, link) {
            ModelLink::Outside => format!(
                "The footprint's 3D model link {link} is outside the Catalog's library, so it only works on one computer. Link the model as {} in KiCad's Footprint Editor, or remove the link, then choose the file again.",
                own_model_link(library, name)
            ),
            ModelLink::Own { path } | ModelLink::Shared { path } if !available(&path) => {
                if crate::cad_file::is_catalog_model_key(&path) {
                    format!(
                        "The footprint links the 3D model {path}, which the Catalog does not have. Choose that 3D model file too, or fix the link in KiCad's Footprint Editor."
                    )
                } else {
                    format!(
                        "The footprint links the 3D model {path}, a place where the Catalog keeps no 3D models. Link the model as {} in KiCad's Footprint Editor, then choose the file again.",
                        own_model_link(library, name)
                    )
                }
            }
            _ => continue,
        };
        if !problems.contains(&problem) {
            problems.push(problem);
        }
    }
    problems
}

/// The name in `candidates` most like `name`: one contains the other
/// (ignoring case, the shorter at least 3 characters long), or they are at
/// most 2 edits apart. The closest wins, then the first in name order.
pub fn similar_name<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let wanted = name.to_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        let lower = candidate.to_lowercase();
        if lower == wanted && candidate == name {
            continue;
        }
        let distance = edit_distance(&wanted, &lower);
        let (short, long) = if wanted.len() <= lower.len() {
            (&wanted, &lower)
        } else {
            (&lower, &wanted)
        };
        let contains = short.chars().count() >= 3 && long.contains(short.as_str());
        if distance > 2 && !contains {
            continue;
        }
        let better = match best {
            None => true,
            Some((kept, kept_name)) => {
                distance < kept || (distance == kept && candidate < kept_name)
            }
        };
        if better {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// The Levenshtein distance between `a` and `b`, by character.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[b.len()]
}

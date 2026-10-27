//! The library check (kicad-parts-da65): compares our own libraries in the
//! local library folder (`symbols/g-*.kicad_sym`, `footprints/g-*.pretty`,
//! `3d-models/**`, read through the default library source profile,
//! [`SourceProfile::own`]) with the Catalog, and says what is in them but
//! not in the Catalog yet (made by hand in KiCad), which symbols name a
//! footprint nobody has, and which 3D model links work only on one computer
//! or name a missing file.
//!
//! [`check`] is one pure function over plain inputs; [`read`] gathers those
//! inputs from the local library folder, the Catalog's CAD items and
//! KiCad's own libraries.
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
//! A problem is an error (to fix) on a footprint whose bytes differ from
//! the Catalog's current revision of it: adding it to a part would bring
//! the problem in. The same problem on a footprint the Catalog has
//! unchanged is clean-up. On a footprint the Catalog does not have, a
//! problem is a warning until a part uses the footprint.

pub mod read;

#[cfg(test)]
mod tests;

use crate::cad_item::CadItemKind;
use crate::ids::CadItemId;
use kicad_integration::library_source::{self, SourceProfile};
use std::collections::{BTreeMap, BTreeSet};

/// How much a finding matters.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// Must be fixed: it stops Add part or Edit part with it.
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
    /// A symbol, footprint or 3D model in our libraries that the Catalog
    /// does not have.
    NotInCatalog,
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
    /// The repository file it is in, such as `symbols/g-ics.kicad_sym` or
    /// `footprints/g-Crystal.pretty/Abracon_ABS07L.kicad_mod`.
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
    /// Whether KiCad's own libraries were found, so symbols naming one of
    /// KiCad's footprints were checked too.
    pub kicad_footprints_checked: bool,
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
}

/// The symbols, footprints and 3D models of our own (`g-`) libraries.
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
    /// The library's repo path, such as `symbols/g-ics.kicad_sym`.
    pub file: String,
    pub name: String,
    /// The symbol's own Footprint property, blank if it has none.
    pub footprint: String,
    /// A power symbol, `(power)`, such as GND: it never becomes a part.
    pub power: bool,
}

impl LibrarySymbol {
    pub fn reference(&self) -> String {
        format!("{}:{}", self.library, self.name)
    }
}

/// One footprint of a `footprints/g-*.pretty` library.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryFootprint {
    /// Such as `g-Crystal`.
    pub library: String,
    /// The file's name without `.kicad_mod`.
    pub name: String,
    /// The file's repo path, such as
    /// `footprints/g-Crystal.pretty/Abracon_ABS07L.kicad_mod`.
    pub file: String,
    /// The SHA-256 of the file's bytes.
    pub sha256: String,
    /// Every 3D model link, exactly as written, in file order.
    pub models: Vec<String>,
}

impl LibraryFootprint {
    pub fn reference(&self) -> String {
        format!("{}:{}", self.library, self.name)
    }
}

/// What the Catalog has, as the library check needs it: for each CAD item,
/// by CAD item ID, the SHA-256 of the bytes of its current revision
/// ([`crate::pocketbase::read_current_cad_bytes`]).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KnownCad {
    pub items: BTreeMap<String, String>,
}

impl KnownCad {
    fn has(&self, kind: CadItemKind, key: &str) -> bool {
        self.items.contains_key(CadItemId::new(kind, key).as_str())
    }

    fn bytes_of(&self, kind: CadItemKind, key: &str) -> Option<&String> {
        self.items.get(CadItemId::new(kind, key).as_str())
    }
}

/// Everything [`check`] compares.
#[derive(Clone, Copy, Debug)]
pub struct CheckInputs<'a> {
    pub libraries: &'a Libraries,
    pub known: &'a KnownCad,
    /// KiCad's own footprints (`LIBRARY:NAME`) that symbols name, when
    /// KiCad's libraries were found; `None` leaves symbols naming a
    /// footprint outside our libraries unchecked.
    pub kicad_footprints: Option<&'a BTreeSet<String>>,
}

/// Compares our own libraries with the Catalog. The same inputs always
/// give the same findings, in the same order.
pub fn check(inputs: CheckInputs<'_>) -> LibraryCheck {
    let CheckInputs {
        libraries,
        known,
        kicad_footprints,
    } = inputs;
    let own = SourceProfile::own();
    let mut findings = Vec::new();
    let footprint_names: BTreeSet<String> = libraries
        .footprints
        .iter()
        .map(LibraryFootprint::reference)
        .collect();

    // Symbols nobody uses yet, and symbols naming a footprint nobody has.
    for symbol in &libraries.symbols {
        let reference = symbol.reference();
        // A power symbol (GND, VCC...) never becomes a part.
        if !symbol.power && !known.has(CadItemKind::Symbol, &reference) {
            findings.push(not_in_catalog(&reference, symbol.file.clone(), "part"));
        }
        let footprint = symbol.footprint.trim();
        if let Some((library, _)) = footprint.split_once(':') {
            let ours = own.is_own_name(library);
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
                    file: symbol.file.clone(),
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
        let state = match known.bytes_of(CadItemKind::Footprint, &reference) {
            Some(bytes) if *bytes == footprint.sha256 => FootprintState::Unchanged,
            Some(_) => FootprintState::Changed,
            None => FootprintState::Unused,
        };
        if state == FootprintState::Unused {
            findings.push(not_in_catalog(&reference, footprint.file.clone(), "part"));
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

    for (file, why) in &libraries.unreadable {
        findings.push(Finding {
            severity: Severity::Warning,
            kind: FindingKind::Unreadable,
            item: file.clone(),
            file: file.clone(),
            message: format!("Could not be read: {why}"),
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
    LibraryCheck {
        findings,
        kicad_footprints_checked: kicad_footprints.is_some(),
    }
}

/// Where a footprint stands against the Catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FootprintState {
    /// The Catalog's current revision has these bytes.
    Unchanged,
    /// The Catalog has the footprint with other bytes.
    Changed,
    /// Not in the Catalog, so no part uses it.
    Unused,
}

fn not_in_catalog(item: &str, file: String, user: &str) -> Finding {
    Finding {
        severity: Severity::Warning,
        kind: FindingKind::NotInCatalog,
        item: item.to_owned(),
        file,
        message: format!("No {user} uses it yet: the Catalog does not have it."),
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
/// `g-Crystal`), through our layout, the default library source profile:
/// only a `${GITPLM_PARTS}/3d-models/...` path counts as our library.
pub fn judge_model_link(library: &str, name: &str, link: &str) -> ModelLink {
    judge_model_link_in(&SourceProfile::own(), library, name, link)
}

/// [`judge_model_link`] through the library source `source`: a link into
/// one of its model folders is in its library, and a footprint's own model
/// sits in its library's own model folder, named like the footprint.
pub fn judge_model_link_in(
    source: &SourceProfile,
    library: &str,
    name: &str,
    link: &str,
) -> ModelLink {
    if link.starts_with("kicad-embed://") {
        return ModelLink::Embedded;
    }
    if is_kicad_model_link(link) {
        return ModelLink::Kicad;
    }
    match source.model_link(link) {
        library_source::ModelLink::InRepo {
            path,
            in_model_folder: true,
        } => {
            let own = source.own_model_folder(library).is_some_and(|folder| {
                path.strip_prefix(&format!("{folder}/"))
                    .is_some_and(|file| {
                        !file.contains('/')
                            && file.rsplit_once('.').is_some_and(|(stem, _)| stem == name)
                    })
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
    let own = SourceProfile::own();
    own.link_to(&library_source::join(
        &own_model_folder(library),
        &format!("{name}.step"),
    ))
    .unwrap_or_default()
}

/// The repo folder of a footprint library's own 3D models in our layout,
/// such as `3d-models/g-Crystal.3dshapes`.
pub fn own_model_folder(library: &str) -> String {
    SourceProfile::own()
        .own_model_folder(library)
        .unwrap_or_default()
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
                file: footprint.file.clone(),
                message: format!(
                    "The footprint uses the 3D model {path}, which is named for another footprint. That is fine if they share it; check that it fits."
                ),
            });
        }
        ModelLink::Outside => (
            FindingKind::ModelOutside,
            format!(
                "The footprint links the 3D model {link}, which is outside our library, so it only works on one computer. Put the file in {}/ and link it as {}.",
                own_model_folder(&footprint.library),
                own_model_link(&footprint.library, &footprint.name)
            ),
        ),
        _ => return None,
    };
    let (severity, message) = match state {
        FootprintState::Changed => (
            Severity::Error,
            format!("{problem} The footprint differs from the Catalog's."),
        ),
        FootprintState::Unused => (
            Severity::Warning,
            format!("{problem} Fix it before a part uses this footprint."),
        ),
        FootprintState::Unchanged => (
            Severity::CleanUp,
            format!("{problem} The Catalog already has the footprint like this."),
        ),
    };
    Some(Finding {
        severity,
        kind,
        item: footprint.reference(),
        file: footprint.file.clone(),
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

//! Add part from our libraries (kicad-parts-37a7): a symbol and a footprint
//! already in the CSV catalog's own libraries (drawn by hand in KiCad, or
//! committed by someone else) instead of a download.
//!
//! The chosen items are read from the CSV catalog folder the app reads,
//! with the library check's safety (no link is followed; every path stays
//! inside the folder), and handed to the same reading as a download
//! ([`crate::add_part::read_files`]), so the category guess, the next free
//! IPN and the checks are the same. The footprint keeps its own 3D model
//! link; the model is sent when the link points into our library. The
//! files are sent exactly as they are in the library, with references
//! unchanged, and each records the repository file and the commit it was
//! read from, so a later CSV import naming them finds nothing to change.
//! Symbols and footprints the Catalog already has are used, not sent.

use crate::add_part::{self, Context, Origin};
use crate::git::commit_holding;
use catalog::CadItemKind;
use catalog::cad_file::{
    footprint_upload, is_catalog_model_key, is_plain_name, model_upload, read_symbol_file,
    symbol_upload,
};
use catalog::download::{Download, DownloadFile, DownloadKind, model_links};
use catalog::library_check::read::read_library_file;
use catalog::library_check::{FindingKind, LibraryCheck, Severity, judge_model_link};
use catalog::pocketbase::{CadUpload, LibrarySource};
use catalog::sha256_hex;
use desktop_contract::{TeamDownload, TeamLibraryChoice, TeamLibraryReading};
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup, symbol_properties};
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

/// Where a part from our libraries came from, as its history says it.
pub(crate) const FROM_OUR_LIBRARIES: &str = "Our libraries";

/// A symbol of our libraries.
struct Symbol {
    /// Such as `g-ics:IC_TP4333`.
    reference: String,
    name: String,
    /// Such as `symbols/g-ics.kicad_sym`.
    path: String,
    /// The whole library file.
    file: Vec<u8>,
    /// The symbol's entry in it.
    span: Range<usize>,
    version: Option<String>,
    extends: bool,
}

/// A footprint of our libraries, or the 3D model it links there.
struct File {
    /// `LIB:NAME` for a footprint; the repository path for a 3D model.
    reference: String,
    /// Such as `footprints/g-Package_SO.pretty/SOIC-8.kicad_mod`.
    path: String,
    bytes: Vec<u8>,
}

/// The chosen items, read, and what the app makes of them before saving.
pub(crate) struct Plan {
    symbol: Option<Symbol>,
    footprint: Option<File>,
    model: Option<File>,
    /// The chosen items the Catalog already has (by reference or path).
    in_catalog: BTreeSet<String>,
    /// The commit holding every file to send, or why there is none.
    commit: Result<String, String>,
    problems: Vec<String>,
    findings: Vec<String>,
    notes: Vec<String>,
}

/// The library and name of `reference`, one of our libraries' symbols or
/// footprints (`g-<library>:<name>`, plain names), or why it is not.
fn ours(reference: &str) -> Result<(&str, &str), String> {
    reference
        .split_once(':')
        .filter(|(library, name)| {
            library.starts_with("g-")
                && library.len() > 2
                && is_plain_name(library)
                && is_plain_name(name)
        })
        .ok_or_else(|| {
            format!(
                "{reference} is not one of our libraries' symbols or footprints, such as g-ics:IC_TP4333."
            )
        })
}

fn read_symbol(root: &Path, reference: &str) -> Result<Symbol, String> {
    let (library, name) = ours(reference)?;
    let path = format!("symbols/{library}.kicad_sym");
    let file = read_library_file(root, &path)?;
    let parsed = SymbolLibrary::parse(&file)
        .map_err(|error| format!("{path} is not a KiCad symbol library: {error}."))?;
    let entry = match parsed.find(name) {
        SymbolLookup::Found(entry) => entry,
        SymbolLookup::Missing => return Err(format!("{path} has no symbol {name}.")),
        SymbolLookup::Ambiguous => {
            return Err(format!("{path} has more than one symbol named {name}."));
        }
    };
    Ok(Symbol {
        reference: reference.to_owned(),
        name: name.to_owned(),
        path,
        span: entry.span.clone(),
        extends: entry.extends.is_some(),
        version: parsed.version.clone(),
        file,
    })
}

fn read_footprint(root: &Path, reference: &str) -> Result<File, String> {
    let (library, name) = ours(reference)?;
    let path = format!("footprints/{library}.pretty/{name}.kicad_mod");
    let bytes = read_library_file(root, &path)?;
    Ok(File {
        reference: reference.to_owned(),
        path,
        bytes,
    })
}

/// Reads the chosen symbol and footprint from the CSV catalog under
/// `root` and works out what saving them needs: whether the Catalog has
/// each (`has`), the commit holding the files to send, and what stops the
/// save (`check`'s errors and model problems for these items, files that
/// are not committed as they are, files the Catalog cannot take). With
/// `follow_symbol` and no footprint chosen, the footprint is the one the
/// symbol names, when our libraries have it.
pub(crate) fn plan(
    root: &Path,
    choice: &TeamLibraryChoice,
    follow_symbol: bool,
    check: &LibraryCheck,
    has: &dyn Fn(CadItemKind, &str) -> Result<bool, String>,
) -> Result<Plan, String> {
    let wanted = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let mut notes = Vec::new();
    let symbol = wanted(&choice.symbol)
        .map(|reference| read_symbol(root, &reference))
        .transpose()?;
    let mut footprint = wanted(&choice.footprint)
        .map(|reference| read_footprint(root, &reference))
        .transpose()?;
    if footprint.is_none() && follow_symbol {
        // The footprint the symbol names, when it is one of ours.
        let named = symbol.as_ref().and_then(|symbol| {
            symbol_properties(&symbol.file[symbol.span.clone()])
                .into_iter()
                .find(|(name, _)| name == "Footprint")
                .map(|(_, value)| value.trim().to_owned())
        });
        if let Some(found) = named.and_then(|named| read_footprint(root, &named).ok()) {
            notes.push(format!(
                "The footprint is the one the symbol names, {}.",
                found.reference
            ));
            footprint = Some(found);
        }
    }
    // The footprint's own 3D model link into our library, when the file is
    // there.
    let mut model = None;
    if let Some(footprint) = &footprint {
        let (library, name) = ours(&footprint.reference)?;
        for link in model_links(&String::from_utf8_lossy(&footprint.bytes)) {
            let judged = judge_model_link(library, name, &link);
            let Some(path) = judged.path().filter(|path| is_catalog_model_key(path)) else {
                continue;
            };
            if let Ok(bytes) = read_library_file(root, path) {
                model = Some(File {
                    reference: path.to_owned(),
                    path: path.to_owned(),
                    bytes,
                });
                break;
            }
        }
    }

    let mut in_catalog = BTreeSet::new();
    if let Some(symbol) = &symbol {
        if has(CadItemKind::Symbol, &symbol.reference)? {
            in_catalog.insert(symbol.reference.clone());
        }
    }
    if let Some(footprint) = &footprint {
        if has(CadItemKind::Footprint, &footprint.reference)? {
            in_catalog.insert(footprint.reference.clone());
        }
    }
    if let Some(model) = &model {
        if has(CadItemKind::Model3d, &model.path)? {
            in_catalog.insert(model.path.clone());
        }
    }

    let mut plan = Plan {
        symbol,
        footprint,
        model,
        in_catalog,
        commit: Ok(String::new()),
        problems: Vec::new(),
        findings: Vec::new(),
        notes,
    };
    // What the library check found about the chosen items. On an item to
    // send, an error, a 3D model link outside our library or a missing 3D
    // model file stops the save; the rest is said.
    for reference in plan.references() {
        let sent = !plan.in_catalog.contains(&reference);
        for finding in check.findings.iter().filter(|finding| {
            finding.item == reference && finding.kind != FindingKind::NotInCatalog
        }) {
            let stops = sent
                && (finding.severity == Severity::Error
                    || matches!(
                        finding.kind,
                        FindingKind::ModelOutside | FindingKind::MissingModel
                    ));
            let said = format!("{}: {}", finding.item, finding.message);
            let list = if stops {
                &mut plan.problems
            } else {
                &mut plan.findings
            };
            if !list.contains(&said) {
                list.push(said);
            }
        }
    }
    // Files the Catalog cannot take, said as the upload says it.
    if let Err(problem) = plan.uploads_without_commit("") {
        if !plan.problems.contains(&problem) {
            plan.problems.push(problem);
        }
    }
    let paths: Vec<String> = plan.sent_paths().into_iter().map(str::to_owned).collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    plan.commit = commit_holding(root, &paths);
    if let Err(problem) = &plan.commit {
        if !paths.is_empty() {
            plan.problems.push(problem.clone());
        }
    }
    Ok(plan)
}

impl Plan {
    /// The chosen symbol and footprint references.
    fn references(&self) -> Vec<String> {
        self.symbol
            .iter()
            .map(|symbol| symbol.reference.clone())
            .chain(self.footprint.iter().map(|file| file.reference.clone()))
            .collect()
    }

    fn sends_symbol(&self) -> Option<&Symbol> {
        self.symbol
            .as_ref()
            .filter(|symbol| !self.in_catalog.contains(&symbol.reference))
    }

    fn sends_footprint(&self) -> Option<&File> {
        self.footprint
            .as_ref()
            .filter(|file| !self.in_catalog.contains(&file.reference))
    }

    /// The 3D model goes with a footprint that is sent, unless the Catalog
    /// has it.
    fn sends_model(&self) -> Option<&File> {
        self.sends_footprint()?;
        self.model
            .as_ref()
            .filter(|file| !self.in_catalog.contains(&file.path))
    }

    /// The repository paths of the files to send.
    fn sent_paths(&self) -> Vec<&str> {
        self.sends_symbol()
            .map(|symbol| symbol.path.as_str())
            .into_iter()
            .chain(self.sends_footprint().map(|file| file.path.as_str()))
            .chain(self.sends_model().map(|file| file.path.as_str()))
            .collect()
    }

    /// The files to send, each naming where it is in our libraries and
    /// `commit`.
    fn uploads_without_commit(&self, commit: &str) -> Result<Vec<CadUpload>, String> {
        let label = |path: &str| {
            if commit.is_empty() {
                path.to_owned()
            } else {
                format!("{path} at {}", &commit[..12.min(commit.len())])
            }
        };
        let source = |path: &str, bytes: &[u8], byte_range: Option<[u64; 2]>| LibrarySource {
            path: path.to_owned(),
            byte_sha256: sha256_hex(bytes),
            commit: commit.to_owned(),
            byte_range,
        };
        let mut uploads = Vec::new();
        if let Some(symbol) = self.sends_symbol() {
            let mut upload = symbol_upload(
                &symbol.file,
                &symbol.name,
                &symbol.reference,
                &label(&symbol.path),
            )
            .map_err(|why| format!("{}: {why}", symbol.reference))?;
            upload.library = Some(source(
                &symbol.path,
                &symbol.file,
                Some([symbol.span.start as u64, symbol.span.end as u64]),
            ));
            uploads.push(upload);
        }
        if let Some(file) = self.sends_footprint() {
            let mut upload = footprint_upload(&file.bytes, &file.reference, &label(&file.path))
                .map_err(|why| format!("{}: {why}", file.path))?;
            upload.library = Some(source(&file.path, &file.bytes, None));
            uploads.push(upload);
        }
        if let Some(file) = self.sends_model() {
            let mut upload = model_upload(&file.bytes, &file.path, &label(&file.path))?;
            upload.library = Some(source(&file.path, &file.bytes, None));
            uploads.push(upload);
        }
        Ok(uploads)
    }

    /// What saving sends, or why it cannot: everything that stops the
    /// save, said once, then "Nothing was saved."
    pub(crate) fn uploads(&self) -> Result<Vec<CadUpload>, String> {
        if !self.problems.is_empty() {
            return Err(format!("{} Nothing was saved.", self.problems.join(" ")));
        }
        let commit = self.commit.clone().unwrap_or_default();
        self.uploads_without_commit(&commit)
    }

    /// The files the part came from, as its history lists them, such as
    /// `symbols/g-ics.kicad_sym at 1a2b3c4d5e6f`.
    pub(crate) fn sources(&self) -> Vec<String> {
        let commit = self.commit.clone().unwrap_or_default();
        self.sent_paths()
            .into_iter()
            .map(|path| match commit.get(..12) {
                Some(short) => format!("{path} at {short}"),
                None => path.to_owned(),
            })
            .collect()
    }

    /// The download the reading works on: the symbol in a library file of
    /// its own (with its version, so it is drawn and sent the same), the
    /// footprint file and its 3D model, each labelled by its repository
    /// path.
    fn download(&self) -> (Download, Option<usize>) {
        let mut files = Vec::new();
        let mut at = None;
        if let Some(symbol) = &self.symbol {
            // A symbol drawn from another needs the whole file to be drawn
            // and counted; it cannot be sent on its own anyway.
            let bytes = if symbol.extends {
                symbol.file.clone()
            } else {
                let mut text = b"(kicad_symbol_lib\n".to_vec();
                if let Some(version) = &symbol.version {
                    text.extend_from_slice(format!("\t(version {version})\n").as_bytes());
                }
                text.extend_from_slice(b"\t(generator \"kicad_parts\")\n\t");
                text.extend_from_slice(&symbol.file[symbol.span.clone()]);
                text.extend_from_slice(b"\n)\n");
                text
            };
            at = read_symbol_file(&bytes)
                .ok()
                .and_then(|file| file.names.iter().position(|name| *name == symbol.name));
            files.push(DownloadFile {
                label: symbol.path.clone(),
                file_name: symbol
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                kind: DownloadKind::Symbol,
                bytes,
            });
        }
        for (file, kind) in [
            (&self.footprint, DownloadKind::Footprint),
            (&self.model, DownloadKind::Model3d),
        ] {
            if let Some(file) = file {
                files.push(DownloadFile {
                    label: file.path.clone(),
                    file_name: file.path.rsplit('/').next().unwrap_or_default().to_owned(),
                    kind,
                    bytes: file.bytes.clone(),
                });
            }
        }
        (
            Download {
                files,
                vendor: None,
                ignored: 0,
            },
            at,
        )
    }

    /// What Add part shows for the chosen items: the same reading as a
    /// download's, with the references exactly as in our libraries.
    pub(crate) fn reading(&self, context: &Context<'_>) -> TeamDownload {
        let footprint_reference = self
            .footprint
            .as_ref()
            .map(|file| file.reference.clone())
            .unwrap_or_default();
        let (download, symbol_at) = self.download();
        let mut reading = add_part::read_files(
            &download,
            symbol_at,
            Some(0),
            true,
            context,
            Origin::Library {
                footprint: &footprint_reference,
            },
        );
        reading.from = FROM_OUR_LIBRARIES.to_owned();
        let mut notes = self.notes.clone();
        // The footprint stays in its own library.
        if let Some((library, _)) = footprint_reference.split_once(':') {
            reading.footprint_library = library.to_owned();
            for category in &mut reading.categories {
                category.footprint_library = library.to_owned();
            }
        }
        match (&self.footprint, &self.model) {
            (Some(_), Some(model)) if self.in_catalog.contains(&model.path) => notes.push(format!(
                "The footprint's 3D model is {}, which the Catalog already has.",
                model.path
            )),
            (Some(_), Some(model)) => notes.push(format!(
                "The footprint's 3D model {} from our library is saved with it.",
                model.path
            )),
            (Some(_), None) => {}
            _ => {}
        }
        for item in self.references() {
            if self.in_catalog.contains(&item) {
                notes.push(format!(
                    "The Catalog already has {item}, so the part uses it as the Catalog has it."
                ));
            }
        }
        notes.append(&mut reading.notes);
        reading.notes = notes;
        reading.library = Some(TeamLibraryReading {
            symbol: self
                .symbol
                .as_ref()
                .map(|symbol| symbol.reference.clone())
                .unwrap_or_default(),
            footprint: footprint_reference,
            model: self
                .model
                .as_ref()
                .map(|file| file.path.clone())
                .unwrap_or_default(),
            commit: self.commit.clone().unwrap_or_default(),
            in_catalog: self.in_catalog.iter().cloned().collect(),
            problems: self.problems.clone(),
            findings: self.findings.clone(),
        });
        reading
    }
}

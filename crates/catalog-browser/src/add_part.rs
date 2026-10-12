//! Add part (kicad-parts-9693): read the CAD files an engineer downloaded,
//! fill in what the symbol says, guess the category, offer the next free
//! IPN, and check before saving: the MPN already in the Catalog, a
//! footprint the Catalog or KiCad already has, and the pin and pad counts.
//! Then turn the confirmed form into the files a new part sends.
//!
//! Everything here works on what the app already has: the team catalog's
//! categories, remembered matches and current parts, the CSV catalog's
//! IPNs and footprint files, and KiCad's own footprint libraries. No
//! network, so the guess is the same every time.

use catalog::cad_file::{
    footprint_upload, is_catalog_model_key, is_plain_name, model_upload, read_footprint_file,
    read_symbol_file, suggested_footprint_library, symbol_upload,
};
use catalog::download::{
    self, Download, DownloadFile, DownloadKind, PadKey, fill_fields, link_model, manufacturer_part,
    next_free_ipn, pad_count, pad_signature, pin_count, symbol_fields,
};
use catalog::guess::{Clues, ExamplePart, RememberedMatch, best_guess, guess_category};
use catalog::pocketbase::CadUpload;
use catalog::{CategoryDetails, PartDetails, PartRevision};
use desktop_contract::{
    Field, FileKind, TeamDownload, TeamDownloadRequest, TeamFile, TeamGuess, TeamKnownFootprint,
    TeamNewCategory, TeamNewPartRequest, TeamSameMpn,
};
use kicad_integration::library_file::{SymbolLibrary, symbol_reference};
use kicad_integration::{LibraryResolver, footprint_choices};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The largest name a file may have as its source on the team server.
const MAX_FILE_NAME: usize = 255;

/// What Add part reads besides the download.
pub(crate) struct Context<'a> {
    /// Every category's current revision.
    pub categories: Vec<CategoryDetails>,
    pub remembered: Vec<RememberedMatch>,
    /// Every part's current details and revision, from the followed
    /// replica.
    pub parts: Vec<(PartDetails, PartRevision)>,
    /// Every IPN known: the team catalog's and the CSV catalog's.
    pub ipns: BTreeSet<String>,
    /// The CSV catalog's own footprints.
    pub footprints: &'a [LocalFootprint],
    /// KiCad's data folder, when KiCad was found.
    pub kicad_share: Option<PathBuf>,
}

/// One footprint file of the CSV catalog (`footprints/g-*.pretty`).
#[derive(Clone, Debug)]
pub(crate) struct LocalFootprint {
    pub reference: String,
    pub name: String,
    pub pads: Option<BTreeSet<PadKey>>,
}

/// Every footprint of the CSV catalog under `root`, with its pads.
pub(crate) fn local_footprints(root: &Path) -> Vec<LocalFootprint> {
    let mut found = Vec::new();
    let Ok(libraries) = std::fs::read_dir(root.join("footprints")) else {
        return found;
    };
    let mut libraries: Vec<PathBuf> = libraries.flatten().map(|entry| entry.path()).collect();
    libraries.sort();
    for library in libraries {
        let Some(name) = library
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".pretty"))
            .map(str::to_owned)
        else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(&library) else {
            continue;
        };
        let mut files: Vec<PathBuf> = files.flatten().map(|entry| entry.path()).collect();
        files.sort();
        for file in files {
            let Some(item) = file
                .file_name()
                .and_then(|item| item.to_str())
                .and_then(|item| item.strip_suffix(".kicad_mod"))
            else {
                continue;
            };
            let bytes = std::fs::read(&file).unwrap_or_default();
            found.push(LocalFootprint {
                reference: format!("{name}:{item}"),
                name: item.to_owned(),
                pads: pad_signature(&bytes),
            });
        }
    }
    found
}

/// Standard base64.
fn encoded(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `label` cut to what the team server keeps as a file's source.
fn shortened(label: &str) -> String {
    if label.chars().count() <= MAX_FILE_NAME {
        return label.to_owned();
    }
    label.chars().take(MAX_FILE_NAME - 1).chain(['…']).collect()
}

/// One symbol of the download.
struct SymbolChoice<'a> {
    file: &'a DownloadFile,
    name: String,
}

/// Reads the downloaded files: see [`desktop_contract::TeamDownload`].
pub(crate) fn read(
    request: &TeamDownloadRequest,
    context: &Context<'_>,
) -> Result<TeamDownload, String> {
    let mut inputs = Vec::new();
    for file in &request.files {
        inputs.push((
            file.file_name.clone(),
            crate::team::decoded(&file.base64, &file.file_name)?,
        ));
    }
    let download = download::read_download(&inputs)?;
    if download.files.is_empty() {
        return Err(
            "The files hold no KiCad symbol (.kicad_sym), footprint (.kicad_mod) or 3D model (.step, .stp or .wrl)."
                .into(),
        );
    }
    let mut notes = Vec::new();
    let symbols = symbol_choices(&download, &mut notes);
    let footprints: Vec<&DownloadFile> = download
        .of_kind(DownloadKind::Footprint)
        .filter(|file| match read_footprint_file(&file.bytes) {
            Ok(_) => true,
            Err(why) => {
                notes.push(format!("{} could not be read: {why}", file.label));
                false
            }
        })
        .collect();
    let models: Vec<&DownloadFile> = download.of_kind(DownloadKind::Model3d).collect();
    let symbol_at = request
        .symbol
        .unwrap_or(0)
        .min(symbols.len().saturating_sub(1));
    let footprint_at = request
        .footprint
        .unwrap_or(0)
        .min(footprints.len().saturating_sub(1));
    let symbol = symbols.get(symbol_at);
    let footprint = footprints.get(footprint_at).copied();
    if symbol.is_none() {
        notes.push("The download has no KiCad symbol (.kicad_sym).".into());
    }
    if footprint.is_none() {
        notes.push("The download has no KiCad footprint (.kicad_mod).".into());
    }

    // What the symbol says.
    let entry = symbol.and_then(|symbol| symbol_entry(&symbol.file.bytes, &symbol.name));
    let properties = entry.as_deref().map(symbol_fields).unwrap_or_default();
    let (mpn, manufacturer) = manufacturer_part(&properties);
    let footprint_info = footprint.and_then(|file| read_footprint_file(&file.bytes).ok());
    let footprint_name = footprint_info
        .as_ref()
        .map(|info| info.name.clone())
        .unwrap_or_default();

    // The category guess, from the clues a download gives.
    let clues = Clues {
        reference: entry
            .as_deref()
            .and_then(symbol_reference)
            .unwrap_or_default(),
        footprints: footprint_info
            .iter()
            .map(|info| info.name.clone())
            .collect(),
        symbol: symbol.map(|symbol| symbol.name.clone()).unwrap_or_default(),
        description: download::description(&properties),
        keywords: download::keywords(&properties),
        manufacturer: manufacturer.clone(),
        mpn: mpn.clone(),
        supplier_category: String::new(),
    };
    let examples: Vec<ExamplePart> = context.parts.iter().map(example).collect();
    let ranked = guess_category(&clues, &context.categories, &context.remembered, &examples);
    let guesses: Vec<TeamGuess> = if best_guess(&ranked).is_some() {
        ranked
            .iter()
            .filter(|guess| guess.score > 0)
            .take(3)
            .map(|guess| TeamGuess {
                code: guess.category.clone(),
                name: guess.name.clone(),
                reasons: guess.reasons.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };

    let categories = context
        .categories
        .iter()
        .map(|category| {
            let (fields, unused) = fill_fields(&category.fields, &properties);
            TeamNewCategory {
                code: category.code.clone(),
                name: category.name.clone(),
                next_ipn: next_free_ipn(
                    &category.ipn_prefix,
                    context.ipns.iter().map(String::as_str),
                )
                .unwrap_or_default(),
                fields: as_fields(fields),
                unused: as_fields(unused),
            }
        })
        .collect();

    let footprint_library = suggested_footprint_library(&footprint_name)
        .unwrap_or_default()
        .to_owned();
    let known_footprints = match footprint {
        Some(file) => known_footprints(file, &footprint_name, &footprint_library, context),
        None => Vec::new(),
    };
    let model = footprint_info
        .as_ref()
        .and_then(|info| linked_model(info, &models))
        .or((!models.is_empty()).then_some(0));
    match (footprint, model) {
        (Some(_), Some(at)) => notes.push(format!(
            "The footprint will use the 3D model {} from the download.",
            models[at].file_name
        )),
        (Some(_), None) => notes.push("The download has no 3D model.".into()),
        _ => {}
    }

    Ok(TeamDownload {
        from: download
            .vendor
            .map(|vendor| vendor.name().to_owned())
            .unwrap_or_default(),
        ignored: download.ignored,
        symbols: symbols
            .iter()
            .map(|symbol| format!("{} in {}", symbol.name, symbol.file.label))
            .collect(),
        symbol: symbol.map(|_| symbol_at),
        footprints: footprints.iter().map(|file| file.label.clone()).collect(),
        footprint: footprint.map(|_| footprint_at),
        models: models.iter().map(|file| file.label.clone()).collect(),
        model,
        symbol_file: symbol.map(|symbol| TeamFile {
            kind: FileKind::Symbol,
            file_name: symbol.file.file_name.clone(),
            base64: encoded(&symbol.file.bytes),
            symbol: Some(symbol.name.clone()),
            model_key: None,
            replaces: None,
            label: Some(shortened(&symbol.file.label)),
        }),
        footprint_file: footprint.map(|file| team_file(file, FileKind::Footprint)),
        model_files: models
            .iter()
            .map(|file| team_file(file, FileKind::Model3d))
            .collect(),
        symbol_name: symbol.map(|symbol| symbol.name.clone()).unwrap_or_default(),
        footprint_name,
        footprint_library,
        same_mpn: same_mpn(&mpn, &manufacturer, &context.parts),
        mpn,
        manufacturer,
        categories,
        guesses,
        known_footprints,
        pins: entry.as_deref().map(|entry| pin_count(entry) as u32),
        pads: footprint.map(|file| pad_count(&file.bytes) as u32),
        notes,
    })
}

fn team_file(file: &DownloadFile, kind: FileKind) -> TeamFile {
    TeamFile {
        kind,
        file_name: file.file_name.clone(),
        base64: encoded(&file.bytes),
        symbol: None,
        model_key: None,
        replaces: None,
        label: Some(shortened(&file.label)),
    }
}

fn as_fields(pairs: Vec<(String, String)>) -> Vec<Field> {
    pairs
        .into_iter()
        .map(|(name, value)| Field { name, value })
        .collect()
}

/// Every symbol in the download's symbol libraries, in file order.
fn symbol_choices<'a>(download: &'a Download, notes: &mut Vec<String>) -> Vec<SymbolChoice<'a>> {
    let mut choices = Vec::new();
    for file in download.of_kind(DownloadKind::Symbol) {
        match read_symbol_file(&file.bytes) {
            Ok(library) => choices.extend(
                library
                    .names
                    .into_iter()
                    .map(|name| SymbolChoice { file, name }),
            ),
            Err(why) => notes.push(format!("{} could not be read: {why}", file.label)),
        }
    }
    choices
}

/// The bytes of the top-level symbol named `name` in a library file.
fn symbol_entry(library: &[u8], name: &str) -> Option<Vec<u8>> {
    let parsed = SymbolLibrary::parse(library).ok()?;
    let entry = parsed.entries.iter().find(|entry| entry.name == name)?;
    Some(library[entry.span.clone()].to_vec())
}

/// An existing part as the guess compares a new one with it.
fn example((details, revision): &(PartDetails, PartRevision)) -> ExamplePart {
    ExamplePart {
        ipn: details.ipn.clone(),
        category: details.category.clone(),
        // Reading each part's symbol would need a request per part; the
        // guess's other layers do without it.
        reference: String::new(),
        footprints: footprint_choices(revision.field("Footprint").unwrap_or_default())
            .into_iter()
            .map(str::to_owned)
            .collect(),
        description: details.field("Description").unwrap_or_default().to_owned(),
        manufacturers: details
            .manufacturer_parts
            .iter()
            .map(|part| part.manufacturer.clone())
            .collect(),
        mpns: details
            .manufacturer_parts
            .iter()
            .map(|part| part.mpn.clone())
            .collect(),
    }
}

/// Parts whose manufacturer parts have `mpn`, without regard to case.
fn same_mpn(
    mpn: &str,
    manufacturer: &str,
    parts: &[(PartDetails, PartRevision)],
) -> Vec<TeamSameMpn> {
    let mpn = mpn.trim();
    if mpn.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<TeamSameMpn> = parts
        .iter()
        .filter_map(|(details, _)| {
            let matching: Vec<_> = details
                .manufacturer_parts
                .iter()
                .filter(|part| part.mpn.trim().eq_ignore_ascii_case(mpn))
                .collect();
            let first = matching.first()?;
            Some(TeamSameMpn {
                ipn: details.ipn.clone(),
                description: details.field("Description").unwrap_or_default().to_owned(),
                manufacturer: first.manufacturer.clone(),
                mpn: first.mpn.clone(),
                listed: matching.iter().any(|part| {
                    part.manufacturer
                        .trim()
                        .eq_ignore_ascii_case(manufacturer.trim())
                }),
            })
        })
        .collect();
    found.sort_by(|a, b| a.ipn.cmp(&b.ipn));
    found
}

/// Footprints that match the chosen one: the same name in the Catalog or
/// in KiCad's own library for its package group, or the same pads.
fn known_footprints(
    file: &DownloadFile,
    name: &str,
    library: &str,
    context: &Context<'_>,
) -> Vec<TeamKnownFootprint> {
    let mut found: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    let mut add = |reference: String, why: &'static str| {
        let reasons = found.entry(reference).or_default();
        if !reasons.contains(&why) {
            reasons.push(why);
        }
    };
    const SAME_NAME: &str = "the same name";
    const SAME_PADS: &str = "the same pads";
    for (_, revision) in &context.parts {
        for choice in footprint_choices(revision.field("Footprint").unwrap_or_default()) {
            if let Some((lib, item)) = choice.split_once(':') {
                if item == name && lib.starts_with("g-") {
                    add(choice.to_owned(), SAME_NAME);
                }
            }
        }
    }
    let pads = pad_signature(&file.bytes);
    for local in context.footprints {
        if local.name == name {
            add(local.reference.clone(), SAME_NAME);
        }
        if pads.is_some() && local.pads == pads {
            add(local.reference.clone(), SAME_PADS);
        }
    }
    let mut known: Vec<TeamKnownFootprint> = found
        .into_iter()
        .map(|(reference, why)| TeamKnownFootprint {
            reference,
            why: why.join(" and "),
        })
        .collect();
    // KiCad's own library for the package group, such as Resistor_SMD.
    if let (Some(share), Some(kicad_library)) = (&context.kicad_share, library.strip_prefix("g-")) {
        let reference = format!("{kicad_library}:{name}");
        if LibraryResolver::new(None, Some(share.clone()))
            .footprint(&reference)
            .is_ok()
        {
            known.push(TeamKnownFootprint {
                reference,
                why: "KiCad's own library has one with the same name".into(),
            });
        }
    }
    known
}

/// The 3D model of the download the footprint names, by file name without
/// regard to case.
fn linked_model(
    footprint: &catalog::cad_file::FootprintFileInfo,
    models: &[&DownloadFile],
) -> Option<usize> {
    footprint.models.iter().find_map(|model| {
        let named = model
            .reference
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_lowercase();
        models
            .iter()
            .position(|file| file.file_name.to_lowercase() == named)
    })
}

/// The uploads a new part sends: the symbol from its library file, the
/// footprint pointed at its 3D model in the Catalog's folder for the
/// footprint's library, and the 3D model. Files for references that are not
/// the Catalog's own are not sent (a footprint the Catalog or KiCad already
/// has).
pub(crate) fn uploads(request: &TeamNewPartRequest) -> Result<Vec<CadUpload>, String> {
    let decoded = |file: &TeamFile| crate::team::decoded(&file.base64, &file.file_name);
    let source = |file: &TeamFile| file.label.clone().unwrap_or_else(|| file.file_name.clone());
    let of_kind = |kind: FileKind| request.files.iter().find(|file| file.kind == kind);
    let mut uploads = Vec::new();
    if let Some(file) = of_kind(FileKind::Symbol) {
        let name = file
            .symbol
            .as_deref()
            .ok_or_else(|| "Choose which symbol of the file to use.".to_owned())?;
        uploads.push(symbol_upload(
            &decoded(file)?,
            name,
            request.symbol.trim(),
            &source(file),
        )?);
    }
    let footprint = footprint_choices(&request.footprint)
        .first()
        .map(|choice| (*choice).to_owned())
        .unwrap_or_default();
    if let Some(file) = of_kind(FileKind::Footprint) {
        let mut bytes = decoded(file)?;
        if let Some(model) = of_kind(FileKind::Model3d) {
            let library = footprint
                .split_once(':')
                .map(|(library, _)| library)
                .unwrap_or_default();
            let name = model.file_name.clone();
            if !is_plain_name(&name) {
                return Err(format!(
                    "The 3D model's file name {name:?} cannot be used: a name cannot be . or .., start with -, or hold / \\ : or control characters."
                ));
            }
            let key = format!("3d-models/{library}.3dshapes/{name}");
            if !is_catalog_model_key(&key) {
                return Err("Give the footprint one of the Catalog's libraries, such as g-Package_SO, so its 3D model has a place.".to_owned());
            }
            let text = String::from_utf8(bytes).map_err(|_| {
                "The footprint file is not text, so it is not a file KiCad saved.".to_owned()
            })?;
            bytes = link_model(&text, &format!("${{GITPLM_PARTS}}/{key}"), &name).into_bytes();
            uploads.push(model_upload(&decoded(model)?, &key, &source(model))?);
        }
        uploads.push(footprint_upload(&bytes, &footprint, &source(file))?);
    }
    Ok(uploads)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_labels_are_cut_to_what_the_server_keeps() {
        let long = "x".repeat(300);
        assert_eq!(shortened(&long).chars().count(), MAX_FILE_NAME);
        assert_eq!(shortened("a.zip"), "a.zip");
    }
}

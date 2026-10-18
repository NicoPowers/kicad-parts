//! Choosing a symbol or footprint in the Libraries browser for Edit part
//! and Add part (kicad-parts-bc6f). Where the item lives decides what
//! choosing it does:
//!
//! - the Catalog has it: the part is bound to it as the Catalog has it;
//! - one of our libraries' items the Catalog lacks: it is added from the
//!   folder when saving, as committed (kicad-parts-37a7);
//! - KiCad's own: the part refers to it as KiCad's, as imported parts do;
//! - another installed library's: it is copied into one of our libraries,
//!   like a download, with the 3D models its footprint links when they can
//!   be read.
//!
//! Items are read with the browser's safety: no link inside a library is
//! followed and files over a size limit are refused. The file an installed
//! library's row names, and a 3D model it links, may be a link: the person
//! chose it, and KiCad follows it.

use crate::CatalogBrowser;
use crate::libraries::browse::symdir_bytes;
use crate::libraries::query::{self, SymbolOrFootprint};
use crate::libraries::read::{
    Links, MAX_FOOTPRINT_FILE_BYTES, MAX_SYMBOL_FILE_BYTES, read_limited,
};
use crate::libraries::sources::{Format, Source};
use crate::model3d::{NotShown, checked_model, openable_path, starts_as_model};
use crate::team::TeamCatalog;
use catalog::CadItemKind;
use catalog::cad_file::{is_model_file_name, is_plain_name, read_footprint_file, symbol_upload};
use catalog::download::model_links;
use catalog::library_check::{ModelLink, judge_model_link};
use desktop_contract::{
    FileKind, InstalledFile, LibraryGroup, LibraryKey, OurLibraries, PickUse, PreviewKind,
    TeamFile, TeamLibraryChoice, TeamPick, TeamPickRequest,
};
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup, symbol_properties};
use std::path::PathBuf;

/// The largest 3D model copied with a footprint: what one save can send of
/// a file that is not text.
const MAX_MODEL_BYTES: u64 = 24 * 1024 * 1024;

/// How long a choice waits for its library to be read.
const READ_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Where a part made from KiCad's own symbols and footprints came from, as
/// its history says it.
pub(crate) const FROM_KICAD: &str = "KiCad's libraries";

/// Where a part made from another installed library came from, such as
/// `Installed library MyParts`.
pub(crate) fn from_installed(nickname: &str) -> String {
    format!("Installed library {nickname}")
}

/// One chosen item, read: a symbol library file holding just the symbol
/// (after the symbols it is drawn from), or the footprint file.
pub(crate) struct Item {
    pub reference: String,
    pub name: String,
    pub bytes: Vec<u8>,
    /// The file it was read from.
    pub path: PathBuf,
    /// The same file inside its library, such as `C_0603.kicad_mod`.
    pub inside: String,
}

impl Item {
    /// The last part of the file's name, such as `Mine.kicad_sym`.
    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn cad_kind(kind: PreviewKind) -> CadItemKind {
    match kind {
        PreviewKind::Symbol => CadItemKind::Symbol,
        PreviewKind::Footprint => CadItemKind::Footprint,
    }
}

/// A symbol library file holding `entries` (each a top-level `(symbol ...)`)
/// with the `(version ...)` of the file they came from, so it is drawn and
/// saved the same.
fn symbol_library_file(entries: &[&[u8]], version: Option<&str>) -> Vec<u8> {
    let mut text = b"(kicad_symbol_lib\n".to_vec();
    if let Some(version) = version {
        text.extend_from_slice(format!("\t(version {version})\n").as_bytes());
    }
    text.extend_from_slice(b"\t(generator \"kicad_parts\")\n");
    for entry in entries {
        text.push(b'\t');
        text.extend_from_slice(entry);
        text.push(b'\n');
    }
    text.extend_from_slice(b")\n");
    text
}

/// `label` cut to what the team server keeps as a file's source.
fn shortened(label: &str) -> String {
    const LONGEST: usize = 255;
    if label.chars().count() <= LONGEST {
        return label.to_owned();
    }
    label.chars().take(LONGEST - 1).chain(['…']).collect()
}

fn encoded(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl CatalogBrowser {
    /// Our libraries' nicknames, by role.
    pub fn our_libraries(&self) -> OurLibraries {
        crate::libraries::sources::our_library_names(&self.config.catalog_root)
    }

    /// The library `key` of `kind`, as the browser lists it, once read.
    fn listed(&self, kind: PreviewKind, key: &LibraryKey) -> Result<Source, String> {
        self.libraries
            .wait_for(|| self.library_places(), kind, key, READ_WAIT);
        self.libraries
            .snapshot(kind)
            .into_iter()
            .find(|library| library.source.key == *key)
            .map(|library| library.source)
            .ok_or_else(|| {
                format!(
                    "The library {} is no longer listed. Open the Libraries browser again.",
                    key.nickname
                )
            })
    }

    /// Reads the chosen item from its library.
    pub(crate) fn item(&self, request: &TeamPickRequest) -> Result<Item, String> {
        let reference = format!("{}:{}", request.library.nickname, request.name);
        let source = self.listed(request.kind, &request.library)?;
        let libraries = self.libraries.snapshot(request.kind);
        let found = query::find_item(&libraries, &request.library, &request.name)
            .ok_or_else(|| format!("{reference} is not in its library any more."))?;
        // The file a row names may be a link the person chose; links inside
        // a library are never followed, and none at all in ours.
        let row_links = if source.strict_links {
            Links::Refuse { strict: true }
        } else {
            Links::Follow
        };
        let inside_links = Links::Refuse {
            strict: source.strict_links,
        };
        let name = request.name.clone();
        match (found, source.format) {
            (SymbolOrFootprint::Symbol(_), Format::SymbolFile) => {
                let file = read_limited(&source.path, MAX_SYMBOL_FILE_BYTES, row_links)?;
                let parsed = SymbolLibrary::parse(&file).map_err(|error| {
                    format!(
                        "{} is not a KiCad symbol library: {error}.",
                        source.path.display()
                    )
                })?;
                let entry = match parsed.find(&name) {
                    SymbolLookup::Found(entry) => entry,
                    SymbolLookup::Missing => {
                        return Err(format!("{reference} is not in its library any more."));
                    }
                    SymbolLookup::Ambiguous => {
                        return Err(format!(
                            "{} has more than one symbol named {name}.",
                            source.path.display()
                        ));
                    }
                };
                // The symbols it is drawn from come first, so it can be
                // drawn and counted.
                let mut entries = vec![&file[entry.span.clone()]];
                let mut parent = entry.extends.clone();
                for _ in 0..8 {
                    let Some(wanted) = parent.take() else { break };
                    if let SymbolLookup::Found(found) = parsed.find(&wanted) {
                        entries.insert(0, &file[found.span.clone()]);
                        parent = found.extends.clone();
                    }
                }
                Ok(Item {
                    bytes: symbol_library_file(&entries, parsed.version.as_deref()),
                    inside: source
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    reference,
                    name,
                    path: source.path.clone(),
                })
            }
            (SymbolOrFootprint::Symbol(symbol), Format::SymbolFolder) => {
                let file_of =
                    |wanted: &str| match query::find_item(&libraries, &request.library, wanted) {
                        Some(SymbolOrFootprint::Symbol(parent)) => parent.file.clone(),
                        _ => None,
                    };
                let (bytes, version) =
                    symdir_bytes(&source, symbol.file.as_deref(), &name, &file_of)?;
                let inside = symbol.file.clone().unwrap_or_default();
                Ok(Item {
                    bytes: symbol_library_file(&[&bytes], version.as_deref()),
                    inside: inside.clone(),
                    path: source.path.join(inside),
                    reference,
                    name,
                })
            }
            (SymbolOrFootprint::Footprint(_), Format::Footprints) => {
                let path = source.path.join(format!("{name}.kicad_mod"));
                let bytes = read_limited(&path, MAX_FOOTPRINT_FILE_BYTES, inside_links)?;
                Ok(Item {
                    bytes,
                    inside: format!("{name}.kicad_mod"),
                    path,
                    reference,
                    name,
                })
            }
            _ => Err(format!("{reference} is not a {:?} as asked.", request.kind).to_lowercase()),
        }
    }

    /// What choosing `request` in the Libraries browser does for a part.
    pub fn pick(&self, request: &TeamPickRequest, team: &TeamCatalog) -> Result<TeamPick, String> {
        let item = self.item(request)?;
        let mut pick = TeamPick {
            kind: request.kind,
            library: request.library.clone(),
            name: request.name.clone(),
            reference: item.reference.clone(),
            use_as: PickUse::Kicad,
            file: None,
            models: Vec::new(),
            model_problems: Vec::new(),
            problems: Vec::new(),
            notes: Vec::new(),
        };
        match request.library.group {
            LibraryGroup::Ours => {
                if team.has_cad_item(cad_kind(request.kind), &item.reference)? {
                    pick.use_as = PickUse::Catalog;
                    pick.notes.push(
                        "The Catalog has it, so the part uses it as the Catalog has it.".into(),
                    );
                } else {
                    pick.use_as = PickUse::Ours;
                    let choice = match request.kind {
                        PreviewKind::Symbol => TeamLibraryChoice {
                            symbol: Some(item.reference.clone()),
                            footprint: None,
                        },
                        PreviewKind::Footprint => TeamLibraryChoice {
                            symbol: None,
                            footprint: Some(item.reference.clone()),
                        },
                    };
                    let plan = self.library_plan(&choice, team)?;
                    let (problems, findings) = plan.said();
                    pick.problems = problems;
                    pick.notes.push(
                        "From our libraries. The Catalog does not have it yet, so it is added when you save, as committed."
                            .into(),
                    );
                    if let Some(commit) = plan.commit() {
                        pick.notes.push(format!(
                            "{} is read from commit {}.",
                            item.reference,
                            &commit[..12.min(commit.len())]
                        ));
                    }
                    pick.notes.extend(plan.model_notes());
                    pick.notes.extend(findings);
                }
            }
            LibraryGroup::Kicad => {
                pick.notes.push(
                    "KiCad's own: the part refers to it, as imported parts do. Every engineer has KiCad."
                        .into(),
                );
            }
            LibraryGroup::Installed => {
                pick.use_as = PickUse::Copy;
                pick.notes.push(format!(
                    "From the installed library {}, which teammates may not have. It is copied into one of our libraries when you save.",
                    request.library.nickname
                ));
                self.copy(&item, request, &mut pick);
            }
        }
        Ok(pick)
    }

    /// Makes the copy of an installed library's item: its file, and for a
    /// footprint the 3D models its links name.
    fn copy(&self, item: &Item, request: &TeamPickRequest, pick: &mut TeamPick) {
        let nickname = &request.library.nickname;
        // Where it is in its library, never the person's own folders.
        let installed = |path: &str, link: Option<String>| InstalledFile {
            library: nickname.clone(),
            path: path.to_owned(),
            link,
        };
        let label = |file_name: &str| {
            shortened(&format!(
                "{file_name} from the installed library {nickname}"
            ))
        };
        if !is_plain_name(&item.name) {
            pick.problems.push(format!(
                "Its name {:?} cannot be used: a name cannot be . or .., start with -, or hold / \\ : or control characters.",
                item.name
            ));
        }
        let file_name = item.file_name();
        match request.kind {
            PreviewKind::Symbol => {
                // The checks a saved symbol gets, with a stand-in library.
                if let Err(why) = symbol_upload(
                    &item.bytes,
                    &item.name,
                    &format!("g-copy:{}", item.name),
                    &file_name,
                ) {
                    pick.problems.push(why);
                }
                pick.file = Some(TeamFile {
                    kind: FileKind::Symbol,
                    file_name: file_name.clone(),
                    base64: encoded(&item.bytes),
                    symbol: Some(item.name.clone()),
                    model_key: None,
                    replaces: None,
                    label: Some(label(&file_name)),
                    installed: Some(installed(&item.inside, None)),
                    no_models: false,
                    choice: None,
                });
            }
            PreviewKind::Footprint => {
                if let Err(why) = read_footprint_file(&item.bytes) {
                    pick.problems.push(why);
                }
                pick.file = Some(TeamFile {
                    kind: FileKind::Footprint,
                    file_name: file_name.clone(),
                    base64: encoded(&item.bytes),
                    symbol: None,
                    model_key: None,
                    replaces: None,
                    label: Some(label(&file_name)),
                    installed: Some(installed(&item.inside, None)),
                    no_models: false,
                    choice: None,
                });
                let text = String::from_utf8_lossy(&item.bytes).into_owned();
                for link in model_links(&text) {
                    // Only links outside our library are copied; KiCad's
                    // own and embedded models stay as they are.
                    if judge_model_link("g-copy", &item.name, &link) != ModelLink::Outside {
                        continue;
                    }
                    match self.model_file(&link) {
                        Ok((path, bytes)) => {
                            let model_name = path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            pick.models.push(TeamFile {
                                kind: FileKind::Model3d,
                                file_name: model_name.clone(),
                                base64: encoded(&bytes),
                                symbol: None,
                                model_key: None,
                                replaces: None,
                                label: Some(label(&model_name)),
                                // A model is named by the link as written.
                                installed: Some(installed(&link, Some(link.clone()))),
                                no_models: false,
                                choice: None,
                            });
                        }
                        Err(why) => pick.model_problems.push(format!(
                            "Its 3D model {link} could not be read, so it cannot be copied: {why} Choose No 3D model to copy the footprint without it."
                        )),
                    }
                }
                if !pick.models.is_empty() {
                    pick.notes.push(if pick.models.len() == 1 {
                        "Its 3D model is copied with it.".to_owned()
                    } else {
                        format!("Its {} 3D models are copied with it.", pick.models.len())
                    });
                }
            }
        }
    }

    /// The 3D model file `link` names, through the person's KiCad path
    /// variables. Only KiCad's 3D model files are read, and only a link
    /// that says where the file is: one with no folder and no variable is
    /// not guessed at.
    fn model_file(&self, link: &str) -> Result<(PathBuf, Vec<u8>), String> {
        let variables = self.model_places().variables;
        let path = model_path(link, &variables).map_err(|why| as_clause(&why.words()))?;
        let bytes = read_model(&path)?;
        Ok((path, bytes))
    }

    /// The footprint the symbol `item` names, when a library has it: ours
    /// first, then the person's installed libraries, then KiCad's, as KiCad
    /// finds a nickname.
    pub(crate) fn named_footprint(&self, item: &Item) -> Option<TeamPickRequest> {
        let parsed = SymbolLibrary::parse(&item.bytes).ok()?;
        let SymbolLookup::Found(entry) = parsed.find(&item.name) else {
            return None;
        };
        let named = symbol_properties(&item.bytes[entry.span.clone()])
            .into_iter()
            .find(|(name, _)| name == "Footprint")
            .map(|(_, value)| value.trim().to_owned())?;
        let (nickname, name) = named.split_once(':')?;
        [
            LibraryGroup::Ours,
            LibraryGroup::Installed,
            LibraryGroup::Kicad,
        ]
        .into_iter()
        .map(|group| LibraryKey {
            group,
            nickname: nickname.to_owned(),
        })
        .find(|key| {
            self.libraries.wait_for(
                || self.library_places(),
                PreviewKind::Footprint,
                key,
                READ_WAIT,
            );
            let libraries = self.libraries.snapshot(PreviewKind::Footprint);
            query::find_item(&libraries, key, name).is_some()
        })
        .map(|key| TeamPickRequest {
            kind: PreviewKind::Footprint,
            library: key,
            name: name.to_owned(),
        })
    }

    /// The uploads that add our libraries' items `wanted` names and the
    /// Catalog lacks, read from the folder as committed (kicad-parts-37a7),
    /// with the files the history lists. An item the folder does not have
    /// is left for the team server to refuse plainly.
    pub(crate) fn ours_uploads(
        &self,
        wanted: &[(CadItemKind, String)],
        team: &TeamCatalog,
    ) -> Result<(Vec<catalog::pocketbase::CadUpload>, Vec<String>), String> {
        let root = &self.config.catalog_root;
        let mut uploads: Vec<catalog::pocketbase::CadUpload> = Vec::new();
        let mut sources: Vec<String> = Vec::new();
        for (kind, reference) in wanted {
            let Some((library, name)) = reference.split_once(':') else {
                continue;
            };
            if !library.starts_with("g-") || !is_plain_name(library) || !is_plain_name(name) {
                continue;
            }
            let (in_folder, choice) = match kind {
                CadItemKind::Symbol => (
                    catalog::library_check::read::read_library_file(
                        root,
                        &format!("symbols/{library}.kicad_sym"),
                    )
                    .ok()
                    .and_then(|file| SymbolLibrary::parse(&file).ok())
                    .is_some_and(|parsed| matches!(parsed.find(name), SymbolLookup::Found(_))),
                    TeamLibraryChoice {
                        symbol: Some(reference.clone()),
                        footprint: None,
                    },
                ),
                CadItemKind::Footprint => (
                    root.join("footprints")
                        .join(format!("{library}.pretty"))
                        .join(format!("{name}.kicad_mod"))
                        .is_file(),
                    TeamLibraryChoice {
                        symbol: None,
                        footprint: Some(reference.clone()),
                    },
                ),
                CadItemKind::Model3d => continue,
            };
            if !in_folder || team.has_cad_item(*kind, reference)? {
                continue;
            }
            let plan = self.library_plan(&choice, team)?;
            for upload in plan.uploads()? {
                let sent = uploads
                    .iter()
                    .any(|known| known.role == upload.role && known.reference == upload.reference);
                if !sent {
                    uploads.push(upload);
                }
            }
            for source in plan.sources() {
                if !sources.contains(&source) {
                    sources.push(source);
                }
            }
        }
        Ok((uploads, sources))
    }
}

/// Where the 3D model file `link` names may be opened, through the
/// person's KiCad path `variables`, by the 3D view's rules: only KiCad's
/// model files, and never a network path the person didn't set.
fn model_path(
    link: &str,
    variables: &kicad_integration::lib_table::PathVariables,
) -> Result<PathBuf, NotShown> {
    let expanded = variables
        .expand(link)
        .map_err(|missing| NotShown::VariableNotSet(missing.0))?;
    if !is_model_file_name(&expanded) {
        return Err(NotShown::NotModelFile);
    }
    openable_path(link, &expanded, variables)
}

/// The bytes of the 3D model file at `path`, only when it really is one.
/// The path, or a folder on it, may be a link (a library cloned with
/// links, say), so [`checked_model`] checks the real file, as the 3D view
/// does. A key or other file behind a model's name is then never copied
/// to the team server. The error is a clause that follows a colon.
pub(crate) fn read_model(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let real = checked_model(path).map_err(|why| as_clause(&why.words()))?;
    let bytes = read_limited(&real, MAX_MODEL_BYTES, Links::Follow).map_err(|why| {
        if why.contains("larger than") {
            format!("{why} One save can send at most 24 MB.")
        } else {
            why
        }
    })?;
    // The file may have changed since its first bytes were checked, and
    // these bytes are what the team server stores.
    let real_name = real
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !starts_as_model(&real_name, &bytes) {
        return Err(as_clause(&NotShown::NotModel(real).words()));
    }
    Ok(bytes)
}

/// `sentence` with a small first letter, to follow a colon.
fn as_clause(sentence: &str) -> String {
    let mut chars = sentence.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_real_3d_model_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let step = write(
            "x.step",
            b"\xEF\xBB\xBF\n ISO-10303-21;\nEND-ISO-10303-21;\n",
        );
        assert!(read_model(&step).is_ok());
        assert!(read_model(&write("x.WRL", b"#VRML V2.0 utf8\n")).is_ok());
        assert!(read_model(&write("x.stpz", &[0x1f, 0x8b, 8, 0])).is_ok());
        // Something else under a 3D model's name.
        let key = write("id.step", b"-----BEGIN OPENSSH PRIVATE KEY-----\n");
        let refused = read_model(&key).unwrap_err();
        assert!(refused.contains("does not hold a 3D model"), "{refused}");
        assert!(read_model(&write("id.wrz", b"not gzip")).is_err());
        // The same checks and words as the 3D view's, as a clause.
        assert_eq!(
            read_model(&dir.path().join("gone.step")).unwrap_err(),
            "the file is missing."
        );
    }

    /// Copying a model follows the 3D view's network rule (ADR 0013): a
    /// network path is refused before anything is opened, unless the
    /// person set a path variable to that folder.
    #[test]
    fn copying_a_model_never_opens_a_network_path_the_person_did_not_set() {
        use kicad_integration::lib_table::PathVariables;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.step"), "ISO-10303-21;\n").unwrap();
        let mut set = std::collections::BTreeMap::new();
        set.insert("SHARE".to_owned(), r"\\host.invalid\share".to_owned());
        set.insert("MINE".to_owned(), dir.path().to_string_lossy().into_owned());
        let variables = PathVariables::new(set);
        // The links name host.invalid, which never resolves.
        for link in [
            r"\\host.invalid\share\x.step",
            "//host.invalid/share/x.step",
            r"\\?\UNC\host.invalid\share\x.step",
            "/??/UNC/host.invalid/share/x.step",
        ] {
            assert_eq!(
                model_path(link, &variables),
                Err(NotShown::NetworkPath),
                "{link}"
            );
        }
        // The person's own network folder is trusted as they set it.
        assert_eq!(
            model_path("${SHARE}/x.step", &variables),
            Ok(PathBuf::from(r"\\host.invalid\share/x.step"))
        );
        assert_eq!(model_path("x.step", &variables), Err(NotShown::NoFolder));
        assert_eq!(
            model_path("${MINE}/id_rsa", &variables),
            Err(NotShown::NotModelFile)
        );
        // A local model is read.
        let path = model_path("${MINE}/x.step", &variables).unwrap();
        assert!(read_model(&path).is_ok());
    }

    /// A folder junction (Windows makes one without special rights) to a
    /// folder whose `x.step` is a key: read through the junction, its
    /// content still decides.
    #[cfg(windows)]
    #[test]
    fn a_junction_to_a_file_that_is_no_3d_model_is_not_read() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(
            elsewhere.join("x.step"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        )
        .unwrap();
        let models = dir.path().join("models");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&models)
            .arg(&elsewhere)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .is_ok_and(|output| output.status.success());
        if !made {
            eprintln!("junctions cannot be made here; skipped");
            return;
        }
        let refused = read_model(&models.join("x.step")).unwrap_err();
        assert!(refused.contains("does not hold a 3D model"), "{refused}");
    }

    #[test]
    fn a_link_to_a_file_that_is_no_3d_model_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_rsa");
        std::fs::write(&key, "ISO-10303-21; but really a key\n").unwrap();
        let link = dir.path().join("x.step");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&key, &link).is_ok();
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&key, &link).is_ok();
        if !made {
            eprintln!("links cannot be made here; skipped");
            return;
        }
        let refused = read_model(&link).unwrap_err();
        assert!(
            refused.contains("leads to the file id_rsa, which is not a 3D model file"),
            "{refused}"
        );
    }

    #[test]
    fn a_symbol_library_file_keeps_the_version_and_every_entry() {
        let file = symbol_library_file(&[b"(symbol \"A\")", b"(symbol \"B\")"], Some("20241209"));
        let parsed = SymbolLibrary::parse(&file).unwrap();
        assert_eq!(parsed.version.as_deref(), Some("20241209"));
        let names: Vec<&str> = parsed
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["A", "B"]);
    }

    #[test]
    fn history_names_say_where_a_part_came_from() {
        assert_eq!(from_installed("MyParts"), "Installed library MyParts");
        assert_eq!(shortened(&"x".repeat(300)).chars().count(), 255);
    }
}

//! A footprint's 3D view (kicad-parts-c9c1).
//!
//! The footprint goes on a board of its own ([`kicad_integration::board`]),
//! with each 3D model link pointed at the exact file found for it. Then
//! kicad-cli exports the board as GLB, through the preview cache.
//!
//! Each link is looked for in this order; the first place that has it
//! decides:
//!
//! 1. the Catalog's stored model, for a Catalog footprint revision, after
//!    the same checks previews use;
//! 2. a 3D model file chosen with the footprint, in Edit part or Add part;
//! 3. our library: `${GITPLM_PARTS}/3d-models/...` or
//!    `${GITPLM_3DMODELS}/...`, in the catalog folder's `3d-models/`;
//! 4. KiCad's own models: the person's `KICAD10_3DMODEL_DIR`, or the
//!    `3dmodels` folder KiCad installed;
//! 5. any other link, through the person's KiCad path variables: an
//!    installed library.
//!
//! A footprint from a download in Add part draws only the download's own
//! 3D model: its other links point at the maker's computer.
//!
//! Only `.step .stp .stpz .wrl .wrz` files are read, and only once their
//! real path and first bytes show a 3D model, as copies from installed
//! libraries are checked (mx-4b6662). A network path (`\\host\share`) is
//! opened only when it comes from a path variable the person set, because
//! opening one can send their Windows sign-in to that host. kicad-cli is
//! never left to find a model itself, so the file the panel names is the
//! file drawn.

use crate::CatalogBrowser;
use crate::libraries::sources::{is_link, path_variables};
use crate::team::{StoredModelError, TeamCatalog};
use base64::Engine;
use catalog::cad_file::{is_catalog_model_key, is_model_file_name, is_plain_name};
use desktop_contract::{
    Footprint3dRequest, Footprint3dView, GlbReady, ModelLinkView, ModelSource, TeamFile,
};
use kicad_integration::board::{ModelBlock, ModelUse, model_blocks, one_footprint_board};
use kicad_integration::lib_table::{PathVariables, VariableSources};
use kicad_integration::{
    BoardExport, CacheOutcome, Discovery, KicadInstall, PreviewError, SUPPORTED_MAJOR,
    preview::MODEL_COMPLAINTS as CLI_COMPLAINTS, sha256_hex,
};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest GLB sent to the UI. A bigger one would freeze the view.
pub const MAX_GLB_BYTES: u64 = 150 * 1024 * 1024;
/// How much of a file is read to see that it holds a 3D model.
const HEADER_BYTES: u64 = 4096;

/// Where 3D model files are looked for.
#[derive(Clone, Debug)]
pub(crate) struct ModelPlaces {
    /// The catalog folder, which holds our library's `3d-models/`.
    pub catalog_root: PathBuf,
    /// KiCad's own 3D models folder.
    pub kicad_models: Option<PathBuf>,
    /// The person's KiCad path variables.
    pub variables: PathVariables,
}

/// Model files the request brings: the Catalog's stored models and files
/// chosen with the footprint, each checked and copied into the cache.
#[derive(Clone, Debug, Default)]
pub(crate) struct Supplies {
    /// The Catalog's stored models.
    pub stored: Vec<Stored>,
    pub chosen: Vec<Chosen>,
    /// A download's footprint: draw only the chosen files.
    pub only_chosen: bool,
}

/// A 3D model the Catalog stores for the footprint revision.
#[derive(Clone, Debug)]
pub(crate) struct Stored {
    /// The link as the footprint writes it.
    pub link: String,
    /// Where the Catalog keeps it, when its record could be read.
    pub key: Option<String>,
    /// Its checked copy in the cache.
    pub copy: Result<PathBuf, NotShown>,
}

/// A 3D model file chosen with the footprint.
#[derive(Clone, Debug)]
pub(crate) struct Chosen {
    /// The link it was chosen for, when known.
    pub link: Option<String>,
    /// The Catalog place it was chosen for, such as
    /// `3d-models/g-Package_SO.3dshapes/SOIC-8.step`.
    pub key: Option<String>,
    pub file_name: String,
    /// Its checked copy in the cache.
    pub copy: Result<PathBuf, NotShown>,
}

impl Chosen {
    fn matches(&self, link: &str) -> bool {
        self.link.as_deref() == Some(link)
            || (self.key.is_some() && self.key == our_key(link))
            || last_part(link).eq_ignore_ascii_case(&self.file_name)
    }
}

/// Why a 3D model is not drawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NotShown {
    /// KiCad's hide flag is set.
    Hidden,
    /// The link is blank.
    NoLink,
    /// The link does not name a 3D model file.
    NotModelFile,
    /// The link names no folder and no path variable.
    NoFolder,
    /// A path variable the link uses is not set.
    VariableNotSet(String),
    /// The link uses `${KIPRJMOD}`, a KiCad project's folder.
    ProjectFolder,
    /// A network or system path that no path variable of the person's
    /// gave.
    NetworkPath,
    /// A download's link to something other than its own 3D model.
    OutsideDownload,
    Missing(PathBuf),
    /// In our library, the path goes through a link.
    Linked(PathBuf),
    /// The path leads to a file `real` that is not named as a 3D model.
    LeadsElsewhere {
        path: PathBuf,
        real: String,
    },
    /// The file's content is not a 3D model of its kind.
    NotModel(PathBuf),
    Unreadable {
        path: PathBuf,
        why: String,
    },
    /// kicad-cli could not load the file.
    CliUnreadable(PathBuf),
    /// The Catalog's stored model failed its checks, because of `0`.
    HashFailed(String),
    /// The Catalog's copy cannot be had now, because of `0`.
    Unavailable(String),
}

impl NotShown {
    /// Why, in plain words.
    pub(crate) fn words(&self) -> String {
        match self {
            Self::Hidden => {
                "The footprint hides this model with KiCad's hide flag, so it is not drawn, as in KiCad.".into()
            }
            Self::NoLink => "The link names no file.".into(),
            Self::NotModelFile => {
                "It is not a 3D model file: KiCad's are .step, .stp, .stpz, .wrl or .wrz.".into()
            }
            Self::NoFolder => {
                "The link has no folder or path variable, so the app can't tell where the file is."
                    .into()
            }
            Self::VariableNotSet(name) => {
                format!("The path variable ${{{name}}} is not set in KiCad's path settings.")
            }
            Self::ProjectFolder => {
                "${KIPRJMOD} means a KiCad project's folder, which a library footprint doesn't have."
                    .into()
            }
            Self::NetworkPath => {
                "It names a network or system path, which the app opens only through a path variable you set in KiCad's path settings.".into()
            }
            Self::OutsideDownload => {
                "It points outside the download, so the app doesn't look for the file.".into()
            }
            Self::Missing(_) => "The file is missing.".into(),
            Self::Linked(_) => {
                "Its path goes through a link, which our library does not allow.".into()
            }
            Self::LeadsElsewhere { real, .. } => {
                format!("It leads to the file {real}, which is not a 3D model file.")
            }
            Self::NotModel(_) => "The file does not hold a 3D model of its kind.".into(),
            Self::Unreadable { why, .. } => format!("The file could not be read: {why}."),
            Self::CliUnreadable(_) => "kicad-cli could not read this file.".into(),
            Self::HashFailed(why) => {
                format!("The Catalog's stored model failed its hash check because {why}.")
            }
            Self::Unavailable(why) => {
                format!("The Catalog's copy isn't available right now, because {why}.")
            }
        }
    }

    /// The path looked for, when there is one.
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Missing(path)
            | Self::Linked(path)
            | Self::NotModel(path)
            | Self::CliUnreadable(path)
            | Self::LeadsElsewhere { path, .. }
            | Self::Unreadable { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// The footprint file a 3D view is made from.
struct FootprintFor3d {
    bytes: Vec<u8>,
    /// Its name, which names the board file.
    name: String,
    /// What the board's outline goes around.
    board_box: [f64; 4],
    /// Its 3D model links.
    blocks: Vec<ModelBlock>,
}

/// What one link resolves to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Resolved {
    Found(ModelSource, PathBuf),
    /// Embedded in the footprint file: the board keeps the link.
    Embedded,
    NotShown(NotShown),
}

/// Finds the file for one 3D model `link`, in the order the module
/// describes.
pub(crate) fn resolve(
    link: &str,
    hidden: bool,
    places: &ModelPlaces,
    supplies: &Supplies,
) -> Resolved {
    let not_shown = Resolved::NotShown;
    if hidden {
        return not_shown(NotShown::Hidden);
    }
    if link.starts_with("kicad-embed://") {
        return Resolved::Embedded;
    }
    if link.trim().is_empty() {
        return not_shown(NotShown::NoLink);
    }
    let found = |source: ModelSource, result: &Result<PathBuf, NotShown>| match result {
        Ok(path) => Resolved::Found(source, path.clone()),
        Err(why) => Resolved::NotShown(why.clone()),
    };
    if let Some(stored) = supplies.stored.iter().find(|stored| stored.link == link) {
        return found(ModelSource::Catalog, &stored.copy);
    }
    if let Some(chosen) = supplies.chosen.iter().find(|chosen| chosen.matches(link)) {
        return found(ModelSource::Chosen, &chosen.copy);
    }
    if supplies.only_chosen {
        return not_shown(NotShown::OutsideDownload);
    }
    if !is_model_file_name(link) {
        return not_shown(NotShown::NotModelFile);
    }
    if let Some(key) = our_key(link) {
        let path = places.catalog_root.join(&key);
        // Our library allows no link anywhere below the catalog folder, as
        // the library check reads it.
        let mut below = places.catalog_root.clone();
        for part in key.split('/') {
            below.push(part);
            if is_link(&below, true) {
                return not_shown(NotShown::Linked(path));
            }
        }
        return found(ModelSource::Ours, &checked_model(&path));
    }
    // KiCad 5 called KiCad's own folder KISYS3DMOD; KiCad still reads it so,
    // unless the person set it to something else.
    let old_name = places.variables.get("KISYS3DMOD").is_none();
    if let Some(rest) = kicad_model(link, old_name) {
        let Some(folder) = &places.kicad_models else {
            return not_shown(NotShown::VariableNotSet(format!(
                "KICAD{SUPPORTED_MAJOR}_3DMODEL_DIR"
            )));
        };
        let mut path = folder.clone();
        path.extend(rest.split(['/', '\\']));
        return found(ModelSource::Kicad, &checked_model(&path));
    }
    let project = ["${KIPRJMOD}", "$(KIPRJMOD)"];
    if project.iter().any(|name| link.contains(name)) && places.variables.get("KIPRJMOD").is_none()
    {
        return not_shown(NotShown::ProjectFolder);
    }
    let expanded = match places.variables.expand(link) {
        Ok(expanded) => expanded,
        Err(missing) => return not_shown(NotShown::VariableNotSet(missing.0)),
    };
    let path = PathBuf::from(&expanded);
    // `/models/x.step` names a folder on the current drive, as KiCad reads it.
    if path.is_relative() && !path.has_root() {
        return not_shown(NotShown::NoFolder);
    }
    // Only an ordinary path on this computer, unless the person set the
    // link's leading path variable to another kind, such as a network
    // folder. Anything else could open a network share.
    if !ordinary_path(&expanded) && !leading_value_is_unusual(link, &places.variables) {
        return not_shown(NotShown::NetworkPath);
    }
    found(ModelSource::Installed, &checked_model(&path))
}

/// Whether `text` is an ordinary path on this computer: a drive path
/// (`C:\` or `C:/`), or one slash and then a folder or file on the current
/// drive, as `/home/...`. Network and device paths, such as
/// `\\host\share`, `//host`, `\\?\UNC\host`, `\??\C:\` or `\\.\pipe`, are
/// not.
///
/// After one slash, only an empty first part (a second slash) or a `?` or
/// `*` can leave the current drive. So odd first parts, as in `/./x.step`
/// or `/C:/x.step`, stay on this computer, and the file is looked for.
fn ordinary_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return matches!(bytes[2], b'/' | b'\\');
    }
    let Some(rest) = text.strip_prefix(['/', '\\']) else {
        return false;
    };
    // A drive's root on its own, `/`, is ordinary too.
    if rest.is_empty() {
        return true;
    }
    let first = rest.split(['/', '\\']).next().unwrap_or_default();
    !first.is_empty() && !first.contains(['?', '*'])
}

/// Whether `link` starts with a path variable whose own value, which the
/// person set, is something other than an ordinary path, such as a network
/// folder. What the rest of the link adds is never trusted that way.
fn leading_value_is_unusual(link: &str, variables: &PathVariables) -> bool {
    let Some(rest) = link.strip_prefix('$') else {
        return false;
    };
    let close = match rest.chars().next() {
        Some('{') => '}',
        Some('(') => ')',
        _ => return false,
    };
    let Some(end) = rest.find(close) else {
        return false;
    };
    let name = &rest[1..end];
    variables
        .expand(&format!("${{{name}}}"))
        .is_ok_and(|value| !value.is_empty() && !ordinary_path(&value))
}

/// The place below the catalog folder a link into our library names, such
/// as `3d-models/g-Package_SO.3dshapes/SOIC-8.step`. Each part must be a
/// plain name, so it can never leave `3d-models/`.
fn our_key(link: &str) -> Option<String> {
    let rest = ["${GITPLM_PARTS}/3d-models", "${GITPLM_3DMODELS}"]
        .iter()
        .find_map(|prefix| link.strip_prefix(prefix))?
        .strip_prefix(['/', '\\'])?;
    let parts: Vec<&str> = rest.split(['/', '\\']).collect();
    parts
        .iter()
        .all(|part| is_plain_name(part))
        .then(|| format!("3d-models/{}", parts.join("/")))
}

/// The path below KiCad's 3D models folder of a `${KICAD<n>_3DMODEL_DIR}`
/// link, or, with `old_name`, a `${KISYS3DMOD}` one. KiCad reads an older
/// version's variable as its own. Each part must be a plain name, so the
/// path never leaves the folder.
fn kicad_model(link: &str, old_name: bool) -> Option<&str> {
    let rest = match link.strip_prefix("${KISYS3DMOD}") {
        Some(rest) if old_name => rest,
        _ => {
            let rest = link.strip_prefix("${KICAD")?;
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            rest[digits..].strip_prefix("_3DMODEL_DIR}")?
        }
    };
    let rest = rest.strip_prefix(['/', '\\'])?;
    rest.split(['/', '\\']).all(is_plain_name).then_some(rest)
}

fn last_part(link: &str) -> &str {
    link.rsplit(['/', '\\']).next().unwrap_or(link)
}

/// The real path of the 3D model file at `path`, once its real name and
/// first bytes show a 3D model. A link on the way may lead elsewhere, so
/// the real file is what is checked and drawn.
pub(crate) fn checked_model(path: &Path) -> Result<PathBuf, NotShown> {
    let unreadable = |why: std::io::Error| NotShown::Unreadable {
        path: path.to_path_buf(),
        why: why.to_string(),
    };
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(NotShown::Missing(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound || names_nothing(&error) => {
            return Err(NotShown::Missing(path.to_path_buf()));
        }
        Err(error) => return Err(unreadable(error)),
    }
    let real = plain_path(fs::canonicalize(path).map_err(unreadable)?);
    let real_name = real
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !is_model_file_name(&real_name) {
        return Err(NotShown::LeadsElsewhere {
            path: path.to_path_buf(),
            real: real_name,
        });
    }
    let mut header = Vec::new();
    fs::File::open(&real)
        .and_then(|file| file.take(HEADER_BYTES).read_to_end(&mut header))
        .map_err(unreadable)?;
    if !starts_as_model(&real_name, &header) {
        return Err(NotShown::NotModel(path.to_path_buf()));
    }
    Ok(real)
}

/// Whether `error` says the path is not a valid name, as `\C:\x.step` is
/// on Windows. No file can have such a name, so the file is missing.
/// (Rust 1.85 has no stable `ErrorKind` for it.)
fn names_nothing(error: &std::io::Error) -> bool {
    /// Windows' ERROR_INVALID_NAME.
    const INVALID_NAME: i32 = 123;
    cfg!(windows) && error.raw_os_error() == Some(INVALID_NAME)
}

/// Whether `bytes` start as the 3D model their file `name` says: STEP with
/// `ISO-10303-21;`, VRML with `#VRML` (each after an optional BOM and
/// white space), and the gzipped .stpz and .wrz with gzip's mark.
pub(crate) fn starts_as_model(name: &str, bytes: &[u8]) -> bool {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let text = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let start = text
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(text.len());
    let text = &text[start..];
    match extension.as_str() {
        "step" | "stp" => text.starts_with(b"ISO-10303-21;"),
        "wrl" => text.starts_with(b"#VRML"),
        "stpz" | "wrz" => bytes.starts_with(&[0x1f, 0x8b]),
        _ => false,
    }
}

/// `path` without the `\\?\` that `canonicalize` adds on Windows, which
/// other programs may not read (mx-98ef75).
fn plain_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

/// Whether kicad-cli's `messages` say it could not load the file at
/// `path`. kicad-cli names the file as the board does.
fn cli_could_not_read(messages: &str, path: &Path) -> bool {
    let normal = |text: &str| text.replace('/', "\\").to_lowercase();
    let wanted = normal(&path.to_string_lossy());
    messages.lines().any(|line| {
        CLI_COMPLAINTS
            .iter()
            .any(|complaint| line.contains(complaint))
            && normal(line).contains(&wanted)
    })
}

impl CatalogBrowser {
    /// The footprint's 3D view: its 3D model links, where each file was
    /// found or why it is not drawn, and the GLB kicad-cli made.
    pub fn footprint_3d(
        &self,
        request: &Footprint3dRequest,
        team: &TeamCatalog,
    ) -> Footprint3dView {
        let mut view = Footprint3dView::default();
        if let Err(problem) = self.make_footprint_3d(request, team, &mut view) {
            view.problem = Some(problem);
        }
        view
    }

    /// The steps of [`Self::footprint_3d`], in order: find kicad-cli, read
    /// the footprint, gather the model files the request brings, find each
    /// link's file, build the board and export it. An error is a problem
    /// that stops the view before kicad-cli runs.
    fn make_footprint_3d(
        &self,
        request: &Footprint3dRequest,
        team: &TeamCatalog,
        view: &mut Footprint3dView,
    ) -> Result<(), String> {
        let install = self.kicad_for_3d()?;
        let mut footprint = self.footprint_for_3d(&request.key)?;
        let supplies = self.supplies(&install, request, team, &mut footprint, &mut view.notes);
        let places = self.model_places();
        let mut uses = Vec::new();
        for block in &footprint.blocks {
            let resolved = resolve(&block.link, block.hidden, &places, &supplies);
            let (model, shown) = link_view(block, &resolved, &supplies);
            view.models.push(shown);
            uses.push((block.clone(), model));
        }
        let board = one_footprint_board(&footprint.bytes, &uses, footprint.board_box)
            .map_err(|error| format!("The footprint file cannot be read: {error}."))?;
        self.export_board(&install, &footprint.name, &board, request, &mut uses, view);
        if let Some(note) = board_only_note(&footprint.blocks, &uses) {
            view.notes.insert(0, note.into());
        }
        Ok(())
    }

    /// The KiCad install whose kicad-cli makes 3D views.
    fn kicad_for_3d(&self) -> Result<KicadInstall, String> {
        let kicad = self.kicad.read().unwrap_or_else(|e| e.into_inner());
        match &kicad.discovery {
            Discovery::Ready(install) => Ok(install.clone()),
            _ => Err("3D views are made by KiCad 10's kicad-cli, which is not available.".into()),
        }
    }

    /// The footprint file the close-up shows, from the preview cache.
    fn footprint_for_3d(&self, key: &str) -> Result<FootprintFor3d, String> {
        let bytes = self.previews.footprint_source(key).ok_or_else(|| {
            "The footprint file is no longer in the app's cache. Close the close-up and open it again."
                .to_owned()
        })?;
        let drawing = crate::drawing::footprint_drawing(&bytes)
            .ok_or_else(|| "The footprint file cannot be read.".to_owned())?;
        let name = catalog::cad_file::read_footprint_file(&bytes)
            .map(|footprint| footprint.name)
            .unwrap_or_else(|_| "footprint".into());
        let blocks = model_blocks(&bytes)
            .map_err(|error| format!("The footprint file cannot be read: {error}."))?;
        Ok(FootprintFor3d {
            board_box: crate::drawing::board_box(&drawing),
            bytes,
            name,
            blocks,
        })
    }

    /// The model files the request brings: a download's own model, files
    /// chosen with the footprint, and the Catalog's stored models. A
    /// download's footprint with no model link gets one, as saving does.
    fn supplies(
        &self,
        install: &KicadInstall,
        request: &Footprint3dRequest,
        team: &TeamCatalog,
        footprint: &mut FootprintFor3d,
        notes: &mut Vec<String>,
    ) -> Supplies {
        let mut supplies = Supplies {
            only_chosen: request.from_download,
            ..Supplies::default()
        };
        if let Some(file) = &request.download_model {
            // As saving links it: the link named like the file, or the
            // first one, or a new link when there is none.
            let chosen = self.chosen(install, file, None);
            let link = footprint
                .blocks
                .iter()
                .find(|block| last_part(&block.link).eq_ignore_ascii_case(&file.file_name))
                .or(footprint.blocks.first())
                .map(|block| block.link.clone());
            if link.is_none() {
                let text = String::from_utf8_lossy(&footprint.bytes).into_owned();
                footprint.bytes =
                    catalog::download::link_model(&text, &file.file_name, &file.file_name)
                        .into_bytes();
                footprint.blocks = model_blocks(&footprint.bytes).unwrap_or_default();
                notes.push(format!(
                    "The footprint has no 3D model link. Saving links it to the download's 3D model {}.",
                    file.file_name
                ));
            }
            supplies.chosen.push(Chosen {
                link: Some(link.unwrap_or_else(|| file.file_name.clone())),
                ..chosen
            });
        }
        for file in &request.files {
            let link = file
                .installed
                .as_ref()
                .and_then(|installed| installed.link.clone());
            supplies.chosen.push(self.chosen(install, file, link));
        }
        if let Some(stored) = &request.stored {
            match team.footprint_models(stored) {
                Ok(models) => {
                    for (link, model) in models {
                        supplies
                            .stored
                            .push(self.stored_copy(install, team, link, model));
                    }
                }
                Err(error) => notes.push(format!(
                    "The Catalog's stored 3D models could not be checked ({}), so the models below come from your libraries.",
                    team_words(&error)
                )),
            }
        }
        supplies
    }

    /// Exports the board as GLB and puts it, or why there is none, in
    /// `view`. A model kicad-cli could not load is then not drawn.
    fn export_board(
        &self,
        install: &KicadInstall,
        name: &str,
        board: &[u8],
        request: &Footprint3dRequest,
        uses: &mut [(ModelBlock, ModelUse)],
        view: &mut Footprint3dView,
    ) {
        let files: Vec<PathBuf> = uses
            .iter()
            .filter_map(|(_, model)| match model {
                ModelUse::File(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        let export = BoardExport {
            name,
            board,
            models: &files,
        };
        let glb = match self.previews.board_glb(install, &export, request.selection) {
            Ok(glb) => glb,
            Err(PreviewError::Replaced) => {
                view.problem = Some(
                    "A newer request replaced this 3D view. Turn 3D on again to make it.".into(),
                );
                return;
            }
            Err(PreviewError::ExportFailed { cli_message, .. }) => {
                view.problem = Some("kicad-cli could not make the 3D view.".into());
                view.cli_message = Some(cli_message);
                return;
            }
            Err(error) => {
                view.problem = Some(format!("The 3D view could not be made: {error}"));
                return;
            }
        };
        for ((_, model), shown) in uses.iter_mut().zip(&mut view.models) {
            let ModelUse::File(path) = model else {
                continue;
            };
            if cli_could_not_read(&glb.messages, path) {
                let why = NotShown::CliUnreadable(path.clone());
                shown.source = None;
                shown.catalog_key = None;
                shown.problem = Some(why.words());
                *model = ModelUse::Leave;
            }
        }
        let unadded: Vec<&str> = glb
            .messages
            .lines()
            .map(str::trim)
            .filter(|line| CLI_COMPLAINTS.iter().any(|known| line.contains(known)))
            .collect();
        if !unadded.is_empty() {
            view.notes.push(format!(
                "kicad-cli could not draw every 3D model: {}",
                unadded.join(" ")
            ));
        }
        if glb.size > MAX_GLB_BYTES {
            view.problem = Some(format!(
                "The 3D view is {} MB, more than the {} MB the app can show. The 2D view still works.",
                glb.size.div_ceil(1024 * 1024),
                MAX_GLB_BYTES / (1024 * 1024)
            ));
        } else {
            view.glb = Some(GlbReady {
                key: glb.key,
                size: glb.size,
                from_cache: glb.cache == CacheOutcome::Hit,
            });
        }
    }

    /// The GLB of a 3D view [`Self::footprint_3d`] made, by its key; `None`
    /// for a key that names none, or a GLB over [`MAX_GLB_BYTES`].
    pub fn footprint_glb(&self, key: &str) -> Option<Vec<u8>> {
        self.previews
            .glb_bytes(key)
            .filter(|bytes| bytes.len() as u64 <= MAX_GLB_BYTES)
    }

    /// A stored model's checked copy in the cache, or why there is none.
    fn stored_copy(
        &self,
        install: &KicadInstall,
        team: &TeamCatalog,
        link: String,
        model: Result<crate::team::StoredModel, StoredModelError>,
    ) -> Stored {
        let model = match model {
            Ok(model) => model,
            Err(StoredModelError::Check(why)) => {
                return Stored {
                    link,
                    key: None,
                    copy: Err(NotShown::HashFailed(why)),
                };
            }
            Err(StoredModelError::Unavailable(why)) => {
                return Stored {
                    link,
                    key: None,
                    copy: Err(NotShown::Unavailable(why)),
                };
            }
        };
        let key = Some(model.key.clone());
        // Its type first, so no other file is ever fetched or written.
        if !is_model_file_name(&model.key) {
            return Stored {
                link,
                key,
                copy: Err(NotShown::NotModelFile),
            };
        }
        let mut fetch_failed = None;
        let copied =
            self.previews
                .model_copy(install, &model.blob_sha256, last_part(&model.key), || {
                    team.stored_model_bytes(&model).inspect_err(|why| {
                        fetch_failed = Some(why.clone());
                    })
                });
        let copy = match (copied, fetch_failed) {
            (Ok(path), _) => checked_model(&path),
            (Err(_), Some(why)) => Err(NotShown::Unavailable(why)),
            (Err(PreviewError::StoredBytes(why)), None) => Err(NotShown::HashFailed(why)),
            (Err(other), None) => Err(NotShown::Unavailable(other.to_string())),
        };
        Stored { link, key, copy }
    }

    /// A 3D model file chosen with the footprint, checked and copied into
    /// the cache.
    fn chosen(&self, install: &KicadInstall, file: &TeamFile, link: Option<String>) -> Chosen {
        let path = PathBuf::from(&file.file_name);
        let copy = (|| {
            if !is_model_file_name(&file.file_name) {
                return Err(NotShown::NotModelFile);
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(file.base64.as_bytes())
                .map_err(|_| NotShown::Unreadable {
                    path: path.clone(),
                    why: "it was not sent whole".into(),
                })?;
            if !starts_as_model(&file.file_name, &bytes) {
                return Err(NotShown::NotModel(path.clone()));
            }
            self.previews
                .model_copy(
                    install,
                    &sha256_hex(&bytes),
                    last_part(&file.file_name),
                    || Ok(bytes),
                )
                .map_err(|error| NotShown::Unreadable {
                    path: path.clone(),
                    why: error.to_string(),
                })
        })();
        Chosen {
            link,
            key: file.model_key.clone(),
            file_name: file.file_name.clone(),
            copy,
        }
    }

    /// Where 3D model files are looked for, from the person's KiCad
    /// settings, read only.
    pub(crate) fn model_places(&self) -> ModelPlaces {
        let places = self.library_places();
        // Only what the person set themselves: KiCad's own folder is the
        // fallback.
        let (own, _) = PathVariables::for_kicad(&VariableSources {
            environment: places.environment.clone(),
            config_dir: places.kicad_config.clone(),
            share_dir: None,
            documents_dir: None,
            major: SUPPORTED_MAJOR,
        });
        let kicad_models = own
            .get(&format!("KICAD{SUPPORTED_MAJOR}_3DMODEL_DIR"))
            .and_then(|value| own.expand(value).ok())
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| places.share.as_ref().map(|share| share.join("3dmodels")));
        let variables = match &places.kicad_config {
            Some(config) => path_variables(&places, config).0,
            None => PathVariables::new(places.environment.clone()),
        };
        ModelPlaces {
            catalog_root: places.catalog_root,
            kicad_models,
            variables,
        }
    }
}

/// What the board does with a link, and what the panel says about it.
fn link_view(
    block: &ModelBlock,
    resolved: &Resolved,
    supplies: &Supplies,
) -> (ModelUse, ModelLinkView) {
    let mut shown = ModelLinkView {
        link: block.link.clone(),
        source: None,
        path: None,
        catalog_key: None,
        problem: None,
        note: None,
    };
    let model = match resolved {
        Resolved::Found(source, path) => {
            shown.source = Some(*source);
            shown.path = Some(path.display().to_string());
            if *source == ModelSource::Catalog {
                shown.catalog_key = supplies
                    .stored
                    .iter()
                    .find(|stored| stored.link == block.link)
                    .and_then(|stored| stored.key.clone());
            }
            ModelUse::File(path.clone())
        }
        Resolved::Embedded => {
            shown.source = Some(ModelSource::Embedded);
            ModelUse::Keep
        }
        Resolved::NotShown(why) => {
            shown.path = why.path().map(|path| path.display().to_string());
            shown.problem = Some(why.words());
            ModelUse::Leave
        }
    };
    let from_library = !matches!(
        shown.source,
        Some(ModelSource::Catalog | ModelSource::Chosen)
    ) && !supplies.only_chosen;
    if from_library {
        if let Some(key) = our_key(&block.link).filter(|key| !is_catalog_model_key(key)) {
            // mx-292514: the Catalog keeps 3D models only in a footprint
            // library's own folder.
            shown.note = Some(format!(
                "Edit part would save this link as missing: the Catalog keeps a 3D model only in its footprint library's folder, such as 3d-models/g-Package_SO.3dshapes, not at {key}."
            ));
        } else if block.link.starts_with("${KISYS3DMOD}")
            && shown.source == Some(ModelSource::Kicad)
        {
            shown.note = Some(
                "${KISYS3DMOD} is KiCad 5's name for KiCad's own 3D models folder, so the app looks there."
                    .into(),
            );
        }
    }
    (model, shown)
}

/// The note that only the board is shown, when no 3D model is drawn.
fn board_only_note(blocks: &[ModelBlock], uses: &[(ModelBlock, ModelUse)]) -> Option<&'static str> {
    if blocks.is_empty() {
        return Some(
            "The footprint has no 3D model, so only the board with its pads, silkscreen and mask is shown.",
        );
    }
    let drawn = uses.iter().any(|(_, model)| *model != ModelUse::Leave);
    (!drawn).then_some(
        "No 3D model can be shown, so only the board with its pads, silkscreen and mask is shown.",
    )
}

fn team_words(error: &crate::team::TeamCadError) -> String {
    use crate::team::TeamCadError;
    match error {
        TeamCadError::SignedOut => "you are signed out".into(),
        TeamCadError::NotFound(why) | TeamCadError::Unreachable(why) => why.clone(),
        TeamCadError::Mismatch { why, .. } => {
            format!("the footprint's record failed its check: {why}")
        }
    }
}

#[cfg(test)]
mod tests;

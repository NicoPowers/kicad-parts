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
//! 3. our library, `3d-models/g-*.3dshapes/` in the catalog folder;
//! 4. KiCad's own models: the person's `KICAD10_3DMODEL_DIR`, or the
//!    `3dmodels` folder KiCad installed;
//! 5. any other link, through the person's KiCad path variables: an
//!    installed library.
//!
//! Only `.step .stp .stpz .wrl .wrz` files are read, and only once their
//! real path and first bytes show a 3D model, as copies from installed
//! libraries are checked (mx-4b6662). kicad-cli is never left to find a
//! model itself, so the file the panel names is the file drawn.

use crate::CatalogBrowser;
use crate::libraries::sources::{is_link, path_variables};
use crate::pick::starts_as_model;
use crate::team::TeamCatalog;
use base64::Engine;
use catalog::cad_file::{is_catalog_model_key, is_model_file_name};
use desktop_contract::{
    Footprint3dRequest, Footprint3dView, GlbReady, ModelLinkView, ModelSource, TeamFile,
};
use kicad_integration::board::{ModelBlock, ModelUse, model_blocks, one_footprint_board};
use kicad_integration::lib_table::{PathVariables, VariableSources};
use kicad_integration::{
    BoardExport, CacheOutcome, Discovery, KicadInstall, PreviewError, SUPPORTED_MAJOR, sha256_hex,
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
    /// The Catalog's stored models, by link as the footprint writes it.
    pub stored: Vec<(String, Result<PathBuf, NotShown>)>,
    pub chosen: Vec<Chosen>,
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
    /// The Catalog's stored model failed its checks, because of `0`.
    HashFailed(String),
    /// The Catalog's stored model could not be fetched, because of `0`.
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
                "It names no folder and no path variable, so where the file is is not known.".into()
            }
            Self::VariableNotSet(name) => {
                format!("The path variable ${{{name}}} is not set in KiCad's path settings.")
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
            Self::HashFailed(why) => {
                format!("The Catalog's stored model failed its hash check because {why}.")
            }
            Self::Unavailable(why) => {
                format!("The Catalog's stored model could not be fetched because {why}.")
            }
        }
    }

    /// The path looked for, when there is one.
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Missing(path)
            | Self::Linked(path)
            | Self::NotModel(path)
            | Self::LeadsElsewhere { path, .. }
            | Self::Unreadable { path, .. } => Some(path),
            _ => None,
        }
    }
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
    if let Some((_, stored)) = supplies.stored.iter().find(|(known, _)| known == link) {
        return found(ModelSource::Catalog, stored);
    }
    if let Some(chosen) = supplies.chosen.iter().find(|chosen| chosen.matches(link)) {
        return found(ModelSource::Chosen, &chosen.copy);
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
    if let Some(rest) = kicad_model(link) {
        let Some(folder) = &places.kicad_models else {
            return not_shown(NotShown::VariableNotSet(format!(
                "KICAD{SUPPORTED_MAJOR}_3DMODEL_DIR"
            )));
        };
        let mut path = folder.clone();
        path.extend(rest.split(['/', '\\']).filter(|part| !part.is_empty()));
        return found(ModelSource::Kicad, &checked_model(&path));
    }
    let expanded = match places.variables.expand(link) {
        Ok(expanded) => expanded,
        Err(missing) => return not_shown(NotShown::VariableNotSet(missing.0)),
    };
    let path = PathBuf::from(expanded);
    if path.is_relative() {
        return not_shown(NotShown::NoFolder);
    }
    found(ModelSource::Installed, &checked_model(&path))
}

/// The Catalog place a link into our library names, such as
/// `3d-models/g-Package_SO.3dshapes/SOIC-8.step`.
fn our_key(link: &str) -> Option<String> {
    let rest = ["${GITPLM_PARTS}/3d-models", "${GITPLM_3DMODELS}"]
        .iter()
        .find_map(|prefix| link.strip_prefix(prefix))?
        .strip_prefix(['/', '\\'])?;
    let key = format!("3d-models/{}", rest.replace('\\', "/"));
    is_catalog_model_key(&key).then_some(key)
}

/// The path below KiCad's 3D models folder of a `${KICAD<n>_3DMODEL_DIR}`
/// link, with no `..` that would climb out of it. KiCad reads an older
/// version's variable as its own.
fn kicad_model(link: &str) -> Option<&str> {
    let rest = link.strip_prefix("${KICAD")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let rest = rest[digits..]
        .strip_prefix("_3DMODEL_DIR}")?
        .strip_prefix(['/', '\\'])?;
    (digits > 0 && !rest.split(['/', '\\']).any(|part| part == "..")).then_some(rest)
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
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

impl CatalogBrowser {
    /// The footprint's 3D view: its 3D model links, where each file was
    /// found or why it is not drawn, and the GLB kicad-cli made.
    pub fn footprint_3d(
        &self,
        request: &Footprint3dRequest,
        team: &TeamCatalog,
    ) -> Footprint3dView {
        let mut view = Footprint3dView::default();
        let install = {
            let kicad = self.kicad.read().unwrap_or_else(|e| e.into_inner());
            match &kicad.discovery {
                Discovery::Ready(install) => install.clone(),
                _ => {
                    view.problem = Some(
                        "3D views are made by KiCad 10's kicad-cli, which is not available.".into(),
                    );
                    return view;
                }
            }
        };
        let Some(mut bytes) = self.previews.footprint_source(&request.key) else {
            view.problem = Some(
                "The footprint file is no longer in the app's cache. Close the close-up and open it again."
                    .into(),
            );
            return view;
        };
        let Some(drawing) = crate::drawing::footprint_drawing(&bytes) else {
            view.problem = Some("The footprint file cannot be read.".into());
            return view;
        };
        let name = catalog::cad_file::read_footprint_file(&bytes)
            .map(|footprint| footprint.name)
            .unwrap_or_else(|_| "footprint".into());
        let mut supplies = Supplies::default();
        let mut blocks = match model_blocks(&bytes) {
            Ok(blocks) => blocks,
            Err(error) => {
                view.problem = Some(format!("The footprint file cannot be read: {error}."));
                return view;
            }
        };
        if let Some(file) = &request.download_model {
            // As saving links it: the link named like the file, or the
            // first one, or a new link when there is none.
            let chosen = self.chosen(&install, file, None);
            let link = blocks
                .iter()
                .find(|block| last_part(&block.link).eq_ignore_ascii_case(&file.file_name))
                .or(blocks.first())
                .map(|block| block.link.clone());
            if link.is_none() {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                bytes = catalog::download::link_model(&text, &file.file_name, &file.file_name)
                    .into_bytes();
                blocks = model_blocks(&bytes).unwrap_or_default();
                view.notes.push(format!(
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
            supplies.chosen.push(self.chosen(&install, file, link));
        }
        if let Some(stored) = &request.stored {
            match team.footprint_models(stored) {
                Ok(models) => {
                    for (link, model) in models {
                        let copy = match model {
                            Ok(model) => {
                                let extension = Path::new(&model.key)
                                    .extension()
                                    .map(|extension| extension.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                self.previews
                                    .model_copy(&install, &model.blob_sha256, &extension, || {
                                        team.stored_model_bytes(&model)
                                    })
                                    .map_err(|error| match error {
                                        PreviewError::StoredBytes(why) if why.contains("match") => {
                                            NotShown::HashFailed(why)
                                        }
                                        other => NotShown::Unavailable(other.to_string()),
                                    })
                                    .and_then(|path| checked_model(&path))
                            }
                            Err(why) => Err(NotShown::HashFailed(why)),
                        };
                        supplies.stored.push((link, copy));
                    }
                }
                Err(error) => view.notes.push(format!(
                    "The Catalog's stored 3D models could not be checked ({}), so the models below come from your libraries.",
                    team_words(&error)
                )),
            }
        }

        let places = self.model_places();
        let mut uses = Vec::new();
        let mut files = Vec::new();
        for block in &blocks {
            let resolved = resolve(&block.link, block.hidden, &places, &supplies);
            let (model, shown) = link_view(block, &resolved);
            view.models.push(shown);
            if let ModelUse::File(path) = &model {
                files.push(path.clone());
            }
            uses.push((block.clone(), model));
        }
        let drawn = uses.iter().any(|(_, model)| *model != ModelUse::Leave);
        if blocks.is_empty() {
            view.notes.push(
                "The footprint has no 3D model, so only the board with its pads, silkscreen and mask is shown."
                    .into(),
            );
        } else if !drawn {
            view.notes.push(
                "No 3D model can be shown, so only the board with its pads, silkscreen and mask is shown."
                    .into(),
            );
        }
        let board = match one_footprint_board(&bytes, &uses, crate::drawing::board_box(&drawing)) {
            Ok(board) => board,
            Err(error) => {
                view.problem = Some(format!("The footprint file cannot be read: {error}."));
                return view;
            }
        };
        let export = BoardExport {
            name: &name,
            board: &board,
            models: &files,
        };
        match self
            .previews
            .board_glb(&install, &export, request.selection)
        {
            Ok(glb) => {
                let unadded: Vec<&str> = glb
                    .messages
                    .lines()
                    .filter(|line| {
                        line.contains("Could not add 3D model")
                            || line.starts_with("File not found")
                    })
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
            Err(PreviewError::Replaced) => {
                view.problem = Some(
                    "A newer request replaced this 3D view. Turn 3D on again to make it.".into(),
                );
            }
            Err(PreviewError::ExportFailed { cli_message, .. }) => {
                view.problem = Some("kicad-cli could not make the 3D view.".into());
                view.cli_message = Some(cli_message);
            }
            Err(error) => {
                view.problem = Some(format!("The 3D view could not be made: {error}"));
            }
        }
        view
    }

    /// The GLB of a 3D view [`Self::footprint_3d`] made, by its key; `None`
    /// for a key that names none, or a GLB over [`MAX_GLB_BYTES`].
    pub fn footprint_glb(&self, key: &str) -> Option<Vec<u8>> {
        self.previews
            .glb_bytes(key)
            .filter(|bytes| bytes.len() as u64 <= MAX_GLB_BYTES)
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
            let extension = path
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.previews
                .model_copy(install, &sha256_hex(&bytes), &extension, || Ok(bytes))
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
fn link_view(block: &ModelBlock, resolved: &Resolved) -> (ModelUse, ModelLinkView) {
    let shown = |source, path: Option<&Path>, problem: Option<String>| ModelLinkView {
        link: block.link.clone(),
        source,
        path: path.map(|path| path.display().to_string()),
        problem,
    };
    match resolved {
        Resolved::Found(source, path) => (
            ModelUse::File(path.clone()),
            shown(Some(*source), Some(path), None),
        ),
        Resolved::Embedded => (
            ModelUse::Keep,
            shown(Some(ModelSource::Embedded), None, None),
        ),
        Resolved::NotShown(why) => (ModelUse::Leave, shown(None, why.path(), Some(why.words()))),
    }
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

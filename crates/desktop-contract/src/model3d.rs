//! A footprint's 3D view (kicad-parts-c9c1): which 3D model files it
//! draws, why a model is not drawn, and the GLB kicad-cli made.
//!
//! The UI asks for the 3D view with [`Footprint3dRequest`] and gets a
//! [`Footprint3dView`]. When it is ready, the UI reads the GLB itself as
//! raw bytes with the `footprint_glb` command and the [`GlbReady::key`].

use crate::TeamFile;
use serde::{Deserialize, Serialize};

/// A footprint revision the Catalog stores, with the exact 3D model
/// revisions it uses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredFootprint {
    /// The server address of the team that stores it, which its 3D view
    /// asks for the models.
    #[serde(default)]
    pub team: String,
    /// The CAD item's ID.
    pub cad_item: String,
    pub number: u32,
    /// The SHA-256 of the revision's record, which is checked.
    pub snapshot_sha256: String,
}

/// Make the 3D view of a footprint the UI is showing.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Footprint3dRequest {
    /// The footprint's drawing key (see `PreviewImage::drawing_key`).
    pub key: String,
    /// When the footprint is a Catalog revision: draw the 3D models it
    /// stores for that revision.
    #[serde(default)]
    pub stored: Option<StoredFootprint>,
    /// 3D model files chosen with the footprint in Edit part: each is used
    /// for the model link it was chosen for.
    #[serde(default)]
    pub files: Vec<TeamFile>,
    /// Add part: the footprint is from a download, so only the download's
    /// own 3D model is drawn. Its other links point at the maker's
    /// computer, so the app never looks for them.
    #[serde(default)]
    pub from_download: bool,
    /// Add part: the download's 3D model, used as saving links it.
    #[serde(default)]
    pub download_model: Option<TeamFile>,
    /// See `PreviewRequest::selection`.
    #[serde(default)]
    pub selection: Option<u64>,
}

/// Where a 3D model file was found.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// The Catalog's stored model for this footprint revision.
    Catalog,
    /// Our library: `3d-models/` in the catalog folder.
    Ours,
    /// KiCad's own 3D models.
    Kicad,
    /// Another library, through the person's KiCad path variables.
    Installed,
    /// A file chosen with the footprint, or the download's 3D model.
    Chosen,
    /// Embedded in the footprint file.
    Embedded,
}

/// One 3D model link of the footprint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelLinkView {
    /// The link as the footprint writes it.
    pub link: String,
    /// Where its file was found, when it is drawn.
    pub source: Option<ModelSource>,
    /// The full path of the file drawn, or of the file looked for. For a
    /// Catalog model, its copy in the app's cache.
    pub path: Option<String>,
    /// For a Catalog model: where the Catalog keeps it, such as
    /// `3d-models/g-Package_SO.3dshapes/SOIC-8.step`.
    #[serde(default)]
    pub catalog_key: Option<String>,
    /// Why it is not drawn, in plain words.
    pub problem: Option<String>,
    /// Something else to know about it, in plain words.
    #[serde(default)]
    pub note: Option<String>,
}

/// The GLB of a 3D view, ready to read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GlbReady {
    /// What to ask the `footprint_glb` command for.
    pub key: String,
    pub size: u64,
    /// Served from the cache, without running kicad-cli.
    pub from_cache: bool,
}

/// What the 3D view shows.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Footprint3dView {
    /// Each 3D model link, in file order.
    pub models: Vec<ModelLinkView>,
    /// Things to know, such as the footprint having no 3D model.
    pub notes: Vec<String>,
    /// The GLB, when kicad-cli made it.
    pub glb: Option<GlbReady>,
    /// Why there is no 3D view.
    pub problem: Option<String>,
    /// What kicad-cli printed when the export failed.
    pub cli_message: Option<String>,
}

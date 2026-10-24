//! Everything the application needs from the installed KiCad, with no UI or
//! Tauri dependency:
//!
//! - [`discovery`] finds kicad-cli (standard locations or a user override)
//!   and records its version. Every kicad-cli run keeps its settings in a
//!   [`CliSettingsFolder`] of our own, never in the person's KiCad folders.
//! - [`library_source`] says where one library repo keeps its libraries,
//!   3D models and path variables: a library source profile. Our own
//!   layout is the default profile.
//! - [`resolver`] turns catalog references such as `g-res:R_US` into library
//!   files, through a library source profile first, then KiCad's bundled
//!   libraries.
//! - [`preview`] exports symbol and footprint SVGs with kicad-cli, from
//!   library files or from a CAD item revision's stored bytes, and caches
//!   them by content hash and KiCad version.
//! - [`library_file`] reads saved symbol libraries and footprints: one
//!   symbol's exact bytes, file versions and 3D model references.
//! - [`board`] puts one footprint on a board of its own, with its 3D
//!   models pointed at exact files, for kicad-cli's 3D export.
//! - [`lib_table`] reads the person's KiCad library tables
//!   (`sym-lib-table`, `fp-lib-table`) and expands KiCad's path variables.
//! - [`library_summary`] reads what a library browser lists about every
//!   symbol and footprint in one quick pass, and [`footprint_filter`]
//!   applies KiCad's footprint filters for a symbol.

pub mod board;
pub mod discovery;
pub mod footprint_filter;
pub mod lib_table;
pub mod library_file;
pub mod library_source;
pub mod library_summary;
pub mod preview;
mod process;
pub mod resolver;

pub use discovery::{
    Discovery, DiscoveryOptions, InstallSource, KicadInstall, KicadVersion, SUPPORTED_MAJOR,
    SearchLocations, discover, installed_share_dir,
};
pub use preview::{
    BoardExport, CacheOutcome, Glb, MAX_RUNNING, Preview, PreviewCache, PreviewError, PreviewImage,
    Selection, StoredItem, remove_old_cache, sha256_hex,
};
pub use process::{CliOutput, CliSettingsFolder, RunError, run_kicad_cli};
pub use resolver::{
    LibRef, LibraryOrigin, LibraryResolver, ResolveError, ResolvedFootprint, ResolvedSymbol,
    footprint_choices,
};

//! Everything the application needs from the installed KiCad, with no UI or
//! Tauri dependency:
//!
//! - [`discovery`] finds kicad-cli (standard locations or a user override)
//!   and records its version.
//! - [`resolver`] turns catalog references such as `g-pas:R_US` into library
//!   files, searching project libraries before KiCad's bundled ones.
//! - [`preview`] exports symbol and footprint SVGs with kicad-cli and caches
//!   them by library content hash and KiCad version.
//! - [`library_file`] reads saved symbol libraries and footprints: one
//!   symbol's exact bytes, file versions and 3D model references.

pub mod discovery;
pub mod library_file;
pub mod preview;
mod process;
pub mod resolver;

pub use discovery::{
    Discovery, DiscoveryOptions, InstallSource, KicadInstall, KicadVersion, SUPPORTED_MAJOR,
    SearchLocations, discover,
};
pub use preview::{CacheOutcome, Preview, PreviewCache, PreviewError, PreviewImage};
pub use resolver::{
    LibRef, LibraryOrigin, LibraryResolver, ResolveError, ResolvedFootprint, ResolvedSymbol,
    footprint_choices,
};

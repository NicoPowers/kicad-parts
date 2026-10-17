//! Writing the KiCad libraries (`crates/kicad-libraries`) from a
//! disposable team server, as `catalog-import kicad-libraries` and the
//! full-catalog proof do.

use catalog::pocketbase::CatalogReadback;
use kicad_libraries::{Naming, Outcome, Team};
use std::path::Path;

/// The team ID the libraries of a disposable team server record. Every
/// disposable team server is made from the same CSV catalog, so they count
/// as one team: writing again updates the same folder instead of refusing
/// it as another team's.
pub const DISPOSABLE_TEAM: &str = "disposable-local-team-server";

/// The team of a disposable team server, named by `naming`.
pub fn disposable_team(naming: Naming) -> Team {
    Team {
        id: DISPOSABLE_TEAM.to_owned(),
        server: "a disposable local team server".to_owned(),
        naming,
    }
}

/// Brings `team`'s libraries under `root` to `readback`. The blobs are
/// already in the readback, so nothing is downloaded.
pub fn write(root: &Path, readback: &CatalogReadback, team: &Team) -> Result<Outcome, String> {
    kicad_libraries::update(root, readback, team, &mut |blob| Ok(blob.bytes.clone()))
        .map_err(|error| error.message(root))
}

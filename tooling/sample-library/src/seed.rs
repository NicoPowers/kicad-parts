//! A team server seeded from the sample library.

use crate::{Sample, SamplePart};
use catalog::cad_file::{footprint_upload, model_upload, read_footprint_file, symbol_upload};
use catalog::pocketbase::{CadUpload, CreateCommand, CreateOutcome, submit_create};
use collaboration::PocketBaseClient;
use std::collections::BTreeSet;
use std::path::Path;

/// What [`seed`] saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seeded {
    /// The IPNs, in order.
    pub parts: Vec<String>,
    /// The seq of the last Change saved: one per part, after the one that
    /// set up the starting categories.
    pub last_seq: u64,
}

/// Adds the sample parts in the categories `codes` (every part when empty)
/// to the team server `admin` signs in to, as a superuser: each part with
/// `POST /api/catalog/parts`, as Add part saves one. The server has their
/// categories already, as every new team server does. A symbol, footprint
/// or 3D model of the sample library goes with the first part that uses
/// it.
pub fn seed(admin: &PocketBaseClient, codes: &[&str]) -> Result<Seeded, String> {
    let sample = Sample::load();
    let root = crate::root();
    if let Some(unknown) = codes
        .iter()
        .find(|code| !sample.categories.iter().any(|c| c.code == **code))
    {
        return Err(format!(
            "no part of parts.toml is in the category {unknown}"
        ));
    }
    let mut seeded = Seeded {
        parts: Vec::new(),
        last_seq: 0,
    };
    let mut sent = BTreeSet::new();
    for part in sample.parts_in(codes) {
        let category = sample.category(&part.category);
        let mut command = CreateCommand::new(
            &part.ipn,
            &part.category,
            &format!("Add the sample part {}", part.ipn),
        );
        command.fields = part.detail_fields(category);
        command.manufacturer_parts = part.manufacturer_parts();
        command.cad_fields = part.cad_fields();
        command.uploads = uploads(&root, part, &mut sent)?;
        let reply = submit_create(admin, &command).map_err(|error| error.to_string())?;
        let CreateOutcome::Applied { seq, .. } = reply.outcome else {
            return Err(format!("{} was not added: {}", part.ipn, reply.body));
        };
        seeded.parts.push(part.ipn.clone());
        seeded.last_seq = seq;
    }
    Ok(seeded)
}

/// The files of the sample library `part` uses that no earlier part sent:
/// its symbol and footprints in g- libraries, and the 3D models those
/// footprints link in the library.
fn uploads(
    root: &Path,
    part: &SamplePart,
    sent: &mut BTreeSet<String>,
) -> Result<Vec<CadUpload>, String> {
    let mut uploads = Vec::new();
    if let Some((library, name)) = own_reference(&part.symbol) {
        if sent.insert(format!("symbol {}", part.symbol)) {
            let file = format!("symbols/{library}.kicad_sym");
            let bytes = read(root, &file)?;
            uploads.push(symbol_upload(&bytes, name, &part.symbol, &file)?);
        }
    }
    for footprint in part.footprints() {
        let Some((library, name)) = own_reference(footprint) else {
            continue;
        };
        if !sent.insert(format!("footprint {footprint}")) {
            continue;
        }
        let file = format!("footprints/{library}.pretty/{name}.kicad_mod");
        let bytes = read(root, &file)?;
        for model in read_footprint_file(&bytes)?.models {
            if let Some(key) = model.key {
                if sent.insert(format!("model {key}")) {
                    uploads.push(model_upload(&read(root, &key)?, &key, &key)?);
                }
            }
        }
        uploads.push(footprint_upload(&bytes, footprint, &file)?);
    }
    Ok(uploads)
}

/// The library and name of a reference to one of the sample library's own
/// g- libraries; `None` for KiCad's own libraries and blanks.
fn own_reference(reference: &str) -> Option<(&str, &str)> {
    reference
        .split_once(':')
        .filter(|(library, _)| library.starts_with("g-"))
}

fn read(root: &Path, file: &str) -> Result<Vec<u8>, String> {
    std::fs::read(root.join(file)).map_err(|error| format!("{file}: {error}"))
}

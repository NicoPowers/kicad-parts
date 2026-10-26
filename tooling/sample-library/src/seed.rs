//! A team server seeded from the sample library.

use crate::{Sample, SamplePart};
use catalog::cad_file::{footprint_upload, model_upload, read_footprint_file, symbol_upload};
use catalog::pocketbase::{
    CadUpload, CategoryCommand, CategoryOutcome, CreateCommand, CreateOutcome, submit_category,
    submit_create,
};
use collaboration::PocketBaseClient;
use std::collections::BTreeSet;
use std::path::Path;

/// What [`seed`] saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seeded {
    /// The category codes, in order.
    pub categories: Vec<String>,
    /// The IPNs, in order.
    pub parts: Vec<String>,
    /// The seq of the last Change saved: one per category and one per part.
    pub last_seq: u64,
}

/// Sets up the sample categories `codes` (every category when empty) and
/// their parts on the team server `admin` signs in to, as a superuser:
/// each category with `POST /api/catalog/categories`, then each part with
/// `POST /api/catalog/parts`, as Add part saves one. A symbol, footprint or
/// 3D model of the sample library goes with the first part that uses it.
/// Never uses the CSV import.
pub fn seed(admin: &PocketBaseClient, codes: &[&str]) -> Result<Seeded, String> {
    let sample = Sample::load();
    let root = crate::root();
    if let Some(unknown) = codes
        .iter()
        .find(|code| !sample.categories.iter().any(|c| c.code == **code))
    {
        return Err(format!("parts.toml has no category {unknown}"));
    }
    let mut seeded = Seeded {
        categories: Vec::new(),
        parts: Vec::new(),
        last_seq: 0,
    };
    for category in &sample.categories {
        if !codes.is_empty() && !codes.contains(&category.code.as_str()) {
            continue;
        }
        let command = CategoryCommand::new(
            category.details(),
            &format!("Set up the sample category {}", category.code),
        );
        let reply = submit_category(admin, &command).map_err(|error| error.to_string())?;
        let CategoryOutcome::Applied { seq, .. } = reply.outcome else {
            return Err(format!(
                "the category {} was not set up: {}",
                category.code, reply.body
            ));
        };
        seeded.categories.push(category.code.clone());
        seeded.last_seq = seq;
    }
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

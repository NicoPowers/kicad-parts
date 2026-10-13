//! Today's CSV catalog: the footprints and CSV files this product took
//! over, with each footprint's bytes, frozen in `today.rs` (not read from
//! the CSV catalog fixture, which `csv-catalog-fixture write` rewrites after
//! any footprint edit). Built into the program, so the CSV import and the
//! desktop app agree.
//!
//! A footprint the Catalog does not have yet keeps its old problems as
//! clean-up at its CSV file's first import only while its bytes are still
//! today's and every CSV file using it is one of today's; any other is
//! judged like a new footprint, so a new or renamed CSV file cannot bring a
//! bad 3D model link in.

use super::today::{TODAYS_CSV_FILES, TODAYS_FOOTPRINTS};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// Today's CSV catalog, as the library check compares with it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Today {
    /// Each footprint file (`footprints/...kicad_mod`) and the SHA-256 of
    /// its bytes.
    pub footprints: BTreeMap<String, String>,
    /// The CSV files, such as `database/g-res.csv`.
    pub csv_files: BTreeSet<String>,
}

/// Today's CSV catalog, frozen in the program.
pub fn today() -> &'static Today {
    static TODAY: OnceLock<Today> = OnceLock::new();
    TODAY.get_or_init(|| Today {
        footprints: TODAYS_FOOTPRINTS
            .iter()
            .map(|(path, sha256)| ((*path).to_owned(), (*sha256).to_owned()))
            .collect(),
        csv_files: TODAYS_CSV_FILES
            .iter()
            .map(|path| (*path).to_owned())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen list still matches the CSV catalog fixture today. If the
    /// fixture changes (a footprint edited, a CSV file added), this fails on
    /// purpose: leave the frozen list as it is and update this test's
    /// expectations, since changing the list reopens clean-up for those
    /// footprints.
    #[test]
    fn the_frozen_list_matches_todays_csv_catalog_fixture() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/import/csv-catalog/manifest.json"
        ))
        .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        let footprints: BTreeMap<String, String> = manifest["assets"]["footprints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| {
                (
                    asset["path"].as_str().unwrap().to_owned(),
                    asset["byte_sha256"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let csv_files: BTreeSet<String> = manifest["catalog"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|category| category["path"].as_str().unwrap().to_owned())
            .collect();
        let today = today();
        assert_eq!(today.footprints, footprints);
        assert_eq!(today.csv_files, csv_files);
        assert_eq!((today.footprints.len(), today.csv_files.len()), (76, 21));
    }
}

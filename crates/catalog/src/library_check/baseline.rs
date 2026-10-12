//! Today's footprints: the bytes each footprint of the CSV catalog had when
//! this product took it over, from the CSV catalog fixture
//! (`fixtures/import/csv-catalog/manifest.json`, which the fixture check
//! keeps equal to the repository). Built into the program, so the CSV
//! import and the desktop app agree.
//!
//! A footprint the Catalog does not have yet, used only by CSV files never
//! imported, keeps its old problems as clean-up only while its bytes are
//! still these; a footprint drawn or changed since is judged like any new
//! one, so a new or renamed CSV file cannot bring a bad 3D model link in.

use std::collections::BTreeMap;
use std::sync::OnceLock;

const MANIFEST: &str = include_str!("../../../../fixtures/import/csv-catalog/manifest.json");

/// Each footprint file of today's CSV catalog (`footprints/...kicad_mod`)
/// and the SHA-256 of its bytes.
pub fn todays_footprints() -> &'static BTreeMap<String, String> {
    static FOOTPRINTS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    FOOTPRINTS.get_or_init(|| {
        let manifest: serde_json::Value =
            serde_json::from_str(MANIFEST).expect("the CSV catalog fixture is JSON");
        manifest["assets"]["footprints"]
            .as_array()
            .expect("the fixture lists footprints")
            .iter()
            .filter_map(|asset| {
                Some((
                    asset["path"].as_str()?.to_owned(),
                    asset["byte_sha256"].as_str()?.to_owned(),
                ))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todays_footprints_are_the_fixtures() {
        let footprints = todays_footprints();
        assert_eq!(footprints.len(), 76);
        let path = "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod";
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod"
        ))
        .unwrap();
        assert_eq!(footprints[path], crate::hash::sha256_hex(bytes));
    }
}

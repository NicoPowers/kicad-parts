//! CSV catalog files other than the resistors, imported into a real,
//! disposable PocketBase and proved lossless against the oracle: the real 3D
//! models, the extra MPN columns and Excluded from BOM.
//!
//! Each test starts its own PocketBase on a random 127.0.0.1 port with a
//! temp data directory and stops it at the end. The repository is only
//! read; changed inputs are made in temp copies.

use catalog::pocketbase::{ImportCommand, ImportOutcome, read_back, submit_import};
use catalog::{CsvCatalog, sha256_hex};
use catalog_import::services;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn repository() -> PathBuf {
    services::repository_root()
}

fn oracle() -> legacy_import_fixture::Manifest {
    legacy_import_fixture::load_manifest(
        &repository().join("fixtures/import/legacy-catalog/manifest.json"),
    )
    .unwrap()
}

/// The SHA-256 of every file under the CSV catalog's trees, by path.
fn catalog_hashes(root: &Path) -> Vec<(PathBuf, String)> {
    fn walk(directory: &Path, hashes: &mut Vec<(PathBuf, String)>) {
        let mut entries: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, hashes);
            } else {
                let hash = sha256_hex(fs::read(&path).unwrap());
                hashes.push((path, hash));
            }
        }
    }
    let mut hashes = Vec::new();
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        walk(&root.join(tree), &mut hashes);
    }
    hashes
}

/// The CSV files whose footprints use every 3D model in `3d-models/`.
const CSV_FILES_WITH_3D_MODELS: [&str; 7] = [
    "g-con", "g-cpd", "g-ics", "g-ind", "g-pwr", "g-reg", "g-rfm",
];

#[test]
fn every_real_3d_model_imports_in_one_request_per_csv_file() {
    let root = repository();
    let before = catalog_hashes(&root);
    let oracle = oracle();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();

    let mut slowest = Duration::ZERO;
    for csv_file in CSV_FILES_WITH_3D_MODELS {
        let import = CsvCatalog::new(&root).import(csv_file, None).unwrap();
        let state = catalog::pocketbase::read_import_state(&admin).unwrap();
        let command = ImportCommand::new(&import, &format!("Import {csv_file}")).expecting(&state);
        let started = Instant::now();
        let reply = submit_import(&admin, &command).unwrap();
        let took = started.elapsed();
        slowest = slowest.max(took);
        let blob_bytes: usize = import.blobs.iter().map(|blob| blob.bytes.len()).sum();
        eprintln!(
            "{csv_file}: {} blobs, {blob_bytes} bytes, request {} bytes, {took:?}",
            import.blobs.len(),
            serde_json::to_vec(&command.body()).unwrap().len()
        );
        assert!(
            matches!(reply.outcome, ImportOutcome::Applied { .. }),
            "{csv_file}: {}",
            reply.body
        );
    }
    let stored = read_back(&admin).unwrap();
    server.stop();

    // Every file in 3d-models/ is now a stored blob, byte for byte.
    let mut models = 0;
    for entry in fs::read_dir(root.join("3d-models")).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        let sha = sha256_hex(&bytes);
        let blob = stored.blobs.iter().find(|blob| blob.sha256 == sha);
        assert_eq!(blob.map(|blob| &blob.bytes), Some(&bytes), "{sha}");
        models += 1;
    }
    assert_eq!(models, 13);
    for csv_file in CSV_FILES_WITH_3D_MODELS {
        let report = catalog_import::report(&oracle, csv_file, &stored, &root);
        assert!(
            report.is_lossless(),
            "{csv_file}: {}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
    }
    assert_eq!(
        catalog_hashes(&root),
        before,
        "the import changed its inputs"
    );
    // UTF-8 blobs are hashed by PocketBase's own code: g-con, with the
    // largest model (1.5 MiB), takes about 0.2 s. Hashed in JavaScript it
    // took about 5 s, so this catches a return to that.
    assert!(
        slowest < Duration::from_secs(3),
        "the slowest import request took {slowest:?}"
    );
}

/// Copies the CSV catalog trees the oracle reads into `target`.
fn copy_catalog(target: &Path) {
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let destination = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        copy_tree(&repository().join(tree), &target.join(tree));
    }
}

/// Imports `csv_file` from `root` into a new PocketBase and reads every
/// Catalog record back.
fn import_one(root: &Path, csv_file: &str) -> catalog::pocketbase::CatalogReadback {
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    let import = CsvCatalog::new(root).import(csv_file, None).unwrap();
    let reply = submit_import(
        &admin,
        &ImportCommand::new(&import, &format!("Import {csv_file}")),
    )
    .unwrap();
    assert!(
        matches!(reply.outcome, ImportOutcome::Applied { .. }),
        "{csv_file}: {}",
        reply.body
    );
    let stored = read_back(&admin).unwrap();
    server.stop();
    stored
}

/// The import run of `csv_file` that `stored` records last.
fn latest_run(
    stored: &catalog::pocketbase::CatalogReadback,
    csv_file: &str,
) -> catalog::pocketbase::ImportRun {
    stored
        .audit_entries
        .iter()
        .filter(|entry| entry.csv_file == csv_file && entry.csv_import_number > 0)
        .max_by_key(|entry| entry.csv_import_number)
        .and_then(|entry| entry.import_run())
        .unwrap()
}

#[test]
fn an_empty_3d_model_file_becomes_an_import_problem() {
    // An empty file used to fail the whole import with 400, because
    // PocketBase cannot store an empty blob. A temp copy empties the model
    // that g-cpd's diode footprint uses; 3d-models/ itself is never edited.
    let copy = tempfile::tempdir().unwrap();
    copy_catalog(copy.path());
    let model = "3d-models/D_SOD-923.step";
    fs::write(copy.path().join(model), "").unwrap();
    let oracle = legacy_import_fixture::build_manifest(copy.path()).unwrap();

    let stored = import_one(copy.path(), "g-cpd");
    let report = catalog_import::report(&oracle, "g-cpd", &stored, copy.path());
    assert!(
        report.is_lossless(),
        "{}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    // The import run keeps the empty file as a source...
    let file = latest_run(&stored, "g-cpd").file(model).cloned().unwrap();
    assert_eq!((file.role.as_str(), file.size), ("3d-model", 0));
    assert_eq!(file.byte_sha256, sha256_hex(b""));
    // ...and the footprint's reference to it is an import problem, not a
    // CAD item.
    let problem = stored
        .problems
        .iter()
        .find(|problem| problem.code == "empty-3d-model-reference")
        .unwrap();
    assert!(
        problem.location.ends_with("/D_SOD-923.step"),
        "{}",
        problem.location
    );
    assert!(
        stored
            .cad_items
            .iter()
            .all(|cad_item| cad_item.key != model)
    );
    assert!(
        report
            .model_references
            .iter()
            .any(|account| account.stored == "empty file + import problem")
    );
}

/// A CSV file, its part-details columns besides `IPN`, its manufacturer
/// part columns and its import-problem columns. Every other column is
/// supplier data.
type ColumnRow = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

/// The table in `zero_loss.rs`.
#[rustfmt::skip]
const COLUMN_TABLE: [ColumnRow; 21] = [
    ("g-ana", &["Description", "Symbol", "Footprint", "Datasheet", "Sim_Library", "Sim_Name", "Sim_Device", "Sim_Pins"], &["MPN", "Manufacturer"], &[]),
    ("g-art", &["Description", "Symbol", "Footprint", "Comments"], &[], &[]),
    ("g-cap", &["Description", "Symbol", "Footprint", "Value", "Voltage", "Material", "Tolerance", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-con", &["Value", "Description", "Symbol", "Footprint", "Pins", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-cpd", &["Description", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-dio", &["Value", "Description", "Symbol", "Footprint", "Current", "Voltage", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-ics", &["Description", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-ind", &["Description", "Symbol", "Footprint", "Value", "Current", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-mcu", &["Description", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-mec", &["Description", "Symbol", "Footprint", "Type", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-mpu", &["Description", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-opt", &["Description", "Symbol", "Footprint", "Color", "I-forward-max", "V-forward", "Wavelength", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-osc", &["Description", "Symbol", "Footprint", "Frequency", "Stability", "Load", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-pcb", &["Description", "Project link"], &[], &[]),
    ("g-pwr", &["Description", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-reg", &["Description", "Voltage", "Current", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-res", &["Description", "Value", "Symbol", "Footprint", "Tolerance", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-rfm", &["Value", "Description", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &["MPN2"]),
    ("g-rvr", &["Description", "Resistance", "Symbol", "Footprint", "Tolerance", "Turns", "Datasheet"], &["MPN", "Manufacturer"], &["MPN TBC"]),
    ("g-swi", &["Description", "Form", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
    ("g-trs", &["Value", "Type", "max(Vce)", "max(Ic)", "Description", "Symbol", "Footprint", "Datasheet"], &["MPN", "Manufacturer"], &[]),
];

#[test]
fn every_column_of_every_csv_file_has_one_destination() {
    use catalog::csv_import::{ColumnRoute, SUPPLIER_DATA_COLUMNS, route_column};
    let oracle = oracle();
    assert_eq!(oracle.catalog.categories.len(), COLUMN_TABLE.len());
    for (csv_file, part_details, manufacturer_parts, import_problems) in COLUMN_TABLE {
        let category = oracle
            .catalog
            .categories
            .iter()
            .find(|category| format!("g-{}", category.table) == csv_file)
            .unwrap_or_else(|| panic!("the oracle has no {csv_file}"));
        let mut expected: Vec<(&str, &str)> = vec![("IPN", "part-details")];
        expected.extend(part_details.iter().map(|column| (*column, "part-details")));
        expected.extend(
            manufacturer_parts
                .iter()
                .map(|column| (*column, "manufacturer-parts")),
        );
        expected.extend(
            import_problems
                .iter()
                .map(|column| (*column, "import-problem")),
        );
        expected.extend(
            SUPPLIER_DATA_COLUMNS
                .iter()
                .map(|column| (*column, "supplier-data")),
        );
        expected.sort();
        let mut actual: Vec<(&str, &str)> = category
            .document
            .headers
            .iter()
            .map(|header| {
                let destination = match route_column(header) {
                    ColumnRoute::Ipn | ColumnRoute::PartDetails => "part-details",
                    ColumnRoute::MainMpn | ColumnRoute::MainManufacturer => "manufacturer-parts",
                    ColumnRoute::ExtraMpn | ColumnRoute::MpnToBeConfirmed => "import-problem",
                    ColumnRoute::SupplierData => "supplier-data",
                };
                (header.as_str(), destination)
            })
            .collect();
        actual.sort();
        assert_eq!(actual, expected, "{csv_file}");
    }
}

#[test]
fn extra_mpn_columns_are_kept_as_import_problems_end_to_end() {
    // g-rfm's MPN2 is another approved MPN whose manufacturer the CSV file
    // does not name; g-rvr's MPN TBC lists MPNs still to be confirmed.
    // Neither becomes a manufacturer part or a part-details field.
    let copy = tempfile::tempdir().unwrap();
    copy_catalog(copy.path());
    let oracle = legacy_import_fixture::build_manifest(copy.path()).unwrap();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    for csv_file in ["g-rfm", "g-rvr"] {
        let import = CsvCatalog::new(copy.path()).import(csv_file, None).unwrap();
        let state = catalog::pocketbase::read_import_state(&admin).unwrap();
        let command = ImportCommand::new(&import, &format!("Import {csv_file}")).expecting(&state);
        let reply = submit_import(&admin, &command).unwrap();
        assert!(
            matches!(reply.outcome, ImportOutcome::Applied { .. }),
            "{}",
            reply.body
        );
    }
    let stored = read_back(&admin).unwrap();
    server.stop();

    for (csv_file, ipn, column, code, kept) in [
        (
            "g-rfm",
            "RFM-0000-0001",
            "MPN2",
            "mpn-without-manufacturer",
            "MS128-10F",
        ),
        (
            "g-rvr",
            "RVR-0000-1001",
            "MPN TBC",
            "mpn-to-be-confirmed",
            "ST-4ETA102, 23AR1KLFTR, TS53YJ102MR10",
        ),
    ] {
        let report = catalog_import::report(&oracle, csv_file, &stored, copy.path());
        assert!(
            report.is_lossless(),
            "{csv_file}: {}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
        assert!(
            report
                .column_routes
                .contains(&(column.to_owned(), "import-problem".to_owned())),
            "{:?}",
            report.column_routes
        );
        let problem = stored
            .problems
            .iter()
            .find(|problem| problem.code == code && problem.subject == ipn)
            .unwrap_or_else(|| panic!("no {code} for {ipn}"));
        let detail: serde_json::Value = serde_json::from_str(&problem.detail).unwrap();
        assert_eq!(detail["column"], column);
        assert!(problem.detail.contains(kept), "{}", problem.detail);
        // Only the row's own MPN and Manufacturer make a manufacturer part.
        let part = stored.part(ipn).unwrap();
        let revision = stored.part_revision(&part.current_revision).unwrap();
        let details: catalog::PartDetails = serde_json::from_str(&revision.snapshot).unwrap();
        assert_eq!(details.manufacturer_parts.len(), 1, "{ipn}");
        assert!(details.field(column).is_none(), "{ipn}");
        assert!(!revision.snapshot.contains(kept), "{ipn}");
    }
}

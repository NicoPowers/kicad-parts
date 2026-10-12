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

fn oracle() -> csv_catalog_fixture::Manifest {
    csv_catalog_fixture::load_manifest(
        &repository().join("fixtures/import/csv-catalog/manifest.json"),
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
        // One request per CSV file, with every UTF-8 blob (the real 3D
        // models are STEP text) sent as text, which PocketBase's own code
        // hashes; only other bytes go as base64, hashed in JavaScript.
        let body = command.body();
        for (blob, sent) in import.blobs.iter().zip(body["blobs"].as_array().unwrap()) {
            let as_text = std::str::from_utf8(&blob.bytes).is_ok();
            assert_eq!(
                sent.get("text").is_some(),
                as_text,
                "{csv_file}: {}",
                blob.sha256
            );
        }
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

    // Every file in 3d-models/ (one folder per footprint library) is now a
    // stored blob, byte for byte.
    let mut models = 0;
    let model_files = catalog_hashes(&root)
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| path.starts_with(root.join("3d-models")));
    for path in model_files {
        let bytes = fs::read(path).unwrap();
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
    // largest model (1.5 MiB), takes about 0.2 s (about 5 s hashed in
    // JavaScript). The text check above catches a return to that; this
    // bound is only a generous backstop, so a busy machine never fails it.
    assert!(
        slowest < Duration::from_secs(30),
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
    let model = "3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step";
    fs::write(copy.path().join(model), "").unwrap();
    let oracle = csv_catalog_fixture::build_manifest(copy.path()).unwrap();

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
        // The table lists Symbol and Footprint with the part details; they
        // go to the part revision.
        expected.extend(part_details.iter().map(|column| {
            let destination = if catalog::CAD_COLUMNS.contains(column) {
                "part-revision"
            } else {
                "part-details"
            };
            (*column, destination)
        }));
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
                    ColumnRoute::Cad => "part-revision",
                    ColumnRoute::MainMpn | ColumnRoute::MainManufacturer => "manufacturer-parts",
                    ColumnRoute::ExtraMpn
                    | ColumnRoute::MpnToBeConfirmed
                    | ColumnRoute::UnknownMpn => "import-problem",
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
    let oracle = csv_catalog_fixture::build_manifest(copy.path()).unwrap();
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
        let saved = stored.part_details(&part.current_details).unwrap();
        let revision = stored.part_revision(&part.current_revision).unwrap();
        let details: catalog::PartDetails = serde_json::from_str(&saved.snapshot).unwrap();
        assert_eq!(details.manufacturer_parts.len(), 1, "{ipn}");
        assert!(details.field(column).is_none(), "{ipn}");
        assert!(!saved.snapshot.contains(kept), "{ipn}");
        assert!(!revision.snapshot.contains(kept), "{ipn}");
    }
}

#[test]
fn excluded_from_bom_comes_from_the_symbol_and_belongs_to_the_revision() {
    // ART-0000-0001's symbol g-art:TestPoint says `(in_bom no)`, which
    // matches its row's note "Artwork part only, does not appear in BOM".
    // ART-0001-0006 has no evidence either way: its symbol
    // g-con:Conn_PIC_TagConnect_TC2030 is missing from the library
    // (missing-member) and its footprint is in KiCad's own library, which
    // is not read. So it is left unset.
    use catalog::pocketbase::{EditCommand, submit_edit};
    let root = repository();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let import = CsvCatalog::new(&root).import("g-art", None).unwrap();
    let reply = submit_import(&admin, &ImportCommand::new(&import, "Import g-art")).unwrap();
    assert!(
        matches!(reply.outcome, ImportOutcome::Applied { .. }),
        "{}",
        reply.body
    );
    let current = |stored: &catalog::pocketbase::CatalogReadback, ipn: &str| {
        let part = stored.part(ipn).unwrap();
        let revision = stored.part_revision(&part.current_revision).unwrap();
        let parsed: catalog::PartRevision = serde_json::from_str(&revision.snapshot).unwrap();
        (revision.number, revision.snapshot.clone(), parsed)
    };
    let stored = read_back(&admin).unwrap();
    let report = catalog_import::report(&oracle(), "g-art", &stored, &root);
    assert!(
        report.is_lossless(),
        "{}",
        serde_json::to_string_pretty(&report.summary()).unwrap()
    );
    assert_eq!(report.excluded_from_bom, ["ART-0000-0001"]);
    let (_, snapshot, details) = current(&stored, "ART-0000-0001");
    assert!(details.excluded_from_bom);
    assert!(
        snapshot.ends_with(r#","excluded_from_bom":true}"#),
        "{snapshot}"
    );
    let (_, snapshot, details) = current(&stored, "ART-0001-0006");
    assert!(!details.excluded_from_bom);
    assert!(!snapshot.contains("excluded_from_bom"), "{snapshot}");

    // A details edit leaves the revision, and so the setting, as it is.
    let part = stored.part("ART-0000-0001").unwrap();
    let saved = stored.part_details(&part.current_details).unwrap();
    let details: catalog::PartDetails = serde_json::from_str(&saved.snapshot).unwrap();
    let mut edit = EditCommand::new("ART-0000-0001", "Say the pad size")
        .editing_details(&details, saved.number);
    assert!(edit.set_field("Description", "SMT test point, 1.5 mm round pad"));
    let reply = submit_edit(&admin, &edit).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let (number, _, revision) = current(&read_back(&admin).unwrap(), "ART-0000-0001");
    assert_eq!(number, 1);
    assert!(revision.excluded_from_bom);
    // A CAD edit copies it to the new revision, in Rust's canonical JSON
    // (how it follows new CAD is kicad-parts-ccc0).
    let footprint = revision.field("Footprint").unwrap().to_owned();
    let mut edit =
        EditCommand::new("ART-0000-0001", "Name the footprint plainly").editing_cad(&revision, 1);
    assert!(edit.set_field("Footprint", &format!(" {footprint}")));
    let reply = submit_edit(&admin, &edit).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let (number, snapshot, revision) = current(&read_back(&admin).unwrap(), "ART-0000-0001");
    server.stop();
    assert_eq!(number, 2);
    assert!(revision.excluded_from_bom);
    assert_eq!(revision.canonical_json(), snapshot);
}

/// Rewrites the current revision of `ipn` in a read-back copy, keeping its
/// hash consistent, so that only the report's own checks can notice.
fn rewrite_revision(
    stored: &mut catalog::pocketbase::CatalogReadback,
    ipn: &str,
    change: impl FnOnce(&mut catalog::PartRevision),
) {
    let current = stored.part(ipn).unwrap().current_revision.clone();
    let revision = stored
        .part_revisions
        .iter_mut()
        .find(|revision| revision.id == current)
        .unwrap();
    let mut parsed: catalog::PartRevision = serde_json::from_str(&revision.snapshot).unwrap();
    change(&mut parsed);
    revision.snapshot = parsed.canonical_json();
    revision.snapshot_sha256 = sha256_hex(&revision.snapshot);
}

/// Rewrites the current details of `ipn` in a read-back copy, keeping
/// their hash consistent.
fn rewrite_details(
    stored: &mut catalog::pocketbase::CatalogReadback,
    ipn: &str,
    change: impl FnOnce(&mut catalog::PartDetails),
) {
    let current = stored.part(ipn).unwrap().current_details.clone();
    let saved = stored
        .part_details
        .iter_mut()
        .find(|details| details.id == current)
        .unwrap();
    let mut details: catalog::PartDetails = serde_json::from_str(&saved.snapshot).unwrap();
    change(&mut details);
    saved.snapshot = details.canonical_json();
    saved.snapshot_sha256 = sha256_hex(&saved.snapshot);
}

/// Changes the detail of every stored import problem with `code`, or drops
/// those problems when `change` is `None`.
fn retouch_problems(
    stored: &mut catalog::pocketbase::CatalogReadback,
    code: &str,
    change: Option<(&str, &str)>,
) {
    match change {
        Some((from, to)) => {
            for problem in stored
                .problems
                .iter_mut()
                .filter(|problem| problem.code == code)
            {
                assert!(problem.detail.contains(from), "{}", problem.detail);
                problem.detail = problem.detail.replace(from, to);
            }
        }
        None => stored.problems.retain(|problem| problem.code != code),
    }
}

#[test]
fn the_zero_loss_report_fails_when_its_new_checks_find_a_loss() {
    // A temp copy adds the cases the real catalog lacks: g-swi's Form column
    // renamed to `Alt MPN` (an MPN column with no route), ART-0001-0006 given
    // two footprint choices that disagree on the BOM, and g-cpd's diode
    // model emptied. database/ and 3d-models/ themselves are never edited.
    let copy = tempfile::tempdir().unwrap();
    copy_catalog(copy.path());
    let edit = |path: &str, from: &str, to: &str| {
        let path = copy.path().join(path);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{from}");
        fs::write(&path, text.replacen(from, to, 1)).unwrap();
    };
    edit(
        "database/g-swi.csv",
        "Description,Form,",
        "Description,Alt MPN,",
    );
    edit(
        "database/g-art.csv",
        ",Connector:Tag-Connect",
        ",g-Fiducial:Fiducial_1mm_Mask2mm;Connector:Tag-Connect",
    );
    fs::write(
        copy.path()
            .join("3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step"),
        "",
    )
    .unwrap();
    let oracle = csv_catalog_fixture::build_manifest(copy.path()).unwrap();

    let csv_files = ["g-art", "g-cpd", "g-rfm", "g-rvr", "g-swi"];
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&repository(), state.path()).unwrap();
    for csv_file in csv_files {
        let import = CsvCatalog::new(copy.path()).import(csv_file, None).unwrap();
        let state = catalog::pocketbase::read_import_state(&admin).unwrap();
        let command = ImportCommand::new(&import, &format!("Import {csv_file}")).expecting(&state);
        let reply = submit_import(&admin, &command).unwrap();
        assert!(
            matches!(reply.outcome, ImportOutcome::Applied { .. }),
            "{csv_file}: {}",
            reply.body
        );
    }
    let stored = read_back(&admin).unwrap();
    server.stop();
    let report = |stored: &catalog::pocketbase::CatalogReadback, csv_file: &str| {
        catalog_import::report(&oracle, csv_file, stored, copy.path())
    };
    for csv_file in csv_files {
        let untouched = report(&stored, csv_file);
        assert!(
            untouched.is_lossless(),
            "{csv_file}: {}",
            serde_json::to_string_pretty(&untouched.summary()).unwrap()
        );
    }
    let swi = report(&stored, "g-swi");
    assert!(
        swi.column_routes
            .contains(&("Alt MPN".to_owned(), "import-problem".to_owned()))
    );
    // Footprint choices that disagree leave the setting unset.
    let part = stored.part("ART-0001-0006").unwrap();
    let snapshot = &stored
        .part_revision(&part.current_revision)
        .unwrap()
        .snapshot;
    assert!(!snapshot.contains("excluded_from_bom"), "{snapshot}");
    assert!(stored.problems.iter().any(|problem| problem.code
        == "footprint-choices-disagree-on-bom"
        && problem.subject == "ART-0001-0006"));

    // Each broken copy must fail, saying why.
    type Break = Box<dyn Fn(&mut catalog::pocketbase::CatalogReadback)>;
    let cases: Vec<(&str, &str, Break, &str)> = vec![
        (
            "Excluded from BOM unset",
            "g-art",
            Box::new(|stored| {
                rewrite_revision(stored, "ART-0000-0001", |d| d.excluded_from_bom = false)
            }),
            "Excluded from BOM is false",
        ),
        (
            "Excluded from BOM set on mixed footprint choices",
            "g-art",
            Box::new(|stored| {
                rewrite_revision(stored, "ART-0001-0006", |d| d.excluded_from_bom = true)
            }),
            "Excluded from BOM is true",
        ),
        (
            "mixed footprint choices without their import problem",
            "g-art",
            Box::new(|stored| retouch_problems(stored, "footprint-choices-disagree-on-bom", None)),
            "no import problem says so",
        ),
        (
            "MPN2 problem missing",
            "g-rfm",
            Box::new(|stored| retouch_problems(stored, "mpn-without-manufacturer", None)),
            "field MPN2",
        ),
        (
            "MPN2 problem with another value",
            "g-rfm",
            Box::new(|stored| {
                retouch_problems(
                    stored,
                    "mpn-without-manufacturer",
                    Some(("MS128-10F", "MS128-10X")),
                )
            }),
            "field MPN2",
        ),
        (
            "MPN TBC problem missing",
            "g-rvr",
            Box::new(|stored| retouch_problems(stored, "mpn-to-be-confirmed", None)),
            "field MPN TBC",
        ),
        (
            "MPN TBC problem with another value",
            "g-rvr",
            Box::new(|stored| {
                retouch_problems(
                    stored,
                    "mpn-to-be-confirmed",
                    Some(("ST-4ETA102", "ST-4ETA103")),
                )
            }),
            "field MPN TBC",
        ),
        (
            "unrouted MPN column problem missing",
            "g-swi",
            Box::new(|stored| retouch_problems(stored, "unknown-mpn-column", None)),
            "field Alt MPN",
        ),
        (
            "empty 3D model problem missing",
            "g-cpd",
            Box::new(|stored| retouch_problems(stored, "empty-3d-model-reference", None)),
            "no import problem empty-3d-model-reference",
        ),
        (
            "supplier number leaked into the part details",
            "g-rfm",
            Box::new(|stored| {
                rewrite_details(stored, "RFM-0000-0001", |d| {
                    d.fields.push(catalog::DetailField {
                        name: "Order code".into(),
                        value: "C4365834".into(),
                    })
                })
            }),
            "supplier data LCSC=C4365834 also appears in the part details",
        ),
    ];
    for (name, csv_file, break_it, says) in cases {
        let mut broken = stored.clone();
        break_it(&mut broken);
        let report = report(&broken, csv_file);
        assert!(!report.is_lossless(), "{name}: the report passed");
        assert!(
            report.problems.iter().any(|problem| problem.contains(says)),
            "{name}: {:?}",
            report.problems
        );
    }
}

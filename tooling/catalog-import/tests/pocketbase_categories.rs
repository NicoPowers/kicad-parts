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

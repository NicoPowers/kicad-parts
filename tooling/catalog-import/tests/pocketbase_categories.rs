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

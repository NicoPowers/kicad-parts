use legacy_import_fixture::{build_manifest, compare, load_manifest};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn expected_path(root: &Path) -> PathBuf {
    root.join("fixtures/import/legacy-catalog/manifest.json")
}

#[test]
fn checked_in_manifest_is_stable_and_complete() {
    let root = root();
    let expected = load_manifest(&expected_path(&root)).unwrap();
    let actual = build_manifest(&root).unwrap();
    assert_eq!(actual.schema_version, 2);
    assert_eq!(actual.catalog.category_count, 21);
    assert_eq!(actual.catalog.row_count, 178);
    assert_eq!(actual.catalog.generated_sqlite_tables.len(), 21);
    assert_eq!(actual.sqlite_projection.table_count, 21);
    assert_eq!(actual.sqlite_projection.row_count, 178);
    assert_eq!(actual.descriptor.entry_count, 19);
    assert_eq!(actual.descriptor.csv_tables_not_exposed, ["mec", "pcb"]);
    assert!(actual.descriptor.descriptor_tables_without_csv.is_empty());
    assert_eq!(
        expected,
        actual,
        "{}",
        compare(&expected, &actual)
            .into_iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn inventories_substitutes_assets_and_references() {
    let manifest = build_manifest(&root()).unwrap();
    assert_eq!(
        manifest.substitutes.headers,
        [
            "IPN",
            "MPN",
            "Manufacturer",
            "Datasheet",
            "Supplier",
            "SupplierPN"
        ]
    );
    assert!(manifest.substitutes.rows.is_empty());
    assert_eq!(manifest.assets.symbols.len(), 18);
    assert_eq!(manifest.assets.footprints.len(), 76);
    assert_eq!(manifest.assets.models_3d.len(), 13);
    assert!(
        manifest
            .assets
            .symbol_references
            .iter()
            .any(|r| r.local && r.status == "resolved")
    );
    assert!(
        manifest
            .assets
            .footprint_references
            .iter()
            .any(|r| r.local && r.status == "resolved")
    );
    assert!(
        manifest
            .assets
            .model_references
            .iter()
            .any(|r| r.scope == "local" && r.status == "resolved")
    );
    assert!(
        manifest
            .diagnostics
            .iter()
            .any(|d| d.code == "update-script-omits-csv-table" && d.location == "table[pcb]")
    );
}

#[test]
fn controlled_field_mutation_has_actionable_diff_and_does_not_touch_source() {
    let source_root = root();
    let before = tree_hash(&source_root);
    let temporary = tempfile::tempdir().unwrap();
    copy_inputs(&source_root, temporary.path());
    let expected = build_manifest(temporary.path()).unwrap();
    let csv_path = temporary.path().join("database/g-ana.csv");
    let original = fs::read_to_string(&csv_path).unwrap();
    fs::write(
        &csv_path,
        original.replacen("ANA-0001-0001", "ANA-9999-0001", 1),
    )
    .unwrap();
    let actual = build_manifest(temporary.path()).unwrap();
    let differences = compare(&expected, &actual);
    assert!(
        differences.iter().any(|d| d.path
            == "catalog.categories[ana].rows[IPN=ANA-0001-0001].fields[IPN]"
            && d.expected.contains("ANA-0001-0001")
            && d.actual.contains("ANA-9999-0001")),
        "{differences:#?}"
    );
    assert_eq!(before, tree_hash(&source_root));
}

#[test]
fn descriptor_semantic_hash_ignores_json_formatting_but_byte_hash_does_not() {
    let source_root = root();
    let temporary = tempfile::tempdir().unwrap();
    copy_inputs(&source_root, temporary.path());
    let before = build_manifest(temporary.path()).unwrap();
    let path = temporary.path().join("database/#gplm.kicad_dbl");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let after = build_manifest(temporary.path()).unwrap();
    assert_ne!(before.descriptor.byte_sha256, after.descriptor.byte_sha256);
    assert_eq!(
        before.descriptor.semantic_sha256,
        after.descriptor.semantic_sha256
    );
}

#[test]
fn malformed_descriptor_field_reports_its_semantic_location() {
    let source_root = root();
    let temporary = tempfile::tempdir().unwrap();
    copy_inputs(&source_root, temporary.path());
    let path = temporary.path().join("database/#gplm.kicad_dbl");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["libraries"][0]["fields"][0]["visible_on_add"] = serde_json::json!("false");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let error = build_manifest(temporary.path()).unwrap_err().to_string();
    assert!(
        error.contains(".libraries[0].fields[0].visible_on_add")
            && error.contains("must be a boolean"),
        "{error}"
    );
}

#[test]
fn implementation_has_no_python_or_pyqt_runtime_bridge() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for source in [
        crate_root.join("src/lib.rs"),
        crate_root.join("src/main.rs"),
    ] {
        let text = fs::read_to_string(source).unwrap();
        assert!(!text.contains("std::process::Command"));
        assert!(!text.to_ascii_lowercase().contains("pyqt"));
        assert!(!text.to_ascii_lowercase().contains("python"));
    }
}

#[test]
fn ambiguous_local_symbol_is_retained_as_a_diagnostic() {
    let source_root = root();
    let temporary = tempfile::tempdir().unwrap();
    copy_inputs(&source_root, temporary.path());
    let path = temporary.path().join("symbols/g-ana.kicad_sym");
    let mut content = fs::read_to_string(&path).unwrap();
    content.push_str("\n(symbol \"OPA990\")\n");
    fs::write(path, content).unwrap();
    let manifest = build_manifest(temporary.path()).unwrap();
    assert!(manifest.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "ambiguous-local-symbol-reference"
            && diagnostic.location == "Symbol=g-ana:OPA990"
            && diagnostic.message.contains("ANA-0002-0002")
    }));
}

#[test]
fn escaped_catalog_and_model_references_are_diagnosed_without_reading_external_sentinel() {
    let sandbox = tempfile::tempdir().unwrap();
    let repository = sandbox.path().join("repository");
    fs::create_dir(&repository).unwrap();
    copy_inputs(&root(), &repository);

    fs::create_dir(repository.join("symbols/g-escape")).unwrap();
    let sentinel = sandbox.path().join("sentinel.kicad_sym");
    fs::write(&sentinel, [0xff, 0xfe, 0xfd]).unwrap();
    let csv_path = repository.join("database/g-ana.csv");
    let csv = fs::read_to_string(&csv_path).unwrap();
    fs::write(
        &csv_path,
        csv.replacen("g-ana:OPA990S", "g-escape/../../../sentinel:OPA990S", 1),
    )
    .unwrap();

    let footprint_path =
        repository.join(&build_manifest(&repository).unwrap().assets.footprints[0].path);
    let mut footprint = fs::read_to_string(&footprint_path).unwrap();
    footprint.push_str("\n(model \"${GITPLM_PARTS}/../sentinel.step\")\n");
    fs::write(footprint_path, footprint).unwrap();

    let manifest = build_manifest(&repository).unwrap();
    assert!(manifest.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "unsafe-path-local-symbol-reference"
            && diagnostic.location == "Symbol=g-escape/../../../sentinel:OPA990S"
    }));
    assert!(manifest.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "unsafe-path-3d-model-reference"
            && diagnostic.location == "model=${GITPLM_PARTS}/../sentinel.step"
    }));
    assert!(
        manifest
            .diagnostics
            .iter()
            .all(|diagnostic| !Path::new(&diagnostic.path).is_absolute())
    );
    assert_eq!(fs::read(&sentinel).unwrap(), [0xff, 0xfe, 0xfd]);
}

#[test]
fn sqlite_null_is_distinct_from_csv_blank_and_has_field_diagnostic() {
    let source_root = root();
    let temporary = tempfile::tempdir().unwrap();
    copy_inputs(&source_root, temporary.path());
    let before = build_manifest(temporary.path()).unwrap();
    let sqlite_path = temporary.path().join("database/parts.sqlite");
    {
        let connection = rusqlite::Connection::open(&sqlite_path).unwrap();
        connection
            .execute("UPDATE ana SET DigiKey_Price = NULL WHERE rowid = 1", [])
            .unwrap();
    }
    let after = build_manifest(temporary.path()).unwrap();
    assert_ne!(
        before.sqlite_projection.semantic_sha256,
        after.sqlite_projection.semantic_sha256
    );
    assert!(after.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "sqlite-cell-differs-from-csv"
            && diagnostic.location == "table[ana].rows[IPN=ANA-0001-0001].fields[DigiKey_Price]"
            && diagnostic.message == "SQLite NULL does not match CSV empty string"
    }));
    let ana = after
        .sqlite_projection
        .tables
        .iter()
        .find(|table| table.name == "ana")
        .unwrap();
    let column = ana
        .headers
        .iter()
        .position(|header| header == "DigiKey_Price")
        .unwrap();
    assert_eq!(ana.rows[0][column], None);
}

#[test]
fn semantic_hash_ignores_csv_serialization_but_byte_hash_does_not() {
    let source_root = root();
    let temporary = tempfile::tempdir().unwrap();
    copy_inputs(&source_root, temporary.path());
    let before = build_manifest(temporary.path()).unwrap();
    let path = temporary.path().join("database/g-ana.csv");
    let bytes = fs::read(&path).unwrap();
    let changed = String::from_utf8(bytes).unwrap().replace("\r\n", "\n");
    fs::write(&path, changed).unwrap();
    let after = build_manifest(temporary.path()).unwrap();
    assert_ne!(
        before.catalog.categories[0].document.byte_sha256,
        after.catalog.categories[0].document.byte_sha256
    );
    assert_eq!(
        before.catalog.categories[0].document.semantic_sha256,
        after.catalog.categories[0].document.semantic_sha256
    );
}

#[test]
fn cad_semantic_hash_ignores_formatting_whitespace_but_byte_hash_does_not() {
    let source_root = root();
    let temporary = tempfile::tempdir().unwrap();
    copy_inputs(&source_root, temporary.path());
    let before = build_manifest(temporary.path()).unwrap();
    let asset_path = &before.assets.footprints[0].path;
    let path = temporary
        .path()
        .join(asset_path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, original.replacen('\n', "\n    ", 1)).unwrap();
    let after = build_manifest(temporary.path()).unwrap();
    let changed = after
        .assets
        .footprints
        .iter()
        .find(|asset| asset.path == *asset_path)
        .unwrap();
    assert_ne!(before.assets.footprints[0].byte_sha256, changed.byte_sha256);
    assert_eq!(
        before.assets.footprints[0].semantic_sha256,
        changed.semantic_sha256
    );
}

fn copy_inputs(source: &Path, destination: &Path) {
    for directory in ["database", "symbols", "footprints", "3d-models"] {
        copy_tree(&source.join(directory), &destination.join(directory));
    }
}
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn tree_hash(root: &Path) -> String {
    let mut paths = Vec::new();
    for directory in ["database", "symbols", "footprints", "3d-models"] {
        collect(&root.join(directory), &mut paths);
    }
    paths.sort();
    let mut hash = Sha256::new();
    for path in paths {
        hash.update(
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        );
        hash.update(fs::read(path).unwrap());
    }
    format!("{:x}", hash.finalize())
}
fn collect(directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, paths)
        } else {
            paths.push(path)
        }
    }
}

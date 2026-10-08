use super::*;
use std::fs;

const SYMBOLS: &str = "(kicad_symbol_lib\n\t(version 20241209)\n\t(symbol \"R_US\"\n\t\t(property \"Reference\" \"R\")\n\t)\n\t(symbol \"R_Child\"\n\t\t(extends \"R_US\")\n\t)\n)\n";
const FOOTPRINT: &str = "(footprint \"R_0603\"\n\t(version 20241229)\n\t(model \"${KICAD9_3DMODEL_DIR}/R.step\")\n\t(model \"${GITPLM_PARTS}/3d-models/local.step\")\n\t(model \"${GITPLM_3DMODELS}/gone.step\")\n)\n";

/// A small repository with every case the import must keep.
fn fixture(csv: &str, substitutes: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    for directory in [
        "database",
        "symbols",
        "footprints/g-res.pretty",
        "3d-models",
    ] {
        fs::create_dir_all(path.join(directory)).unwrap();
    }
    fs::write(path.join("database/g-res.csv"), csv).unwrap();
    fs::write(path.join("database/substitutes.csv"), substitutes).unwrap();
    fs::write(path.join("symbols/g-pas.kicad_sym"), SYMBOLS).unwrap();
    fs::write(
        path.join("footprints/g-res.pretty/R_0603.kicad_mod"),
        FOOTPRINT,
    )
    .unwrap();
    fs::write(path.join("3d-models/local.step"), "ISO-10303-21;\n").unwrap();
    root
}

const HEADER: &str = "IPN,MPN,Manufacturer,Value,Symbol,Footprint,LCSC,DigiKey_Price\n";

#[test]
fn routes_every_field_and_binds_exact_assets() {
    let csv = format!(
        "{HEADER}RES-0000-0470,MPN47,Yageo,47,g-pas:R_US,g-res:R_0603,C23182,0.10\nRES-0000-1000,MPN100,Yageo,100,Device:R,g-res:R_0603;g-res:Missing\n"
    );
    let substitutes = "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\nRES-0000-0470,ALT47,Vishay,https://ds,DigiKey,541-47\nRES-0000-0470,MPN47,Yageo,,,\nRES-0000-9999,X,Y,,,\nCAP-0000-0001,C,D,,,\n";
    let root = fixture(&csv, substitutes);
    let import = LegacyCatalog::new(root.path())
        .import("g-res", Some("abc123".into()))
        .unwrap();

    assert_eq!(import.parts.len(), 2);
    let first = &import.parts[0];
    assert_eq!(first.ipn.as_str(), "RES-0000-0470");
    assert_eq!(first.id, ManagedPartId::for_ipn(&first.ipn));
    let snapshot = &first.snapshot;
    // Semantic columns only, in source order, under their source names.
    let names: Vec<&str> = snapshot.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Value", "Symbol", "Footprint"]);
    assert_eq!(snapshot.ipn, "RES-0000-0470");
    assert_eq!(
        snapshot.approved_manufacturer_parts,
        vec![
            ApprovedManufacturerPart {
                manufacturer: "Yageo".into(),
                mpn: "MPN47".into(),
                datasheet: None
            },
            ApprovedManufacturerPart {
                manufacturer: "Vishay".into(),
                mpn: "ALT47".into(),
                datasheet: Some("https://ds".into())
            },
        ]
    );
    // Supplier codes and prices are Operational Facts in Provenance.
    assert_eq!(
        first.provenance.operational_facts,
        vec![
            OperationalFact {
                column: "LCSC".into(),
                value: "C23182".into()
            },
            OperationalFact {
                column: "DigiKey_Price".into(),
                value: "0.10".into()
            },
        ]
    );
    assert!(!first.snapshot_json.contains("C23182"));
    assert_eq!(first.provenance.substitutes.len(), 1);
    assert_eq!(
        first.provenance.substitutes[0].operational_facts[1],
        OperationalFact {
            column: "SupplierPN".into(),
            value: "541-47".into()
        }
    );
    assert_eq!(first.provenance.source_commit.as_deref(), Some("abc123"));

    // The short second row is padded in the snapshot; Provenance keeps
    // the row exactly as written.
    let second = &import.parts[1];
    assert_eq!(second.provenance.record.len(), 6);
    assert_eq!(second.provenance.operational_facts[0].value, "");
    let targets: Vec<_> = second
        .snapshot
        .asset_bindings
        .iter()
        .map(|binding| (binding.role, binding.ordinal, binding.target.clone()))
        .collect();
    assert_eq!(
        targets[0],
        (BindingRole::Symbol, 0, BindingTarget::External)
    );
    assert!(matches!(
        targets[1],
        (
            BindingRole::Footprint,
            0,
            BindingTarget::AssetRevision { .. }
        )
    ));
    assert_eq!(
        targets[2],
        (
            BindingRole::Footprint,
            1,
            BindingTarget::Unresolved {
                status: "missing-member".into()
            }
        )
    );

    // The file can be rebuilt byte for byte from Provenance.
    let roles: Vec<(&str, &str)> = import
        .sources
        .iter()
        .map(|source| (source.role.as_str(), source.path.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("3d-model", "3d-models/local.step"),
            ("catalog-csv", "database/g-res.csv"),
            ("substitutes-csv", "database/substitutes.csv"),
            ("footprint", "footprints/g-res.pretty/R_0603.kicad_mod"),
            ("symbol-library", "symbols/g-pas.kicad_sym"),
        ]
    );
    let header = import.sources[1].header_raw.clone().unwrap();
    let rebuilt: String = std::iter::once(header)
        .chain(
            import
                .parts
                .iter()
                .map(|part| part.provenance.raw_record.clone()),
        )
        .collect();
    assert_eq!(rebuilt, csv);

    // Assets: one symbol span, one footprint file, one local 3D model.
    let kinds: Vec<_> = import
        .asset_revisions
        .iter()
        .map(|revision| (revision.asset.kind, revision.asset.key.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (AssetKind::Symbol, "g-pas:R_US"),
            (AssetKind::Model3d, "3d-models/local.step"),
            (AssetKind::Footprint, "g-res:R_0603"),
        ]
    );
    let symbol = &import.asset_revisions[0];
    let range = symbol.provenance.byte_range.unwrap();
    let span = &SYMBOLS[range[0] as usize..range[1] as usize];
    assert!(span.starts_with("(symbol \"R_US\"") && span.ends_with(')'));
    assert_eq!(symbol.declaration.blob_sha256, sha256_hex(span));
    assert_eq!(symbol.provenance.byte_sha256, sha256_hex(SYMBOLS));
    assert_eq!(
        symbol.declaration.format_version.as_deref(),
        Some("20241209")
    );
    assert_eq!(
        symbol.declaration.license,
        LicenseDeclaration::UndeclaredNeedsReview
    );
    let footprint = &import.asset_revisions[2];
    assert_eq!(footprint.declaration.blob_sha256, sha256_hex(FOOTPRINT));
    assert_eq!(footprint.provenance.byte_range, None);
    let models: Vec<_> = footprint
        .declaration
        .model_references
        .iter()
        .map(|model| (model.scope.as_str(), model.target.clone()))
        .collect();
    assert_eq!(
        models[0],
        (
            "external-variable",
            ModelTarget::Unresolved {
                status: "unresolved".into()
            }
        )
    );
    assert!(matches!(
        models[1],
        ("local", ModelTarget::AssetRevision { .. })
    ));
    assert_eq!(
        models[2],
        (
            "local",
            ModelTarget::Unresolved {
                status: "missing".into()
            }
        )
    );
    assert_eq!(import.blobs.len(), 3);

    let codes: Vec<(&str, &str)> = import
        .diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.location.as_str()))
        .collect();
    for expected in [
        ("duplicate-approved-manufacturer-part", "rows[2]"),
        ("substitute-without-managed-part", "rows[3]"),
        (
            "unresolved-3d-model-reference",
            "model=${KICAD9_3DMODEL_DIR}/R.step",
        ),
        (
            "missing-3d-model-reference",
            "model=${GITPLM_3DMODELS}/gone.step",
        ),
        (
            "missing-member-local-footprint-reference",
            "Footprint=g-res:Missing",
        ),
    ] {
        assert!(codes.contains(&expected), "{expected:?} not in {codes:?}");
    }
    // A substitute of another IPN Family is another import's business.
    assert!(!codes.iter().any(|(_, location)| *location == "rows[4]"));
    assert_eq!(
        (import.substitute_rows, import.substitute_rows_in_scope),
        (4, 2)
    );
}

#[test]
fn rows_that_cannot_become_parts_are_kept_as_diagnostics() {
    let csv = format!(
        "{HEADER}res-0000-0001,M,Y,1,,,,\nRES-0000-0002,M,Y,2,,,,\nRES-0000-0002,M2,Y,2,,,,\n,,,,,,,\nRES-0000-0003,M,Y,3,,,,,extra\nRES-0000-0004,,Y,4,,,,\n"
    );
    let root = fixture(&csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let import = LegacyCatalog::new(root.path())
        .import("g-res", None)
        .unwrap();
    let ipns: Vec<&str> = import.parts.iter().map(|p| p.ipn.as_str()).collect();
    assert_eq!(ipns, ["RES-0000-0002", "RES-0000-0004"]);
    let mut codes: Vec<&str> = import.diagnostics.iter().map(|d| d.code.as_str()).collect();
    codes.sort();
    assert_eq!(
        codes,
        [
            "duplicate-ipn",
            "incomplete-primary-manufacturer-part",
            "invalid-ipn",
            "invalid-ipn",
            "row-has-extra-fields"
        ]
    );
    // Each rejected row keeps its exact bytes.
    let duplicate = import
        .diagnostics
        .iter()
        .find(|d| d.code == "duplicate-ipn")
        .unwrap();
    let detail: Value = serde_json::from_str(&duplicate.detail_json).unwrap();
    assert_eq!(detail["raw_record"], "RES-0000-0002,M2,Y,2,,,,\n");
    assert_eq!(detail["row_number"], 3);
    // A part without a complete primary manufacturer part has none.
    assert!(
        import.parts[1]
            .snapshot
            .approved_manufacturer_parts
            .is_empty()
    );
}

#[test]
fn the_real_resistor_grouping_imports_deterministically_without_writing() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let inputs = [
        "database/g-res.csv",
        "database/substitutes.csv",
        "symbols/g-pas.kicad_sym",
        "footprints/g-res.pretty/R_0603_1608Metric.kicad_mod",
    ];
    let before: Vec<_> = inputs
        .iter()
        .map(|path| sha256_hex(fs::read(root.join(path)).unwrap()))
        .collect();
    let catalog = LegacyCatalog::new(&root);
    let first = catalog.import("g-res", None).unwrap();
    let second = catalog.import("g-res", None).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert_eq!(first.parts.len(), 29);
    assert_eq!(first.asset_revisions.len(), 2);
    assert_eq!(first.diagnostics.len(), 1);
    assert_eq!(first.diagnostics[0].code, "unresolved-3d-model-reference");
    let after: Vec<_> = inputs
        .iter()
        .map(|path| sha256_hex(fs::read(root.join(path)).unwrap()))
        .collect();
    assert_eq!(before, after);
}

#[test]
fn refuses_unknown_groupings() {
    let root = fixture(HEADER, "IPN\n");
    let catalog = LegacyCatalog::new(root.path());
    assert!(catalog.import("res", None).is_err());
    assert!(catalog.import("g-../x", None).is_err());
    assert!(catalog.import("g-cap", None).is_err());
}

#[test]
fn model_references_classify_like_the_oracle() {
    for (reference, scope, status) in [
        ("/home/user/model.step", "posix-absolute", "unresolved"),
        (
            "C:/Models/model.step",
            "windows-drive-absolute",
            "unresolved",
        ),
        (
            "\\\\server\\share\\model.step",
            "unc-absolute",
            "unresolved",
        ),
        ("\\Models\\model.step", "windows-rooted", "unresolved"),
        ("C:model.step", "windows-drive-relative", "unresolved"),
        (
            "${KICAD9_3DMODEL_DIR}/model.step",
            "external-variable",
            "unresolved",
        ),
        ("models/model.step", "relative", "unresolved"),
        ("${GITPLM_PARTS}/3d-models/model.step", "local", "missing"),
        ("${GITPLM_PARTS}/symbols/model.step", "local", "unsafe-path"),
        ("${GITPLM_3DMODELS}/../model.step", "local", "unsafe-path"),
    ] {
        let classified = classify_model_reference(reference);
        assert_eq!(
            (classified.scope, classified.status),
            (scope, status),
            "{reference}"
        );
    }
}

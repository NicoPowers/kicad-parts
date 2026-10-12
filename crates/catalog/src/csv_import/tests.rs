use super::*;
use std::fs;

const SYMBOLS: &str = "(kicad_symbol_lib\n\t(version 20241209)\n\t(symbol \"R_US\"\n\t\t(property \"Reference\" \"R\")\n\t)\n\t(symbol \"R_Child\"\n\t\t(extends \"R_US\")\n\t)\n)\n";
const FOOTPRINT: &str = "(footprint \"R_0603\"\n\t(version 20241229)\n\t(model \"${KICAD9_3DMODEL_DIR}/R.step\")\n\t(model \"${GITPLM_PARTS}/3d-models/local.step\")\n\t(model \"${GITPLM_3DMODELS}/gone.step\")\n)\n";

/// A small repository with every case the import must keep.
fn fixture(csv: &str, substitutes_csv_rows: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    for directory in [
        "database",
        "symbols",
        "footprints/g-Resistor_SMD.pretty",
        "3d-models",
    ] {
        fs::create_dir_all(path.join(directory)).unwrap();
    }
    fs::write(path.join("database/g-res.csv"), csv).unwrap();
    fs::write(path.join("database/substitutes.csv"), substitutes_csv_rows).unwrap();
    fs::write(path.join("symbols/g-res.kicad_sym"), SYMBOLS).unwrap();
    fs::write(
        path.join("footprints/g-Resistor_SMD.pretty/R_0603.kicad_mod"),
        FOOTPRINT,
    )
    .unwrap();
    fs::write(path.join("3d-models/local.step"), "ISO-10303-21;\n").unwrap();
    root
}

const HEADER: &str = "IPN,MPN,Manufacturer,Value,Symbol,Footprint,LCSC,DigiKey_Price\n";

#[test]
fn routes_every_field_and_binds_exact_cad_items() {
    let csv = format!(
        "{HEADER}RES-0000-0470,MPN47,Yageo,47,g-res:R_US,g-Resistor_SMD:R_0603,C23182,0.10\nRES-0000-1000,MPN100,Yageo,100,Device:R,g-Resistor_SMD:R_0603;g-Resistor_SMD:Missing\n"
    );
    let substitutes_csv_rows = "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\nRES-0000-0470,ALT47,Vishay,https://ds,DigiKey,541-47\nRES-0000-0470,MPN47,Yageo,,,\nRES-0000-9999,X,Y,,,\nCAP-0000-0001,C,D,,,\n";
    let root = fixture(&csv, substitutes_csv_rows);
    let import = CsvCatalog::new(root.path())
        .import("g-res", Some("abc123".into()))
        .unwrap();

    assert_eq!(import.parts.len(), 2);
    let first = &import.parts[0];
    assert_eq!(first.ipn.as_str(), "RES-0000-0470");
    assert_eq!(first.id, PartId::for_ipn(&first.ipn));
    let snapshot = &first.details;
    // Part details columns only, in source order, under their source names;
    // Symbol and Footprint belong to the revision, exactly as written.
    let names: Vec<&str> = snapshot.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Value"]);
    assert_eq!(snapshot.ipn, "RES-0000-0470");
    let cad: Vec<&str> = first
        .revision
        .cad_fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(cad, ["Symbol", "Footprint"]);
    assert_eq!(first.revision.ipn, "RES-0000-0470");
    assert_eq!(route_column("Symbol"), ColumnRoute::Cad);
    assert_eq!(route_column("Footprint"), ColumnRoute::Cad);
    assert_eq!(
        snapshot.manufacturer_parts,
        vec![
            ManufacturerPart {
                manufacturer: "Yageo".into(),
                mpn: "MPN47".into(),
            },
            ManufacturerPart {
                manufacturer: "Vishay".into(),
                mpn: "ALT47".into(),
            },
        ]
    );
    // Supplier codes and prices are supplier data in the source.
    assert_eq!(
        first.source.supplier_data,
        vec![
            SupplierData {
                column: "LCSC".into(),
                value: "C23182".into()
            },
            SupplierData {
                column: "DigiKey_Price".into(),
                value: "0.10".into()
            },
        ]
    );
    assert!(!first.details_json.contains("C23182"));
    assert!(!first.revision_json.contains("C23182"));
    assert!(!first.details_json.contains("cad_references"));
    assert_eq!(first.source.substitutes_csv_rows.len(), 1);
    assert_eq!(
        first.source.substitutes_csv_rows[0].supplier_data[1],
        SupplierData {
            column: "SupplierPN".into(),
            value: "541-47".into()
        }
    );
    assert_eq!(first.source.source_commit.as_deref(), Some("abc123"));

    // The short second row is padded in the part details; the source keeps
    // the row exactly as written.
    let second = &import.parts[1];
    assert_eq!(second.source.record.len(), 6);
    assert_eq!(second.source.supplier_data[0].value, "");
    let targets: Vec<_> = second
        .revision
        .cad_references
        .iter()
        .map(|binding| (binding.role, binding.ordinal, binding.target.clone()))
        .collect();
    assert_eq!(
        targets[0],
        (CadReferenceRole::Symbol, 0, CadReferenceTarget::External)
    );
    assert!(matches!(
        targets[1],
        (
            CadReferenceRole::Footprint,
            0,
            CadReferenceTarget::CadItemRevision { .. }
        )
    ));
    assert_eq!(
        targets[2],
        (
            CadReferenceRole::Footprint,
            1,
            CadReferenceTarget::Unresolved {
                status: "missing-member".into()
            }
        )
    );

    // The file can be rebuilt byte for byte from the sources.
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
            (
                "footprint",
                "footprints/g-Resistor_SMD.pretty/R_0603.kicad_mod"
            ),
            ("symbol-library", "symbols/g-res.kicad_sym"),
        ]
    );
    let header = import.sources[1].header_raw.clone().unwrap();
    let rebuilt: String = std::iter::once(header)
        .chain(
            import
                .parts
                .iter()
                .map(|part| part.source.raw_record.clone()),
        )
        .collect();
    assert_eq!(rebuilt, csv);
    // The source keeps every row, so the file can be rebuilt from it alone.
    let from_source: String = std::iter::once(import.sources[1].header_raw.clone().unwrap())
        .chain(import.sources[1].rows.clone().unwrap())
        .collect();
    assert_eq!(from_source, csv);
    assert_eq!(import.sources[0].rows, None);

    // CadItems: one symbol span, one footprint file, one local 3D model.
    let kinds: Vec<_> = import
        .cad_item_revisions
        .iter()
        .map(|revision| (revision.cad_item.kind, revision.cad_item.key.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (CadItemKind::Symbol, "g-res:R_US"),
            (CadItemKind::Model3d, "3d-models/local.step"),
            (CadItemKind::Footprint, "g-Resistor_SMD:R_0603"),
        ]
    );
    let symbol = &import.cad_item_revisions[0];
    let range = symbol.source.byte_range.unwrap();
    let span = &SYMBOLS[range[0] as usize..range[1] as usize];
    assert!(span.starts_with("(symbol \"R_US\"") && span.ends_with(')'));
    assert_eq!(symbol.snapshot.blob_sha256, sha256_hex(span));
    assert_eq!(symbol.source.byte_sha256, sha256_hex(SYMBOLS));
    assert_eq!(symbol.snapshot.format_version.as_deref(), Some("20241209"));
    assert_eq!(symbol.snapshot.license, License::UnknownNeedsReview);
    let footprint = &import.cad_item_revisions[2];
    assert_eq!(footprint.snapshot.blob_sha256, sha256_hex(FOOTPRINT));
    assert_eq!(footprint.source.byte_range, None);
    let models: Vec<_> = footprint
        .snapshot
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
        ("local", ModelTarget::CadItemRevision { .. })
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
        .problems
        .iter()
        .map(|d| (d.code.as_str(), d.location.as_str()))
        .collect();
    for expected in [
        ("duplicate-manufacturer-part", "rows[2]"),
        // The part has no datasheet, so the substitutes.csv row's is another one.
        ("substitutes-csv-datasheet-differs", "rows[1]"),
        ("substitutes-csv-row-without-part", "rows[3]"),
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
            "Footprint=g-Resistor_SMD:Missing",
        ),
    ] {
        assert!(codes.contains(&expected), "{expected:?} not in {codes:?}");
    }
    // A substitutes.csv row of another IPN prefix is another import's business.
    assert!(!codes.iter().any(|(_, location)| *location == "rows[4]"));
    assert_eq!(
        (
            import.substitutes_csv_row_count,
            import.substitutes_csv_rows_in_scope
        ),
        (4, 2)
    );
}

#[test]
fn an_empty_3d_model_file_is_an_import_problem_and_stays_a_source() {
    let csv = format!("{HEADER}RES-0000-0470,MPN47,Yageo,47,g-res:R_US,g-Resistor_SMD:R_0603,,\n");
    let root = fixture(&csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    fs::write(root.path().join("3d-models/local.step"), "").unwrap();
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();

    // The file is a source of the import, with its size and hash...
    let source = import
        .sources
        .iter()
        .find(|source| source.path == "3d-models/local.step")
        .unwrap();
    assert_eq!(
        (
            source.role.as_str(),
            source.size,
            source.byte_sha256.as_str()
        ),
        ("3d-model", 0, sha256_hex(b"").as_str())
    );
    // ...but no CAD item revision or blob, which PocketBase could not store.
    assert!(import.blobs.iter().all(|blob| !blob.bytes.is_empty()));
    assert!(
        import
            .cad_item_revisions
            .iter()
            .all(|revision| revision.cad_item.kind != CadItemKind::Model3d)
    );
    let footprint = import
        .cad_item_revisions
        .iter()
        .find(|revision| revision.cad_item.kind == CadItemKind::Footprint)
        .unwrap();
    assert_eq!(
        footprint.snapshot.model_references[1].target,
        ModelTarget::Unresolved {
            status: "empty".into()
        }
    );
    let problem = import
        .problems
        .iter()
        .find(|problem| problem.code == "empty-3d-model-reference")
        .unwrap();
    assert_eq!(
        (
            problem.path.as_str(),
            problem.location.as_str(),
            problem.subject.as_str()
        ),
        (
            "footprints/g-Resistor_SMD.pretty/R_0603.kicad_mod",
            "model=${GITPLM_PARTS}/3d-models/local.step",
            "g-Resistor_SMD:R_0603"
        )
    );
    assert!(
        problem.message.contains("empty file"),
        "{}",
        problem.message
    );
}

#[test]
fn rows_that_cannot_become_parts_are_kept_as_import_problems() {
    let csv = format!(
        "{HEADER}res-0000-0001,M,Y,1,,,,\nRES-0000-0002,M,Y,2,,,,\nRES-0000-0002,M2,Y,2,,,,\n,,,,,,,\nRES-0000-0003,M,Y,3,,,,,extra\nRES-0000-0004,,Y,4,,,,\n"
    );
    let root = fixture(&csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let ipns: Vec<&str> = import.parts.iter().map(|p| p.ipn.as_str()).collect();
    assert_eq!(ipns, ["RES-0000-0002", "RES-0000-0004"]);
    let mut codes: Vec<&str> = import.problems.iter().map(|d| d.code.as_str()).collect();
    codes.sort();
    assert_eq!(
        codes,
        [
            "duplicate-ipn",
            "incomplete-main-manufacturer-part",
            "invalid-ipn",
            "invalid-ipn",
            "row-has-extra-fields"
        ]
    );
    // Each rejected row keeps its exact bytes.
    let duplicate = import
        .problems
        .iter()
        .find(|d| d.code == "duplicate-ipn")
        .unwrap();
    let detail: Value = serde_json::from_str(&duplicate.detail_json).unwrap();
    assert_eq!(detail["raw_record"], "RES-0000-0002,M2,Y,2,,,,\n");
    assert_eq!(detail["row_number"], 3);
    // A part without a complete main manufacturer part has none.
    assert!(import.parts[1].details.manufacturer_parts.is_empty());
}

#[test]
fn the_resistor_csv_file_imports_deterministically_without_writing() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let inputs = [
        "database/g-res.csv",
        "database/substitutes.csv",
        "symbols/g-res.kicad_sym",
        "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod",
    ];
    let before: Vec<_> = inputs
        .iter()
        .map(|path| sha256_hex(fs::read(root.join(path)).unwrap()))
        .collect();
    let catalog = CsvCatalog::new(&root);
    let first = catalog.import("g-res", None).unwrap();
    let second = catalog.import("g-res", None).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert_eq!(first.parts.len(), 29);
    assert_eq!(first.cad_item_revisions.len(), 2);
    assert_eq!(first.problems.len(), 1);
    assert_eq!(first.problems[0].code, "unresolved-3d-model-reference");
    let after: Vec<_> = inputs
        .iter()
        .map(|path| sha256_hex(fs::read(root.join(path)).unwrap()))
        .collect();
    assert_eq!(before, after);
}

#[test]
fn refuses_names_that_are_not_csv_catalog_files() {
    let root = fixture(HEADER, "IPN\n");
    let catalog = CsvCatalog::new(root.path());
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

#[test]
fn a_part_has_one_datasheet() {
    let csv = "IPN,MPN,Manufacturer,Datasheet\nRES-0000-0470,MPN47,Yageo,https://part\n";
    let substitutes_csv_rows = "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\nRES-0000-0470,A,Vishay,,,\nRES-0000-0470,B,Bourns,https://part,,\nRES-0000-0470,C,Panasonic,https://other,,\n";
    let root = fixture(csv, substitutes_csv_rows);
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let details = &import.parts[0].details;
    // Every substitutes.csv row is a manufacturer part; only the part keeps a
    // datasheet.
    let mpns: Vec<&str> = details
        .manufacturer_parts
        .iter()
        .map(|part| part.mpn.as_str())
        .collect();
    assert_eq!(mpns, ["MPN47", "A", "B", "C"]);
    assert_eq!(details.field("Datasheet"), Some("https://part"));
    // A blank datasheet or the part's own is fine; another is a problem,
    // and the row's source still keeps it.
    let problems: Vec<(&str, &str)> = import
        .problems
        .iter()
        .map(|problem| (problem.code.as_str(), problem.location.as_str()))
        .collect();
    assert_eq!(problems, [("substitutes-csv-datasheet-differs", "rows[3]")]);
    let kept = &import.parts[0].source.substitutes_csv_rows[2];
    assert_eq!(kept.record[3], "https://other");
}

#[test]
fn extra_mpns_and_mpns_to_be_confirmed_are_import_problems() {
    for (column, route) in [
        ("MPN", ColumnRoute::MainMpn),
        ("MPN2", ColumnRoute::ExtraMpn),
        ("MPN12", ColumnRoute::ExtraMpn),
        ("MPN TBC", ColumnRoute::MpnToBeConfirmed),
        ("MPN2x", ColumnRoute::UnknownMpn),
        ("MPN_", ColumnRoute::UnknownMpn),
        ("Value", ColumnRoute::PartDetails),
        ("Mouser_PN", ColumnRoute::SupplierData),
    ] {
        assert_eq!(route_column(column), route, "{column}");
    }
    let csv = "IPN,MPN,Manufacturer,MPN2,MPN TBC,Value\nRES-0000-0470,MPN47,Yageo,ALT47,\"X1, X2\",47\nRES-0000-1000,MPN100,Yageo,,,100\n";
    let root = fixture(csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    // Import problems are ordered by their ID.
    let problems: Vec<(&str, &str, &str)> = import
        .problems
        .iter()
        .map(|problem| {
            (
                problem.code.as_str(),
                problem.location.as_str(),
                problem.detail_json.as_str(),
            )
        })
        .collect();
    assert_eq!(
        problems,
        [
            (
                "mpn-without-manufacturer",
                "rows[IPN=RES-0000-0470].MPN2",
                r#"{"column":"MPN2","mpn":"ALT47","row_manufacturer":"Yageo"}"#
            ),
            (
                "mpn-to-be-confirmed",
                "rows[IPN=RES-0000-0470].MPN TBC",
                r#"{"column":"MPN TBC","value":"X1, X2"}"#
            ),
        ]
    );
    // Neither is a part-details field or a manufacturer part; blank cells
    // add nothing.
    for part in &import.parts {
        let names: Vec<&str> = part
            .details
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["Value"]);
        assert_eq!(part.details.manufacturer_parts.len(), 1);
    }
}

#[test]
fn excluded_from_bom_is_set_only_by_a_marked_symbol_or_footprint() {
    let csv = format!(
        "{HEADER}RES-0000-0001,M1,Yageo,1,g-res:TP,g-Resistor_SMD:R_0603,,\nRES-0000-0002,M2,Yageo,2,g-res:R_US,g-Resistor_SMD:NoBom,,\nRES-0000-0003,M3,Yageo,3,g-res:R_US,g-Resistor_SMD:R_0603,,\nRES-0000-0004,M4,Yageo,does not appear in BOM,Device:R,g-Resistor_SMD:R_0603,,\n"
    );
    let root = fixture(&csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let symbols = SYMBOLS.replace("\n)\n", "\n\t(symbol \"TP\"\n\t\t(in_bom no)\n\t)\n)\n");
    fs::write(root.path().join("symbols/g-res.kicad_sym"), symbols).unwrap();
    fs::write(
        root.path().join("footprints/g-Resistor_SMD.pretty/NoBom.kicad_mod"),
        "(footprint \"NoBom\"\n\t(version 20241229)\n\t(attr smd exclude_from_pos_files exclude_from_bom)\n)\n",
    )
    .unwrap();
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let excluded: Vec<(&str, bool)> = import
        .parts
        .iter()
        .map(|part| (part.ipn.as_str(), part.revision.excluded_from_bom))
        .collect();
    // A note in a field is never read for it (RES-0000-0004).
    assert_eq!(
        excluded,
        [
            ("RES-0000-0001", true),
            ("RES-0000-0002", true),
            ("RES-0000-0003", false),
            ("RES-0000-0004", false),
        ]
    );
    assert!(
        import.parts[0]
            .revision_json
            .ends_with(r#","excluded_from_bom":true}"#)
    );
    assert!(!import.parts[2].revision_json.contains("excluded_from_bom"));
    assert!(!import.parts[0].details_json.contains("excluded_from_bom"));
}

#[test]
fn footprint_choices_that_disagree_on_the_bom_are_an_import_problem() {
    let csv = format!(
        "{HEADER}RES-0000-0001,M1,Yageo,1,g-res:R_US,g-Resistor_SMD:NoBom;g-Resistor_SMD:R_0603,,\nRES-0000-0002,M2,Yageo,2,g-res:R_US,g-Resistor_SMD:NoBom;Resistor_SMD:R_0402,,\nRES-0000-0003,M3,Yageo,3,g-res:R_US,g-Resistor_SMD:NoBom;g-Resistor_SMD:NoBom2,,\n"
    );
    let root = fixture(&csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    for name in ["NoBom", "NoBom2"] {
        fs::write(
            root.path()
                .join(format!("footprints/g-Resistor_SMD.pretty/{name}.kicad_mod")),
            format!("(footprint \"{name}\"\n\t(attr smd exclude_from_bom)\n)\n"),
        )
        .unwrap();
    }
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let excluded: Vec<(&str, bool)> = import
        .parts
        .iter()
        .map(|part| (part.ipn.as_str(), part.revision.excluded_from_bom))
        .collect();
    // A choice that is not marked, or that cannot be read (KiCad's own
    // library), leaves the setting unset; all choices marked sets it.
    assert_eq!(
        excluded,
        [
            ("RES-0000-0001", false),
            ("RES-0000-0002", false),
            ("RES-0000-0003", true),
        ]
    );
    let disagree: Vec<(&str, &str)> = import
        .problems
        .iter()
        .filter(|problem| problem.code == "footprint-choices-disagree-on-bom")
        .map(|problem| (problem.subject.as_str(), problem.detail_json.as_str()))
        .collect();
    assert_eq!(disagree.len(), 2, "{disagree:?}");
    assert!(disagree.contains(&(
        "RES-0000-0001",
        r#"{"footprints":[{"exclude_from_bom":true,"reference":"g-Resistor_SMD:NoBom"},{"exclude_from_bom":false,"reference":"g-Resistor_SMD:R_0603"}]}"#
    )));
}

#[test]
fn unknown_mpn_columns_are_import_problems() {
    for column in ["MPN 2", "Alt MPN", "mpn2", "Mpn", "Supplier MPN"] {
        assert_eq!(route_column(column), ColumnRoute::UnknownMpn, "{column}");
    }
    let csv = "IPN,MPN,Manufacturer,Alt MPN,mpn2,Value\nRES-0000-0470,MPN47,Yageo,ALT47,,47\n";
    let root = fixture(csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let problems: Vec<(&str, &str, &str)> = import
        .problems
        .iter()
        .map(|problem| {
            (
                problem.code.as_str(),
                problem.location.as_str(),
                problem.detail_json.as_str(),
            )
        })
        .collect();
    // The blank mpn2 cell adds nothing.
    assert_eq!(
        problems,
        [(
            "unknown-mpn-column",
            "rows[IPN=RES-0000-0470].Alt MPN",
            r#"{"column":"Alt MPN","value":"ALT47"}"#
        )]
    );
    let names: Vec<&str> = import.parts[0]
        .details
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["Value"]);
}

/// The numbers `import` gives each CAD item, by key: (expected, number).
fn numbers(import: &CsvImport) -> Vec<(&str, u32, u32)> {
    import
        .cad_item_revisions
        .iter()
        .map(|draft| {
            (
                draft.cad_item.key.as_str(),
                draft.expected_revision,
                draft.number,
            )
        })
        .collect()
}

/// What `import` would leave as each CAD item's current revision.
fn current_after(import: &CsvImport) -> CurrentCad {
    // The content hash of each revision this import names, by snapshot hash.
    let contents: BTreeMap<String, String> = import
        .cad_item_revisions
        .iter()
        .map(|draft| {
            (
                draft.snapshot_sha256.clone(),
                draft.snapshot.content_sha256(),
            )
        })
        .collect();
    let binding = |cad_item: &str, number: u32, sha: &str| {
        (
            cad_item.to_owned(),
            Binding {
                number,
                snapshot_sha256: sha.to_owned(),
                content_sha256: contents.get(sha).cloned().unwrap_or_else(|| sha.to_owned()),
            },
        )
    };
    let cad_items = import
        .cad_item_revisions
        .iter()
        .map(|draft| {
            let models = draft
                .snapshot
                .model_references
                .iter()
                .filter_map(|model| match &model.target {
                    ModelTarget::CadItemRevision {
                        cad_item,
                        number,
                        snapshot_sha256,
                    } => Some(binding(cad_item, *number, snapshot_sha256)),
                    ModelTarget::Unresolved { .. } => None,
                })
                .collect();
            (
                draft.cad_item.id.as_str().to_owned(),
                CurrentCadItemRevision {
                    number: draft.number,
                    snapshot_sha256: draft.snapshot_sha256.clone(),
                    models,
                },
            )
        })
        .collect();
    let parts = import
        .parts
        .iter()
        .map(|part| {
            let bindings = part
                .revision
                .cad_references
                .iter()
                .filter_map(|reference| match &reference.target {
                    CadReferenceTarget::CadItemRevision {
                        cad_item,
                        number,
                        snapshot_sha256,
                    } => Some(binding(cad_item, *number, snapshot_sha256)),
                    _ => None,
                })
                .collect();
            (part.ipn.as_str().to_owned(), bindings)
        })
        .collect();
    CurrentCad {
        cad_items,
        parts,
        ..CurrentCad::default()
    }
}

#[test]
fn going_back_to_earlier_cad_content_is_numbered_as_a_new_revision() {
    let csv = format!("{HEADER}RES-0000-0470,MPN47,Yageo,47,g-res:R_US,g-Resistor_SMD:R_0603,,\n");
    let root = fixture(&csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let plan = || CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let footprint_target = |import: &CsvImport| {
        import.parts[0]
            .revision
            .cad_references
            .iter()
            .find(|reference| reference.role == CadReferenceRole::Footprint)
            .unwrap()
            .target
            .clone()
    };

    // A fresh import is numbered as if the catalog were empty.
    let first = plan();
    assert_eq!(first, first.numbered_against(&CurrentCad::default()));
    assert_eq!(
        numbers(&first),
        [
            ("g-res:R_US", 0, 1),
            ("3d-models/local.step", 0, 1),
            ("g-Resistor_SMD:R_0603", 0, 1)
        ]
    );
    let after_a = current_after(&first);
    // The same content again keeps every number and every snapshot, so
    // nothing is new; only the revisions it expects to find differ.
    let again = plan().numbered_against(&after_a);
    assert_eq!(
        numbers(&again),
        [
            ("g-res:R_US", 1, 1),
            ("3d-models/local.step", 1, 1),
            ("g-Resistor_SMD:R_0603", 1, 1)
        ]
    );
    assert!(again.parts[0].revision_json == first.parts[0].revision_json);

    // Footprint content B: revision 2. Then A again: revision 3, not 1.
    let path = root
        .path()
        .join("footprints/g-Resistor_SMD.pretty/R_0603.kicad_mod");
    fs::write(
        &path,
        FOOTPRINT.replace("R_0603\"\n", "R_0603\"\n\t(descr \"B\")\n"),
    )
    .unwrap();
    let second = plan().numbered_against(&after_a);
    assert_eq!(numbers(&second)[2], ("g-Resistor_SMD:R_0603", 1, 2));
    let after_b = current_after(&second);
    fs::write(&path, FOOTPRINT).unwrap();
    let third = plan().numbered_against(&after_b);
    assert_eq!(numbers(&third)[2], ("g-Resistor_SMD:R_0603", 2, 3));
    // Same content as revision 1, so the same blob, but a new revision. The
    // part bound revision 2 (other content), so it now binds revision 3.
    assert_eq!(
        third.cad_item_revisions[2].snapshot_sha256,
        first.cad_item_revisions[2].snapshot_sha256
    );
    assert_eq!(
        third.cad_item_revisions[2].id,
        CadItemRevisionId::new(&third.cad_item_revisions[2].cad_item.id, 3)
    );
    let CadReferenceTarget::CadItemRevision { number, .. } = footprint_target(&third) else {
        panic!("the footprint is bound");
    };
    assert_eq!(number, 3);
    assert_ne!(
        third.parts[0].revision_sha256,
        first.parts[0].revision_sha256
    );
    assert_eq!(
        third.parts[0].revision.canonical_json(),
        third.parts[0].revision_json
    );

    // A part that still binds revision 1 (it missed revision 2, say because
    // an edit kept it) keeps binding 1: the same content under a newer
    // number is not a change, so its details are exactly as they were.
    let mut missed = after_b.clone();
    missed.parts = after_a.parts.clone();
    let kept = plan().numbered_against(&missed);
    assert_eq!(numbers(&kept)[2], ("g-Resistor_SMD:R_0603", 2, 3));
    let CadReferenceTarget::CadItemRevision { number, .. } = footprint_target(&kept) else {
        panic!("the footprint is bound");
    };
    assert_eq!(number, 1);
    assert_eq!(kept.parts[0].revision_json, first.parts[0].revision_json);

    // A new 3D model revision makes a new footprint revision that names it.
    let after_third = current_after(&third);
    fs::write(
        root.path().join("3d-models/local.step"),
        "ISO-10303-21;\nB\n",
    )
    .unwrap();
    let fourth = plan().numbered_against(&after_third);
    assert_eq!(
        numbers(&fourth)[1..],
        [
            ("3d-models/local.step", 1, 2),
            ("g-Resistor_SMD:R_0603", 3, 4)
        ]
    );
    let model_number = |import: &CsvImport| {
        import.cad_item_revisions[2]
            .snapshot
            .model_references
            .iter()
            .find_map(|model| match model.target {
                ModelTarget::CadItemRevision { number, .. } => Some(number),
                ModelTarget::Unresolved { .. } => None,
            })
            .unwrap()
    };
    assert_eq!(model_number(&fourth), 2);
    // The model goes back to A: revision 3. The footprint bound model
    // revision 2 (other content), so it is new content, revision 5, naming
    // model revision 3.
    let after_fourth = current_after(&fourth);
    fs::write(root.path().join("3d-models/local.step"), "ISO-10303-21;\n").unwrap();
    let fifth = plan().numbered_against(&after_fourth);
    assert_eq!(
        numbers(&fifth)[1..],
        [
            ("3d-models/local.step", 2, 3),
            ("g-Resistor_SMD:R_0603", 4, 5)
        ]
    );
    assert_eq!(model_number(&fifth), 3);
    // A footprint that still binds model revision 1 keeps it, so it is
    // unchanged and keeps its number.
    let mut model_missed = after_fourth.clone();
    let footprint_id = fifth.cad_item_revisions[2].cad_item.id.as_str().to_owned();
    model_missed.cad_items.insert(
        footprint_id.clone(),
        after_third.cad_items[&footprint_id].clone(),
    );
    let unchanged = plan().numbered_against(&model_missed);
    assert_eq!(
        numbers(&unchanged)[1..],
        [
            ("3d-models/local.step", 2, 3),
            ("g-Resistor_SMD:R_0603", 3, 3)
        ]
    );
    assert_eq!(model_number(&unchanged), 1);
    // A part that still binds footprint revision 3 (model revision 1) keeps
    // it: revision 5 differs only in naming model revision 3, which has the
    // same content, so the CAD the part uses is the same.
    let mut part_missed = after_fourth.clone();
    part_missed.parts = after_third.parts.clone();
    let kept = plan().numbered_against(&part_missed);
    let CadReferenceTarget::CadItemRevision { number, .. } = footprint_target(&kept) else {
        panic!("the footprint is bound");
    };
    assert_eq!(number, 3);
    assert_eq!(kept.parts[0].revision_json, third.parts[0].revision_json);
}

#[test]
fn a_revision_keeps_symbol_before_footprint_whatever_the_column_order() {
    let csv = "IPN,Footprint,Value,Symbol\nRES-0000-0470,g-Resistor_SMD:R_0603,47,g-res:R_US\n";
    let root = fixture(csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let import = CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let revision = &import.parts[0].revision;
    let fields: Vec<(&str, &str)> = revision
        .cad_fields
        .iter()
        .map(|field| (field.name.as_str(), field.value.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            ("Symbol", "g-res:R_US"),
            ("Footprint", "g-Resistor_SMD:R_0603")
        ]
    );
    // The references follow the same order: symbol, then footprint.
    let roles: Vec<CadReferenceRole> = revision
        .cad_references
        .iter()
        .map(|reference| reference.role)
        .collect();
    assert_eq!(
        roles,
        [CadReferenceRole::Symbol, CadReferenceRole::Footprint]
    );
    // The details keep their own columns in source order.
    assert_eq!(import.parts[0].details.field("Value"), Some("47"));
}

#[test]
fn an_import_never_replaces_a_cad_item_revision_made_in_the_app() {
    let csv = format!(
        "{HEADER}RES-0000-0470,MPN47,Yageo,47,g-res:R_US,g-Resistor_SMD:R_0603,,\nRES-0000-1000,MPN100,Yageo,100,g-res:R_US,g-Resistor_SMD:R_0603,,\n"
    );
    let root = fixture(&csv, "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n");
    let plan = || CsvCatalog::new(root.path()).import("g-res", None).unwrap();
    let first = plan();
    let mut catalog = current_after(&first);
    let footprint = first
        .cad_item_revisions
        .iter()
        .find(|draft| draft.cad_item.kind == CadItemKind::Footprint)
        .unwrap()
        .clone();
    let id = footprint.cad_item.id.as_str().to_owned();
    // Revision 2 of the footprint was made in the app, for the first part.
    let imported = catalog.cad_items[&id].clone();
    let last_imported = LastImported {
        binding: Binding {
            number: 1,
            snapshot_sha256: imported.snapshot_sha256.clone(),
            content_sha256: footprint.snapshot.content_sha256(),
        },
        models: imported.models.clone(),
    };
    let app = Binding {
        number: 2,
        snapshot_sha256: "a".repeat(64),
        content_sha256: "b".repeat(64),
    };
    catalog.cad_items.insert(
        id.clone(),
        CurrentCadItemRevision {
            number: 2,
            snapshot_sha256: app.snapshot_sha256.clone(),
            models: BTreeMap::new(),
        },
    );
    catalog
        .parts
        .get_mut("RES-0000-0470")
        .unwrap()
        .insert(id.clone(), app.clone());
    catalog.edited_in_app.insert(
        id.clone(),
        EditedInApp {
            current: app,
            last_imported: Some(last_imported),
        },
    );
    let footprint_number = |import: &CsvImport, part: usize| {
        import.parts[part]
            .revision
            .cad_references
            .iter()
            .find_map(|reference| match &reference.target {
                CadReferenceTarget::CadItemRevision {
                    cad_item, number, ..
                } if *cad_item == id => Some(*number),
                _ => None,
            })
            .unwrap()
    };

    // The CSV's file is what the import last applied: the footprint is not
    // sent, both rows name revision 1, and nothing is a problem.
    let same = plan().numbered_against(&catalog);
    assert!(same.cad_items().all(|item| item.id.as_str() != id));
    assert!(
        same.blobs
            .iter()
            .all(|blob| blob.sha256 != footprint.snapshot.blob_sha256)
    );
    assert_eq!(footprint_number(&same, 0), 1);
    assert_eq!(footprint_number(&same, 1), 1);
    assert_eq!(same.parts[1].revision_json, first.parts[1].revision_json);
    assert!(
        same.problems
            .iter()
            .all(|problem| problem.code != CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM)
    );

    // The CSV's file changes: still not sent, and each row that uses it
    // keeps a problem with the CSV's own snapshot.
    fs::write(
        root.path()
            .join("footprints/g-Resistor_SMD.pretty/R_0603.kicad_mod"),
        FOOTPRINT.replace("R_0603\"\n", "R_0603\"\n\t(descr \"B\")\n"),
    )
    .unwrap();
    let fresh = plan();
    let changed = fresh.numbered_against(&catalog);
    assert!(changed.cad_items().all(|item| item.id.as_str() != id));
    assert_eq!(footprint_number(&changed, 1), 1);
    let problems: Vec<&ImportProblem> = changed
        .problems
        .iter()
        .filter(|problem| problem.code == CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM)
        .collect();
    let subjects: Vec<&str> = problems.iter().map(|p| p.subject.as_str()).collect();
    assert_eq!(subjects.len(), 2);
    assert!(subjects.contains(&"RES-0000-0470") && subjects.contains(&"RES-0000-1000"));
    let detail: Value = serde_json::from_str(&problems[0].detail_json).unwrap();
    let csv_snapshot = fresh
        .cad_item_revisions
        .iter()
        .find(|draft| draft.cad_item.id.as_str() == id)
        .unwrap()
        .snapshot_json
        .clone();
    assert_eq!(detail["csv_cad_item"], csv_snapshot);
    assert_eq!(detail["kept_revision"], 1);
    assert_eq!(detail["edited_revision"], 2);
    assert_eq!(detail["role"], "footprint");
}

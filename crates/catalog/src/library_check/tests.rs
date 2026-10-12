use super::read::{check_folder, read_csv_references, read_libraries};
use super::*;
use crate::csv_import::CsvCatalog;
use crate::hash::sha256_hex;
use std::fs;
use std::path::Path;

/// Writes `files` (repository path, text) under a new temporary folder.
fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let target = root.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }
    root
}

fn symbol(name: &str, footprint: &str) -> String {
    format!(
        "\t(symbol \"{name}\"\n\t\t(property \"Reference\" \"U\" (at 0 0 0))\n\t\t(property \"Footprint\" \"{footprint}\" (at 0 0 0))\n\t)\n"
    )
}

fn library(symbols: &[String]) -> String {
    format!(
        "(kicad_symbol_lib\n\t(version 20241209)\n{})\n",
        symbols.concat()
    )
}

fn footprint(name: &str, models: &[&str]) -> String {
    let mut text = format!("(footprint \"{name}\"\n\t(version 20241229)\n");
    for model in models {
        text.push_str(&format!(
            "\t(model \"{model}\"\n\t\t(offset (xyz 0 0 0))\n\t)\n"
        ));
    }
    text.push_str(")\n");
    text
}

const CSV_HEADER: &str = "IPN,MPN,Manufacturer,Description,Symbol,Footprint\n";

/// What the Catalog has after importing `root`'s CSV files as they are now.
fn imported(root: &Path) -> KnownCad {
    let catalog = CsvCatalog::new(root);
    let mut items: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut imported_csv_files = BTreeSet::new();
    for entry in fs::read_dir(root.join("database")).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(csv_file) = name.strip_suffix(".csv").filter(|n| n.starts_with("g-")) else {
            continue;
        };
        let import = catalog.import(csv_file, None).unwrap();
        imported_csv_files.insert(format!("database/{name}"));
        for revision in &import.cad_item_revisions {
            items
                .entry(revision.cad_item.id.as_str().to_owned())
                .or_default()
                .insert(revision.snapshot.blob_sha256.clone());
        }
    }
    KnownCad {
        items,
        imported_csv_files,
    }
}

fn kinds(check: &LibraryCheck) -> Vec<(Group, &'static str, &str)> {
    check
        .findings
        .iter()
        .map(|finding| {
            (
                finding.group(),
                finding.kind.as_str(),
                finding.item.as_str(),
            )
        })
        .collect()
}

const OWN: &str = "${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/SOIC-8.step";

/// A CSV catalog with one part using `g-ics:TP4333` (missing; the library
/// has IC_TP4333) and the footprint SOIC-8, whose 3D model is in place.
fn small_catalog(extra: &[(&str, &str)]) -> tempfile::TempDir {
    let symbols = library(&[
        symbol("IC_TP4333", "g-Package_SO:SOIC-8"),
        symbol("LM358", "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm"),
        symbol("ODD", "g-Package_SO:NOPE"),
    ]);
    let soic = footprint("SOIC-8", &[OWN]);
    let csv = format!(
        "{CSV_HEADER}ICS-0001-0001,TP4333,Top,Charger,g-ics:TP4333,g-Package_SO:SOIC-8\nICS-0002-0001,LM358,TI,Op-amp,g-ics:LM358,g-Package_SO:SOIC-8\n"
    );
    let mut files: Vec<(&str, &str)> = vec![
        ("database/g-ics.csv", &csv),
        (
            "database/substitutes.csv",
            "IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN\n",
        ),
        ("symbols/g-ics.kicad_sym", &symbols),
        ("footprints/g-Package_SO.pretty/SOIC-8.kicad_mod", &soic),
        (
            "3d-models/g-Package_SO.3dshapes/SOIC-8.step",
            "ISO-10303-21;\n",
        ),
    ];
    files.extend_from_slice(extra);
    folder(&files)
}

#[test]
fn a_library_in_order_finds_only_what_the_catalog_lacks() {
    let root = small_catalog(&[]);
    let known = imported(root.path());
    let kicad: BTreeSet<String> = ["Package_SO:SOIC-8_3.9x4.9mm_P1.27mm".to_owned()].into();
    let libraries = read_libraries(root.path());
    let csv = read_csv_references(root.path()).unwrap();
    let found = check(CheckInputs {
        libraries: &libraries,
        known: &known,
        csv: &csv,
        kicad_footprints: Some(&kicad),
    });
    assert_eq!(
        kinds(&found),
        [
            (Group::NotInCatalog, "not-in-catalog", "g-ics:IC_TP4333"),
            (Group::NotInCatalog, "not-in-catalog", "g-ics:ODD"),
            (Group::WorthALook, "similar-name", "g-ics:TP4333"),
            (Group::WorthALook, "symbol-footprint", "g-ics:ODD"),
        ]
    );
    let similar = &found.findings[2];
    assert_eq!(similar.file, "database/g-ics.csv");
    assert_eq!(
        similar.message,
        "database/g-ics.csv names the symbol g-ics:TP4333, which symbols/g-ics.kicad_sym does not have. Did you mean g-ics:IC_TP4333?"
    );
    assert_eq!(
        found.findings[0].message,
        "No part uses it, and the Catalog does not have it yet."
    );
    assert_eq!(
        (found.findings[0].library(), found.findings[0].name()),
        ("g-ics", "IC_TP4333")
    );
    assert!(
        found.findings[3].message.starts_with(
            "The symbol's own Footprint, g-Package_SO:NOPE, is not in our libraries or KiCad's."
        ),
        "{}",
        found.findings[3].message
    );
    // Without KiCad's libraries, a KiCad footprint is not checked; with
    // KiCad's libraries lacking it, it is named.
    let empty = BTreeSet::new();
    let without_lm358 = check(CheckInputs {
        kicad_footprints: Some(&empty),
        libraries: &libraries,
        known: &known,
        csv: &csv,
    });
    assert!(
        without_lm358
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::SymbolFootprint && f.item == "g-ics:LM358")
    );
    // The same inputs give the same findings in the same order.
    assert_eq!(
        check_folder(root.path(), &known, None).unwrap(),
        check(CheckInputs {
            kicad_footprints: None,
            libraries: &libraries,
            known: &known,
            csv: &csv,
        })
    );
}

#[test]
fn every_kind_of_3d_model_link_is_judged() {
    let links = [
        (
            "Own",
            "${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/Own.step",
        ),
        (
            "Shared",
            "${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/SOIC-8.step",
        ),
        (
            "Missing",
            "${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/Missing.step",
        ),
        (
            "Kicad",
            "${KICAD9_3DMODEL_DIR}/Package_SO.3dshapes/SOIC-8_3.9x4.9mm_P1.27mm.step",
        ),
        ("Embedded", "kicad-embed://Embedded.step"),
        (
            "Home",
            "/home/jeffs/Workspace/Cliff/3d-models/DFN-12_FTD.step",
        ),
        ("Drive", "C:/Users/me/models/x.step"),
        ("Relative", "models/x.step"),
        ("Variable", "${GITPLM_3DMODELS}/x.step"),
        ("Project", "${KIPRJMOD}/3D/x.stp"),
        ("Escape", "${GITPLM_PARTS}/3d-models/../symbols/x.step"),
    ];
    let mut files: Vec<(String, String)> = Vec::new();
    let mut csv = CSV_HEADER.to_owned();
    for (index, (name, link)) in links.iter().enumerate() {
        files.push((
            format!("footprints/g-Package_SO.pretty/{name}.kicad_mod"),
            footprint(name, &[link]),
        ));
        csv.push_str(&format!(
            "ICS-{:04}-0001,M{index},Maker,Part,,g-Package_SO:{name}\n",
            index + 10
        ));
    }
    files.push(("database/g-ics.csv".into(), csv));
    files.push((
        "3d-models/g-Package_SO.3dshapes/Own.step".into(),
        "ISO-10303-21;
"
        .into(),
    ));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let root = small_catalog(&borrowed);
    // Nothing imported yet: the first import takes the library as it is.
    let first = check_folder(root.path(), &KnownCad::default(), None).unwrap();
    assert_eq!(first.count(Group::ToFix), 0, "{}", first.report());
    assert!(
        first.in_group(Group::CleanUp).all(|f| f
            .message
            .ends_with("It was like this before the first import.")),
        "{}",
        first.report()
    );
    assert_eq!(first.count(Group::CleanUp), 7, "{}", first.report());
    // g-ics.csv was imported before, and none of these footprints is in
    // the Catalog: each is new, so a problem is an error.
    let after_an_import = KnownCad {
        items: BTreeMap::new(),
        imported_csv_files: ["database/g-ics.csv".to_owned()].into(),
    };
    let found = check_folder(root.path(), &after_an_import, None).unwrap();
    let models: Vec<(Severity, &str, &str)> = found
        .findings
        .iter()
        .filter(|f| f.file.starts_with("footprints/") && f.kind != FindingKind::NotInCatalog)
        .map(|f| (f.severity, f.kind.as_str(), f.name()))
        .collect();
    assert_eq!(
        models,
        [
            (Severity::Error, "model-outside", "Drive"),
            (Severity::Error, "model-outside", "Escape"),
            (Severity::Error, "model-outside", "Home"),
            (Severity::Error, "missing-model", "Missing"),
            (Severity::Error, "model-outside", "Project"),
            (Severity::Error, "model-outside", "Relative"),
            (Severity::Error, "model-outside", "Variable"),
            (Severity::SoftWarning, "shared-model", "Shared"),
        ]
    );
    let home = found.findings.iter().find(|f| f.name() == "Home").unwrap();
    assert_eq!(
        home.message,
        "The footprint links the 3D model /home/jeffs/Workspace/Cliff/3d-models/DFN-12_FTD.step, which is outside our library, so it only works on one computer. Put the file in 3d-models/g-Package_SO.3dshapes/ and link it as ${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/Home.step. A CSV row uses the footprint, so the next import would add it."
    );
    assert_eq!(home.file, "footprints/g-Package_SO.pretty/Home.kicad_mod");
    // An error in a file an import reads stops it; one elsewhere does not.
    let files: BTreeSet<&str> = ["footprints/g-Package_SO.pretty/Home.kicad_mod"].into();
    assert_eq!(found.errors_in(&files), [home]);
    assert!(found.errors_in(&BTreeSet::new()).is_empty());
}

#[test]
fn only_new_or_changed_footprints_have_errors() {
    let home = "/home/jeffs/model.step";
    let root = small_catalog(&[
        (
            "footprints/g-Package_SO.pretty/Old.kicad_mod",
            &footprint("Old", &[home]),
        ),
        (
            "footprints/g-Package_SO.pretty/Unused.kicad_mod",
            &footprint("Unused", &[home]),
        ),
        (
            "database/g-old.csv",
            &format!("{CSV_HEADER}OLD-0001-0001,X,Y,Old part,,g-Package_SO:Old\n"),
        ),
    ]);
    let known = imported(root.path());
    let unchanged = check_folder(root.path(), &known, None).unwrap();
    let severity_of = |check: &LibraryCheck, name: &str| {
        check
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::ModelOutside && f.name() == name)
            .map(|f| f.severity)
    };
    assert_eq!(severity_of(&unchanged, "Old"), Some(Severity::CleanUp));
    assert!(unchanged.findings.iter().any(|f| {
        f.name() == "Old"
            && f.message
                .ends_with("The Catalog already has the footprint like this.")
    }));
    // A footprint no part uses and the Catalog lacks: listed, with its
    // problem as a warning until something brings it in.
    assert_eq!(severity_of(&unchanged, "Unused"), Some(Severity::Warning));
    assert!(
        unchanged
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::NotInCatalog && f.item == "g-Package_SO:Unused")
    );
    assert_eq!(unchanged.count(Group::ToFix), 0);

    // Changed in KiCad since the import: an error.
    fs::write(
        root.path()
            .join("footprints/g-Package_SO.pretty/Old.kicad_mod"),
        footprint("Old", &[home, "C:/x.step"]),
    )
    .unwrap();
    let changed = check_folder(root.path(), &known, None).unwrap();
    assert_eq!(severity_of(&changed, "Old"), Some(Severity::Error));
    assert_eq!(changed.count(Group::ToFix), 2, "{}", changed.report());

    // A CSV file never imported uses the unused footprint: its first
    // import takes it as it is, so its problem is clean-up.
    fs::write(
        root.path().join("database/g-new.csv"),
        format!("{CSV_HEADER}NEW-0001-0001,X,Y,New part,,g-Package_SO:Unused\n"),
    )
    .unwrap();
    let first_import = check_folder(root.path(), &known, None).unwrap();
    assert_eq!(
        severity_of(&first_import, "Unused"),
        Some(Severity::CleanUp)
    );
    fs::remove_file(root.path().join("database/g-new.csv")).unwrap();

    // A row of a CSV file imported before now uses it: the next import
    // would bring it in, so its problem is an error.
    fs::write(
        root.path().join("database/g-old.csv"),
        format!("{CSV_HEADER}OLD-0001-0001,X,Y,Old part,,g-Package_SO:Old;g-Package_SO:Unused\n"),
    )
    .unwrap();
    let used = check_folder(root.path(), &known, None).unwrap();
    assert_eq!(severity_of(&used, "Unused"), Some(Severity::Error));
    assert!(
        !used
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::NotInCatalog && f.item == "g-Package_SO:Unused")
    );
}

#[test]
fn a_model_no_footprint_links_is_not_in_the_catalog_yet() {
    let root = small_catalog(&[(
        "3d-models/g-Package_SO.3dshapes/Lonely.step",
        "ISO-10303-21;\n",
    )]);
    let found = check_folder(root.path(), &imported(root.path()), None).unwrap();
    let lonely = found
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::NotInCatalog && f.item.ends_with("Lonely.step"))
        .unwrap();
    assert_eq!(lonely.item, "3d-models/g-Package_SO.3dshapes/Lonely.step");
    assert_eq!(
        (lonely.library(), lonely.name()),
        ("g-Package_SO.3dshapes", "Lonely.step")
    );
    assert_eq!(
        lonely.message,
        "No footprint uses it, and the Catalog does not have it yet."
    );
}

#[test]
fn similar_names_contain_each_other_or_are_a_few_edits_apart() {
    let names = ["IC_TP4333", "TP4056", "LM358", "R"];
    let similar = |name: &str| similar_name(name, names.iter().copied());
    assert_eq!(similar("TP4333"), Some("IC_TP4333"));
    assert_eq!(similar("LM385"), Some("LM358"));
    assert_eq!(similar("lm358"), Some("LM358"));
    assert_eq!(similar("TP4057"), Some("TP4056"));
    assert_eq!(similar("XYZ123"), None);
    // A short name alone is no clue.
    assert_eq!(similar_name("RX", ["R_US_LONG"].into_iter()), None);
}

#[test]
fn a_chosen_footprint_with_a_link_that_would_stop_it_is_refused_plainly() {
    let links = [
        OWN.to_owned(),
        "${KICAD10_3DMODEL_DIR}/Package_SO.3dshapes/SOIC-8.step".to_owned(),
        "${KIPRJMOD}/3D/SOIC-8.stp".to_owned(),
    ];
    let problems = chosen_footprint_problems("g-Package_SO", "SOIC-8", &links, |_| true);
    assert_eq!(
        problems,
        [
            "The footprint's 3D model link ${KIPRJMOD}/3D/SOIC-8.stp is outside the Catalog's library, so it only works on one computer. Link the model as ${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/SOIC-8.step in KiCad's Footprint Editor, or remove the link, then choose the file again."
        ]
    );
    let missing = chosen_footprint_problems("g-Package_SO", "SOIC-8", &links[..1], |_| false);
    assert_eq!(
        missing,
        [
            "The footprint links the 3D model 3d-models/g-Package_SO.3dshapes/SOIC-8.step, which the Catalog does not have. Choose that 3D model file too, or fix the link in KiCad's Footprint Editor."
        ]
    );
    let elsewhere = ["${GITPLM_PARTS}/3d-models/SOIC-8.step".to_owned()];
    assert_eq!(
        chosen_footprint_problems("g-Package_SO", "SOIC-8", &elsewhere, |_| false),
        [
            "The footprint links the 3D model 3d-models/SOIC-8.step, a place where the Catalog keeps no 3D models. Link the model as ${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/SOIC-8.step in KiCad's Footprint Editor, then choose the file again."
        ]
    );
}

#[test]
fn the_report_lists_errors_then_warnings_then_new_items_then_counts_clean_up() {
    let finding = |severity, kind, item: &str| Finding {
        severity,
        kind,
        item: item.to_owned(),
        file: format!("file of {item}"),
        message: format!("about {item}"),
    };
    let check = LibraryCheck {
        findings: vec![
            finding(Severity::Error, FindingKind::ModelOutside, "g-a:Bad"),
            finding(Severity::Warning, FindingKind::NotInCatalog, "g-a:New"),
            finding(
                Severity::SoftWarning,
                FindingKind::SharedModel,
                "g-a:Shared",
            ),
            finding(Severity::CleanUp, FindingKind::MissingModel, "g-a:Old"),
        ],
    };
    assert_eq!(
        check.report(),
        "Library check: 1 to fix, 1 not in the Catalog yet, 1 worth a look, 1 to clean up.\nTo fix:\n  file of g-a:Bad: about g-a:Bad\nWorth a look:\n  file of g-a:Shared: about g-a:Shared\nNot in the Catalog yet:\n  g-a:New (file of g-a:New)\nClean-up: 1 old problem that was there before this import; the desktop app lists them."
    );
    assert_eq!(
        LibraryCheck::default().summary(),
        "Library check: nothing found."
    );
}

/// The repository's CSV catalog, read only.
fn repository() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Today's CSV catalog has no errors, imported or not, so imports keep
/// working: its old problems are clean-up. (Run with `--nocapture` to see
/// the counts.)
#[test]
fn todays_csv_catalog_has_nothing_to_fix() {
    let root = repository();
    let all_imported = imported(root);
    for (what, known) in [
        ("before the first import", KnownCad::default()),
        ("after importing every CSV file", all_imported.clone()),
    ] {
        let found = check_folder(root, &known, None).unwrap();
        println!("{what}:\n{}\n", found.report());
        let mut counts: BTreeMap<(Group, &str), usize> = BTreeMap::new();
        for finding in &found.findings {
            *counts
                .entry((finding.group(), finding.kind.as_str()))
                .or_default() += 1;
        }
        println!("{what}: {counts:?}");
        assert_eq!(found.count(Group::ToFix), 0, "{what}: {}", found.report());
    }
    // With KiCad's own libraries, when installed, symbols naming one of
    // KiCad's footprints are checked too.
    let share = kicad_integration::installed_share_dir(&Default::default());
    if let Some(share) = share {
        let found = check_folder(root, &all_imported, Some(&share)).unwrap();
        let named: Vec<String> = found
            .findings
            .iter()
            .filter(|f| f.kind == FindingKind::SymbolFootprint)
            .map(|f| format!("{}: {}", f.item, f.message))
            .collect();
        println!("with KiCad's libraries at {}: {named:#?}", share.display());
        assert_eq!(found.count(Group::ToFix), 0, "{}", found.report());
    }
    let found = check_folder(root, &all_imported, None).unwrap();
    // g-ics.csv names g-ics:TP4333, while the library has IC_TP4333.
    assert!(
        found
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::SimilarName
                && f.item == "g-ics:TP4333"
                && f.message.ends_with("Did you mean g-ics:IC_TP4333?"))
    );
    // The three footprints linking a model in someone's home folder are
    // used by no part, so they wait in Worth a look.
    let home: Vec<&str> = found
        .findings
        .iter()
        .filter(|f| f.message.contains("/home/jeffs/"))
        .map(|f| f.name())
        .collect();
    assert_eq!(home, ["DFN-12_FTD-L", "DFN-12_FTD-M", "DFN-12_FTD"]);
    // Footprints are hashed whole, like the import's blobs.
    let libraries = read_libraries(root);
    let r0603 = libraries
        .footprints
        .iter()
        .find(|f| f.name == "R_0603_1608Metric")
        .unwrap();
    assert_eq!(
        r0603.sha256,
        sha256_hex(
            fs::read(root.join("footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod"))
                .unwrap()
        )
    );
}

use super::read::{check_folder, read_libraries};
use super::*;
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

/// What the Catalog has: each CAD item of `items` (kind, reference and the
/// repository file it was saved from), with that file's bytes under `root`
/// as they are now.
fn known(root: &Path, items: &[(CadItemKind, &str, &str)]) -> KnownCad {
    KnownCad {
        items: items
            .iter()
            .map(|(kind, reference, file)| {
                let bytes = fs::read(root.join(file)).unwrap();
                (
                    CadItemId::new(*kind, reference).as_str().to_owned(),
                    sha256_hex(bytes),
                )
            })
            .collect(),
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
const SOIC: &str = "footprints/g-Package_SO.pretty/SOIC-8.kicad_mod";
const SOIC_MODEL: &str = "3d-models/g-Package_SO.3dshapes/SOIC-8.step";

/// Our libraries with three symbols and the footprint SOIC-8, whose 3D
/// model is in place.
fn small_library(extra: &[(&str, &str)]) -> tempfile::TempDir {
    let symbols = library(&[
        symbol("IC_TP4333", "g-Package_SO:SOIC-8"),
        symbol("LM358", "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm"),
        symbol("ODD", "g-Package_SO:NOPE"),
    ]);
    let soic = footprint("SOIC-8", &[OWN]);
    let mut files: Vec<(&str, &str)> = vec![
        ("symbols/g-ics.kicad_sym", &symbols),
        (SOIC, &soic),
        (SOIC_MODEL, "ISO-10303-21;\n"),
    ];
    files.extend_from_slice(extra);
    folder(&files)
}

/// The Catalog of [`small_library`]: one part using g-ics:LM358 and
/// g-Package_SO:SOIC-8 with its 3D model.
fn small_catalog(root: &Path) -> KnownCad {
    known(
        root,
        &[
            (
                CadItemKind::Symbol,
                "g-ics:LM358",
                "symbols/g-ics.kicad_sym",
            ),
            (CadItemKind::Footprint, "g-Package_SO:SOIC-8", SOIC),
            (CadItemKind::Model3d, SOIC_MODEL, SOIC_MODEL),
        ],
    )
}

#[test]
fn a_library_in_order_finds_only_what_the_catalog_lacks() {
    let root = small_library(&[]);
    let known = small_catalog(root.path());
    let kicad: BTreeSet<String> = ["Package_SO:SOIC-8_3.9x4.9mm_P1.27mm".to_owned()].into();
    let libraries = read_libraries(root.path());
    let found = check(CheckInputs {
        libraries: &libraries,
        known: &known,
        kicad_footprints: Some(&kicad),
    });
    assert_eq!(
        kinds(&found),
        [
            (Group::NotInCatalog, "not-in-catalog", "g-ics:IC_TP4333"),
            (Group::NotInCatalog, "not-in-catalog", "g-ics:ODD"),
            (Group::WorthALook, "symbol-footprint", "g-ics:ODD"),
        ]
    );
    assert_eq!(
        found.findings[0].message,
        "No part uses it yet: the Catalog does not have it."
    );
    assert_eq!(
        (found.findings[0].library(), found.findings[0].name()),
        ("g-ics", "IC_TP4333")
    );
    assert!(
        found.findings[2].message.starts_with(
            "The symbol's own Footprint, g-Package_SO:NOPE, is not in our libraries or KiCad's."
        ),
        "{}",
        found.findings[2].message
    );
    // Without KiCad's libraries, a KiCad footprint is not checked; with
    // KiCad's libraries lacking it, it is named.
    let empty = BTreeSet::new();
    let without_lm358 = check(CheckInputs {
        kicad_footprints: Some(&empty),
        libraries: &libraries,
        known: &known,
    });
    assert!(
        without_lm358
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::SymbolFootprint && f.item == "g-ics:LM358")
    );
    // The same inputs give the same findings in the same order.
    assert_eq!(
        check_folder(root.path(), &known, None),
        check(CheckInputs {
            kicad_footprints: None,
            libraries: &libraries,
            known: &known,
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
    for (name, link) in &links {
        files.push((
            format!("footprints/g-Package_SO.pretty/{name}.kicad_mod"),
            footprint(name, &[link]),
        ));
    }
    files.push((
        "3d-models/g-Package_SO.3dshapes/Own.step".into(),
        "ISO-10303-21;\n".into(),
    ));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let root = small_library(&borrowed);
    // By footprint name: the groups put them in another order.
    let judged = |check: &LibraryCheck| -> Vec<(Severity, &'static str, String)> {
        let mut judged: Vec<_> = check
            .findings
            .iter()
            .filter(|f| f.file.starts_with("footprints/") && f.kind != FindingKind::NotInCatalog)
            .map(|f| (f.severity, f.kind.as_str(), f.name().to_owned()))
            .collect();
        judged.sort_by(|a, b| a.2.cmp(&b.2));
        judged
    };
    let expected = |severity: Severity| {
        [
            (severity, "model-outside", "Drive"),
            (severity, "model-outside", "Escape"),
            (severity, "model-outside", "Home"),
            (severity, "missing-model", "Missing"),
            (severity, "model-outside", "Project"),
            (severity, "model-outside", "Relative"),
            (Severity::SoftWarning, "shared-model", "Shared"),
            (severity, "model-outside", "Variable"),
        ]
        .map(|(severity, kind, name)| (severity, kind, name.to_owned()))
    };

    // The Catalog has none of them: no part uses them yet, so each
    // problem is a warning.
    let unused = check_folder(root.path(), &KnownCad::default(), None);
    assert_eq!(judged(&unused), expected(Severity::Warning));
    let home_of = |check: &LibraryCheck| {
        check
            .findings
            .iter()
            .find(|f| f.name() == "Home" && f.kind == FindingKind::ModelOutside)
            .cloned()
            .unwrap()
    };
    let home = home_of(&unused);
    assert_eq!(
        home.message,
        "The footprint links the 3D model /home/jeffs/Workspace/Cliff/3d-models/DFN-12_FTD.step, which is outside our library, so it only works on one computer. Put the file in 3d-models/g-Package_SO.3dshapes/ and link it as ${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/Home.step. Fix it before a part uses this footprint."
    );
    assert_eq!(home.file, "footprints/g-Package_SO.pretty/Home.kicad_mod");

    // The Catalog has them as they are: their problems are clean-up.
    let items: Vec<(String, String)> = links
        .iter()
        .map(|(name, _)| {
            (
                format!("g-Package_SO:{name}"),
                format!("footprints/g-Package_SO.pretty/{name}.kicad_mod"),
            )
        })
        .collect();
    let catalog: Vec<(CadItemKind, &str, &str)> = items
        .iter()
        .map(|(reference, file)| (CadItemKind::Footprint, reference.as_str(), file.as_str()))
        .collect();
    let unchanged = check_folder(root.path(), &known(root.path(), &catalog), None);
    assert_eq!(judged(&unchanged), expected(Severity::CleanUp));

    // The Catalog has them with other bytes: adding one to a part would
    // bring its problem in, so each is an error.
    let mut changed = known(root.path(), &catalog);
    for bytes in changed.items.values_mut() {
        *bytes = sha256_hex("an older revision");
    }
    let found = check_folder(root.path(), &changed, None);
    assert_eq!(judged(&found), expected(Severity::Error));
    let home = home_of(&found);
    assert!(
        home.message
            .ends_with("The footprint differs from the Catalog's."),
        "{}",
        home.message
    );
}

#[test]
fn only_changed_footprints_have_errors() {
    let home = "/home/jeffs/model.step";
    let old = "footprints/g-Package_SO.pretty/Old.kicad_mod";
    let root = small_library(&[
        (old, &footprint("Old", &[home])),
        (
            "footprints/g-Package_SO.pretty/Unused.kicad_mod",
            &footprint("Unused", &[home]),
        ),
    ]);
    let catalog = known(
        root.path(),
        &[(CadItemKind::Footprint, "g-Package_SO:Old", old)],
    );
    let unchanged = check_folder(root.path(), &catalog, None);
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
    // A footprint the Catalog lacks: listed, with its problem as a warning
    // until a part uses it.
    assert_eq!(severity_of(&unchanged, "Unused"), Some(Severity::Warning));
    assert!(
        unchanged
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::NotInCatalog && f.item == "g-Package_SO:Unused")
    );
    assert_eq!(unchanged.count(Group::ToFix), 0);

    // Changed in KiCad since the Catalog got it: an error.
    fs::write(
        root.path().join(old),
        footprint("Old", &[home, "C:/x.step"]),
    )
    .unwrap();
    let changed = check_folder(root.path(), &catalog, None);
    assert_eq!(severity_of(&changed, "Old"), Some(Severity::Error));
    assert_eq!(changed.count(Group::ToFix), 2, "{:?}", changed.findings);
}

#[test]
fn a_model_no_footprint_links_is_not_in_the_catalog_yet() {
    let root = small_library(&[(
        "3d-models/g-Package_SO.3dshapes/Lonely.step",
        "ISO-10303-21;\n",
    )]);
    let found = check_folder(root.path(), &small_catalog(root.path()), None);
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
        "No footprint uses it yet: the Catalog does not have it."
    );
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
fn a_kicad_model_link_climbing_out_of_kicads_folder_is_outside() {
    for link in [
        "${KICAD9_3DMODEL_DIR}/../../me/x.step",
        r"${KICAD10_3DMODEL_DIR}\..\me\x.step",
        "${KICAD_3DMODEL_DIR}/x.step",
    ] {
        assert_eq!(
            judge_model_link("g-a", "x", link),
            ModelLink::Outside,
            "{link}"
        );
    }
    assert_eq!(
        judge_model_link(
            "g-a",
            "x",
            "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/x.step"
        ),
        ModelLink::Kicad
    );
}

#[test]
fn power_symbols_are_never_listed_as_not_in_the_catalog() {
    let power =
        "\t(symbol \"GND\"\n\t\t(power)\n\t\t(property \"Reference\" \"#PWR\" (at 0 0 0))\n\t)\n";
    let root = small_library(&[(
        "symbols/g-pwr.kicad_sym",
        &library(&[power.to_owned(), symbol("NOT_POWER", "")]),
    )]);
    let found = check_folder(root.path(), &small_catalog(root.path()), None);
    let listed: Vec<&str> = found
        .in_group(Group::NotInCatalog)
        .map(|f| f.item.as_str())
        .collect();
    assert!(listed.contains(&"g-pwr:NOT_POWER"), "{listed:?}");
    assert!(!listed.contains(&"g-pwr:GND"), "{listed:?}");
}

/// A library file that cannot be read does not stop the check: it is
/// listed, and everything else is checked.
#[test]
fn an_unreadable_library_is_listed_and_the_rest_is_checked() {
    let root = small_library(&[("symbols/g-bad.kicad_sym", "(not a library")]);
    let found = check_folder(root.path(), &small_catalog(root.path()), None);
    let unreadable = found
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::Unreadable)
        .unwrap();
    assert_eq!(unreadable.file, "symbols/g-bad.kicad_sym");
    assert!(
        unreadable.message.starts_with("Could not be read: "),
        "{}",
        unreadable.message
    );
    assert!(
        found
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::NotInCatalog && f.item == "g-ics:IC_TP4333")
    );
}

/// Makes `link` a link to the folder `target` (a junction on Windows,
/// which needs no special rights).
fn link_folder(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// The check never follows a link, whether it is one of the library
/// folders themselves or something inside them; each is listed.
#[test]
fn links_are_not_followed_and_are_listed() {
    let root = small_library(&[]);
    let known = small_catalog(root.path());
    let elsewhere = tempfile::tempdir().unwrap();
    fs::create_dir_all(elsewhere.path().join("g-Outside.3dshapes")).unwrap();
    fs::write(elsewhere.path().join("g-Outside.3dshapes/X.step"), "x").unwrap();
    fs::write(
        elsewhere.path().join("Linked.kicad_mod"),
        footprint("Linked", &["/home/me/x.step"]),
    )
    .unwrap();
    link_folder(
        elsewhere.path(),
        &root.path().join("footprints").join("g-Linked.pretty"),
    );
    fs::remove_dir_all(root.path().join("3d-models")).unwrap();
    link_folder(elsewhere.path(), &root.path().join("3d-models"));
    let found = check_folder(root.path(), &known, None);
    let unreadable: Vec<&str> = found
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::Unreadable)
        .map(|f| f.file.as_str())
        .collect();
    assert_eq!(
        unreadable,
        ["3d-models", "footprints/g-Linked.pretty"],
        "{:?}",
        found.findings
    );
    assert!(
        found
            .findings
            .iter()
            .filter(|f| f.kind != FindingKind::Unreadable)
            .all(|f| !f.item.contains("Linked") && !f.item.contains("Outside")),
        "{:?}",
        found.findings
    );
    // The footprint's own model in the linked folder is not seen either.
    assert!(
        found
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::MissingModel)
    );
}

/// A KiCad 5 footprint writes its 3D model path without quotes; the check
/// judges it like any other link.
#[test]
fn an_unquoted_kicad_5_model_path_is_judged_too() {
    let old = "(module Old5 (layer F.Cu)\n  (descr \"see (model x) note\")\n  (model ${KIPRJMOD}/3D/Old5.wrl\n    (at (xyz 0 0 0))\n  )\n)\n";
    let root = small_library(&[("footprints/g-Package_SO.pretty/Old5.kicad_mod", old)]);
    let found = check_folder(root.path(), &small_catalog(root.path()), None);
    let old5: Vec<(Severity, &str)> = found
        .findings
        .iter()
        .filter(|f| f.name() == "Old5" && f.kind != FindingKind::NotInCatalog)
        .map(|f| (f.severity, f.kind.as_str()))
        .collect();
    assert_eq!(
        old5,
        [(Severity::Warning, "model-outside")],
        "{:?}",
        found.findings
    );
    assert!(
        found
            .findings
            .iter()
            .any(|f| f.message.contains("${KIPRJMOD}/3D/Old5.wrl")),
        "{:?}",
        found.findings
    );
}

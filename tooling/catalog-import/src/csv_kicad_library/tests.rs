//! The comparison with the CSV catalog's own KiCad library, on one small
//! resistor category: what is meant passes with its reason, and anything
//! else fails, such as a changed description, a symbol with other bytes, a
//! wrong 3D model, a stray entry or a changed group setting.

use super::*;
use catalog::sha256_hex;
use kicad_integration::library_file::rename_symbol_entry;

const SYMBOL: &str =
    "(symbol \"R\"\n\t\t(property \"Reference\" \"R\")\n\t\t(symbol \"R_0_1\")\n\t)";
const MODEL_LINK: &str = "${GITPLM_PARTS}/3d-models/r.step";
const FOOTPRINT: &str = "(footprint \"R_0603\"\n\t(pad \"1\" smd rect)\n\t(model \"${GITPLM_PARTS}/3d-models/r.step\")\n\t(model \"/home/someone/old.step\")\n)\n";
const MODEL_BYTES: &str = "ISO-10303-21; a resistor";
const GENERATED_MODEL: &str = "footprints/MA-Resistor_SMD.3dshapes/R_0603_v1.step";

fn row(pairs: &[(&str, &str)]) -> Row {
    pairs
        .iter()
        .map(|(column, value)| ((*column).to_owned(), (*value).to_owned()))
        .collect()
}

fn group(key: &str, bom: Option<&str>, fields: &[&str]) -> Option<Group> {
    Some(Group {
        key: key.to_owned(),
        symbols: "Symbol".to_owned(),
        footprints: "Footprint".to_owned(),
        exclude_from_bom: bom.map(str::to_owned),
        fields: fields
            .iter()
            .map(|field| ((*field).to_owned(), (*field).to_owned()))
            .collect(),
    })
}

fn table(rows: Vec<Row>, group: Option<Group>) -> Table {
    Table {
        columns: rows[0].keys().cloned().collect(),
        rows,
        group,
    }
}

fn resistor() -> Row {
    row(&[
        ("IPN", "RES-0000-0470"),
        ("MPN", "RC0603FR-0747RL"),
        ("Manufacturer", "Yageo"),
        ("Description", "RES 47 OHM 1% 1/10W 0603"),
        ("Value", "47"),
        ("Symbol", "g-res:R"),
        ("Footprint", "g-Resistor_SMD:R_0603;R_0805"),
        ("Datasheet", "https://example.test/rc.pdf"),
        ("LCSC", "C23182"),
    ])
}

/// The CSV catalog's library: one resistor, with a supplier column, whose
/// footprint links one 3D model file and one link that never led to one.
fn csv_side() -> CsvSide {
    let mut side = CsvSide::default();
    let res = table(
        vec![resistor()],
        group(
            "IPN",
            None,
            &[
                "IPN",
                "Value",
                "Manufacturer",
                "MPN",
                "Description",
                "Datasheet",
                "LCSC",
            ],
        ),
    );
    side.frozen.insert("res".to_owned(), res.clone());
    side.library.tables.insert("res".to_owned(), res);
    side.symbols.insert("g-res:R".into(), SYMBOL.into());
    side.footprints.insert(
        "g-Resistor_SMD:R_0603".into(),
        CsvFootprint {
            bytes: FOOTPRINT.into(),
            models: vec![
                ExpectedModel {
                    link: MODEL_LINK.into(),
                    sha256: Some(sha256_hex(MODEL_BYTES)),
                },
                ExpectedModel {
                    link: "/home/someone/old.step".into(),
                    sha256: None,
                },
            ],
        },
    );
    side
}

/// The generated library, made from the CSV catalog as intended.
fn generated_side() -> GeneratedSide {
    let mut side = GeneratedSide::default();
    side.library.tables.insert(
        "category_res".to_owned(),
        table(
            vec![row(&[
                ("Entry", "RES-0000-0470_v1"),
                ("IPN", "RES-0000-0470"),
                ("Revision", "1"),
                ("Symbol", "MA-res-20241209:R_v1"),
                ("Footprint", "MA-Resistor_SMD:R_0603_v1;R_0805"),
                ("Value", "47"),
                ("Manufacturer", "Yageo"),
                ("MPN", "RC0603FR-0747RL"),
                ("Description", "RES 47 OHM 1% 1/10W 0603"),
                ("Datasheet", "https://example.test/rc.pdf"),
                ("Exclude from BOM", "0"),
            ])],
            group(
                "Entry",
                Some("Exclude from BOM"),
                &[
                    "IPN",
                    "Revision",
                    "Value",
                    "Manufacturer",
                    "MPN",
                    "Description",
                    "Datasheet",
                ],
            ),
        ),
    );
    side.library.tables.insert(
        "older_revisions".to_owned(),
        Table {
            columns: vec!["Entry".into()],
            rows: Vec::new(),
            group: group("Entry", Some("Exclude from BOM"), &["IPN", "Revision"]),
        },
    );
    side.symbols.insert(
        "MA-res-20241209:R_v1".into(),
        rename_symbol_entry(SYMBOL.as_bytes(), "R", "R_v1").unwrap(),
    );
    side.footprints.insert(
        "MA-Resistor_SMD:R_0603_v1".into(),
        GeneratedFootprint {
            bytes: FOOTPRINT
                .replace(MODEL_LINK, "../MA-Resistor_SMD.3dshapes/R_0603_v1.step")
                .into_bytes(),
            folder: "footprints/MA-Resistor_SMD.pretty".into(),
        },
    );
    side.files
        .insert(GENERATED_MODEL.into(), sha256_hex(MODEL_BYTES));
    side
}

fn entry(side: &mut GeneratedSide) -> &mut Row {
    &mut side.library.tables.get_mut("category_res").unwrap().rows[0]
}

fn rules(comparison: &Comparison) -> Vec<&'static str> {
    let mut rules: Vec<&str> = comparison
        .differences
        .iter()
        .filter_map(|found| found.rule)
        .collect();
    rules.sort();
    rules.dedup();
    rules
}

fn unexplained(comparison: &Comparison) -> Vec<String> {
    comparison
        .unexplained()
        .map(|found| found.difference.describe())
        .collect()
}

/// Exactly one unexplained difference, which says `says`.
fn fails_once(generated: &GeneratedSide, says: &str) {
    let failed = unexplained(&compare(&csv_side(), generated));
    assert_eq!(failed.len(), 1, "{failed:#?}");
    assert!(failed[0].contains(says), "{failed:#?}");
}

#[test]
fn libraries_made_as_intended_differ_only_with_reasons() {
    let comparison = compare(&csv_side(), &generated_side());
    assert_eq!(unexplained(&comparison), Vec::<String>::new());
    assert_eq!((comparison.csv_rows, comparison.entries_found), (1, 1));
    assert_eq!(
        rules(&comparison),
        ["entry-per-revision", "revision-groups", "supplier-data"]
    );
}

/// The planted difference: one changed description, which no rule
/// explains, must fail, and be named field by field.
#[test]
fn a_changed_value_fails() {
    let mut generated = generated_side();
    entry(&mut generated).insert("Description".into(), "RES 47R".into());
    let comparison = compare(&csv_side(), &generated);
    assert_eq!(
        unexplained(&comparison),
        [
            "res RES-0000-0470 Description: the CSV says \"RES 47 OHM 1% 1/10W 0603\", the generated library \"RES 47R\".",
            "res RES-0000-0470 field Description: KiCad shows \"RES 47 OHM 1% 1/10W 0603\" from the CSV catalog's library and \"RES 47R\" from the generated one.",
        ]
    );
}

#[test]
fn a_symbol_that_is_not_the_csv_catalogs_own_fails() {
    let mut generated = generated_side();
    generated.symbols.insert(
        "MA-res-20241209:R_v1".into(),
        rename_symbol_entry(SYMBOL.replace("\"R\")", "\"U\")").as_bytes(), "R", "R_v1").unwrap(),
    );
    fails_once(
        &generated,
        "is not the CSV catalog's symbol g-res:R, byte for byte",
    );
}

#[test]
fn a_footprint_with_another_change_than_its_model_link_fails() {
    let mut generated = generated_side();
    let footprint = generated
        .footprints
        .get_mut("MA-Resistor_SMD:R_0603_v1")
        .unwrap();
    footprint.bytes = String::from_utf8(footprint.bytes.clone())
        .unwrap()
        .replace("rect", "roundrect")
        .into_bytes();
    fails_once(&generated, "with only its 3D model links changed");
}

/// The 3D model a generated footprint links to must be there, with the
/// CSV catalog's bytes, and a link that never led to a file stays.
#[test]
fn a_missing_wrong_or_moved_3d_model_fails() {
    let mut deleted = generated_side();
    deleted.files.clear();
    fails_once(&deleted, "but there is no such file");

    let mut junk = generated_side();
    junk.files
        .insert(GENERATED_MODEL.into(), sha256_hex("junk"));
    fails_once(&junk, "is not the CSV catalog's model");

    let mut moved = generated_side();
    let footprint = moved
        .footprints
        .get_mut("MA-Resistor_SMD:R_0603_v1")
        .unwrap();
    footprint.bytes = String::from_utf8(footprint.bytes.clone())
        .unwrap()
        .replace("R_0603_v1.step", "nonexistent_v1.step")
        .into_bytes();
    fails_once(&moved, "but there is no such file");

    let mut rewritten = generated_side();
    let footprint = rewritten
        .footprints
        .get_mut("MA-Resistor_SMD:R_0603_v1")
        .unwrap();
    footprint.bytes = String::from_utf8(footprint.bytes.clone())
        .unwrap()
        .replace(
            "/home/someone/old.step",
            "../MA-Resistor_SMD.3dshapes/old.step",
        )
        .into_bytes();
    fails_once(&rewritten, "never led to a file");
}

#[test]
fn a_link_to_another_item_or_a_rewritten_kicad_choice_fails() {
    for footprint in [
        "MA-Resistor_SMD:R_0805_v1;R_0805",
        "MA-Resistor_SMD:R_0603_v1;MA-Resistor_SMD:R_0805",
        "MA-Resistor_SMD:R_0603_v1",
    ] {
        let mut generated = generated_side();
        entry(&mut generated).insert("Footprint".into(), footprint.into());
        let failed = unexplained(&compare(&csv_side(), &generated));
        assert_eq!(failed.len(), 1, "{footprint}: {failed:?}");
    }
}

/// Every generated entry must be a CSV row's, in every table.
#[test]
fn a_stray_entry_fails() {
    let mut generated = generated_side();
    let stray = entry(&mut generated).clone();
    let mut stray_res = stray.clone();
    stray_res.insert("Entry".into(), "RES-9999-9999_v1".into());
    generated
        .library
        .tables
        .get_mut("category_res")
        .unwrap()
        .rows
        .push(stray_res);
    fails_once(
        &generated,
        "category_res RES-9999-9999_v1: an entry that no CSV row makes",
    );

    let mut generated = generated_side();
    generated
        .library
        .tables
        .get_mut("older_revisions")
        .unwrap()
        .rows
        .push(stray);
    fails_once(
        &generated,
        "older_revisions RES-0000-0470_v1: an entry that no CSV row makes",
    );
}

/// A change to a group of the generated library.
type Change = dyn Fn(&mut Group);

/// Each generated group needs its own key, Symbol and Footprint columns,
/// Exclude from BOM setting and Revision field. A setting that falls back
/// to the CSV catalog's own value fails as surely as any other.
#[test]
fn a_changed_group_setting_fails() {
    // Changes `table`'s group in a fresh generated library and returns
    // the failures that name `setting` as a required one.
    let required_failures = |table: &str, setting: &str, change: &Change| {
        let mut generated = generated_side();
        change(
            generated
                .library
                .tables
                .get_mut(table)
                .unwrap()
                .group
                .as_mut()
                .unwrap(),
        );
        unexplained(&compare(&csv_side(), &generated))
            .into_iter()
            .filter(|failure| {
                failure.starts_with(&format!(
                    "{table} {setting}: the generated library must have"
                ))
            })
            .count()
    };
    let cases: [(&str, &str, &Change); 7] = [
        ("category_res", "footprints", &|group| {
            group.footprints = "Symbol".into()
        }),
        ("category_res", "symbols", &|group| {
            group.symbols = "Footprint".into()
        }),
        ("older_revisions", "key", &|group| group.key = "MPN".into()),
        // The CSV catalog's own values: still wrong for the generated one.
        ("category_res", "key", &|group| group.key = "IPN".into()),
        ("older_revisions", "key", &|group| group.key = "IPN".into()),
        ("category_res", "exclude_from_bom", &|group| {
            group.exclude_from_bom = None
        }),
        ("category_res", "Revision field", &|group| {
            group.fields.retain(|(column, _)| column != "Revision")
        }),
    ];
    for (table, setting, change) in cases {
        assert_eq!(
            required_failures(table, setting, change),
            1,
            "{table} {setting}"
        );
    }
}

/// Revision must be the number in the entry's name, and Exclude from BOM
/// what the CSV catalog's symbol and footprints say.
#[test]
fn a_wrong_revision_or_bom_setting_fails() {
    let mut generated = generated_side();
    entry(&mut generated).insert("Revision".into(), "2".into());
    let failed = unexplained(&compare(&csv_side(), &generated));
    assert_eq!(failed.len(), 2, "the column and the field: {failed:#?}");
    assert!(failed[0].contains("Revision: the CSV says \"1\""));

    let mut generated = generated_side();
    entry(&mut generated).insert("Exclude from BOM".into(), "1".into());
    fails_once(&generated, "Exclude from BOM: the CSV says \"0\"");

    // A symbol marked not in the BOM makes the part excluded.
    let mut csv = csv_side();
    csv.symbols.insert(
        "g-res:R".into(),
        SYMBOL
            .replace("(symbol \"R_0_1\")", "(in_bom no)\n\t\t(symbol \"R_0_1\")")
            .into(),
    );
    let mut generated = generated_side();
    generated.symbols.insert(
        "MA-res-20241209:R_v1".into(),
        rename_symbol_entry(&csv.symbols["g-res:R"], "R", "R_v1").unwrap(),
    );
    let failed = unexplained(&compare(&csv, &generated));
    assert_eq!(failed.len(), 1, "{failed:#?}");
    assert!(failed[0].contains("Exclude from BOM: the CSV says \"1\""));
    entry(&mut generated).insert("Exclude from BOM".into(), "1".into());
    assert!(unexplained(&compare(&csv, &generated)).is_empty());
}

/// A part without a symbol cannot be placed, so it has no entry; a part
/// with a symbol must have one.
#[test]
fn only_a_part_without_a_symbol_may_have_no_entry() {
    let mut generated = generated_side();
    generated
        .library
        .tables
        .get_mut("category_res")
        .unwrap()
        .rows
        .clear();
    let comparison = compare(&csv_side(), &generated);
    assert_eq!(comparison.entries_found, 0);
    assert_eq!(
        unexplained(&comparison),
        ["res RES-0000-0470: no entry is generated."]
    );

    let mut csv = csv_side();
    csv.library.tables.get_mut("res").unwrap().rows[0].insert("Symbol".into(), String::new());
    csv.frozen = csv.library.tables.clone();
    let comparison = compare(&csv, &generated);
    assert!(unexplained(&comparison).is_empty());
    assert!(rules(&comparison).contains(&"part-without-symbol"));
}

/// When the CSV catalog's library gives no value, the generated one shows
/// the MPN; any other value is a difference that fails.
#[test]
fn a_blank_value_shows_the_mpn_and_nothing_else() {
    let mut csv = csv_side();
    csv.library.tables.get_mut("res").unwrap().rows[0].insert("Value".into(), String::new());
    csv.frozen = csv.library.tables.clone();
    let mut generated = generated_side();
    entry(&mut generated).insert("Value".into(), "RC0603FR-0747RL".into());
    let comparison = compare(&csv, &generated);
    assert!(unexplained(&comparison).is_empty());
    assert!(rules(&comparison).contains(&"value-never-blank"));

    entry(&mut generated).insert("Value".into(), "47 ohm".into());
    assert_eq!(unexplained(&compare(&csv, &generated)).len(), 2);
}

/// An engineer fixed a CSV value after parts.sqlite was frozen. Once the
/// CSV catalog fixture records the change, the generated library must
/// have the new value; without the record, or with the old value, it
/// fails.
#[test]
fn a_csv_change_the_fixture_records_passes_only_with_the_new_value() {
    let mut csv = csv_side();
    csv.library.tables.get_mut("res").unwrap().rows[0].insert(
        "Description".into(),
        "RES 47 OHM 1% 1/10W 0603 4-PAD".into(),
    );
    let mut generated = generated_side();
    entry(&mut generated).insert(
        "Description".into(),
        "RES 47 OHM 1% 1/10W 0603 4-PAD".into(),
    );

    let failed = unexplained(&compare(&csv, &generated));
    assert_eq!(failed.len(), 1, "not recorded: {failed:#?}");
    assert!(failed[0].starts_with("res RES-0000-0470 Description: parts.sqlite has"));

    csv.notes
        .cells
        .insert(("res".into(), "RES-0000-0470".into(), "Description".into()));
    let comparison = compare(&csv, &generated);
    assert!(unexplained(&comparison).is_empty());
    assert!(rules(&comparison).contains(&"parts-sqlite-is-older"));

    // The generated library still has the old value: that fails.
    let failed = unexplained(&compare(&csv, &generated_side()));
    assert_eq!(failed.len(), 2, "the column and the field: {failed:#?}");
}

#[test]
fn every_rule_has_a_name_and_a_reason_in_whole_sentences() {
    let mut names: Vec<&str> = intended::INTENDED.iter().map(|rule| rule.name).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), intended::INTENDED.len());
    for rule in &intended::INTENDED {
        assert!(rule.reason.ends_with('.'), "{}", rule.name);
    }
}

#[test]
fn a_model_link_is_resolved_from_its_footprint_folder() {
    assert_eq!(
        resolve("footprints/MA-x.pretty", "../MA-x.3dshapes/a.step").as_deref(),
        Some("footprints/MA-x.3dshapes/a.step")
    );
    assert_eq!(resolve("footprints/MA-x.pretty", "../../../a.step"), None);
    assert_eq!(
        resolve("footprints/MA-x.pretty", "${KICAD10_3DMODEL_DIR}/a.step"),
        None
    );
    assert_eq!(resolve("footprints/MA-x.pretty", "C:/a.step"), None);
}

//! The comparison with the CSV catalog's own KiCad library, on one small
//! resistor category: what is meant passes with its reason, and anything
//! else, such as a changed description or a symbol with other bytes, fails.

use super::*;
use kicad_integration::library_file::rename_symbol_entry;

const SYMBOL: &str =
    "(symbol \"R\"\n\t\t(property \"Reference\" \"R\")\n\t\t(symbol \"R_0_1\")\n\t)";
const FOOTPRINT: &str = "(footprint \"R_0603\"\n\t(pad \"1\" smd rect)\n\t(model \"${GITPLM_PARTS}/3d-models/r.step\")\n)\n";

fn row(pairs: &[(&str, &str)]) -> Row {
    pairs
        .iter()
        .map(|(column, value)| ((*column).to_owned(), (*value).to_owned()))
        .collect()
}

fn group(key: &str, fields: &[&str]) -> Option<Group> {
    Some(Group {
        key: key.to_owned(),
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

/// The CSV catalog's library: one resistor, with a supplier column.
fn csv_side() -> CsvSide {
    let mut side = CsvSide::default();
    side.library.tables.insert(
        "res".to_owned(),
        table(
            vec![row(&[
                ("IPN", "RES-0000-0470"),
                ("MPN", "RC0603FR-0747RL"),
                ("Manufacturer", "Yageo"),
                ("Description", "RES 47 OHM 1% 1/10W 0603"),
                ("Value", "47"),
                ("Symbol", "g-res:R"),
                ("Footprint", "g-Resistor_SMD:R_0603;R_0805"),
                ("Datasheet", "https://example.test/rc.pdf"),
                ("LCSC", "C23182"),
            ])],
            group(
                "IPN",
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
        ),
    );
    side.symbols.insert("g-res:R".into(), SYMBOL.into());
    side.footprints
        .insert("g-Resistor_SMD:R_0603".into(), FOOTPRINT.into());
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
            group: group("Entry", &[]),
        },
    );
    side.symbols.insert(
        "MA-res-20241209:R_v1".into(),
        rename_symbol_entry(SYMBOL.as_bytes(), "R", "R_v1").unwrap(),
    );
    side.footprints.insert(
        "MA-Resistor_SMD:R_0603_v1".into(),
        FOOTPRINT
            .replace(
                "${GITPLM_PARTS}/3d-models/r.step",
                "../MA-Resistor_SMD.3dshapes/R_0603_v1.step",
            )
            .into_bytes(),
    );
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
    let failed = unexplained(&compare(&csv_side(), &generated));
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("is not the CSV catalog's symbol g-res:R, byte for byte"));
}

#[test]
fn a_footprint_with_another_change_than_its_model_link_fails() {
    let mut generated = generated_side();
    let changed = String::from_utf8(generated.footprints["MA-Resistor_SMD:R_0603_v1"].clone())
        .unwrap()
        .replace("rect", "roundrect");
    generated
        .footprints
        .insert("MA-Resistor_SMD:R_0603_v1".into(), changed.into_bytes());
    let failed = unexplained(&compare(&csv_side(), &generated));
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("with only its 3D model links changed"));
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
    let mut generated = generated_side();
    entry(&mut generated).insert("Value".into(), "RC0603FR-0747RL".into());
    let comparison = compare(&csv, &generated);
    assert!(unexplained(&comparison).is_empty());
    assert!(rules(&comparison).contains(&"value-never-blank"));

    entry(&mut generated).insert("Value".into(), "47 ohm".into());
    assert_eq!(unexplained(&compare(&csv, &generated)).len(), 2);
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

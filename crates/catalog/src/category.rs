//! Categories: the only grouping of parts that people see.
//!
//! Each category is a Catalog record with revisions, like a CAD item. It
//! has a code (such as `res`), its plain name, the IPN prefix new parts get
//! by default, the fields a part in it has, in order, and the rules that
//! help guess a new part's category ([`crate::guess`]). A part's category
//! is part of its [`crate::PartDetails`]; changing it never changes the
//! part's IPN.
//!
//! A new team server starts with the [`starting_categories`], each saved
//! as its first revision.

use crate::hash::sha256_hex;
use crate::ids::derive_id;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// The schema tag of [`CategoryDetails`].
pub const CATEGORY_SCHEMA: &str = "kicad-parts/category/1";

/// What a category is, saved as one revision of it.
///
/// Serialization is canonical, like [`crate::PartDetails`]: fields in
/// declaration order and every collection an ordered list, so its SHA-256
/// identifies the revision's content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryDetails {
    pub schema: String,
    /// Its code, such as `res`. It never changes and gives the category
    /// its ID.
    pub code: String,
    /// The name people see, such as `Resistors`.
    pub name: String,
    /// The IPN prefix a new part in this category gets by default, such as
    /// `RES`, or blank when the code is not three letters.
    pub ipn_prefix: String,
    /// The fields a part in this category has, in order (never IPN, MPN,
    /// Manufacturer, Symbol, Footprint or supplier data).
    pub fields: Vec<String>,
    pub rules: CategoryRules,
}

/// The clues that point to a category, which people can edit (a
/// follow-up): reference letters, footprint name patterns and keywords.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryRules {
    /// Symbol reference letters, such as `R`. Aliases (`IC` for `U`, `S` for
    /// `SW`) are applied before matching, see [`reference_letter`].
    pub reference_letters: Vec<String>,
    /// Footprint name patterns, where `*` stands for any text, such as
    /// `R_*` (KiCad) or `RESC*` (IPC-7351). Matched without the library and
    /// ignoring case.
    pub footprint_patterns: Vec<String>,
    /// Words or short phrases a description may hold, such as `regulator`.
    pub keywords: Vec<String>,
}

impl CategoryDetails {
    /// A first revision of the category `code`: the starting category with
    /// that code, or, for a code no starting category has, the code as its
    /// name, with no fields and no rules.
    pub fn starting(code: &str) -> Self {
        starting_category(code).cloned().unwrap_or_else(|| Self {
            schema: CATEGORY_SCHEMA.to_owned(),
            code: code.to_owned(),
            name: code.to_owned(),
            ipn_prefix: default_ipn_prefix(code),
            fields: Vec::new(),
            rules: CategoryRules::default(),
        })
    }

    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("categories always serialize")
    }

    pub fn sha256(&self) -> String {
        sha256_hex(self.canonical_json())
    }
}

/// The code in capitals when it is three letters, such as `RES`; otherwise
/// blank, because an IPN prefix is always three capital letters.
pub fn default_ipn_prefix(code: &str) -> String {
    if code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        code.to_ascii_uppercase()
    } else {
        String::new()
    }
}

/// A category's ID, from its code. The server derives the same.
pub fn category_id(code: &str) -> String {
    derive_id("category", code)
}

/// A category revision's ID, from its category ID and number.
pub fn category_revision_id(category: &str, number: u32) -> String {
    derive_id("category-revision", &format!("{category}\n{number}"))
}

/// The 21 categories a new team server starts with, with their fields and
/// the rules that guess each one, one canonical JSON object per line. They
/// came from the old CSV catalog. No rule names `U`: it says only that a
/// part is a chip. A team's own parts are the examples the guess learns
/// from, so they need no rules.
///
/// This file is the only copy: the team server's migration
/// 1792700000_starting_categories.js reads it too. Edit it by hand.
const STARTING_CATEGORIES_JSON: &str =
    include_str!("../../../infra/pocketbase/pb_hooks/catalog_starting_categories.json");

/// The first revision of every starting category, in the file's order.
pub fn starting_categories() -> &'static [CategoryDetails] {
    static STARTING: OnceLock<Vec<CategoryDetails>> = OnceLock::new();
    STARTING.get_or_init(|| {
        serde_json::from_str(STARTING_CATEGORIES_JSON)
            .expect("catalog_starting_categories.json holds categories")
    })
}

/// The starting category with `code`, if there is one.
pub fn starting_category(code: &str) -> Option<&'static CategoryDetails> {
    starting_categories()
        .iter()
        .find(|category| category.code == code)
}

/// Reference letters that mean the same as another: `IC` is `U`, `S` is
/// `SW`, and so on.
pub const REFERENCE_ALIASES: [(&str, &str); 7] = [
    ("IC", "U"),
    ("S", "SW"),
    ("X", "Y"),
    ("XTAL", "Y"),
    ("VR", "RV"),
    ("POT", "RV"),
    ("CN", "J"),
];

/// The letters of a symbol reference, in capitals and with aliases
/// applied: `U5` and `IC?` give `U`, `s1` gives `SW`. Blank when the
/// reference starts with something else.
pub fn reference_letter(reference: &str) -> String {
    let letters: String = reference
        .trim()
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic() || *ch == '#')
        .collect::<String>()
        .to_ascii_uppercase();
    REFERENCE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == letters)
        .map_or(letters, |(_, letter)| (*letter).to_owned())
}

/// A footprint's family: its name without the library, up to the first
/// digit, without trailing `_`, `-` or `.`. `g-Resistor_SMD:R_0603_1608Metric`
/// gives `R`, `SOT-23-5` gives `SOT`, `Crystal_SMD_3225-4Pin` gives
/// `Crystal_SMD`. Blank when the name starts with a digit.
///
/// The team server works out the same family when it remembers a match
/// (catalog_lib.js footprintFamily).
pub fn footprint_family(footprint: &str) -> String {
    let name = footprint
        .rsplit_once(':')
        .map_or(footprint, |(_, name)| name)
        .trim();
    let cut = name
        .find(|ch: char| ch.is_ascii_digit())
        .unwrap_or(name.len());
    name[..cut]
        .trim_end_matches(['_', '-', '.', ' '])
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_start_from_the_file_and_serialize_canonically() {
        let category = CategoryDetails::starting("res");
        assert_eq!(category.name, "Resistors");
        assert_eq!(category.ipn_prefix, "RES");
        assert_eq!(
            category.canonical_json(),
            r#"{"schema":"kicad-parts/category/1","code":"res","name":"Resistors","ipn_prefix":"RES","fields":["Description","Value","Tolerance","Datasheet"],"rules":{"reference_letters":["R"],"footprint_patterns":["R_*","RESC*","RESM*","Resistor*"],"keywords":["resistor","res","ohm"]}}"#
        );
        // A code no starting category has gets its code as its name.
        let unknown = CategoryDetails::starting("misc-2");
        assert_eq!(unknown.name, "misc-2");
        assert_eq!(unknown.ipn_prefix, "");
        assert!(unknown.fields.is_empty());
        assert_eq!(unknown.rules, CategoryRules::default());
        // 21 categories, each code once, each with the schema, its default
        // IPN prefix and fields; no rule names U.
        let starting = starting_categories();
        assert_eq!(starting.len(), 21);
        let mut codes: Vec<&str> = starting
            .iter()
            .map(|category| category.code.as_str())
            .collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), starting.len());
        for category in starting {
            assert_eq!(category.schema, CATEGORY_SCHEMA, "{}", category.code);
            assert_eq!(category.ipn_prefix, default_ipn_prefix(&category.code));
            assert!(!category.fields.is_empty(), "{}", category.code);
            assert!(
                !category
                    .rules
                    .reference_letters
                    .iter()
                    .any(|letter| letter == "U")
            );
        }
        // Pinned: the server derives the same IDs (catalog_lib.js).
        assert_eq!(category_id("res"), derive_id("category", "res"));
        assert_eq!(
            category_revision_id(&category_id("res"), 2),
            derive_id("category-revision", &format!("{}\n2", category_id("res")))
        );
    }

    #[test]
    fn the_starting_categories_file_is_canonical() {
        let text = STARTING_CATEGORIES_JSON;
        // One category per line, each as canonical JSON, as the server
        // saves it.
        let lines: Vec<String> = starting_categories()
            .iter()
            .map(CategoryDetails::canonical_json)
            .collect();
        assert_eq!(
            text,
            format!(
                "[
{}
]
",
                lines.join(
                    ",
"
                )
            )
        );
    }

    #[test]
    fn reference_letters_and_footprint_families() {
        assert_eq!(reference_letter("U5"), "U");
        assert_eq!(reference_letter("IC?"), "U");
        assert_eq!(reference_letter("s1"), "SW");
        assert_eq!(reference_letter("#PWR0138"), "#PWR");
        assert_eq!(reference_letter("RV"), "RV");
        assert_eq!(reference_letter("1"), "");
        assert_eq!(footprint_family("g-Resistor_SMD:R_0603_1608Metric"), "R");
        assert_eq!(footprint_family("SOT-23-5"), "SOT");
        assert_eq!(
            footprint_family("Crystal_SMD_3225-4Pin_3.2x2.5mm"),
            "Crystal_SMD"
        );
        assert_eq!(footprint_family("CAPC1608X90N"), "CAPC");
        assert_eq!(footprint_family("21-0268_T1034N1"), "");
    }
}

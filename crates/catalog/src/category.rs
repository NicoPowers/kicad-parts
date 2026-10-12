//! Categories: the only grouping of parts that people see.
//!
//! Each category is a Catalog record with revisions, like a CAD item. The
//! CSV import makes one per CSV file (`database/g-res.csv` makes `res`),
//! with its plain name, the IPN prefix new parts get by default, the
//! fields a part in it has (the CSV file's part details columns, in
//! order), and the rules that help guess a new part's category
//! ([`crate::guess`]). A part's category is part of its
//! [`crate::PartDetails`]; changing it never changes the part's IPN.
//!
//! The rules every category starts with are the table
//! [`STARTING_CATEGORIES`], shipped as each category's first revision.

use crate::hash::sha256_hex;
use crate::ids::derive_id;
use serde::{Deserialize, Serialize};

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
    /// The CSV file name without `g-` and `.csv`, such as `res`. It never
    /// changes and gives the category its ID.
    pub code: String,
    /// The name people see, such as `Resistors`.
    pub name: String,
    /// The IPN prefix a new part in this category gets by default, such as
    /// `RES`, or blank when the code is not three letters.
    pub ipn_prefix: String,
    /// The fields a part in this category has, in order: the part details
    /// columns of its CSV file (never IPN, MPN, Manufacturer, Symbol,
    /// Footprint or supplier data).
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
    /// The first revision of the category made from `database/g-<code>.csv`
    /// whose part details columns are `fields`: its name and rules from
    /// [`STARTING_CATEGORIES`], or the code as its name and no rules for a
    /// CSV file the table does not know.
    pub fn starting(code: &str, fields: Vec<String>) -> Self {
        let start = starting_category(code);
        let list = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect();
        Self {
            schema: CATEGORY_SCHEMA.to_owned(),
            code: code.to_owned(),
            name: start.map_or_else(|| code.to_owned(), |start| start.name.to_owned()),
            ipn_prefix: default_ipn_prefix(code),
            fields,
            rules: start.map_or_else(CategoryRules::default, |start| CategoryRules {
                reference_letters: list(start.reference_letters),
                footprint_patterns: list(start.footprint_patterns),
                keywords: list(start.keywords),
            }),
        }
    }

    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("categories always serialize")
    }

    pub fn sha256(&self) -> String {
        sha256_hex(self.canonical_json())
    }
}

/// The code of the category a CSV file makes: `g-res` makes `res`.
pub fn category_code(csv_file: &str) -> &str {
    csv_file.strip_prefix("g-").unwrap_or(csv_file)
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

/// The name, rules and keywords one category starts with.
#[derive(Clone, Copy, Debug)]
pub struct StartingCategory {
    pub code: &'static str,
    pub name: &'static str,
    pub reference_letters: &'static [&'static str],
    pub footprint_patterns: &'static [&'static str],
    pub keywords: &'static [&'static str],
}

/// The 21 categories of the CSV catalog, with the rules each starts with.
/// `U` is in no list: it says only that a part is a chip. The existing
/// parts are the examples the guess learns from, so they need no rules.
pub const STARTING_CATEGORIES: [StartingCategory; 21] = [
    StartingCategory {
        code: "res",
        name: "Resistors",
        reference_letters: &["R"],
        footprint_patterns: &["R_*", "RESC*", "RESM*", "Resistor*"],
        keywords: &["resistor", "res", "ohm"],
    },
    StartingCategory {
        code: "cap",
        name: "Capacitors",
        reference_letters: &["C"],
        footprint_patterns: &["C_*", "CP_*", "CAPC*", "CAPM*", "CAPAE*"],
        keywords: &["capacitor", "cap", "ceramic", "electrolytic"],
    },
    StartingCategory {
        code: "ind",
        name: "Inductors",
        reference_letters: &["L"],
        footprint_patterns: &["L_*", "INDC*", "INDM*", "IND_*"],
        keywords: &["inductor", "ind", "choke", "ferrite"],
    },
    StartingCategory {
        code: "dio",
        name: "Diodes",
        reference_letters: &["D"],
        footprint_patterns: &["D_*", "SOD*", "DIOM*", "DIOC*"],
        keywords: &["diode", "rectifier", "schottky", "zener", "varactor"],
    },
    StartingCategory {
        code: "opt",
        name: "LEDs and optoelectronics",
        reference_letters: &["LED"],
        footprint_patterns: &["LED_*", "LEDC*", "LEDM*"],
        keywords: &["led", "optocoupler", "optoisolator", "photodiode"],
    },
    StartingCategory {
        code: "trs",
        name: "Transistors",
        reference_letters: &["Q"],
        footprint_patterns: &[],
        keywords: &["transistor", "mosfet", "bjt", "npn", "pnp"],
    },
    StartingCategory {
        code: "reg",
        name: "Voltage regulators",
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &[
            "regulator",
            "ldo",
            "buck",
            "boost",
            "step down",
            "dc dc",
            "converter",
        ],
    },
    StartingCategory {
        code: "pwr",
        name: "Power ICs and relays",
        reference_letters: &["K"],
        footprint_patterns: &["Relay*"],
        keywords: &[
            "power switch",
            "load switch",
            "relay",
            "charger",
            "ideal diode",
        ],
    },
    StartingCategory {
        code: "ics",
        name: "Integrated circuits",
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &[
            "ic",
            "interface",
            "transceiver",
            "flash",
            "eeprom",
            "memory",
            "translator",
            "buffer",
            "controller",
            "phy",
        ],
    },
    StartingCategory {
        code: "ana",
        name: "Analog ICs",
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["op amp", "opamp", "amplifier", "comparator"],
    },
    StartingCategory {
        code: "mcu",
        name: "Microcontrollers",
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["microcontroller", "mcu", "cortex"],
    },
    StartingCategory {
        code: "mpu",
        name: "Processors and modules",
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["processor", "sodimm", "module", "som"],
    },
    StartingCategory {
        code: "osc",
        name: "Crystals and oscillators",
        reference_letters: &["Y"],
        footprint_patterns: &["Crystal*", "XTAL*", "OSC*", "Oscillator*", "Resonator*"],
        keywords: &["crystal", "oscillator", "xo", "resonator"],
    },
    StartingCategory {
        code: "con",
        name: "Connectors",
        reference_letters: &["J", "P"],
        footprint_patterns: &[
            "Conn*",
            "PinHeader*",
            "PinSocket*",
            "JST*",
            "Molex*",
            "USB*",
            "U.FL*",
            "Jack*",
            "IDC*",
        ],
        keywords: &["connector", "conn", "header", "jack", "receptacle", "plug"],
    },
    StartingCategory {
        code: "swi",
        name: "Switches",
        reference_letters: &["SW"],
        footprint_patterns: &["SW_*", "Button*", "Switch*"],
        keywords: &["switch", "tactile", "button", "pushbutton"],
    },
    StartingCategory {
        code: "cpd",
        name: "Circuit protection",
        reference_letters: &["F", "TVS"],
        footprint_patterns: &["Fuse*", "PTC*", "Varistor*"],
        keywords: &[
            "tvs",
            "esd",
            "fuse",
            "varistor",
            "surge",
            "protection",
            "protector",
            "ptc",
            "clamp",
        ],
    },
    StartingCategory {
        code: "rvr",
        name: "Potentiometers and trimmers",
        reference_letters: &["RV"],
        footprint_patterns: &["Potentiometer*", "Trimmer*"],
        keywords: &["potentiometer", "trimmer", "pot"],
    },
    StartingCategory {
        code: "rfm",
        name: "RF modules and shields",
        reference_letters: &[],
        footprint_patterns: &["RF_Shield*", "Shield*"],
        keywords: &["shield", "rf module", "antenna"],
    },
    StartingCategory {
        code: "mec",
        name: "Mechanical parts",
        reference_letters: &[],
        footprint_patterns: &["Standoff*", "Spacer*"],
        keywords: &[
            "standoff",
            "spacer",
            "screw",
            "nut",
            "enclosure",
            "heatsink",
        ],
    },
    StartingCategory {
        code: "art",
        name: "Board features",
        reference_letters: &["TP", "FD", "FID", "H", "MH"],
        footprint_patterns: &["TestPoint*", "Fiducial*", "MountingHole*", "Tag-Connect*"],
        keywords: &[
            "test point",
            "fiducial",
            "mounting hole",
            "tagconnect",
            "tag connect",
        ],
    },
    StartingCategory {
        code: "pcb",
        name: "Bare boards",
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["pcb", "bare board"],
    },
];

/// The starting entry for `code`, if the table has one.
pub fn starting_category(code: &str) -> Option<&'static StartingCategory> {
    STARTING_CATEGORIES.iter().find(|start| start.code == code)
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
    fn categories_start_from_the_table_and_serialize_canonically() {
        let category = CategoryDetails::starting("res", vec!["Description".into(), "Value".into()]);
        assert_eq!(category.name, "Resistors");
        assert_eq!(category.ipn_prefix, "RES");
        assert_eq!(
            category.canonical_json(),
            r#"{"schema":"kicad-parts/category/1","code":"res","name":"Resistors","ipn_prefix":"RES","fields":["Description","Value"],"rules":{"reference_letters":["R"],"footprint_patterns":["R_*","RESC*","RESM*","Resistor*"],"keywords":["resistor","res","ohm"]}}"#
        );
        // A CSV file the table does not know gets its code as its name.
        let unknown = CategoryDetails::starting("misc-2", Vec::new());
        assert_eq!(unknown.name, "misc-2");
        assert_eq!(unknown.ipn_prefix, "");
        assert_eq!(unknown.rules, CategoryRules::default());
        // Every code in the table is unique and is the code of a CSV file
        // name, and no table names U.
        let mut codes: Vec<&str> = STARTING_CATEGORIES.iter().map(|start| start.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), STARTING_CATEGORIES.len());
        assert!(
            STARTING_CATEGORIES
                .iter()
                .all(|start| !start.reference_letters.contains(&"U"))
        );
        assert_eq!(category_code("g-res"), "res");
        // Pinned: the server derives the same IDs (catalog_lib.js).
        assert_eq!(category_id("res"), derive_id("category", "res"));
        assert_eq!(
            category_revision_id(&category_id("res"), 2),
            derive_id("category-revision", &format!("{}\n2", category_id("res")))
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

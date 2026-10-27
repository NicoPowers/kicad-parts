//! Categories: the only grouping of parts that people see.
//!
//! Each category is a Catalog record with revisions, like a CAD item. It
//! has a code (such as `res`), its plain name, the IPN prefix new parts get
//! by default, the fields a part in it has, in order, and the rules that
//! help guess a new part's category ([`crate::guess`]). A part's category
//! is part of its [`crate::PartDetails`]; changing it never changes the
//! part's IPN.
//!
//! A new team server starts with the table [`STARTING_CATEGORIES`], each
//! saved as its first revision.

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
    /// A first revision of the category `code` with `fields`: its name and
    /// rules from [`STARTING_CATEGORIES`], or the code as its name and no
    /// rules for a code the table does not know.
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

/// What one starting category is: its code, name, fields and the rules
/// that guess it.
#[derive(Clone, Copy, Debug)]
pub struct StartingCategory {
    pub code: &'static str,
    pub name: &'static str,
    /// The fields a part in it has, in order.
    pub fields: &'static [&'static str],
    pub reference_letters: &'static [&'static str],
    pub footprint_patterns: &'static [&'static str],
    pub keywords: &'static [&'static str],
}

/// The 21 categories a new team server starts with, with their fields and
/// the rules each starts with. They were the CSV catalog's categories. `U`
/// is in no list: it says only that a part is a chip. A team's own parts
/// are the examples the guess learns from, so they need no rules.
///
/// The team server sets them up the first time it starts, from
/// `infra/pocketbase/pb_hooks/catalog_starting_categories.json`; a test
/// keeps that file and this table in step.
pub const STARTING_CATEGORIES: [StartingCategory; 21] = [
    StartingCategory {
        code: "res",
        name: "Resistors",
        fields: &["Description", "Value", "Tolerance", "Datasheet"],
        reference_letters: &["R"],
        footprint_patterns: &["R_*", "RESC*", "RESM*", "Resistor*"],
        keywords: &["resistor", "res", "ohm"],
    },
    StartingCategory {
        code: "cap",
        name: "Capacitors",
        fields: &[
            "Description",
            "Value",
            "Voltage",
            "Material",
            "Tolerance",
            "Datasheet",
        ],
        reference_letters: &["C"],
        footprint_patterns: &["C_*", "CP_*", "CAPC*", "CAPM*", "CAPAE*"],
        keywords: &["capacitor", "cap", "ceramic", "electrolytic"],
    },
    StartingCategory {
        code: "ind",
        name: "Inductors",
        fields: &["Description", "Value", "Current", "Datasheet"],
        reference_letters: &["L"],
        footprint_patterns: &["L_*", "INDC*", "INDM*", "IND_*"],
        keywords: &["inductor", "ind", "choke", "ferrite"],
    },
    StartingCategory {
        code: "dio",
        name: "Diodes",
        fields: &["Value", "Description", "Current", "Voltage", "Datasheet"],
        reference_letters: &["D"],
        footprint_patterns: &["D_*", "SOD*", "DIOM*", "DIOC*"],
        keywords: &["diode", "rectifier", "schottky", "zener", "varactor"],
    },
    StartingCategory {
        code: "opt",
        name: "LEDs and optoelectronics",
        fields: &[
            "Description",
            "Color",
            "I-forward-max",
            "V-forward",
            "Wavelength",
            "Datasheet",
        ],
        reference_letters: &["LED"],
        footprint_patterns: &["LED_*", "LEDC*", "LEDM*"],
        keywords: &["led", "optocoupler", "optoisolator", "photodiode"],
    },
    StartingCategory {
        code: "trs",
        name: "Transistors",
        fields: &[
            "Value",
            "Type",
            "max(Vce)",
            "max(Ic)",
            "Description",
            "Datasheet",
        ],
        reference_letters: &["Q"],
        footprint_patterns: &[],
        keywords: &["transistor", "mosfet", "bjt", "npn", "pnp"],
    },
    StartingCategory {
        code: "reg",
        name: "Voltage regulators",
        fields: &["Description", "Voltage", "Current", "Datasheet"],
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
        fields: &["Description", "Datasheet"],
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
        fields: &["Description", "Datasheet"],
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
        fields: &[
            "Description",
            "Datasheet",
            "Sim_Library",
            "Sim_Name",
            "Sim_Device",
            "Sim_Pins",
        ],
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["op amp", "opamp", "amplifier", "comparator"],
    },
    StartingCategory {
        code: "mcu",
        name: "Microcontrollers",
        fields: &["Description", "Datasheet"],
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["microcontroller", "mcu", "cortex"],
    },
    StartingCategory {
        code: "mpu",
        name: "Processors and modules",
        fields: &["Description", "Datasheet"],
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["processor", "sodimm", "module", "som"],
    },
    StartingCategory {
        code: "osc",
        name: "Crystals and oscillators",
        fields: &["Description", "Frequency", "Stability", "Load", "Datasheet"],
        reference_letters: &["Y"],
        footprint_patterns: &["Crystal*", "XTAL*", "OSC*", "Oscillator*", "Resonator*"],
        keywords: &["crystal", "oscillator", "xo", "resonator"],
    },
    StartingCategory {
        code: "con",
        name: "Connectors",
        fields: &["Value", "Description", "Pins", "Datasheet"],
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
        fields: &["Description", "Form", "Datasheet"],
        reference_letters: &["SW"],
        footprint_patterns: &["SW_*", "Button*", "Switch*"],
        keywords: &["switch", "tactile", "button", "pushbutton"],
    },
    StartingCategory {
        code: "cpd",
        name: "Circuit protection",
        fields: &["Description", "Datasheet"],
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
        fields: &[
            "Description",
            "Resistance",
            "Tolerance",
            "Turns",
            "Datasheet",
        ],
        reference_letters: &["RV"],
        footprint_patterns: &["Potentiometer*", "Trimmer*"],
        keywords: &["potentiometer", "trimmer", "pot"],
    },
    StartingCategory {
        code: "rfm",
        name: "RF modules and shields",
        fields: &["Value", "Description", "Datasheet"],
        reference_letters: &[],
        footprint_patterns: &["RF_Shield*", "Shield*"],
        keywords: &["shield", "rf module", "antenna"],
    },
    StartingCategory {
        code: "mec",
        name: "Mechanical parts",
        fields: &["Description", "Type", "Datasheet"],
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
        fields: &["Description", "Comments"],
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
        fields: &["Description", "Project link"],
        reference_letters: &[],
        footprint_patterns: &[],
        keywords: &["pcb", "bare board"],
    },
];

/// The starting entry for `code`, if the table has one.
pub fn starting_category(code: &str) -> Option<&'static StartingCategory> {
    STARTING_CATEGORIES.iter().find(|start| start.code == code)
}

/// The first revision of every starting category, in table order.
pub fn starting_categories() -> Vec<CategoryDetails> {
    STARTING_CATEGORIES
        .iter()
        .map(|start| {
            let fields = start.fields.iter().map(|field| (*field).to_owned());
            CategoryDetails::starting(start.code, fields.collect())
        })
        .collect()
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
        // A code the table does not know gets its code as its name.
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
        // Pinned: the server derives the same IDs (catalog_lib.js).
        assert_eq!(category_id("res"), derive_id("category", "res"));
        assert_eq!(
            category_revision_id(&category_id("res"), 2),
            derive_id("category-revision", &format!("{}\n2", category_id("res")))
        );
    }

    #[test]
    fn the_team_server_sets_up_the_starting_categories() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../infra/pocketbase/pb_hooks/catalog_starting_categories.json");
        let text = std::fs::read_to_string(file).unwrap();
        let on_server: Vec<CategoryDetails> = serde_json::from_str(&text).unwrap();
        assert_eq!(on_server, starting_categories());
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

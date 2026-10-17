//! The differences from the CSV catalog's own KiCad library that are
//! meant, each with why. A difference no rule here matches fails the
//! full-catalog proof, so a new difference needs a decision: fix the
//! generated libraries, or add a rule with its reason.
//!
//! Each rule matches as narrowly as it can, so a wrong value is never
//! excused. A rule about a value checks the value itself. A rule about a
//! whole column, such as `entry-per-revision`, relies on `compare_row`,
//! which checks that column in every row as its own difference: Entry by
//! name, Revision against the number in the entry's name, and Exclude from
//! BOM against the CSV catalog's own symbol and footprints. A column added
//! empty must be empty in every row.

use super::{Difference, Row};
use catalog::csv_import::{ColumnRoute, route_column};

/// One kind of meant difference.
pub struct Intended {
    /// A short name, used in reports and tests.
    pub name: &'static str,
    /// Why the difference is meant, for people.
    pub reason: &'static str,
    pub matches: fn(&Difference) -> bool,
}

/// The rule that explains `difference`, if any.
pub fn explain(difference: &Difference) -> Option<&'static Intended> {
    INTENDED.iter().find(|rule| (rule.matches)(difference))
}

/// Every meant difference.
pub static INTENDED: [Intended; 12] = [
    Intended {
        name: "parts-sqlite-is-older",
        reason: "parts.sqlite is a frozen copy that nothing can build again, so it keeps the values the CSV files had when it was made. Someone changed the CSV file since, and recorded the change in the CSV catalog fixture. The generated library is compared with today's CSV file instead.",
        matches: |difference| {
            matches!(
                difference,
                Difference::PartsSqliteIsOlder { recorded: true, .. }
            )
        },
    },
    Intended {
        name: "part-without-symbol",
        reason: "The row has no symbol: g-pcb.csv lists bare boards and has no Symbol column. A part without a symbol cannot be placed in a schematic, so KiCad gets no entry for it, as the CSV catalog's own library gave none. The part is still in the team catalog.",
        matches: |difference| matches!(difference, Difference::EntryMissing { symbol, .. } if symbol.trim().is_empty()),
    },
    Intended {
        name: "supplier-data",
        reason: "Supplier part numbers and prices change often and never make a revision, so they are not sent to KiCad. The team catalog keeps them with each import.",
        matches: |difference| match difference {
            Difference::ColumnLeftOut { column, .. } => is_supplier_data(column),
            Difference::FieldDiffers {
                field, generated, ..
            } => is_supplier_data(field) && generated.is_none(),
            _ => false,
        },
    },
    Intended {
        name: "mpn-not-approved",
        reason: "An MPN with no manufacturer (MPN2) or one still to be confirmed (MPN TBC) is not an approved manufacturer part, so KiCad does not show it. An import problem keeps it for someone to review.",
        matches: |difference| {
            matches!(difference, Difference::ColumnLeftOut { column, .. } if matches!(
                route_column(column),
                ColumnRoute::ExtraMpn | ColumnRoute::MpnToBeConfirmed | ColumnRoute::UnknownMpn
            ))
        },
    },
    Intended {
        name: "entry-per-revision",
        reason: "Each revision of a part is its own entry (ADR 0012): Entry is its name, such as RES-0000-0470_v1; Revision is its number; and Exclude from BOM is KiCad's own setting, worked out from the symbol and footprints.",
        matches: |difference| match difference {
            Difference::ColumnAdded { column, .. } => {
                matches!(column.as_str(), "Entry" | "Revision" | "Exclude from BOM")
            }
            Difference::FieldDiffers {
                field,
                entry,
                csv: None,
                generated: Some(number),
                ..
            } => field == "Revision" && entry.ends_with(&format!("_v{number}")),
            Difference::LayoutDiffers {
                setting,
                csv,
                generated,
                ..
            } => match setting.as_str() {
                "key" => csv == "IPN" && generated == "Entry",
                "exclude_from_bom" => csv.is_empty() && generated == "Exclude from BOM",
                _ => false,
            },
            _ => false,
        },
    },
    Intended {
        name: "value-column",
        reason: "Every entry has a Value column, so the text beside the symbol on a schematic comes from the part, never from the symbol's placeholder. A category without its own Value column fills it as the rule value-never-blank says.",
        matches: |difference| matches!(difference, Difference::ColumnAdded { column, .. } if column == "Value"),
    },
    Intended {
        name: "shared-columns",
        reason: "Every group has the same shared columns (Symbol, Footprint, Manufacturer, MPN, Description, Datasheet), so KiCad reads every part the same way. A category whose CSV file has no such column leaves it empty.",
        matches: |difference| {
            matches!(difference, Difference::ColumnAdded { column, empty: true, .. } if matches!(
                column.as_str(),
                "Symbol" | "Footprint" | "Manufacturer" | "MPN" | "Description" | "Datasheet"
            ))
        },
    },
    Intended {
        name: "value-never-blank",
        reason: "When the CSV catalog's library gives a part no value, or a blank one, the generated library shows its MPN, or else its IPN, so the schematic never shows the symbol's placeholder.",
        matches: |difference| match difference {
            Difference::FieldDiffers {
                field,
                csv,
                generated: Some(shown),
                row,
                ..
            } => {
                field == "Value"
                    && csv.as_deref().is_none_or(|value| value.trim().is_empty())
                    && is_fallback_value(shown, row)
            }
            Difference::ValueDiffers {
                column,
                csv,
                generated,
                row,
                ..
            } => column == "Value" && csv.trim().is_empty() && is_fallback_value(generated, row),
            _ => false,
        },
    },
    Intended {
        name: "every-field-shown",
        reason: "Every field of the part reaches KiCad. #gplm.kicad_dbl left some out of some groups, such as Type for transistors and Description for LEDs; the generated library shows the row's own value for them.",
        matches: |difference| match difference {
            Difference::FieldDiffers {
                field,
                csv: None,
                generated: Some(shown),
                row,
                ..
            } => column_of(field, row) == Some(shown.as_str()),
            _ => false,
        },
    },
    Intended {
        name: "revision-groups",
        reason: "Older and archived revisions have a group each (ADR 0012). They are listed even when empty, because KiCad reads the .kicad_dbl only when it starts.",
        matches: |difference| {
            matches!(difference, Difference::GroupAdded { table, csv_table: None, .. }
                if table == "older_revisions" || table == "archived_revisions")
        },
    },
    Intended {
        name: "empty-category",
        reason: "The CSV file has no rows yet (g-mec.csv), so #gplm.kicad_dbl left its table out. The generated library lists every category, even when empty, because KiCad reads the .kicad_dbl only when it starts: a part added later then shows up without a restart.",
        matches: |difference| {
            matches!(
                difference,
                Difference::GroupAdded {
                    rows: 0,
                    csv_table: Some(_),
                    csv_rows: 0,
                    csv_has_symbols: true,
                    ..
                }
            )
        },
    },
    Intended {
        name: "category-without-symbols",
        reason: "The CSV file has no Symbol column (g-pcb.csv: bare boards), so #gplm.kicad_dbl left its table out. The generated library lists every category, so the group is there but empty: a part without a symbol cannot be placed.",
        matches: |difference| {
            matches!(
                difference,
                Difference::GroupAdded {
                    rows: 0,
                    csv_table: Some(_),
                    csv_has_symbols: false,
                    ..
                }
            )
        },
    },
];

fn is_supplier_data(column: &str) -> bool {
    route_column(column) == ColumnRoute::SupplierData
}

/// Whether `shown` is what the generated library shows when the part has
/// no value of its own: its MPN, or else its IPN.
fn is_fallback_value(shown: &str, row: &Row) -> bool {
    let mpn = row.get("MPN").map_or("", |mpn| mpn.trim());
    if mpn.is_empty() {
        row.get("IPN").is_some_and(|ipn| ipn == shown)
    } else {
        shown == mpn || row.get("MPN").is_some_and(|mpn| mpn == shown)
    }
}

/// The CSV row's value behind the KiCad field `field`: the column of
/// the same name, or `Sim_Name` for KiCad's `Sim.Name`.
fn column_of<'a>(field: &str, row: &'a Row) -> Option<&'a str> {
    let column = match field.strip_prefix("Sim.") {
        Some(rest) => format!("Sim_{rest}"),
        None => field.to_owned(),
    };
    row.get(&column).map(String::as_str)
}

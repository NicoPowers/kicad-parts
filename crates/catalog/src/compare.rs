//! What differs between two saved part details, or between two part
//! revisions, in plain rows a person can read: one for the category, one
//! per changed field, one for the manufacturer parts, one for Excluded from BOM, and one per
//! symbol or footprint whose exact CAD item revision changed.

use crate::part::{
    CadReference, CadReferenceRole, CadReferenceTarget, DetailField, PartDetails, PartRevision,
};

/// One difference between two part details or two part revisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailDifference {
    pub what: DifferenceKind,
    /// `Category`, the field name, `Manufacturer parts`, `Excluded from BOM`, or
    /// `Symbol revision` or `Footprint revision` (with `choice N` for later
    /// footprint choices).
    pub label: String,
    /// Blank when the older one did not have it.
    pub before: String,
    /// Blank when the newer one does not have it.
    pub after: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DifferenceKind {
    /// The part's category; `before` and `after` are category codes.
    Category,
    Field,
    ManufacturerParts,
    /// The same reference bound to a different exact CAD item revision.
    /// `before` and `after` come from the `describe` callback of
    /// [`compare_revisions`].
    CadBinding,
    /// The Excluded from BOM setting, `yes` or `no`.
    ExcludedFromBom,
}

/// The label of a category difference, and the name of the category
/// choice in an edit and in a CSV review.
pub const CATEGORY_LABEL: &str = "Category";

/// Every difference from `before` to `after`: the category first, then
/// fields in `after`'s order (then fields only `before` had), then the
/// manufacturer parts.
pub fn compare_details(before: &PartDetails, after: &PartDetails) -> Vec<DetailDifference> {
    let mut differences = Vec::new();
    if before.category != after.category {
        differences.push(DetailDifference {
            what: DifferenceKind::Category,
            label: CATEGORY_LABEL.to_owned(),
            before: before.category.clone(),
            after: after.category.clone(),
        });
    }
    differences.extend(compare_fields(&before.fields, &after.fields));
    if before.manufacturer_parts != after.manufacturer_parts {
        let list = |details: &PartDetails| {
            details
                .manufacturer_parts
                .iter()
                .map(|part| format!("{} {}", part.manufacturer, part.mpn))
                .collect::<Vec<_>>()
                .join("\n")
        };
        differences.push(DetailDifference {
            what: DifferenceKind::ManufacturerParts,
            label: "Manufacturer parts".to_owned(),
            before: list(before),
            after: list(after),
        });
    }
    differences
}

/// Every difference from revision `before` to revision `after`: the Symbol
/// and Footprint fields, then Excluded from BOM, then CAD bindings.
/// `describe` turns a CAD reference into text, for example its library
/// reference and CAD item revision number.
pub fn compare_revisions(
    before: &PartRevision,
    after: &PartRevision,
    describe: impl Fn(&CadReference) -> String,
) -> Vec<DetailDifference> {
    let mut differences = compare_fields(&before.cad_fields, &after.cad_fields);
    if before.excluded_from_bom != after.excluded_from_bom {
        let text = |setting: bool| if setting { "Yes" } else { "No" }.to_owned();
        differences.push(DetailDifference {
            what: DifferenceKind::ExcludedFromBom,
            label: "Excluded from BOM".to_owned(),
            before: text(before.excluded_from_bom),
            after: text(after.excluded_from_bom),
        });
    }
    // CAD bindings by role and position: a changed reference already shows
    // as a changed Symbol or Footprint field, so only a binding to another
    // exact revision of the same reference is listed here.
    for reference in &after.cad_references {
        let old = before
            .cad_references
            .iter()
            .find(|old| old.role == reference.role && old.ordinal == reference.ordinal);
        let Some(old) = old else { continue };
        if old.reference == reference.reference && old.target != reference.target {
            differences.push(DetailDifference {
                what: DifferenceKind::CadBinding,
                label: binding_label(reference),
                before: describe(old),
                after: describe(reference),
            });
        }
    }
    differences
}

fn compare_fields(before: &[DetailField], after: &[DetailField]) -> Vec<DetailDifference> {
    let value_in = |fields: &[DetailField], name: &str| {
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
    };
    let mut differences = Vec::new();
    for field in after {
        let old = value_in(before, &field.name);
        if old.as_deref() != Some(field.value.as_str()) {
            differences.push(DetailDifference {
                what: DifferenceKind::Field,
                label: field.name.clone(),
                before: old.unwrap_or_default(),
                after: field.value.clone(),
            });
        }
    }
    for field in before {
        if value_in(after, &field.name).is_none() {
            differences.push(DetailDifference {
                what: DifferenceKind::Field,
                label: field.name.clone(),
                before: field.value.clone(),
                after: String::new(),
            });
        }
    }
    differences
}

fn binding_label(reference: &CadReference) -> String {
    let role = match reference.role {
        CadReferenceRole::Symbol => "Symbol",
        CadReferenceRole::Footprint => "Footprint",
    };
    if reference.ordinal == 0 {
        format!("{role} revision")
    } else {
        format!("{role} revision (choice {})", reference.ordinal + 1)
    }
}

/// `g-res:R_US` with what it is bound to, without looking anything up:
/// the number of the exact CAD item revision, `KiCad library` for an
/// external one, or why it did not resolve.
pub fn describe_binding(reference: &CadReference) -> String {
    match &reference.target {
        CadReferenceTarget::CadItemRevision { number, .. } => {
            format!("{} (revision {number})", reference.reference)
        }
        CadReferenceTarget::External => format!("{} (KiCad library)", reference.reference),
        CadReferenceTarget::Unresolved { status } => {
            format!("{} (not found: {status})", reference.reference)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part::{ManufacturerPart, PART_DETAILS_SCHEMA, PART_REVISION_SCHEMA};

    fn details(value: &str, parts: &[(&str, &str)]) -> PartDetails {
        PartDetails {
            schema: PART_DETAILS_SCHEMA.into(),
            ipn: "RES-0000-0470".into(),
            category: "res".into(),
            fields: vec![DetailField {
                name: "Value".into(),
                value: value.into(),
            }],
            manufacturer_parts: parts
                .iter()
                .map(|(manufacturer, mpn)| ManufacturerPart {
                    manufacturer: (*manufacturer).into(),
                    mpn: (*mpn).into(),
                })
                .collect(),
        }
    }

    fn revision(symbol: &str, number: u32) -> PartRevision {
        PartRevision {
            schema: PART_REVISION_SCHEMA.into(),
            ipn: "RES-0000-0470".into(),
            cad_fields: vec![DetailField {
                name: "Symbol".into(),
                value: symbol.into(),
            }],
            cad_references: vec![CadReference {
                role: CadReferenceRole::Symbol,
                ordinal: 0,
                reference: symbol.into(),
                target: CadReferenceTarget::CadItemRevision {
                    cad_item: "c".into(),
                    number,
                    snapshot_sha256: number.to_string().repeat(64),
                },
            }],
            excluded_from_bom: false,
        }
    }

    #[test]
    fn lists_changed_fields_and_manufacturer_parts() {
        let before = details("47", &[("Yageo", "RC0603FR-0747RL")]);
        assert!(compare_details(&before, &before).is_empty());
        let mut after = details("48", &[("Yageo", "RC0603FR-0747RL"), ("Vishay", "ALT-47")]);
        after.category = "rvr".into();
        let differences = compare_details(&before, &after);
        assert_eq!(
            differences
                .iter()
                .map(|d| (
                    d.what,
                    d.label.as_str(),
                    d.before.as_str(),
                    d.after.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                (DifferenceKind::Category, "Category", "res", "rvr"),
                (DifferenceKind::Field, "Value", "47", "48"),
                (
                    DifferenceKind::ManufacturerParts,
                    "Manufacturer parts",
                    "Yageo RC0603FR-0747RL",
                    "Yageo RC0603FR-0747RL\nVishay ALT-47"
                ),
            ]
        );
    }

    #[test]
    fn lists_changed_cad_fields_excluded_from_bom_and_bindings() {
        let before = revision("g-res:R_US", 1);
        assert!(compare_revisions(&before, &before, describe_binding).is_empty());
        let mut after = revision("g-res:R_US", 3);
        after.excluded_from_bom = true;
        let rows = |differences: Vec<DetailDifference>| {
            differences
                .into_iter()
                .map(|d| (d.what, d.label, d.before, d.after))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(compare_revisions(&before, &after, describe_binding)),
            [
                (
                    DifferenceKind::ExcludedFromBom,
                    "Excluded from BOM".to_owned(),
                    "No".to_owned(),
                    "Yes".to_owned()
                ),
                (
                    DifferenceKind::CadBinding,
                    "Symbol revision".to_owned(),
                    "g-res:R_US (revision 1)".to_owned(),
                    "g-res:R_US (revision 3)".to_owned()
                ),
            ]
        );
        // Another reference shows as a changed field, not as a binding.
        let other = revision("Device:R", 1);
        assert_eq!(
            rows(compare_revisions(&before, &other, describe_binding)),
            [(
                DifferenceKind::Field,
                "Symbol".to_owned(),
                "g-res:R_US".to_owned(),
                "Device:R".to_owned()
            )]
        );
    }
}

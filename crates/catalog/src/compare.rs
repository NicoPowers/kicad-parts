//! What differs between two revisions' part details, in plain rows a
//! person can read: one per changed field, one for the manufacturer parts,
//! one for Excluded from BOM, and one per symbol or footprint whose exact
//! CAD item revision changed.

use crate::part::{CadReference, CadReferenceRole, CadReferenceTarget, PartDetails};

/// One difference between two revisions' part details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailDifference {
    pub what: DifferenceKind,
    /// The field name, `Manufacturer parts`, or `Symbol` or `Footprint`
    /// (with `choice N` for later footprint choices).
    pub label: String,
    /// Blank when the older details did not have it.
    pub before: String,
    /// Blank when the newer details do not have it.
    pub after: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DifferenceKind {
    Field,
    ManufacturerParts,
    /// The same or another reference bound to a different exact CAD item
    /// revision. `before` and `after` come from the `describe` callback of
    /// [`compare_details`].
    CadBinding,
    /// The Excluded from BOM setting, `yes` or `no`.
    ExcludedFromBom,
}

/// Every difference from `before` to `after`, fields first in `after`'s
/// order (then fields only `before` had), then manufacturer parts, then
/// Excluded from BOM, then CAD bindings. `describe` turns a CAD reference into text, for example its
/// library reference and CAD item revision number.
pub fn compare_details(
    before: &PartDetails,
    after: &PartDetails,
    describe: impl Fn(&CadReference) -> String,
) -> Vec<DetailDifference> {
    let mut differences = Vec::new();
    let value_in = |details: &PartDetails, name: &str| details.field(name).map(str::to_owned);
    for field in &after.fields {
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
    for field in &before.fields {
        if after.field(&field.name).is_none() {
            differences.push(DetailDifference {
                what: DifferenceKind::Field,
                label: field.name.clone(),
                before: field.value.clone(),
                after: String::new(),
            });
        }
    }
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
    if before.excluded_from_bom != after.excluded_from_bom {
        let text = |setting: bool| if setting { "yes" } else { "no" }.to_owned();
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

/// `g-pas:R_US` with what it is bound to, without looking anything up:
/// the first 8 hex digits of an exact CAD item revision's hash, `KiCad
/// library` for an external one, or why it did not resolve.
pub fn describe_binding(reference: &CadReference) -> String {
    match &reference.target {
        CadReferenceTarget::CadItemRevision {
            snapshot_sha256, ..
        } => format!(
            "{} (exact revision {})",
            reference.reference,
            &snapshot_sha256[..snapshot_sha256.len().min(8)]
        ),
        CadReferenceTarget::External => format!("{} (KiCad library)", reference.reference),
        CadReferenceTarget::Unresolved { status } => {
            format!("{} (not found: {status})", reference.reference)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part::{DetailField, ManufacturerPart, PART_DETAILS_SCHEMA};

    fn details(value: &str, sha: &str, parts: &[(&str, &str)]) -> PartDetails {
        PartDetails {
            schema: PART_DETAILS_SCHEMA.into(),
            ipn: "RES-0000-0470".into(),
            fields: vec![
                DetailField {
                    name: "Value".into(),
                    value: value.into(),
                },
                DetailField {
                    name: "Symbol".into(),
                    value: "g-pas:R_US".into(),
                },
            ],
            manufacturer_parts: parts
                .iter()
                .map(|(manufacturer, mpn)| ManufacturerPart {
                    manufacturer: (*manufacturer).into(),
                    mpn: (*mpn).into(),
                })
                .collect(),
            cad_references: vec![CadReference {
                role: CadReferenceRole::Symbol,
                ordinal: 0,
                reference: "g-pas:R_US".into(),
                target: CadReferenceTarget::CadItemRevision {
                    cad_item: "c".into(),
                    snapshot_sha256: sha.into(),
                },
            }],
            excluded_from_bom: false,
        }
    }

    #[test]
    fn lists_changed_fields_manufacturer_parts_excluded_from_bom_and_cad_bindings() {
        let before = details("47", &"a".repeat(64), &[("Yageo", "RC0603FR-0747RL")]);
        assert!(compare_details(&before, &before, describe_binding).is_empty());
        let mut after = details(
            "48",
            &"b".repeat(64),
            &[("Yageo", "RC0603FR-0747RL"), ("Vishay", "ALT-47")],
        );
        after.excluded_from_bom = true;
        let differences = compare_details(&before, &after, describe_binding);
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
                (DifferenceKind::Field, "Value", "47", "48"),
                (
                    DifferenceKind::ManufacturerParts,
                    "Manufacturer parts",
                    "Yageo RC0603FR-0747RL",
                    "Yageo RC0603FR-0747RL\nVishay ALT-47"
                ),
                (
                    DifferenceKind::ExcludedFromBom,
                    "Excluded from BOM",
                    "no",
                    "yes"
                ),
                (
                    DifferenceKind::CadBinding,
                    "Symbol revision",
                    "g-pas:R_US (exact revision aaaaaaaa)",
                    "g-pas:R_US (exact revision bbbbbbbb)"
                ),
            ]
        );
    }
}

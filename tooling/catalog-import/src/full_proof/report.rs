//! The full-catalog proof in words a hardware engineer reads: what was
//! imported, what was checked, and anything lost or different, with the
//! reason.

use super::Proof;
use crate::csv_kicad_library::{Difference, Found};
use std::collections::BTreeMap;
use std::fmt::Write;

/// The report. With `every_difference`, each difference from
/// `parts.sqlite` is listed on its own line; otherwise they are counted
/// by kind. Differences with no reason are always listed one by one.
pub fn text(proof: &Proof, every_difference: bool) -> String {
    let mut out = String::new();
    let failures = proof.failures();
    let _ = writeln!(out, "Moving the whole CSV catalog into a team catalog");
    let _ = writeln!(out, "================================================");
    let _ = writeln!(out);
    if failures.is_empty() {
        let _ = writeln!(
            out,
            "PASSED: nothing was lost, and every difference from the CSV catalog's own KiCad library has a reason."
        );
    } else {
        let _ = writeln!(
            out,
            "FAILED: {} thing{} lost or different without a reason. Each is listed at the end.",
            failures.len(),
            plural(failures.len())
        );
    }

    let rows: usize = proof.reports.iter().map(|report| report.rows).sum();
    let fields: usize = proof.reports.iter().map(|report| report.fields).sum();
    let whole = &proof.whole;
    section(&mut out, "1. Import");
    let _ = writeln!(
        out,
        "{} CSV files with {rows} rows were imported one after another into a new team server, as a team would.",
        proof.csv_files.len()
    );
    let applied = proof
        .first_outcomes
        .iter()
        .filter(|(_, outcome)| *outcome == "applied")
        .count();
    let _ = writeln!(
        out,
        "{applied} of {} imports saved a change.",
        proof.first_outcomes.len()
    );
    let _ = writeln!(
        out,
        "{}",
        if proof.second_created_nothing {
            "Importing every file again made nothing new."
        } else {
            "Importing every file again made something new. See the end."
        }
    );
    let _ = writeln!(out, "{}", proof.library_check);

    section(&mut out, "2. Nothing lost");
    let _ = writeln!(
        out,
        "Every field of every row is in the team catalog: {fields} fields in {rows} rows, checked one by one against the CSV files."
    );
    let _ = writeln!(
        out,
        "{} parts, each from exactly one CSV file. Every record ID comes from its content, so importing again finds the same records.",
        whole.parts
    );
    let _ = writeln!(
        out,
        "substitutes.csv: {} row{}, each one accounted for.",
        whole.substitutes_rows,
        plural(whole.substitutes_rows)
    );
    let _ = writeln!(
        out,
        "Stored with their license: {} symbols, {} footprints and {} 3D model files.",
        whole.stored_symbols, whole.stored_footprints, whole.stored_models
    );
    let problems: usize = whole.import_problems.iter().map(|group| group.count).sum();
    let _ = writeln!(
        out,
        "{problems} import problems keep what could not become a part of the catalog, so nothing is dropped:"
    );
    for group in &whole.import_problems {
        let _ = writeln!(
            out,
            "  {:>3} {}: {}",
            group.count, group.code, group.meaning
        );
    }
    if !whole.not_imported.is_empty() {
        let _ = writeln!(out, "Not imported, and why:");
        let mut by_reason: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for item in &whole.not_imported {
            by_reason.entry(item.reason).or_default().push(&item.what);
        }
        for (reason, items) in by_reason {
            for item in items {
                let _ = writeln!(out, "  - {item}");
            }
            let _ = writeln!(out, "    Why: {reason}");
        }
    }

    section(&mut out, "3. KiCad libraries");
    let libraries = &proof.libraries;
    let _ = writeln!(
        out,
        "{} entries, one per part revision, in {} groups; {} files in all.",
        libraries.current + libraries.older + libraries.archived,
        proof.comparison.generated_groups,
        proof.library_files
    );
    let _ = writeln!(
        out,
        "{}",
        if proof.update_again.is_empty() {
            "Updating them again from the same team server wrote nothing."
        } else {
            "Updating them again from the same team server changed files. See the end."
        }
    );
    let _ = writeln!(
        out,
        "{}",
        if proof.second_server.is_empty() {
            "A second, separate team server gave byte-identical libraries, apart from the file that names the team (.kicad-parts.json)."
        } else {
            "A second, separate team server gave different libraries. See the end."
        }
    );

    section(
        &mut out,
        "4. Compared with the CSV catalog's own KiCad library (database/parts.sqlite)",
    );
    let comparison = &proof.comparison;
    let _ = writeln!(
        out,
        "{} of {} rows have their entry, named <IPN>_v1. Every value is the same, apart from the differences below, each with its reason.",
        comparison.entries_found, comparison.csv_rows
    );
    let _ = writeln!(
        out,
        "Each Symbol and Footprint points at a generated copy with the CSV catalog's own bytes, or at KiCad's own library as the CSV writes it. Each 3D model link leads to a copy of the CSV catalog's model file."
    );
    let _ = writeln!(
        out,
        "The CSV catalog's library lists {} groups; the generated one lists {}.",
        comparison.csv_groups, comparison.generated_groups
    );
    let _ = writeln!(out, "Differences, each with its reason:");
    write_differences(
        &mut out,
        comparison.differences.iter().filter(|f| f.rule.is_some()),
        every_difference,
    );

    section(&mut out, "5. The CSV catalog is unchanged");
    let _ = writeln!(
        out,
        "{}",
        if proof.csv_catalog_changes.is_empty() {
            format!(
                "All {} files in database, symbols, footprints and 3d-models have the same bytes as before.",
                proof.csv_catalog_files
            )
        } else {
            "Some files of the CSV catalog changed. See the end.".to_owned()
        }
    );

    let timings = &proof.timings;
    section(&mut out, "Time");
    let _ = writeln!(
        out,
        "Import twice {:.1} s, checks {:.1} s, KiCad libraries {:.1} s, second team server {:.1} s, comparison {:.1} s; {:.1} s in all.",
        timings.import.as_secs_f64(),
        timings.checks.as_secs_f64(),
        timings.libraries.as_secs_f64(),
        timings.second_server.as_secs_f64(),
        timings.comparison.as_secs_f64(),
        timings.total.as_secs_f64()
    );

    if !failures.is_empty() {
        section(&mut out, "Lost or different without a reason");
        for failure in &failures {
            let _ = writeln!(out, "- {failure}");
        }
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "To fix one, correct the CSV file or library it names, commit, and run the proof again. If a difference is meant, add a rule with its reason in tooling/catalog-import/src/csv_kicad_library/intended.rs."
        );
    }
    out
}

fn section(out: &mut String, title: &str) {
    let _ = writeln!(out);
    let _ = writeln!(out, "{title}");
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// What a difference is about, for counting: a column, a KiCad field, an
/// entry or a group.
fn subject(difference: &Difference) -> String {
    match difference {
        Difference::ColumnLeftOut { column, .. } | Difference::ColumnAdded { column, .. } => {
            format!("column {column}")
        }
        Difference::FieldDiffers { field, .. } => format!("KiCad field {field}"),
        Difference::EntryMissing { ipn, .. } => format!("no entry for {ipn}"),
        Difference::GroupAdded { table, .. } => format!("group {table}"),
        Difference::LayoutDiffers { setting, .. } => format!("group setting {setting}"),
        other => other.place(),
    }
}

/// The differences by rule: how many, about what, and why. With `each`,
/// every difference on its own line.
fn write_differences<'a>(out: &mut String, found: impl Iterator<Item = &'a Found>, each: bool) {
    let mut by_rule: BTreeMap<&str, (&str, Vec<&Difference>)> = BTreeMap::new();
    for item in found {
        let entry = by_rule
            .entry(item.rule.unwrap_or_default())
            .or_insert((item.reason.unwrap_or_default(), Vec::new()));
        entry.1.push(&item.difference);
    }
    if by_rule.is_empty() {
        let _ = writeln!(out, "  None.");
    }
    for (rule, (reason, differences)) in by_rule {
        let _ = writeln!(
            out,
            "  {rule} ({} difference{})",
            differences.len(),
            plural(differences.len())
        );
        if each {
            for difference in &differences {
                let _ = writeln!(out, "    - {}", difference.describe());
            }
        } else {
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for difference in &differences {
                *counts.entry(subject(difference)).or_default() += 1;
            }
            let listed: Vec<String> = counts
                .into_iter()
                .map(|(what, count)| {
                    if count == 1 {
                        what
                    } else {
                        format!("{what} ({count})")
                    }
                })
                .collect();
            let _ = writeln!(out, "    {}", listed.join("; "));
        }
        let _ = writeln!(out, "    Why: {reason}");
    }
}

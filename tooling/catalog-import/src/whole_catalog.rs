//! The checks that only make sense with every CSV file imported into one
//! team catalog. The zero-loss report (`zero_loss.rs`) checks one CSV file
//! at a time; these check what lies between the files:
//!
//! - no IPN is a row of two CSV files, and every part in the team catalog
//!   came from exactly one file's latest import;
//! - every record's ID comes from its content (ADR 0006), so importing
//!   again finds the same records instead of making copies;
//! - every `substitutes.csv` row became part of the part it names, or an
//!   import problem keeps it;
//! - every symbol, footprint and 3D model the CSV rows use is stored, with
//!   its license;
//! - every import problem the CSV catalog fixture expects is recorded, and
//!   every recorded one is explained by a CSV file.
//!
//! The few things the fixture lists that an import never takes in are
//! named, each with its reason, in [`NotForTheImport`]. Anything else
//! missing is lost, and the proof fails.

use crate::zero_loss::{ZeroLossReport, latest_import_run};
use catalog::category::category_id;
use catalog::csv_import::SUBSTITUTES_PATH;
use catalog::pocketbase::CatalogReadback;
use catalog::{
    BlobId, CadItemId, CadItemKind, CadItemRevisionId, PartDetailsId, PartId, PartRevisionId,
};
use csv_catalog_fixture::Manifest;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// What the whole-catalog checks found.
#[derive(Clone, Debug, Default, Serialize)]
pub struct WholeCatalog {
    /// Rows in all the CSV files together.
    pub rows: usize,
    /// Parts in the team catalog.
    pub parts: usize,
    pub substitutes_rows: usize,
    /// Symbols, footprints and 3D model files the rows use, stored with
    /// their license.
    pub stored_symbols: usize,
    pub stored_footprints: usize,
    pub stored_models: usize,
    /// Every import problem in the team catalog, by kind.
    pub import_problems: Vec<ProblemGroup>,
    /// What the CSV catalog fixture lists that an import never takes in.
    pub not_imported: Vec<NotImported>,
    /// What is lost. Empty means nothing is.
    pub lost: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProblemGroup {
    pub code: String,
    pub count: usize,
    /// What it means, in a sentence.
    pub meaning: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NotImported {
    pub what: String,
    pub reason: &'static str,
}

/// The kinds of thing the CSV catalog fixture lists that no import takes
/// in, each with why that loses nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotForTheImport {
    /// `database/update_db.sh` leaves a table out.
    OldShellScript,
    /// A 3D model link, or its problem, in a footprint file no CSV row
    /// binds.
    FootprintNoRowBinds,
}

impl NotForTheImport {
    pub fn reason(self) -> &'static str {
        match self {
            Self::OldShellScript => {
                "It is about database/update_db.sh, the old script that built parts.sqlite. The import reads the CSV files themselves, so what the script leaves out is not lost."
            }
            Self::FootprintNoRowBinds => {
                "It is in a footprint file that no CSV row binds: rows name it without its library, as a choice for KiCad's footprint filter, or name KiCad's own footprint. The import stores only footprints a row binds, so it has no 3D model to keep. The library check lists the file."
            }
        }
    }
}

/// What each kind of import problem means, for people. A kind not listed
/// here shows one of its own messages instead.
const MEANINGS: [(&str, &str); 7] = [
    (
        "missing-3d-model-reference",
        "A footprint's 3D model link names a file that is not there.",
    ),
    (
        "unresolved-3d-model-reference",
        "A footprint's 3D model link works on one computer only: it uses an absolute path or a path variable.",
    ),
    (
        "missing-member-local-symbol-reference",
        "A row names a symbol that its library does not have.",
    ),
    (
        "missing-member-local-footprint-reference",
        "A row names a footprint that its library does not have.",
    ),
    (
        "mpn-to-be-confirmed",
        "An MPN still to be confirmed (column MPN TBC) is kept here, not as an approved manufacturer part.",
    ),
    (
        "mpn-without-manufacturer",
        "An extra MPN (such as column MPN2) names no manufacturer, so it is kept here, not as a manufacturer part.",
    ),
    (
        "substitutes-csv-row-without-part",
        "A substitutes.csv row names an IPN that no row of its CSV file has.",
    ),
];

/// Runs every whole-catalog check on `readback`, the team catalog with
/// every CSV file imported, given each file's zero-loss report.
pub fn check(
    oracle: &Manifest,
    readback: &CatalogReadback,
    reports: &[ZeroLossReport],
) -> WholeCatalog {
    let mut lost = Vec::new();
    let mut not_imported = Vec::new();

    let csv_files: Vec<(&str, Vec<&str>)> = oracle
        .catalog
        .categories
        .iter()
        .map(|category| {
            let document = &category.document;
            let ipn = document.headers.iter().position(|header| header == "IPN");
            let ipns = document
                .rows
                .iter()
                .filter_map(|row| ipn.and_then(|column| row.get(column)))
                .map(String::as_str)
                .collect();
            (document.path.as_str(), ipns)
        })
        .collect();
    lost.extend(ipns_in_two_files(&csv_files));

    let runs: Vec<(String, Vec<String>)> = reports
        .iter()
        .map(|report| {
            let ipns = latest_import_run(readback, &report.csv_file)
                .map(|(_, run)| run.parts.into_iter().map(|part| part.ipn).collect())
                .unwrap_or_default();
            (report.csv_file.clone(), ipns)
        })
        .collect();
    let stored: Vec<&str> = readback
        .parts
        .iter()
        .map(|part| part.ipn.as_str())
        .collect();
    lost.extend(parts_from_one_file(&runs, &stored));
    lost.extend(ids_not_from_content(readback));

    let imported: BTreeSet<&str> = runs
        .iter()
        .flat_map(|(_, ipns)| ipns.iter().map(String::as_str))
        .collect();
    let (substitutes_lost, orphans_kept) = substitutes_rows(oracle, readback, &imported);
    lost.extend(substitutes_lost);

    let stored_cad = stored_cad(oracle, readback, reports);
    lost.extend(stored_cad.lost);
    not_imported.extend(stored_cad.not_imported);

    let explained: BTreeSet<&str> = reports
        .iter()
        .flat_map(|report| report.explained_problems.iter().map(String::as_str))
        .chain(orphans_kept.iter().map(String::as_str))
        .collect();
    let (problems_lost, problems_not_imported) =
        fixture_problems(oracle, reports, &stored_cad.bound_footprints);
    lost.extend(problems_lost);
    not_imported.extend(problems_not_imported);
    for problem in &readback.problems {
        if !explained.contains(problem.id.as_str()) {
            lost.push(format!(
                "The team catalog has an import problem that no CSV file explains: {} at {} in {}.",
                problem.code, problem.location, problem.path
            ));
        }
    }

    WholeCatalog {
        rows: oracle.catalog.row_count,
        parts: readback.parts.len(),
        substitutes_rows: oracle.substitutes.rows.len(),
        stored_symbols: stored_cad.symbols,
        stored_footprints: stored_cad.footprints,
        stored_models: stored_cad.models,
        import_problems: problem_groups(readback),
        not_imported,
        lost,
    }
}

/// IPNs that are rows of more than one CSV file, given each file's path
/// and IPNs. One part comes from one file, so the second file's row would
/// replace the first one's details. Repeats within one file are the
/// file's own `duplicate-ipn` import problem.
pub fn ipns_in_two_files(files: &[(&str, Vec<&str>)]) -> Vec<String> {
    let mut found: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (path, ipns) in files {
        for ipn in ipns.iter().filter(|ipn| !ipn.is_empty()) {
            found.entry(ipn).or_default().insert(path);
        }
    }
    found
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(ipn, paths)| {
            let paths: Vec<&str> = paths.into_iter().collect();
            format!(
                "{ipn} is a row of {}. A part comes from one CSV file only, so one file's row would replace the other's.",
                paths.join(" and ")
            )
        })
        .collect()
}

/// Checks that every stored part (`stored`, by IPN) is a part of exactly
/// one CSV file's latest import run (`runs`: each file's parts).
pub fn parts_from_one_file(runs: &[(String, Vec<String>)], stored: &[&str]) -> Vec<String> {
    let mut files: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (csv_file, ipns) in runs {
        for ipn in ipns {
            files.entry(ipn).or_default().push(csv_file);
        }
    }
    let mut lost = Vec::new();
    for ipn in stored {
        match files.get(ipn).map(Vec::as_slice) {
            Some([_]) => {}
            None | Some([]) => lost.push(format!(
                "{ipn} is in the team catalog, but no CSV file's latest import made it."
            )),
            Some(several) => lost.push(format!(
                "{ipn} was made by the latest imports of {}, so one file's row replaced the other's.",
                several.join(" and ")
            )),
        }
    }
    lost
}

/// Records whose ID is not the one their content gives (ADR 0006). The
/// import finds a record by the ID its content gives, so such a record
/// would be copied by the next import instead of found.
fn ids_not_from_content(readback: &CatalogReadback) -> Vec<String> {
    let mut lost = Vec::new();
    // (what, stored ID, the ID its content gives)
    let mut ids: Vec<(String, String, String)> = Vec::new();
    let mut expect = |what: String, stored: &str, derived: String| {
        ids.push((what, stored.to_owned(), derived));
    };
    for part in &readback.parts {
        expect(
            format!("Part {}", part.ipn),
            &part.id,
            PartId::for_text(&part.ipn).to_string(),
        );
    }
    let ipn_of = |part: &str| {
        readback
            .parts
            .iter()
            .find(|stored| stored.id == part)
            .map_or(part.to_owned(), |stored| stored.ipn.clone())
    };
    for revision in &readback.part_revisions {
        expect(
            format!("Revision {} of {}", revision.number, ipn_of(&revision.part)),
            &revision.id,
            PartRevisionId::new(&PartId::from_stored(&revision.part), revision.number).to_string(),
        );
    }
    for details in &readback.part_details {
        expect(
            format!("Details {} of {}", details.number, ipn_of(&details.part)),
            &details.id,
            PartDetailsId::new(&PartId::from_stored(&details.part), details.number).to_string(),
        );
    }
    for item in &readback.cad_items {
        match CadItemKind::parse(&item.kind) {
            Some(kind) => expect(
                format!("The {} {}", item.kind, item.key),
                &item.id,
                CadItemId::new(kind, &item.key).to_string(),
            ),
            None => expect(
                format!("The CAD item {} of unknown kind {}", item.key, item.kind),
                &item.id,
                String::new(),
            ),
        }
    }
    for revision in &readback.cad_item_revisions {
        expect(
            format!(
                "Revision {} of CAD item {}",
                revision.number, revision.cad_item
            ),
            &revision.id,
            CadItemRevisionId::new(&CadItemId::from_stored(&revision.cad_item), revision.number)
                .to_string(),
        );
    }
    for blob in &readback.blobs {
        expect(
            format!("The file with SHA-256 {}", blob.sha256),
            &blob.id,
            BlobId::new(&blob.sha256).to_string(),
        );
    }
    for category in &readback.categories {
        expect(
            format!("The category {}", category.code),
            &category.id,
            category_id(&category.code),
        );
    }
    for (what, stored, derived) in ids {
        if stored != derived {
            lost.push(format!(
                "{what} has the ID {stored}, but its content gives {derived:?}, so the next import would make a second copy."
            ));
        }
    }
    lost
}

/// Checks every `substitutes.csv` row. A row naming a part a CSV file
/// imported (`imported`) was checked by that file's zero-loss report.
/// Any other row must be kept by a `substitutes-csv-row-without-part`
/// import problem. Returns what is lost and the IDs of the problems that
/// keep rows.
fn substitutes_rows(
    oracle: &Manifest,
    readback: &CatalogReadback,
    imported: &BTreeSet<&str>,
) -> (Vec<String>, Vec<String>) {
    let document = &oracle.substitutes;
    let ipn_column = document.headers.iter().position(|header| header == "IPN");
    let rows: Vec<(usize, &str)> = document
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let ipn = ipn_column
                .and_then(|column| row.get(column))
                .map_or("", String::as_str);
            (index + 1, ipn)
        })
        .collect();
    let kept: Vec<(&str, &str, &str)> = readback
        .problems
        .iter()
        .filter(|problem| {
            problem.code == "substitutes-csv-row-without-part" && problem.path == SUBSTITUTES_PATH
        })
        .map(|problem| {
            (
                problem.id.as_str(),
                problem.subject.as_str(),
                problem.location.as_str(),
            )
        })
        .collect();
    substitutes_rows_accounted(&rows, imported, &kept)
}

/// [`substitutes_rows`] on plain values: each row's number and IPN, the
/// imported IPNs, and each keeping problem's ID, subject and location.
pub fn substitutes_rows_accounted(
    rows: &[(usize, &str)],
    imported: &BTreeSet<&str>,
    kept: &[(&str, &str, &str)],
) -> (Vec<String>, Vec<String>) {
    let mut lost = Vec::new();
    let mut used = Vec::new();
    for (number, ipn) in rows {
        if imported.contains(ipn) {
            continue;
        }
        let location = format!("rows[{number}]");
        match kept
            .iter()
            .find(|(_, subject, at)| subject == ipn && *at == location)
        {
            Some((id, _, _)) => used.push((*id).to_owned()),
            None => lost.push(format!(
                "substitutes.csv row {number} names {ipn}, which no CSV file has, and no import problem keeps it, so its manufacturer part is lost."
            )),
        }
    }
    (lost, used)
}

/// What [`stored_cad`] found.
struct StoredCad {
    symbols: usize,
    footprints: usize,
    models: usize,
    /// The repository paths of the footprint files a part binds.
    bound_footprints: BTreeSet<String>,
    not_imported: Vec<NotImported>,
    lost: Vec<String>,
}

/// Checks that every symbol and footprint a CSV row uses from the CSV
/// catalog's own libraries, and every 3D model those footprints link to,
/// is stored with its license.
fn stored_cad(
    oracle: &Manifest,
    readback: &CatalogReadback,
    reports: &[ZeroLossReport],
) -> StoredCad {
    // Each file's report checked the bytes; here only that some file
    // stored each one, with a license.
    let stored: BTreeMap<(&str, &str), &crate::zero_loss::CadItemAccount> = reports
        .iter()
        .flat_map(|report| &report.cad_items)
        .filter(|account| account.cad_item_revision.is_some() && account.license.is_some())
        .map(|account| ((account.kind.as_str(), account.reference.as_str()), account))
        .collect();
    let mut found = StoredCad {
        symbols: 0,
        footprints: 0,
        models: 0,
        bound_footprints: BTreeSet::new(),
        not_imported: Vec::new(),
        lost: Vec::new(),
    };
    for reference in oracle
        .assets
        .symbol_references
        .iter()
        .chain(&oracle.assets.footprint_references)
        .filter(|reference| reference.local && reference.status == "resolved")
    {
        match stored.get(&(reference.kind.as_str(), reference.reference.as_str())) {
            Some(_) if reference.kind == "symbol" => found.symbols += 1,
            Some(account) => {
                found.footprints += 1;
                if let Some(path) = &account.oracle_asset_path {
                    found.bound_footprints.insert(path.clone());
                }
            }
            None => found.lost.push(format!(
                "The {} {} that {} uses is not stored with its license.",
                reference.kind,
                reference.reference,
                reference.source_ipns.join(", ")
            )),
        }
    }
    // Two footprints may link to one 3D model file; it is stored once.
    let mut models = BTreeSet::new();
    for model in oracle
        .assets
        .model_references
        .iter()
        .filter(|model| model.status == "resolved")
    {
        let Some(path) = &model.asset_path else {
            continue;
        };
        if !found.bound_footprints.contains(&model.source_footprint) {
            found.not_imported.push(NotImported {
                what: format!(
                    "The 3D model {path}, linked from {}",
                    model.source_footprint
                ),
                reason: NotForTheImport::FootprintNoRowBinds.reason(),
            });
            continue;
        }
        let empty = oracle
            .assets
            .models_3d
            .iter()
            .any(|file| &file.path == path && file.bytes == 0);
        let id = CadItemId::new(CadItemKind::Model3d, path).to_string();
        let revisions: Vec<_> = readback
            .cad_item_revisions
            .iter()
            .filter(|revision| revision.cad_item == id)
            .collect();
        if empty && revisions.is_empty() {
            // An empty file is kept as an import problem, which the
            // footprint's CSV file explains.
            continue;
        }
        if !revisions.is_empty()
            && revisions
                .iter()
                .all(|revision| !revision.license.is_empty())
        {
            models.insert(path);
        } else {
            found.lost.push(format!(
                "The 3D model {path}, linked from {}, is not stored with its license.",
                model.source_footprint
            ));
        }
    }
    found.models = models.len();
    found
}

/// Checks that every import problem the CSV catalog fixture expects was
/// recorded and explained by some CSV file's report, or is one of the
/// kinds no import takes in. Returns what is lost and what is not for the
/// import.
fn fixture_problems(
    oracle: &Manifest,
    reports: &[ZeroLossReport],
    bound_footprints: &BTreeSet<String>,
) -> (Vec<String>, Vec<NotImported>) {
    let recorded: BTreeSet<(&str, &str, &str)> = reports
        .iter()
        .flat_map(|report| &report.oracle_import_problems)
        .filter(|account| account.stored.is_some())
        .map(|account| {
            (
                account.code.as_str(),
                account.path.as_str(),
                account.location.as_str(),
            )
        })
        .collect();
    let mut lost = Vec::new();
    let mut not_imported = Vec::new();
    for problem in &oracle.import_problems {
        let key = (
            problem.code.as_str(),
            problem.path.as_str(),
            problem.location.as_str(),
        );
        if recorded.contains(&key) {
            continue;
        }
        let why = if problem.code == "update-script-omits-csv-table" {
            Some(NotForTheImport::OldShellScript)
        } else if problem.path.starts_with("footprints/")
            && !bound_footprints.contains(&problem.path)
        {
            Some(NotForTheImport::FootprintNoRowBinds)
        } else {
            None
        };
        match why {
            Some(why) => not_imported.push(NotImported {
                what: format!(
                    "The import problem {} at {} in {}",
                    problem.code, problem.location, problem.path
                ),
                reason: why.reason(),
            }),
            None => lost.push(format!(
                "The import problem {} at {} in {} is not recorded, so what it keeps is lost.",
                problem.code, problem.location, problem.path
            )),
        }
    }
    (lost, not_imported)
}

/// Every import problem in the team catalog, counted by kind.
fn problem_groups(readback: &CatalogReadback) -> Vec<ProblemGroup> {
    let mut groups: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for problem in &readback.problems {
        let group = groups
            .entry(problem.code.as_str())
            .or_insert((0, problem.message.as_str()));
        group.0 += 1;
    }
    groups
        .into_iter()
        .map(|(code, (count, example))| ProblemGroup {
            code: code.to_owned(),
            count,
            meaning: MEANINGS
                .iter()
                .find(|(known, _)| *known == code)
                .map_or_else(|| example.to_owned(), |(_, meaning)| (*meaning).to_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipn_in_two_csv_files_is_lost() {
        let files = [
            ("database/g-res.csv", vec!["RES-0000-0470", "RES-0000-1000"]),
            ("database/g-cap.csv", vec!["CAP-0000-0100", "RES-0000-0470"]),
            // A repeat inside one file is that file's own import problem.
            (
                "database/g-ind.csv",
                vec!["IND-0000-0221", "IND-0000-0221", ""],
            ),
        ];
        let lost = ipns_in_two_files(&files);
        assert_eq!(lost.len(), 1, "{lost:?}");
        assert!(
            lost[0].starts_with(
                "RES-0000-0470 is a row of database/g-cap.csv and database/g-res.csv."
            )
        );
    }

    #[test]
    fn every_part_comes_from_exactly_one_csv_file() {
        let runs = [
            (
                "g-res".to_owned(),
                vec!["RES-1".to_owned(), "SHARED".to_owned()],
            ),
            (
                "g-cap".to_owned(),
                vec!["CAP-1".to_owned(), "SHARED".to_owned()],
            ),
        ];
        let lost = parts_from_one_file(&runs, &["RES-1", "CAP-1", "SHARED", "STRAY"]);
        assert_eq!(
            lost,
            [
                "SHARED was made by the latest imports of g-res and g-cap, so one file's row replaced the other's.",
                "STRAY is in the team catalog, but no CSV file's latest import made it.",
            ]
        );
        assert!(parts_from_one_file(&runs[..1], &["RES-1", "SHARED"]).is_empty());
    }

    /// A substitutes.csv row for an imported part was checked by its CSV
    /// file. Any other row needs the import problem that keeps it; without
    /// one, its manufacturer part is lost.
    #[test]
    fn a_substitutes_row_for_no_part_needs_an_import_problem() {
        let imported: BTreeSet<&str> = ["RES-1"].into();
        let rows = [(1, "RES-1"), (2, "RES-9"), (3, "XYZ-1")];
        let kept = [
            ("problem-a", "RES-9", "rows[2]"),
            ("problem-b", "XYZ-1", "rows[9]"),
        ];
        let (lost, used) = substitutes_rows_accounted(&rows, &imported, &kept);
        assert_eq!(used, ["problem-a"]);
        assert_eq!(lost.len(), 1);
        assert!(lost[0].starts_with("substitutes.csv row 3 names XYZ-1"));
    }

    #[test]
    fn every_kind_of_problem_people_see_has_a_meaning() {
        for (code, meaning) in MEANINGS {
            assert!(!code.is_empty() && meaning.ends_with('.'), "{code}");
        }
    }
}

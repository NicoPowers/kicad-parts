//! The full-catalog proof (`catalog-import full-pilot`): moving the whole
//! CSV catalog into a team catalog loses nothing, and the KiCad libraries
//! made from it are what the CSV catalog meant KiCad to get, every time.
//!
//! In order, it:
//!
//! 1. imports every CSV file into one new, throwaway team server, as a
//!    team would (one after another, in name order, each with the library
//!    check), then imports them all again and shows nothing new is made;
//! 2. reads the team catalog back and runs the zero-loss report for every
//!    file, and the checks across files (`whole_catalog`);
//! 3. writes the KiCad libraries, then updates them again from the same
//!    server and shows nothing is written;
//! 4. starts a second, separate team server, imports the same files,
//!    writes its libraries and shows both folders are byte-identical,
//!    apart from the file that names the team;
//! 5. compares the libraries with the CSV catalog's own KiCad library,
//!    `database/parts.sqlite` (`csv_kicad_library`);
//! 6. shows the CSV catalog's files have the same bytes as before.
//!
//! [`report::text`] says what it found in words a hardware engineer reads.
//! Everything it writes is under the work folder it is given.

pub mod report;

use crate::csv_kicad_library::{self, Comparison};
use crate::imports::{self, FileImport};
use crate::services::{self, Allow};
use crate::snapshot::Snapshot;
use crate::whole_catalog::{self, WholeCatalog};
use crate::zero_loss::ZeroLossReport;
use catalog::csv_import::CsvImport;
use catalog::pocketbase::{CatalogReadback, ImportOutcome, read_back};
use csv_catalog_fixture::Manifest;
use kicad_libraries::{Library, Naming, Outcome, STATE_FILE};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The team code the proof names its libraries with. It belongs to no real
/// team; any code gives the same proof.
pub const TEAM_CODE: &str = "MA";

/// What the proof found. [`Proof::failures`] lists what fails it.
pub struct Proof {
    pub csv_files: Vec<String>,
    /// What the first import of each file did, such as `applied`.
    pub first_outcomes: Vec<(String, &'static str)>,
    pub second_created_nothing: bool,
    /// The library check's summary once every file is imported: what the
    /// CSV catalog's libraries hold that no row uses, and what to fix.
    pub library_check: String,
    pub reports: Vec<ZeroLossReport>,
    pub whole: WholeCatalog,
    pub libraries: Outcome,
    /// What updating the libraries again from the same server wrote or
    /// changed. Empty means nothing.
    pub update_again: Vec<String>,
    /// How the second server's libraries differ from the first's, apart
    /// from the file that names the team. Empty means byte-identical.
    pub second_server: Vec<String>,
    /// Files compared between the two servers' libraries.
    pub library_files: usize,
    pub comparison: Comparison,
    /// How the CSV catalog changed during the proof. Empty means not at
    /// all.
    pub csv_catalog_changes: Vec<String>,
    /// Files in the CSV catalog, all checked before and after.
    pub csv_catalog_files: usize,
    pub timings: Timings,
}

#[derive(Clone, Debug, Default)]
pub struct Timings {
    /// Starting the first server, importing twice and reading back.
    pub import: Duration,
    /// The zero-loss reports and whole-catalog checks.
    pub checks: Duration,
    /// Writing the libraries, and updating them again.
    pub libraries: Duration,
    /// The second server: start, import, read back and write.
    pub second_server: Duration,
    /// The comparison with parts.sqlite.
    pub comparison: Duration,
    pub total: Duration,
}

impl Proof {
    /// Everything lost or different without a reason, in sentences. Empty
    /// means the proof passed.
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if !self.second_created_nothing {
            failures.push(
                "Importing the same files again made something new, so a second import would change the team catalog.".to_owned(),
            );
        }
        for report in &self.reports {
            for problem in &report.problems {
                failures.push(format!("{}: {problem}", report.source_path));
            }
        }
        failures.extend(self.whole.lost.iter().cloned());
        for problem in &self.libraries.problems {
            failures.push(format!("KiCad libraries: {problem}"));
        }
        for change in &self.update_again {
            failures.push(format!("Updating the libraries again: {change}"));
        }
        for difference in &self.second_server {
            failures.push(format!("The second team server's libraries: {difference}"));
        }
        for found in self.comparison.unexplained() {
            failures.push(format!(
                "Different from parts.sqlite with no reason: {}",
                found.difference.describe()
            ));
        }
        for change in &self.csv_catalog_changes {
            failures.push(format!("The CSV catalog changed: {change}"));
        }
        failures
    }

    pub fn passed(&self) -> bool {
        self.failures().is_empty()
    }
}

/// Runs the proof on the CSV catalog in `repository`, checked against the
/// CSV catalog fixture `oracle`, writing only under `work`.
pub fn run(
    repository: &Path,
    oracle: &Manifest,
    work: &Path,
    allow: Allow,
) -> Result<Proof, String> {
    let started = Instant::now();
    let csv_catalog_before =
        Snapshot::of_csv_catalog(repository).map_err(|error| error.to_string())?;
    let csv_files = services::csv_files(repository).map_err(|error| error.to_string())?;
    // Each file on its own, as a team imports it: each import records the
    // commit of exactly the files it read.
    let planned = csv_files
        .iter()
        .map(|csv_file| services::plan(repository, csv_file).map_err(|error| error.to_string()))
        .collect::<Result<Vec<CsvImport>, String>>()?;
    for import in &planned {
        services::check_committed(repository, import, allow)?;
    }
    let naming = Naming::new(TEAM_CODE)?;
    let team = crate::libraries::disposable_team(naming.clone());

    // 1. Import twice into the first team server.
    let (server, admin, _superuser) = start_server(&work.join("server-a"))?;
    let imported = imports::import_twice(&admin, repository, &planned, allow);
    let imported = match imported {
        Ok(imported) => imported,
        Err(error) => {
            server.stop();
            return Err(error);
        }
    };
    let import_time = started.elapsed();

    // 2. Nothing lost, file by file and across files.
    let checking = Instant::now();
    let reports: Vec<ZeroLossReport> = csv_files
        .iter()
        .map(|csv_file| crate::report(oracle, csv_file, &imported.readback, repository))
        .collect();
    let whole = whole_catalog::check(oracle, &imported.readback, &reports);
    let checks_time = checking.elapsed();

    // 3. The libraries, and an update that must write nothing.
    let writing = Instant::now();
    let first_root = work.join("libraries-a");
    let written =
        crate::libraries::write(&first_root, &imported.readback, &team).and_then(|libraries| {
            let before = folder_snapshot(&first_root)?;
            let again = read_back(&admin).map_err(|error| error.to_string())?;
            let outcome = crate::libraries::write(&first_root, &again, &team)?;
            let mut changes = folder_snapshot(&first_root)?.differences_from(&before);
            changes.extend(
                outcome
                    .written
                    .iter()
                    .map(|file| format!("{file} was written.")),
            );
            if outcome.database_changed {
                changes.push("The database changed.".to_owned());
            }
            Ok((libraries, changes))
        });
    server.stop();
    let (libraries, update_again) = written?;
    let libraries_time = writing.elapsed();

    // 4. A second, separate team server.
    let second = Instant::now();
    let second_root = work.join("libraries-b");
    second_server_libraries(
        repository,
        &work.join("server-b"),
        &planned,
        allow,
        &second_root,
        &team,
    )?;
    let first_files = folder_snapshot(&first_root)?;
    let second_server = folder_snapshot(&second_root)?.differences_from(&first_files);
    let second_time = second.elapsed();

    // 5. Compared with parts.sqlite. Read only now: reading a SQLite
    //    database may leave side files, which the byte comparisons above
    //    must not see.
    let comparing = Instant::now();
    let csv_side = csv_kicad_library::load::csv_side(repository, oracle)?;
    let generated = csv_kicad_library::load::generated_side(
        &libraries.folder,
        &naming,
        &without_parts(&libraries.libraries),
    )?;
    let comparison = csv_kicad_library::compare(&csv_side, &generated);
    let comparison_time = comparing.elapsed();

    // 6. The CSV catalog is as it was.
    let csv_catalog_after =
        Snapshot::of_csv_catalog(repository).map_err(|error| error.to_string())?;

    Ok(Proof {
        first_outcomes: imported
            .first
            .iter()
            .map(|file| (file.csv_file.clone(), outcome_name(file)))
            .collect(),
        library_check: imported
            .second
            .first()
            .map(|file| file.check.summary())
            .unwrap_or_default(),
        second_created_nothing: imported.second_created_nothing,
        csv_files,
        reports,
        whole,
        libraries,
        update_again,
        second_server,
        library_files: first_files.len(),
        comparison,
        csv_catalog_changes: csv_catalog_after.differences_from(&csv_catalog_before),
        csv_catalog_files: csv_catalog_before.len(),
        timings: Timings {
            import: import_time,
            checks: checks_time,
            libraries: libraries_time,
            second_server: second_time,
            comparison: comparison_time,
            total: started.elapsed(),
        },
    })
}

/// Starts a second team server under `state`, imports `planned` once as a
/// team would, and writes its libraries under `root`.
fn second_server_libraries(
    repository: &Path,
    state: &Path,
    planned: &[CsvImport],
    allow: Allow,
    root: &Path,
    team: &kicad_libraries::Team,
) -> Result<(), String> {
    let (server, admin, _superuser) = start_server(state)?;
    let readback: Result<CatalogReadback, String> =
        imports::import_in_order(&admin, repository, planned, allow)
            .and_then(|_| read_back(&admin).map_err(|error| error.to_string()));
    server.stop();
    crate::libraries::write(root, &readback?, team).map(|_| ())
}

/// Starts a team server with its state under `state`. Its PocketBase,
/// migrations and hooks come from this tool's own repository, as for
/// `pilot`: the checkout being proved supplies only the CSV catalog.
fn start_server(
    state: &Path,
) -> Result<
    (
        native_services::PocketBaseServer,
        collaboration::PocketBaseClient,
        native_services::Superuser,
    ),
    String,
> {
    services::start(&services::repository_root(), state)
}

/// Every file of the libraries under `root`, apart from the one that names
/// the team (`<CODE>/.kicad-parts.json`), which records the time of each
/// update and so always differs.
fn folder_snapshot(root: &Path) -> Result<Snapshot, String> {
    Ok(Snapshot::of(root)
        .map_err(|error| error.to_string())?
        .without(&format!("{TEAM_CODE}/{STATE_FILE}")))
}

/// The symbol and footprint libraries, without the database library.
fn without_parts(libraries: &[Library]) -> Vec<Library> {
    libraries
        .iter()
        .filter(|library| library.kind != kicad_libraries::LibraryKind::Parts)
        .cloned()
        .collect()
}

fn outcome_name(file: &FileImport) -> &'static str {
    match file.reply.outcome {
        ImportOutcome::Applied { .. } => "applied",
        ImportOutcome::NothingChanged { .. } => "nothing changed",
        ImportOutcome::Noop { .. } => "nothing to do",
        _ => "refused",
    }
}

/// The work folder of a proof run, under `target/`.
pub fn work_base(repository: &Path) -> PathBuf {
    repository.join("target").join("full-catalog-proof")
}

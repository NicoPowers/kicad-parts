//! Importing CSV files into a team server the way a team does: one file
//! after another, each reading the team catalog first, refusing files with
//! changes that are not committed, refusing an older checkout, and running
//! the library check before anything is sent. `pilot` imports one file
//! this way and the full-catalog proof imports all of them.

use crate::services::{self, Allow};
use catalog::csv_import::CsvImport;
use catalog::library_check::LibraryCheck;
use catalog::pocketbase::{CatalogReadback, ImportOutcome, ImportReply, read_back, submit_import};
use collaboration::PocketBaseClient;
use std::path::Path;

/// The reason every import of `csv_file` records, such as "Import the parts
/// from database/g-res.csv".
pub fn reason(csv_file: &str) -> String {
    format!("Import the parts from database/{csv_file}.csv")
}

/// One CSV file's import: what the team server answered, and the library
/// check that ran before it was sent.
pub struct FileImport {
    pub csv_file: String,
    pub reply: ImportReply,
    pub check: LibraryCheck,
}

impl FileImport {
    /// The team server's answer as JSON, for summaries.
    pub fn reply_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.reply.body).unwrap_or(serde_json::Value::Null)
    }
}

/// Imports `imports` in order, as a team does. Stops at the first import
/// the team server refuses (a conflict, a reused correlation ID or an ID
/// collision), because every later file would be checked against a team
/// catalog that is not what a team would have.
pub fn import_in_order(
    admin: &PocketBaseClient,
    repository: &Path,
    imports: &[CsvImport],
    allow: Allow,
) -> Result<Vec<FileImport>, String> {
    let mut done = Vec::new();
    for import in imports {
        let csv_file = &import.csv_file;
        let (command, check) =
            services::prepare_checked_import(admin, repository, import, &reason(csv_file), allow)?;
        let reply = submit_import(admin, &command).map_err(|error| error.to_string())?;
        let accepted = matches!(
            reply.outcome,
            ImportOutcome::Applied { .. }
                | ImportOutcome::NothingChanged { .. }
                | ImportOutcome::Noop { .. }
        );
        if !accepted {
            return Err(format!(
                "The team server refused the import of database/{csv_file}.csv: {}",
                reply.body
            ));
        }
        done.push(FileImport {
            csv_file: csv_file.clone(),
            reply,
            check,
        });
    }
    Ok(done)
}

/// `imports` imported twice into one team server, with the team catalog
/// read back after each round.
pub struct ImportedTwice {
    pub first: Vec<FileImport>,
    pub second: Vec<FileImport>,
    /// The team catalog after the second round.
    pub readback: CatalogReadback,
    /// Every import of the second round said it had nothing to do (`Noop`),
    /// and the team catalog read back after it is exactly the one read
    /// back after the first round: importing the same files again makes
    /// nothing new.
    pub second_created_nothing: bool,
}

/// Imports `imports` in order, then all of them again, as a team that runs
/// its import twice by mistake would.
pub fn import_twice(
    admin: &PocketBaseClient,
    repository: &Path,
    imports: &[CsvImport],
    allow: Allow,
) -> Result<ImportedTwice, String> {
    let first = import_in_order(admin, repository, imports, allow)?;
    let after_first = read_back(admin).map_err(|error| error.to_string())?;
    let second = import_in_order(admin, repository, imports, allow)?;
    let readback = read_back(admin).map_err(|error| error.to_string())?;
    let second_created_nothing = second
        .iter()
        .all(|file| matches!(file.reply.outcome, ImportOutcome::Noop { .. }))
        && after_first == readback;
    Ok(ImportedTwice {
        first,
        second,
        readback,
        second_created_nothing,
    })
}

//! Proves a CSV catalog import lossless.
//!
//! - [`zero_loss`] compares the Catalog records read back from PocketBase
//!   with the CSV catalog fixture (`fixtures/import/csv-catalog/
//!   manifest.json`), field by field, CAD item by CAD item and import problem
//!   by import problem.
//! - [`services`] starts a disposable PocketBase with the repository's
//!   migrations and hooks, and prepares import commands.
//! - [`checkout`] refuses to import from a checkout older than the team
//!   catalog's latest imports.
//! - [`imports`] imports CSV files as a team does, once or twice.
//! - [`whole_catalog`] checks what lies between the CSV files once all of
//!   them are imported into one team catalog.
//! - [`csv_kicad_library`] compares the generated KiCad libraries with the
//!   CSV catalog's own, `database/parts.sqlite`, and holds the list of
//!   meant differences.
//! - [`full_proof`] runs all of it for `catalog-import full-pilot`, with
//!   [`libraries`] writing the KiCad libraries and [`snapshot`] showing
//!   which files kept their bytes.

pub mod checkout;
pub mod csv_kicad_library;
pub mod full_proof;
pub mod imports;
pub mod libraries;
pub mod run_folder;
pub mod services;
pub mod snapshot;
pub mod whole_catalog;
pub mod zero_loss;

pub use zero_loss::{ZeroLossReport, report};

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

pub mod checkout;
pub mod run_folder;
pub mod services;
pub mod zero_loss;

pub use zero_loss::{ZeroLossReport, report};

//! Proves a CSV catalog import lossless.
//!
//! - [`zero_loss`] compares the Catalog records read back from PocketBase
//!   with the legacy import oracle (`fixtures/import/legacy-catalog/
//!   manifest.json`), field by field, CAD item by CAD item and diagnostic by
//!   problem.
//! - [`services`] starts a disposable PocketBase with the repository's
//!   migrations and hooks.

pub mod services;
pub mod zero_loss;

pub use zero_loss::{ZeroLossReport, report};

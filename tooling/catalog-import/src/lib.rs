//! Proves a Legacy Catalog import lossless.
//!
//! - [`zero_loss`] compares the Catalog records read back from PocketBase
//!   with the legacy import oracle (`fixtures/import/legacy-catalog/
//!   manifest.json`), field by field, asset by asset and diagnostic by
//!   diagnostic.
//! - [`services`] starts a disposable PocketBase with the repository's
//!   migrations and hooks.

pub mod services;
pub mod zero_loss;

pub use zero_loss::{ZeroLossReport, report};

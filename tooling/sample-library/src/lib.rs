//! The sample library for tests (kicad-parts-fbc2).
//!
//! `fixtures/sample-library/` is a small, made-up KiCad library laid out
//! like a library repo: symbols, footprints, 3D models and its own lib
//! tables. Its `parts.toml` lists the parts a test team server starts with.
//! Its README says what each part is for.
//!
//! - [`Sample`] reads `parts.toml`.
//! - [`SampleRepo`] makes a throwaway git repo of the library under
//!   `target/`, for features that read a library folder or a linked repo.
//! - [`seed`] adds the parts to a team server through the same route Add
//!   part uses. The server already has their categories: every new team
//!   server starts with the starting categories.

mod parts;
mod repo;
mod seed;

pub use parts::{Sample, SamplePart};
pub use repo::{SampleRepo, git_isolated};
pub use seed::{Seeded, seed};

use std::path::PathBuf;

/// The sample library in this repository: `fixtures/sample-library`.
pub fn root() -> PathBuf {
    native_services::repository_root()
        .join("fixtures")
        .join("sample-library")
}

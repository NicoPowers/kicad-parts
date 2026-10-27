//! Tools for trying and testing the team catalog without the desktop app.
//!
//! - [`services`] starts a disposable PocketBase with the repository's
//!   migrations and hooks, and adds team members.
//! - [`run_folder`] keeps a local team server's data under `target/`.
//! - [`libraries`] writes a team's KiCad libraries from a disposable team
//!   server, for `local-team kicad-libraries`.

pub mod libraries;
pub mod run_folder;
pub mod services;

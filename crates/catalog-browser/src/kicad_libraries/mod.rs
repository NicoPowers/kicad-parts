//! Keeps the KiCad libraries of every team in the person's list up to date
//! at once (ADR 0015), each in its own folder under `kicad-libraries`. One
//! [`TeamLibraries`] per team does the work, so each team only ever writes
//! its own folder, and a folder that belongs to another team is never
//! written.
//!
//! The KiCad settings panel shows one team at a time: the team the catalog
//! view shows ([`Teams::shown`]).

mod team;

pub use team::{QUIET, RETRY, TeamLibraries};

use crate::team::TeamCatalog;
use crate::teams::Teams;
use desktop_contract::{KicadLibrariesStatus, TeamCodeCheck, TeamsStatus};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How often the list of teams is checked for teams added or removed.
const POLL: Duration = Duration::from_millis(250);

pub struct KicadLibraries {
    teams: Arc<Teams>,
    shared: Arc<Shared>,
    /// What the panel shows while no team is shown: nothing is updated.
    idle: TeamLibraries,
}

struct Shared {
    /// One per team in the list, by server address.
    each: Mutex<Vec<(String, Arc<TeamLibraries>)>>,
    stop: AtomicBool,
}

impl KicadLibraries {
    /// Starts keeping every team's libraries in `teams` up to date under
    /// `root`, the `kicad-libraries` folder. Teams added later get theirs
    /// too, and a team taken off the list stops being updated; its folder
    /// stays, so KiCad keeps working with it.
    pub fn start(root: PathBuf, teams: Arc<Teams>) -> Self {
        let shared = Arc::new(Shared {
            each: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        {
            let (root, teams, shared) = (root.clone(), teams.clone(), shared.clone());
            std::thread::spawn(move || {
                while !shared.stop.load(Ordering::SeqCst) {
                    follow_the_list(&root, &teams, &shared);
                    std::thread::sleep(POLL);
                }
            });
        }
        Self {
            idle: TeamLibraries::start(root, Arc::new(TeamCatalog::new())),
            teams,
            shared,
        }
    }

    /// The shown team's libraries, if it has started them.
    fn shown(&self) -> Option<Arc<TeamLibraries>> {
        let shown = self.teams.shown();
        self.shared
            .lock()
            .iter()
            .find(|(_, libraries)| Arc::ptr_eq(libraries.team(), &shown))
            .map(|(_, libraries)| libraries.clone())
    }

    fn with_shown<T>(&self, work: impl FnOnce(&TeamLibraries) -> T) -> T {
        match self.shown() {
            Some(libraries) => work(&libraries),
            None => work(&self.idle),
        }
    }

    /// The shown team's folder, or the folder of every team's libraries
    /// while it has no code.
    pub fn folder(&self) -> PathBuf {
        self.with_shown(TeamLibraries::folder)
    }

    /// The shown team's libraries, as the KiCad settings panel shows them.
    pub fn status(&self) -> KicadLibrariesStatus {
        self.with_shown(TeamLibraries::status)
    }

    /// The libraries of the team at `server_url`, if it is in the list.
    fn of(&self, server_url: &str) -> Option<Arc<TeamLibraries>> {
        self.shared
            .lock()
            .iter()
            .find(|(server, _)| server == server_url)
            .map(|(_, libraries)| libraries.clone())
    }

    /// The status of the libraries of the team at `server_url`, if it is in
    /// the list.
    pub fn status_of(&self, server_url: &str) -> Option<KicadLibrariesStatus> {
        Some(self.of(server_url)?.status())
    }

    /// Notes, on each team in `status`, whether its libraries were made
    /// from another server and wait for the person's answer, so the Teams
    /// menu can ask on that team's row.
    pub fn note_other_servers(&self, status: &mut TeamsStatus) {
        for team in &mut status.teams {
            team.libraries_made_from = self
                .of(&team.server_url)
                .and_then(|libraries| libraries.made_from_other_server());
        }
    }

    /// The person said the team at `server_url` moved there: its libraries
    /// take updates from it from now on.
    pub fn use_new_server(&self, server_url: &str) -> Result<(), String> {
        match self.of(server_url) {
            Some(libraries) => libraries.use_new_server(),
            None => Ok(()),
        }
    }

    /// Asks for an update of the shown team's libraries now.
    pub fn update_now(&self) -> KicadLibrariesStatus {
        self.with_shown(TeamLibraries::update_now)
    }

    /// The person copied the shown team's setup text for `kind`.
    pub fn copied(&self, kind: &str) -> KicadLibrariesStatus {
        self.with_shown(|libraries| libraries.copied(kind))
    }

    /// What choosing `code` for the shown team would do.
    pub fn check_code(&self, code: &str) -> TeamCodeCheck {
        self.with_shown(|libraries| libraries.check_code(code))
    }

    /// Chooses the shown team's code on its team server, once.
    pub fn set_code(&self, code: &str) -> Result<KicadLibrariesStatus, String> {
        self.with_shown(|libraries| libraries.set_code(code))
    }
}

impl Drop for KicadLibraries {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Vec<(String, Arc<TeamLibraries>)>> {
        self.each.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Starts the libraries of each team new to the list, and stops those of
/// each team no longer in it.
fn follow_the_list(root: &std::path::Path, teams: &Teams, shared: &Shared) {
    let listed = teams.every();
    let mut each = shared.lock();
    each.retain(|(server, libraries)| {
        listed
            .iter()
            .any(|(listed, team)| listed == server && Arc::ptr_eq(team, libraries.team()))
    });
    for (server, team) in listed {
        if !each.iter().any(|(known, _)| *known == server) {
            let libraries = TeamLibraries::start(root.to_path_buf(), team);
            each.push((server, Arc::new(libraries)));
        }
    }
}

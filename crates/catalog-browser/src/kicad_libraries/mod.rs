//! Keeps the KiCad libraries of every team in the person's list up to date
//! at once (ADR 0015), each in its own folder under `kicad-libraries`. One
//! [`TeamLibraries`] per team does the work, so each team only ever writes
//! its own folder, and a folder that belongs to another team is never
//! written.
//!
//! The KiCad settings panel shows one team at a time, and every request
//! from it names that team by server address.

mod team;

pub use team::{QUIET, RETRY, TeamLibraries};

use crate::team::TeamCatalog;
use crate::teams::Teams;
use desktop_contract::{
    KicadLibrariesState, KicadLibrariesStatus, LibrariesMadeFrom, TeamCodeCheck, TeamsStatus,
    UseNewServerRequest, two_servers_claim_a_team,
};
use kicad_libraries::same_server;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How often the list of teams is checked for teams added or removed.
const POLL: Duration = Duration::from_millis(250);

pub struct KicadLibraries {
    teams: Arc<Teams>,
    shared: Arc<Shared>,
    /// What the panel shows while no team is chosen: nothing is updated.
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

    /// Runs `work` on the libraries of the team at `server_url`, or on idle
    /// libraries that update nothing when that team is not in the list (or
    /// `server_url` is blank, as before any team is chosen).
    fn with_team<T>(&self, server_url: &str, work: impl FnOnce(&TeamLibraries) -> T) -> T {
        match self.of(server_url) {
            Some(libraries) => work(&libraries),
            None => work(&self.idle),
        }
    }

    /// The folder of the libraries of the team at `server_url`, or the
    /// folder of every team's libraries while it has no code.
    pub fn folder(&self, server_url: &str) -> PathBuf {
        self.with_team(server_url, TeamLibraries::folder)
    }

    /// The status `work` gives for the libraries of the team at
    /// `server_url`, with what only the list of teams knows
    /// ([`Self::with_list`]). Every status the KiCad settings panel gets
    /// comes through here or `set_code`.
    fn team_status(
        &self,
        server_url: &str,
        work: impl FnOnce(&TeamLibraries) -> KicadLibrariesStatus,
    ) -> KicadLibrariesStatus {
        self.with_list(self.with_team(server_url, work))
    }

    /// The libraries of the team at `server_url`, as the KiCad settings
    /// panel shows them.
    pub fn status(&self, server_url: &str) -> KicadLibrariesStatus {
        self.team_status(server_url, TeamLibraries::status)
    }

    /// Whether `server` is in the person's list of teams.
    fn in_your_teams(&self, server: &str) -> bool {
        self.teams
            .every()
            .iter()
            .any(|(listed, _)| same_server(listed, server))
    }

    /// `status` with what only the list of teams knows: whether the server
    /// the libraries come from is in the person's teams too.
    fn with_list(&self, mut status: KicadLibrariesStatus) -> KicadLibrariesStatus {
        if let KicadLibrariesState::OtherServer {
            recorded_server,
            recorded_server_in_your_teams,
            ..
        } = &mut status.state
        {
            *recorded_server_in_your_teams = self.in_your_teams(recorded_server);
        }
        status
    }

    /// The libraries of the team at `server_url`, if it is in the list.
    fn of(&self, server_url: &str) -> Option<Arc<TeamLibraries>> {
        self.shared
            .lock()
            .iter()
            .find(|(server, _)| server == server_url)
            .map(|(_, libraries)| libraries.clone())
    }

    /// Notes, on each team in `status`, whether its libraries come from
    /// another server and wait for the person, so the Teams menu can say
    /// so on that team's row.
    pub fn note_other_servers(&self, status: &mut TeamsStatus) {
        let listed: Vec<String> = status
            .teams
            .iter()
            .map(|team| team.server_url.clone())
            .collect();
        for team in &mut status.teams {
            team.libraries_made_from = self
                .of(&team.server_url)
                .and_then(|libraries| libraries.made_from_other_server())
                .map(|(code, server)| LibrariesMadeFrom {
                    in_your_teams: listed.iter().any(|listed| same_server(listed, &server)),
                    code,
                    server,
                });
        }
    }

    /// The person said the team at `request.server_url` moved there: its
    /// libraries take updates from it from now on.
    ///
    /// Refused while the server they come from is in the person's teams
    /// too. Two servers then say they are one team, and one click must not
    /// hand the libraries from one to the other and back. The person first
    /// removes the server the team left.
    pub fn use_new_server(&self, request: &UseNewServerRequest) -> Result<(), String> {
        if self.in_your_teams(&request.made_from) {
            return Err(two_servers_claim_a_team(
                &request.code,
                &request.made_from,
                &request.server_url,
            ));
        }
        match self.of(&request.server_url) {
            Some(libraries) => libraries.use_new_server(&request.code, &request.made_from),
            None => Ok(()),
        }
    }

    /// Asks for an update of the libraries of the team at `server_url` now.
    pub fn update_now(&self, server_url: &str) -> KicadLibrariesStatus {
        self.team_status(server_url, TeamLibraries::update_now)
    }

    /// The person copied the setup text for `kind` of the team at
    /// `server_url`.
    pub fn copied(&self, server_url: &str, kind: &str) -> KicadLibrariesStatus {
        self.team_status(server_url, |libraries| libraries.copied(kind))
    }

    /// What choosing `code` for the team at `server_url` would do.
    pub fn check_code(&self, server_url: &str, code: &str) -> TeamCodeCheck {
        self.with_team(server_url, |libraries| libraries.check_code(code))
    }

    /// Chooses the code of the team at `server_url` on its team server,
    /// once.
    pub fn set_code(&self, server_url: &str, code: &str) -> Result<KicadLibrariesStatus, String> {
        self.with_team(server_url, |libraries| libraries.set_code(code))
            .map(|status| self.with_list(status))
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

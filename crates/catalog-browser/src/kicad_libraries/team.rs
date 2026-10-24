//! Keeps one team's KiCad libraries (kicad-parts-f397) up to date:
//! once the sign-in has caught up, and again about 2 seconds after the last
//! new Change, so a burst of Changes causes one update. "Update now" runs
//! one at once. A failed update, such as when KiCad holds a file open, is
//! tried again after a while. When another team's libraries on this
//! computer use the team's code, nothing is tried again until the sign-in
//! or the code changes: KiCad cannot hold both, and there is no override.
//! When the team's folder was made from another team server, nothing is
//! written until the person says the team moved
//! ([`TeamLibraries::use_new_server`]), because another server could claim
//! the team's ID (kicad-parts-092b).
//!
//! The libraries are named with the team's code, which the team server
//! keeps: no libraries are made until someone who can edit chooses it.
//! Each team has its own folder under `kicad-libraries`; signing in to a
//! team updates only that team's folder, so other teams' libraries keep
//! working in KiCad.
//!
//! One background thread per team does every update of that team's folder,
//! so a folder has one writer.
//! Each update reads the Catalog's records and downloads only the files it
//! has not written yet.

use crate::team::TeamCatalog;
use catalog::pocketbase::{
    TeamCodeOutcome, TeamSettings, download_blob, read_revisions, read_team, submit_team_code,
};
use desktop_contract::{
    KicadLibrariesState, KicadLibrariesStatus, KicadLibrary, LiveUpdates, OdbcDriver,
    OtherTeamLibraries, TeamCode, TeamCodeCheck, TeamSession,
};
use kicad_libraries::{
    Library, LibraryKind, Naming, Team, UpdateError, kicad_process, odbc, setup, team_code_problem,
    team_folder, team_folders,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long the libraries wait after the last new Change before they
/// update.
pub const QUIET: Duration = Duration::from_secs(2);
/// How long they wait to try again after an update failed.
pub const RETRY: Duration = Duration::from_secs(30);
/// How often the team's settings are read again while it has no code, in
/// case someone else chooses it.
const CODE_POLL: Duration = Duration::from_secs(5);
/// How often the thread looks at the team catalog's position.
const POLL: Duration = Duration::from_millis(250);

/// One team's KiCad libraries and the thread that updates them. Dropping
/// it stops the thread.
pub struct TeamLibraries {
    /// The `kicad-libraries` folder, which holds one folder per team.
    root: PathBuf,
    team: Arc<TeamCatalog>,
    shared: Arc<Shared>,
}

pub(super) struct Shared {
    state: Mutex<Known>,
    /// Set by "Update now".
    asked: AtomicBool,
    /// Set when the team's settings must be read again, such as after
    /// choosing its code.
    reread: AtomicBool,
    /// Set when the person said the team moved to its new server: the
    /// folder takes updates from it now.
    moved: AtomicBool,
    stop: AtomicBool,
}

/// What the updates found, kept for the status.
struct Known {
    state: KicadLibrariesState,
    /// The signed-in team's settings, once read.
    team: Option<TeamSettings>,
    can_edit: bool,
    libraries: Vec<Library>,
    /// Libraries made since the setup text was last copied.
    new_libraries: Vec<Library>,
    layout_changed_at_ms: Option<u64>,
    problems: Vec<String>,
}

impl Known {
    fn signed_out() -> Self {
        Self {
            state: KicadLibrariesState::SignedOut,
            team: None,
            can_edit: false,
            libraries: Vec::new(),
            new_libraries: Vec::new(),
            layout_changed_at_ms: None,
            problems: Vec::new(),
        }
    }
}

fn kind_name(kind: LibraryKind) -> &'static str {
    match kind {
        LibraryKind::Parts => "parts",
        LibraryKind::Symbols => "symbols",
        LibraryKind::Footprints => "footprints",
    }
}

fn shown(libraries: &[Library]) -> Vec<KicadLibrary> {
    libraries
        .iter()
        .map(|library| KicadLibrary {
            kind: kind_name(library.kind).to_owned(),
            nickname: library.nickname.clone(),
            path: library.path.clone(),
        })
        .collect()
}

impl TeamLibraries {
    /// Starts following `team` and writing its libraries into its folder
    /// under `root`, the `kicad-libraries` folder.
    pub fn start(root: PathBuf, team: Arc<TeamCatalog>) -> Self {
        let shared = Arc::new(Shared::new());
        {
            let (root, shared, team) = (root.clone(), shared.clone(), team.clone());
            std::thread::spawn(move || follow(&root, &team, &shared));
        }
        Self { root, team, shared }
    }

    /// The team these libraries are made from.
    pub(super) fn team(&self) -> &Arc<TeamCatalog> {
        &self.team
    }

    /// The signed-in team's folder, or the folder of every team's
    /// libraries while it has no code.
    pub fn folder(&self) -> PathBuf {
        let known = self.shared.lock();
        match known.team.as_ref().filter(|team| !team.code.is_empty()) {
            Some(team) => team_folder(&self.root, &team.code),
            None => self.root.clone(),
        }
    }

    pub fn status(&self) -> KicadLibrariesStatus {
        let folder = self.folder();
        let known = self.shared.lock();
        let tables = !known.libraries.is_empty();
        let team_code = match (&known.state, &known.team) {
            (KicadLibrariesState::SignedOut, _) | (_, None) => TeamCode::Unknown,
            (_, Some(team)) if team.code.is_empty() => TeamCode::Missing {
                can_set: known.can_edit,
                editors: team.editors.clone(),
                unnamed_editors: team.unnamed_editors,
            },
            (_, Some(team)) => TeamCode::Set {
                code: team.code.clone(),
                set_by: team.code_set_by.clone(),
            },
        };
        let own_team = known.team.as_ref().map(|team| team.id.clone());
        let other_teams = team_folders(&self.root)
            .into_iter()
            .filter(|(_, state)| Some(&state.team_id) != own_team.as_ref())
            .map(|(_, state)| OtherTeamLibraries {
                code: state.code,
                server: state.server,
                updated_at_ms: state.updated_at_ms,
            })
            .collect();
        KicadLibrariesStatus {
            // KicadLibraries names the team it was asked about.
            team: String::new(),
            state: known.state.clone(),
            team_code,
            folder: folder.display().to_string(),
            symbol_table: if tables {
                setup::symbol_table(&known.libraries)
            } else {
                String::new()
            },
            footprint_table: if tables {
                setup::footprint_table(&known.libraries)
            } else {
                String::new()
            },
            libraries: shown(&known.libraries),
            new_libraries: shown(&known.new_libraries),
            restart_kicad: known.layout_changed_at_ms.is_some_and(|at| {
                // Where KiCad cannot be asked, say so until the next start.
                kicad_process::started_before(at).unwrap_or(true)
            }),
            problems: known.problems.clone(),
            odbc_driver: match odbc::sqlite_driver() {
                odbc::Driver::Installed => OdbcDriver::Installed,
                odbc::Driver::Missing => OdbcDriver::Missing,
                odbc::Driver::Unknown => OdbcDriver::Unknown,
            },
            refresh_seconds: kicad_libraries::REFRESH_SECONDS,
            other_teams,
        }
    }

    /// Asks for an update now. It runs once the app is signed in.
    pub fn update_now(&self) -> KicadLibrariesStatus {
        self.shared.asked.store(true, Ordering::SeqCst);
        self.status()
    }

    /// The person copied the setup text for `kind` (`symbols` or
    /// `footprints`), so new libraries of that kind are no longer news.
    pub fn copied(&self, kind: &str) -> KicadLibrariesStatus {
        self.shared
            .lock()
            .new_libraries
            .retain(|library| kind_name(library.kind) != kind);
        self.status()
    }

    /// What choosing `code` would do: whether it is a team code, and
    /// whether another team's libraries on this computer already use it,
    /// which KiCad would refuse.
    pub fn check_code(&self, code: &str) -> TeamCodeCheck {
        if let Some(problem) = team_code_problem(code) {
            return TeamCodeCheck {
                problem: Some(problem.to_owned()),
                warning: None,
            };
        }
        let own = self.shared.lock().team.as_ref().map(|team| team.id.clone());
        let warning = kicad_libraries::team::code_used_by_other_team(
            &self.root,
            code,
            own.as_deref().unwrap_or_default(),
        )
        .map(|other| {
            format!(
                "The {code} libraries of another team, from the team server {}, are already on this computer. KiCad cannot have two libraries with the same name, so choose another code unless that team is this one.",
                other.server
            )
        });
        TeamCodeCheck {
            problem: None,
            warning,
        }
    }

    /// Chooses the team's code on the team server. It is set once: the
    /// server refuses to change it, because boards save it.
    pub fn set_code(&self, code: &str) -> Result<KicadLibrariesStatus, String> {
        if let Some(problem) = team_code_problem(code) {
            return Err(problem.to_owned());
        }
        let client = self.team.client_for_libraries()?;
        let outcome = submit_team_code(&client, code).map_err(|error| match error {
            collaboration::CollabError::Forbidden(_) => {
                "Only team members who can edit the catalog can choose the team code.".to_owned()
            }
            other => format!("The team code could not be saved: {other}"),
        })?;
        if let TeamCodeOutcome::AlreadySet { code } = outcome {
            return Err(format!(
                "The team already has the code {code}, which never changes."
            ));
        }
        self.shared.reread.store(true, Ordering::SeqCst);
        self.shared.asked.store(true, Ordering::SeqCst);
        if let Ok(team) = read_team(&client) {
            self.team.remember_code(&team.code);
            self.shared.lock().team = Some(team);
        }
        Ok(self.status())
    }

    /// The team's code and the server its libraries come from, while it is
    /// not the team's server and the person has not said the team moved.
    pub fn made_from_other_server(&self) -> Option<(String, String)> {
        match &self.shared.lock().state {
            KicadLibrariesState::OtherServer {
                code,
                recorded_server,
                ..
            } => Some((code.clone(), recorded_server.clone())),
            _ => None,
        }
    }

    /// The person said the team with `code` moved to its server from
    /// `made_from`, as the question showed them: the folder records the
    /// new server and takes updates from it from now on. Does nothing
    /// unless the libraries wait for that answer, and refuses an answer to
    /// a question that changed meanwhile.
    pub fn use_new_server(&self, code: &str, made_from: &str) -> Result<(), String> {
        let out_of_date = || {
            format!(
                "The KiCad libraries for {code} changed meanwhile, so your answer was not saved. Read the question again."
            )
        };
        let (settings, server) = {
            let known = self.shared.lock();
            match (&known.state, &known.team) {
                (
                    KicadLibrariesState::OtherServer {
                        code: asked_code,
                        recorded_server,
                        server,
                        ..
                    },
                    Some(settings),
                ) => {
                    if asked_code != code || recorded_server != made_from {
                        return Err(out_of_date());
                    }
                    (settings.clone(), server.clone())
                }
                _ => return Ok(()),
            }
        };
        let naming = Naming::new(&settings.code)?;
        let folder = team_folder(&self.root, naming.code());
        let team = Team {
            id: settings.id,
            server,
            naming,
        };
        kicad_libraries::use_new_server(&self.root, &team, made_from).map_err(|error| {
            match error {
                // The folder records another server now.
                UpdateError::OtherServer { .. } => out_of_date(),
                other => other.message(&folder),
            }
        })?;
        self.shared.moved.store(true, Ordering::SeqCst);
        // The thread updates on its next look; until then, the question
        // is answered.
        self.shared.set(KicadLibrariesState::Updating);
        Ok(())
    }
}

impl Drop for TeamLibraries {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Shared {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(Known::signed_out()),
            asked: AtomicBool::new(false),
            reread: AtomicBool::new(false),
            moved: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Known> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, state: KicadLibrariesState) {
        self.lock().state = state;
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// When the next update is due.
#[derive(Debug, Default)]
struct Schedule {
    /// The team catalog's position (who is signed in, the team's code and
    /// the last Change) the libraries were last brought to, or tried for.
    done: Option<(String, u64)>,
    /// The position seen last, and when it was first seen.
    seen: Option<((String, u64), Instant)>,
    /// When to try again after a failure.
    retry: Option<Instant>,
    /// Who is signed in and the team's code, when another team's libraries
    /// use that code: no update runs until they change, even when asked.
    stuck: Option<String>,
}

impl Schedule {
    /// Whether to update now, at `position`, with "Update now" `asked`.
    fn due(&mut self, position: &(String, u64), asked: bool, now: Instant) -> bool {
        if self.seen.as_ref().map(|(seen, _)| seen) != Some(position) {
            self.seen = Some((position.clone(), now));
        }
        if self.stuck.as_ref() == Some(&position.0) {
            return false;
        }
        if asked {
            return true;
        }
        let Some((_, since)) = &self.seen else {
            return false;
        };
        match &self.done {
            // The first update after signing in runs once the sign-in has
            // caught up; there is nothing to wait for.
            None => true,
            Some(done) if done.0 != position.0 => true,
            Some(done) if done.1 != position.1 => now.duration_since(*since) >= QUIET,
            Some(_) => self.retry.is_some_and(|retry| now >= retry),
        }
    }
}

/// The thread: waits for a reason to update, then updates.
fn follow(root: &Path, team: &TeamCatalog, shared: &Shared) {
    let mut schedule = Schedule::default();
    let mut signed_in_as = String::new();
    let mut settings_read: Option<Instant> = None;
    while !shared.stop.load(Ordering::SeqCst) {
        std::thread::sleep(POLL);
        let status = team.status();
        let TeamSession::SignedIn {
            email,
            live,
            checkpoint,
            can_edit,
            ..
        } = &status.session
        else {
            schedule = Schedule::default();
            signed_in_as.clear();
            shared.asked.store(false, Ordering::SeqCst);
            *shared.lock() = Known::signed_out();
            continue;
        };
        let who = format!("{}\n{email}", status.server_url);
        if who != signed_in_as {
            // Someone else, or another server: start afresh.
            signed_in_as = who.clone();
            schedule = Schedule::default();
            settings_read = None;
            *shared.lock() = Known::signed_out();
            shared.set(KicadLibrariesState::Waiting);
        }
        shared.lock().can_edit = *can_edit;
        if *live != LiveUpdates::Live {
            // Not caught up yet: wait, unless an update already ran.
            if schedule.done.is_none() {
                shared.set(KicadLibrariesState::Waiting);
            }
            continue;
        }
        let code_missing = shared
            .lock()
            .team
            .as_ref()
            .is_none_or(|team| team.code.is_empty());
        let read_again = settings_read.is_none()
            || shared.reread.swap(false, Ordering::SeqCst)
            || (code_missing && settings_read.is_some_and(|at| at.elapsed() >= CODE_POLL));
        if read_again {
            settings_read = Some(Instant::now());
            match team.client_for_libraries().and_then(|client| {
                read_team(&client)
                    .map_err(|error| format!("The team's settings could not be read: {error}"))
            }) {
                Ok(settings) => {
                    team.remember_code(&settings.code);
                    shared.lock().team = Some(settings);
                }
                Err(reason) => {
                    shared.set(KicadLibrariesState::Failed {
                        at_ms: now_ms(),
                        reason,
                    });
                    continue;
                }
            }
        }
        let Some(settings) = shared.lock().team.clone() else {
            continue;
        };
        let naming = match Naming::new(&settings.code) {
            Ok(naming) => naming,
            Err(_) => {
                shared.set(KicadLibrariesState::NeedsCode);
                continue;
            }
        };
        let position = (format!("{who}\n{}", settings.code), *checkpoint);
        if shared.moved.swap(false, Ordering::SeqCst) {
            // Updates from this server again, at once.
            schedule.stuck = None;
            schedule.done = None;
        }
        // An "Update now" that cannot run is dropped, not kept for later.
        let asked = shared.asked.swap(false, Ordering::SeqCst);
        if !schedule.due(&position, asked, Instant::now()) {
            continue;
        }
        shared.set(KicadLibrariesState::Updating);
        let target = Team {
            id: settings.id.clone(),
            server: status.server_url.clone(),
            naming,
        };
        let result = update_once(root, team, &target);
        schedule.done = Some(position.clone());
        let mut known = shared.lock();
        match result {
            Ok(outcome) => {
                schedule.retry = None;
                known.state = KicadLibrariesState::UpToDate {
                    at_ms: now_ms(),
                    current: outcome.current as u32,
                    older: outcome.older as u32,
                    archived: outcome.archived as u32,
                };
                for library in outcome.new_libraries {
                    if !known.new_libraries.contains(&library) {
                        known.new_libraries.push(library);
                    }
                }
                known.libraries = outcome.libraries;
                known.layout_changed_at_ms = outcome.layout_changed_at_ms;
                known.problems = outcome.problems;
            }
            Err(Stopped::Busy) => {
                schedule.retry = Some(Instant::now() + RETRY);
                known.state = KicadLibrariesState::Busy { at_ms: now_ms() };
            }
            Err(Stopped::CodeTaken {
                code,
                other_server,
                server,
            }) => {
                schedule.retry = None;
                schedule.stuck = Some(position.0);
                known.state = KicadLibrariesState::CodeTaken {
                    code,
                    other_server,
                    server,
                };
            }
            Err(Stopped::OtherServer {
                code,
                recorded_server,
                server,
            }) => {
                // Asks the person once, and waits for the answer.
                schedule.retry = None;
                schedule.stuck = Some(position.0);
                known.state = KicadLibrariesState::OtherServer {
                    code,
                    recorded_server,
                    server,
                    // Only the list of teams knows; see KicadLibraries.
                    recorded_server_in_your_teams: false,
                };
            }
            Err(Stopped::Failed(reason)) => {
                schedule.retry = Some(Instant::now() + RETRY);
                known.state = KicadLibrariesState::Failed {
                    at_ms: now_ms(),
                    reason,
                };
            }
        }
    }
}

/// Why an update did not finish.
enum Stopped {
    /// Another update of the folder is running.
    Busy,
    /// Another team's libraries use the code.
    CodeTaken {
        code: String,
        other_server: String,
        server: String,
    },
    /// The team's libraries were made from another team server.
    OtherServer {
        code: String,
        recorded_server: String,
        server: String,
    },
    /// Anything else, in plain sentences.
    Failed(String),
}

/// One update from the team server the app is signed in to.
fn update_once(
    root: &Path,
    team: &TeamCatalog,
    target: &Team,
) -> Result<kicad_libraries::Outcome, Stopped> {
    let client = team.client_for_libraries().map_err(Stopped::Failed)?;
    let records = read_revisions(&client)
        .map_err(|error| Stopped::Failed(format!("The team catalog could not be read: {error}")))?;
    kicad_libraries::update(root, &records, target, &mut |blob| {
        download_blob(&client, blob).map_err(|error| error.to_string())
    })
    .map_err(|error| match error {
        UpdateError::Busy => Stopped::Busy,
        UpdateError::OtherTeam {
            code,
            other_server,
            server,
            ..
        } => Stopped::CodeTaken {
            code,
            other_server,
            server,
        },
        UpdateError::OtherServer {
            code,
            recorded_server,
            server,
            ..
        } => Stopped::OtherServer {
            code,
            recorded_server,
            server,
        },
        other => Stopped::Failed(other.message(&team_folder(root, target.naming.code()))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_once_caught_up_then_after_a_quiet_spell() {
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut schedule = Schedule::default();
        let first = ("server\nme\nMA".to_owned(), 7);
        // Once the sign-in has caught up.
        assert!(schedule.due(&first, false, at(0.0)));
        schedule.done = Some(first.clone());
        assert!(!schedule.due(&first, false, at(1.0)));
        // A burst of Changes: nothing until 2 seconds after the last one.
        for (seconds, seq) in [(1.0, 8), (1.5, 9), (2.5, 10)] {
            assert!(!schedule.due(&("server\nme\nMA".to_owned(), seq), false, at(seconds)));
        }
        let last = ("server\nme\nMA".to_owned(), 10);
        assert!(!schedule.due(&last, false, at(4.0)));
        assert!(schedule.due(&last, false, at(4.6)));
        schedule.done = Some(last.clone());
        // Update now runs at once.
        assert!(schedule.due(&last, true, at(5.0)));
        // After a failure, it tries again later.
        schedule.retry = Some(at(40.0));
        assert!(!schedule.due(&last, false, at(39.0)));
        assert!(schedule.due(&last, false, at(40.0)));
        // Someone else signing in, or a new team code, updates at once.
        schedule.retry = None;
        assert!(schedule.due(&("server\nyou\nMA".to_owned(), 10), false, at(41.0)));
    }

    #[test]
    fn a_code_another_team_uses_stops_updates_until_it_changes() {
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut schedule = Schedule::default();
        let taken = ("server\nme\nMA".to_owned(), 7);
        assert!(schedule.due(&taken, false, at(0.0)));
        schedule.done = Some(taken.clone());
        schedule.stuck = Some(taken.0.clone());
        // Not after a while, not after new Changes, not when asked.
        assert!(!schedule.due(&taken, false, at(100.0)));
        let later = ("server\nme\nMA".to_owned(), 9);
        assert!(!schedule.due(&later, false, at(101.0)));
        assert!(!schedule.due(&later, false, at(110.0)));
        assert!(!schedule.due(&later, true, at(111.0)));
        // A new code does.
        assert!(schedule.due(&("server\nme\nMB".to_owned(), 9), false, at(112.0)));
    }
}

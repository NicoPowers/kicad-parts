//! Keeps the KiCad libraries (kicad-parts-f397) up to date while the app is
//! signed in to a team catalog: once the sign-in has caught up, and again
//! about 2 seconds after the last new Change, so a burst of Changes causes
//! one update. "Update now" runs one at once. A failed update, such as
//! when KiCad holds a file open, is tried again after a while.
//!
//! One background thread does every update, so the folder has one writer.
//! Each update reads the Catalog's records and downloads only the files it
//! has not written yet.

use crate::team::TeamCatalog;
use catalog::pocketbase::{download_blob, read_revisions};
use desktop_contract::{
    KicadLibrariesState, KicadLibrariesStatus, KicadLibrary, OdbcDriver, TeamSession,
};
use kicad_libraries::{Library, LibraryKind, odbc, setup};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long the libraries wait after the last new Change before they
/// update.
pub const QUIET: Duration = Duration::from_secs(2);
/// How long they wait to try again after an update failed.
pub const RETRY: Duration = Duration::from_secs(30);
/// How often the thread looks at the team catalog's position.
const POLL: Duration = Duration::from_millis(250);

pub struct KicadLibraries {
    folder: PathBuf,
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<Known>,
    /// Set by "Update now".
    asked: AtomicBool,
}

/// What the last update said, kept for the status.
struct Known {
    state: KicadLibrariesState,
    libraries: Vec<Library>,
    problems: Vec<String>,
}

impl KicadLibraries {
    /// Starts following `team` and writing the libraries into `folder`.
    pub fn start(folder: PathBuf, team: Arc<TeamCatalog>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(Known {
                state: KicadLibrariesState::SignedOut,
                libraries: Vec::new(),
                problems: Vec::new(),
            }),
            asked: AtomicBool::new(false),
        });
        {
            let (folder, shared) = (folder.clone(), shared.clone());
            std::thread::spawn(move || follow(&folder, &team, &shared));
        }
        Self { folder, shared }
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn status(&self) -> KicadLibrariesStatus {
        let known = self.shared.lock();
        let libraries: Vec<KicadLibrary> = known
            .libraries
            .iter()
            .map(|library| KicadLibrary {
                kind: match library.kind {
                    LibraryKind::Parts => "parts",
                    LibraryKind::Symbols => "symbols",
                    LibraryKind::Footprints => "footprints",
                }
                .to_owned(),
                nickname: library.nickname.clone(),
                path: library.path.clone(),
            })
            .collect();
        let tables = !known.libraries.is_empty();
        KicadLibrariesStatus {
            state: known.state.clone(),
            folder: self.folder.display().to_string(),
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
            libraries,
            problems: known.problems.clone(),
            odbc_driver: match odbc::sqlite_driver() {
                odbc::Driver::Installed => OdbcDriver::Installed,
                odbc::Driver::Missing => OdbcDriver::Missing,
                odbc::Driver::Unknown => OdbcDriver::Unknown,
            },
            refresh_seconds: kicad_libraries::REFRESH_SECONDS,
        }
    }

    /// Asks for an update now. It runs once the app is signed in.
    pub fn update_now(&self) -> KicadLibrariesStatus {
        self.shared.asked.store(true, Ordering::SeqCst);
        self.status()
    }
}

impl Shared {
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
    /// The team catalog's position (who is signed in, and the last Change)
    /// the libraries were last brought to, or tried for.
    done: Option<(String, u64)>,
    /// The position seen last, and when it was first seen.
    seen: Option<((String, u64), Instant)>,
    /// When to try again after a failure.
    retry: Option<Instant>,
}

impl Schedule {
    /// Whether to update now, at `position`, with "Update now" `asked`.
    fn due(&mut self, position: &(String, u64), asked: bool, now: Instant) -> bool {
        if self.seen.as_ref().map(|(seen, _)| seen) != Some(position) {
            self.seen = Some((position.clone(), now));
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
fn follow(folder: &Path, team: &TeamCatalog, shared: &Shared) {
    let mut schedule = Schedule::default();
    loop {
        std::thread::sleep(POLL);
        let status = team.status();
        let TeamSession::SignedIn {
            email,
            live,
            checkpoint,
            ..
        } = &status.session
        else {
            schedule = Schedule::default();
            shared.asked.store(false, Ordering::SeqCst);
            shared.set(KicadLibrariesState::SignedOut);
            continue;
        };
        let position = (format!("{}\n{email}", status.server_url), *checkpoint);
        let live = *live == desktop_contract::LiveUpdates::Live;
        if !live {
            // Not caught up yet: wait, unless an update already ran.
            if schedule.done.is_none() {
                shared.set(KicadLibrariesState::Waiting);
            }
            continue;
        }
        let asked = shared.asked.load(Ordering::SeqCst);
        if !schedule.due(&position, asked, Instant::now()) {
            continue;
        }
        shared.asked.store(false, Ordering::SeqCst);
        shared.set(KicadLibrariesState::Updating);
        let result = update_once(folder, team);
        schedule.done = Some(position);
        let mut known = shared.lock();
        match result {
            Ok(outcome) => {
                schedule.retry = None;
                known.state = KicadLibrariesState::UpToDate {
                    at_ms: now_ms(),
                    current: outcome.current as u32,
                    older: outcome.older as u32,
                    archived: outcome.archived as u32,
                    restart_kicad: outcome.layout_changed,
                };
                known.libraries = outcome.libraries;
                known.problems = outcome.problems;
            }
            Err(reason) => {
                schedule.retry = Some(Instant::now() + RETRY);
                known.state = KicadLibrariesState::Failed {
                    at_ms: now_ms(),
                    reason,
                };
            }
        }
    }
}

/// One update from the team server the app is signed in to.
fn update_once(folder: &Path, team: &TeamCatalog) -> Result<kicad_libraries::Outcome, String> {
    let client = team.client_for_libraries()?;
    let records = read_revisions(&client)
        .map_err(|error| format!("The team catalog could not be read: {error}"))?;
    kicad_libraries::update(folder, &records, &mut |blob| {
        download_blob(&client, blob).map_err(|error| error.to_string())
    })
    .map_err(|error| error.message(folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_once_caught_up_then_after_a_quiet_spell() {
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut schedule = Schedule::default();
        let first = ("server\nme".to_owned(), 7);
        // Once the sign-in has caught up.
        assert!(schedule.due(&first, false, at(0.0)));
        schedule.done = Some(first.clone());
        assert!(!schedule.due(&first, false, at(1.0)));
        // A burst of Changes: nothing until 2 seconds after the last one.
        for (seconds, seq) in [(1.0, 8), (1.5, 9), (2.5, 10)] {
            assert!(!schedule.due(&("server\nme".to_owned(), seq), false, at(seconds)));
        }
        let last = ("server\nme".to_owned(), 10);
        assert!(!schedule.due(&last, false, at(4.0)));
        assert!(schedule.due(&last, false, at(4.6)));
        schedule.done = Some(last.clone());
        // Update now runs at once.
        assert!(schedule.due(&last, true, at(5.0)));
        // After a failure, it tries again later.
        schedule.retry = Some(at(40.0));
        assert!(!schedule.due(&last, false, at(39.0)));
        assert!(schedule.due(&last, false, at(40.0)));
        // Someone else signing in updates at once.
        schedule.retry = None;
        assert!(schedule.due(&("server\nyou".to_owned(), 10), false, at(41.0)));
    }
}

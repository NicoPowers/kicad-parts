//! Who may run kicad-cli, and when.
//!
//! - **Single-flight**: requests for the same cache entry share one export.
//!   The first request (the leader) exports; the others wait for its result.
//! - **A fixed limit**: at most [`MAX_RUNNING`] exports run at once, however
//!   quickly the person clicks.
//! - **Superseded requests**: a request can say which selection it is for,
//!   such as a click in the results list. Once a newer selection arrives, an
//!   export that is still waiting for a turn, and that only older selections
//!   want, is dropped without running. An export that has started finishes,
//!   so its result is still cached.

use super::PreviewError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// How many kicad-cli exports may run at the same time.
pub const MAX_RUNNING: usize = 2;

/// Which selection a request is for. Numbers grow with each new selection;
/// `None` means the request is never superseded.
pub type Selection = Option<u64>;

pub(super) struct Queue {
    turns: Mutex<Turns>,
    /// Signalled when a turn is given back or a newer selection arrives.
    turns_changed: Condvar,
    flights: Mutex<HashMap<PathBuf, Arc<Flight>>>,
}

struct Turns {
    running: usize,
    /// The newest selection any request has named.
    newest_selection: u64,
    /// The most exports that ever ran at once, for tests.
    peak_running: usize,
}

/// One export in progress, shared by every request for the same entry.
pub(super) struct Flight {
    /// The newest selection among the requests waiting on this export.
    wanted_by: AtomicU64,
    outcome: Mutex<Option<Result<(), PreviewError>>>,
    finished: Condvar,
}

/// What joining an entry's export made this request.
pub(super) enum Role<'a> {
    /// Runs the export, then must call [`Leader::finish`].
    Leader(Leader<'a>),
    /// Waits for the leader with [`Flight::wait`].
    Follower(Arc<Flight>),
}

pub(super) struct Leader<'a> {
    queue: &'a Queue,
    entry: PathBuf,
    flight: Arc<Flight>,
    finished: bool,
}

/// A turn to run kicad-cli. Dropping it gives the turn back.
pub(super) struct Turn<'a> {
    queue: &'a Queue,
}

impl Queue {
    pub(super) fn new() -> Self {
        Self {
            turns: Mutex::new(Turns {
                running: 0,
                newest_selection: 0,
                peak_running: 0,
            }),
            turns_changed: Condvar::new(),
            flights: Mutex::new(HashMap::new()),
        }
    }

    /// Records that `selection` exists. Waiting exports wanted only by
    /// older selections are dropped.
    pub(super) fn note_selection(&self, selection: Selection) {
        let Some(selection) = selection else {
            return;
        };
        let mut turns = self.lock_turns();
        if selection > turns.newest_selection {
            turns.newest_selection = selection;
            self.turns_changed.notify_all();
        }
    }

    /// Joins the export of `entry`: the first request leads it, later ones
    /// follow and share its result.
    pub(super) fn join(&self, entry: &Path, selection: Selection) -> Role<'_> {
        let mut flights = self.lock_flights();
        if let Some(flight) = flights.get(entry) {
            flight
                .wanted_by
                .fetch_max(rank(selection), Ordering::SeqCst);
            return Role::Follower(flight.clone());
        }
        let flight = Arc::new(Flight {
            wanted_by: AtomicU64::new(rank(selection)),
            outcome: Mutex::new(None),
            finished: Condvar::new(),
        });
        flights.insert(entry.to_path_buf(), flight.clone());
        Role::Leader(Leader {
            queue: self,
            entry: entry.to_path_buf(),
            flight,
            finished: false,
        })
    }

    /// The most exports that ran at the same time so far.
    pub(super) fn peak_running(&self) -> usize {
        self.lock_turns().peak_running
    }

    fn lock_turns(&self) -> MutexGuard<'_, Turns> {
        self.turns.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_flights(&self) -> MutexGuard<'_, HashMap<PathBuf, Arc<Flight>>> {
        self.flights.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<'a> Leader<'a> {
    /// Waits for a turn to run kicad-cli. Returns `None` if every request
    /// for this entry was superseded while waiting; the export is then
    /// dropped, and so is this entry's flight.
    pub(super) fn wait_for_turn(&mut self) -> Option<Turn<'a>> {
        let queue = self.queue;
        let mut turns = queue.lock_turns();
        loop {
            let wanted_by = self.flight.wanted_by.load(Ordering::SeqCst);
            if wanted_by < turns.newest_selection {
                // Decide under the flights lock, so no newer request can
                // join the flight between this check and its removal.
                let mut flights = queue.lock_flights();
                if self.flight.wanted_by.load(Ordering::SeqCst) < turns.newest_selection {
                    flights.remove(&self.entry);
                    drop(flights);
                    drop(turns);
                    self.publish(Err(PreviewError::Superseded));
                    return None;
                }
            } else if turns.running < MAX_RUNNING {
                turns.running += 1;
                turns.peak_running = turns.peak_running.max(turns.running);
                return Some(Turn { queue });
            }
            turns = queue
                .turns_changed
                .wait(turns)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Shares the export's result with every follower and ends the flight.
    pub(super) fn finish(mut self, outcome: Result<(), PreviewError>) {
        self.queue.lock_flights().remove(&self.entry);
        self.publish(outcome);
    }

    fn publish(&mut self, outcome: Result<(), PreviewError>) {
        self.finished = true;
        *self
            .flight
            .outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(outcome);
        self.flight.finished.notify_all();
    }
}

impl Drop for Leader<'_> {
    /// A leader that panicked must not leave its followers waiting forever.
    fn drop(&mut self) {
        if !self.finished {
            self.queue.lock_flights().remove(&self.entry);
            self.publish(Err(PreviewError::Io(
                "the export stopped before it finished".into(),
            )));
        }
    }
}

impl Flight {
    /// Waits until the leader finishes and returns its result.
    pub(super) fn wait(&self) -> Result<(), PreviewError> {
        let mut outcome = self.outcome.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(outcome) = outcome.as_ref() {
                return outcome.clone();
            }
            outcome = self
                .finished
                .wait(outcome)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut turns = self.queue.lock_turns();
        turns.running -= 1;
        self.queue.turns_changed.notify_all();
    }
}

/// Requests with no selection outrank every selection, so they are never
/// superseded.
fn rank(selection: Selection) -> u64 {
    selection.unwrap_or(u64::MAX)
}

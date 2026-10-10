//! Who may run kicad-cli, and when.
//!
//! - **One export per entry**: requests for the same cache entry share one
//!   export. The first request runs it; the others wait for its result.
//! - **A fixed limit**: at most [`MAX_RUNNING`] exports run at once, however
//!   quickly the person clicks.
//! - **Replaced requests**: a request can say which selection it is for,
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
/// `None` means the request is never replaced.
pub type Selection = Option<u64>;

pub(super) struct Queue {
    turns: Mutex<Turns>,
    /// Signalled when a turn is given back or a newer selection arrives.
    turns_changed: Condvar,
    /// The export in progress for each cache entry.
    exports: Mutex<HashMap<PathBuf, Arc<SharedExport>>>,
    /// How many requests have joined an export, for tests.
    #[cfg(test)]
    joins: std::sync::atomic::AtomicUsize,
}

struct Turns {
    running: usize,
    /// The newest selection any request has named.
    newest_selection: u64,
    /// The most exports that ever ran at once, for tests.
    peak_running: usize,
}

/// One export in progress, shared by every request for the same entry.
pub(super) struct SharedExport {
    /// The newest selection among the requests waiting on this export.
    wanted_by: AtomicU64,
    outcome: Mutex<Option<Result<(), PreviewError>>>,
    finished: Condvar,
}

/// What joining an entry's export made this request.
pub(super) enum Joined<'a> {
    /// The first request: it runs the export, then calls
    /// [`Exporter::finish`].
    Export(Exporter<'a>),
    /// A later request: it waits with [`SharedExport::wait`].
    Wait(Arc<SharedExport>),
}

/// The request that runs an entry's export.
pub(super) struct Exporter<'a> {
    queue: &'a Queue,
    entry: PathBuf,
    export: Arc<SharedExport>,
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
            exports: Mutex::new(HashMap::new()),
            #[cfg(test)]
            joins: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Joins the export of `entry`: the first request runs it, later ones
    /// wait and share its result. Then records `selection` as the newest
    /// if it is, which drops waiting exports only older selections want.
    ///
    /// The export learns this request's selection before the selection is
    /// recorded, so an older request for the same entry can never drop the
    /// export this request is about to wait on.
    pub(super) fn join(&self, entry: &Path, selection: Selection) -> Joined<'_> {
        let joined = {
            let mut exports = self.lock_exports();
            match exports.get(entry) {
                Some(export) => {
                    export
                        .wanted_by
                        .fetch_max(selection_order(selection), Ordering::SeqCst);
                    Joined::Wait(export.clone())
                }
                None => {
                    let export = Arc::new(SharedExport {
                        wanted_by: AtomicU64::new(selection_order(selection)),
                        outcome: Mutex::new(None),
                        finished: Condvar::new(),
                    });
                    exports.insert(entry.to_path_buf(), export.clone());
                    Joined::Export(Exporter {
                        queue: self,
                        entry: entry.to_path_buf(),
                        export,
                        finished: false,
                    })
                }
            }
        };
        self.note_selection(selection);
        #[cfg(test)]
        self.joins.fetch_add(1, Ordering::SeqCst);
        joined
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

    /// The most exports that ran at the same time so far.
    pub(super) fn peak_running(&self) -> usize {
        self.lock_turns().peak_running
    }

    /// How many requests have joined an export so far, for tests.
    #[cfg(test)]
    pub(super) fn joins(&self) -> usize {
        self.joins.load(Ordering::SeqCst)
    }

    /// How many entries have an export in progress, for tests.
    #[cfg(test)]
    pub(super) fn exports_in_progress(&self) -> usize {
        self.lock_exports().len()
    }

    fn lock_turns(&self) -> MutexGuard<'_, Turns> {
        self.turns.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_exports(&self) -> MutexGuard<'_, HashMap<PathBuf, Arc<SharedExport>>> {
        self.exports.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<'a> Exporter<'a> {
    /// Waits for a turn to run kicad-cli. Returns `None` if every request
    /// for this entry was replaced by a newer selection while waiting; the
    /// export is then dropped and its waiting requests get
    /// [`PreviewError::Replaced`].
    pub(super) fn wait_for_turn(&mut self) -> Option<Turn<'a>> {
        let queue = self.queue;
        let mut turns = queue.lock_turns();
        loop {
            if self.export.wanted_by.load(Ordering::SeqCst) < turns.newest_selection {
                // Decide under the exports lock, so no newer request can
                // join this export between the check and its removal.
                let exports = queue.lock_exports();
                if self.export.wanted_by.load(Ordering::SeqCst) < turns.newest_selection {
                    drop(turns);
                    self.end(exports, Err(PreviewError::Replaced));
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

    /// Shares the export's result with every waiting request and ends the
    /// export. Does nothing if it already ended (it was replaced).
    pub(super) fn finish(mut self, outcome: Result<(), PreviewError>) {
        if !self.finished {
            let exports = self.queue.lock_exports();
            self.end(exports, outcome);
        }
    }

    /// Removes this export from `exports`, only if it is still the one
    /// stored there: a newer export for the same entry may have started
    /// after this one was replaced. Then publishes the outcome.
    fn end(
        &mut self,
        mut exports: MutexGuard<'_, HashMap<PathBuf, Arc<SharedExport>>>,
        outcome: Result<(), PreviewError>,
    ) {
        if exports
            .get(&self.entry)
            .is_some_and(|stored| Arc::ptr_eq(stored, &self.export))
        {
            exports.remove(&self.entry);
        }
        drop(exports);
        self.finished = true;
        *self
            .export
            .outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(outcome);
        self.export.finished.notify_all();
    }
}

impl Drop for Exporter<'_> {
    /// An exporter that panicked must not leave waiting requests waiting
    /// forever.
    fn drop(&mut self) {
        if !self.finished {
            let exports = self.queue.lock_exports();
            self.end(
                exports,
                Err(PreviewError::Io(
                    "the export stopped before it finished".into(),
                )),
            );
        }
    }
}

impl SharedExport {
    /// Waits until the export ends and returns its result.
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

/// Where a selection sorts among selections. Requests with no selection
/// come after every selection, so they are never replaced.
fn selection_order(selection: Selection) -> u64 {
    selection.unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_export_never_ends_the_newer_export_for_the_same_entry() {
        let queue = Queue::new();
        let entry = Path::new("entry");
        // A: selection 1 starts the export of the entry.
        let Joined::Export(mut older) = queue.join(entry, Some(1)) else {
            panic!("the first request runs the export");
        };
        // Selection 2 arrives for something else, so A's waiting export
        // is dropped.
        queue.note_selection(Some(2));
        assert!(older.wait_for_turn().is_none());
        // B: selection 2 asks for the same entry and starts a new export.
        let Joined::Export(newer) = queue.join(entry, Some(2)) else {
            panic!("the replaced export has ended, so B starts a new one");
        };
        // A finishes after B started; B's export must stay.
        older.finish(Err(PreviewError::Replaced));
        assert_eq!(queue.exports_in_progress(), 1);
        let Joined::Wait(shared) = queue.join(entry, Some(2)) else {
            panic!("a third request must wait on B's export, not start another");
        };
        newer.finish(Ok(()));
        assert_eq!(shared.wait(), Ok(()));
        assert_eq!(queue.exports_in_progress(), 0);
    }

    #[test]
    fn a_newer_request_that_joins_keeps_the_export() {
        let queue = Queue::new();
        let entry = Path::new("entry");
        let Joined::Export(mut exporter) = queue.join(entry, Some(1)) else {
            panic!("the first request runs the export");
        };
        // The newer request joins, and only then is selection 2 recorded.
        let Joined::Wait(_) = queue.join(entry, Some(2)) else {
            panic!("the second request waits");
        };
        assert!(exporter.wait_for_turn().is_some());
    }
}

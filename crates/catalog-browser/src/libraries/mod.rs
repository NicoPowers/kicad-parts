//! The Libraries browser's index (kicad-parts-d152): every symbol and
//! footprint of our libraries, KiCad's own and the person's installed
//! libraries, with what the list and its filters need.
//!
//! Opening the browser asks for a refresh: the libraries are found again
//! (a table may have changed) and a few background threads go through them,
//! ours first and KiCad's last. Each library is kept from memory or the
//! disk cache while its fingerprint (sizes and modification times) is
//! unchanged, and read again otherwise ([`read`]). What is ready can be
//! browsed at once; searches and filters run over the index in memory
//! ([`query`]), never over the files.

pub(crate) mod browse;
pub(crate) mod query;
pub(crate) mod read;
pub(crate) mod sources;

use desktop_contract::{LibrariesView, LibraryEntry, LibraryKey, PreviewKind};
use read::Loaded;
use sources::{Places, Source, find_sources};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How many libraries are read at once.
const READERS: usize = 3;

/// One library and what is known of it.
#[derive(Clone, Debug)]
pub(crate) struct Library {
    pub source: Source,
    /// Its items, once read; kept while it is checked again.
    pub loaded: Option<Arc<Loaded>>,
    pub problem: Option<String>,
    /// Waiting to be checked or read.
    pub pending: bool,
}

#[derive(Default)]
struct State {
    libraries: Vec<Library>,
    notes: Vec<String>,
    installed_note: Option<String>,
    /// Whether old cache layouts were removed this run.
    cleaned: bool,
    /// Places in `libraries` still to check, the next first.
    queue: VecDeque<usize>,
    /// Bumped by every refresh, so readers of an older one stop.
    generation: u64,
    /// Reader threads still running.
    readers: usize,
}

struct Shared {
    cache_dir: PathBuf,
    state: Mutex<State>,
    /// Signalled whenever a library is done.
    progress: Condvar,
}

pub(crate) struct LibraryIndex {
    shared: Arc<Shared>,
}

impl LibraryIndex {
    /// An empty index caching under `cache_dir`. Nothing is read until
    /// [`Self::refresh`].
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            shared: Arc::new(Shared {
                cache_dir,
                state: Mutex::new(State::default()),
                progress: Condvar::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Finds the libraries again and checks each one in the background.
    /// Does nothing while a refresh is still running.
    pub fn refresh(&self, places: &Places) {
        if self.lock().readers > 0 {
            return;
        }
        let found = find_sources(places);
        let (sources, notes) = (found.sources, found.notes);
        let mut state = self.lock();
        if state.readers > 0 {
            return;
        }
        if !state.cleaned {
            state.cleaned = true;
            read::clean_cache(&self.shared.cache_dir);
        }
        state.installed_note = found.installed_note;
        let old = std::mem::take(&mut state.libraries);
        state.libraries = sources
            .into_iter()
            .map(|source| {
                let loaded = old
                    .iter()
                    .find(|library| library.source == source)
                    .and_then(|library| library.loaded.clone());
                let problem = source.problem.clone();
                Library {
                    pending: problem.is_none(),
                    source,
                    loaded,
                    problem,
                }
            })
            .collect();
        state.notes = notes;
        // Ours first, then the person's installed libraries, then KiCad's.
        let mut order: Vec<usize> = (0..state.libraries.len())
            .filter(|&at| state.libraries[at].pending)
            .collect();
        order.sort_by_key(|&at| {
            let group = state.libraries[at].source.key.group;
            match group {
                desktop_contract::LibraryGroup::Ours => 0,
                desktop_contract::LibraryGroup::Installed => 1,
                desktop_contract::LibraryGroup::Kicad => 2,
            }
        });
        state.queue = order.into();
        state.generation += 1;
        let readers = READERS.min(state.queue.len());
        state.readers = readers;
        let generation = state.generation;
        drop(state);
        for _ in 0..readers {
            let shared = self.shared.clone();
            std::thread::spawn(move || read_libraries(&shared, generation));
        }
        self.shared.progress.notify_all();
    }

    /// Reads `key` next, if it is still waiting.
    pub fn want(&self, kind: PreviewKind, key: &LibraryKey) {
        let mut state = self.lock();
        let found = state.queue.iter().position(|&at| {
            let source = &state.libraries[at].source;
            source.kind == kind && source.key == *key
        });
        if let Some(position) = found {
            if let Some(at) = state.queue.remove(position) {
                state.queue.push_front(at);
            }
        }
    }

    /// The libraries and how far the reading is.
    pub fn view(&self) -> LibrariesView {
        let state = self.lock();
        let entry = |library: &Library| LibraryEntry {
            key: library.source.key.clone(),
            description: library.source.description.clone(),
            items: library.loaded.as_ref().map(|loaded| loaded.items.len()),
            problem: library.problem.clone(),
            note: library
                .loaded
                .as_ref()
                .and_then(|loaded| skipped_note(&loaded.skipped)),
        };
        let of_kind = |kind: PreviewKind| {
            state
                .libraries
                .iter()
                .filter(|library| library.source.kind == kind)
                .map(entry)
                .collect()
        };
        LibrariesView {
            symbols: of_kind(PreviewKind::Symbol),
            footprints: of_kind(PreviewKind::Footprint),
            done: state
                .libraries
                .iter()
                .filter(|library| !library.pending)
                .count(),
            total: state.libraries.len(),
            notes: state.notes.clone(),
            installed_note: state.installed_note.clone(),
        }
    }

    /// Every library of `kind`, as known now.
    pub fn snapshot(&self, kind: PreviewKind) -> Vec<Library> {
        self.lock()
            .libraries
            .iter()
            .filter(|library| library.source.kind == kind)
            .cloned()
            .collect()
    }

    /// Whether some library is still waiting to be checked or read.
    pub fn reading(&self) -> bool {
        self.lock().libraries.iter().any(|library| library.pending)
    }

    /// Waits until every library is done, at most `timeout`; for tests.
    pub fn wait_until_ready(&self, timeout: Duration) -> bool {
        let state = self.lock();
        let (state, _) = self
            .shared
            .progress
            .wait_timeout_while(state, timeout, |state| {
                state.readers > 0 || state.libraries.iter().any(|library| library.pending)
            })
            .unwrap_or_else(|e| e.into_inner());
        state.readers == 0
    }
}

/// What a library's skipped files say, such as "3 files could not be
/// read. ..." with the first reasons; `None` when nothing was skipped.
fn skipped_note(skipped: &[String]) -> Option<String> {
    match skipped {
        [] => None,
        [only] => Some(only.clone()),
        many => {
            let first: Vec<&str> = many.iter().take(3).map(String::as_str).collect();
            let more = if many.len() > 3 { " ..." } else { "" };
            Some(format!(
                "{} files could not be read. {}{more}",
                many.len(),
                first.join(" ")
            ))
        }
    }
}

/// Lowers the count of running readers when a reader thread ends, even
/// by a panic, so the index never waits for a reader that is gone.
struct ReaderDone<'a>(&'a Shared);

impl Drop for ReaderDone<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.readers = state.readers.saturating_sub(1);
        drop(state);
        self.0.progress.notify_all();
    }
}

/// One reader thread: takes the next waiting library until none is left.
/// A library whose reading panics is marked as not read.
fn read_libraries(shared: &Shared, generation: u64) {
    let _done = ReaderDone(shared);
    loop {
        let (at, source, known) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = (state.generation == generation)
                .then(|| state.queue.pop_front())
                .flatten();
            let Some(at) = next else {
                return;
            };
            let library = &state.libraries[at];
            (at, library.source.clone(), library.loaded.clone())
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            read::load(&source, known.as_ref(), &shared.cache_dir)
        }))
        .unwrap_or_else(|_| {
            Err(format!(
                "{} could not be read: reading it failed unexpectedly.",
                source.path.display()
            ))
        });
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation == generation {
            let library = &mut state.libraries[at];
            match result {
                Ok(loaded) => {
                    library.loaded = Some(loaded);
                    library.problem = None;
                }
                Err(why) => {
                    library.loaded = None;
                    library.problem = Some(why);
                }
            }
            library.pending = false;
        }
        drop(state);
        shared.progress.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_that_panics_still_counts_as_ended() {
        let shared = Arc::new(Shared {
            cache_dir: PathBuf::new(),
            state: Mutex::new(State {
                readers: 1,
                ..State::default()
            }),
            progress: Condvar::new(),
        });
        let reader = shared.clone();
        let ended = std::thread::spawn(move || {
            let _done = ReaderDone(&reader);
            panic!("a reader failed");
        })
        .join();
        assert!(ended.is_err());
        assert_eq!(shared.state.lock().unwrap().readers, 0);
    }

    #[test]
    fn skipped_files_are_counted_with_the_first_reasons() {
        assert_eq!(skipped_note(&[]), None);
        assert_eq!(
            skipped_note(&["a.kicad_mod could not be read.".into()]).as_deref(),
            Some("a.kicad_mod could not be read.")
        );
        let many: Vec<String> = (1..=5)
            .map(|n| format!("{n}.kicad_mod could not be read."))
            .collect();
        assert_eq!(
            skipped_note(&many).as_deref(),
            Some(
                "5 files could not be read. 1.kicad_mod could not be read. 2.kicad_mod could not be read. 3.kicad_mod could not be read. ..."
            )
        );
    }
}

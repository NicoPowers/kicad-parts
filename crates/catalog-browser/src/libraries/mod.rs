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

mod browse;
pub(crate) mod query;
pub(crate) mod read;
pub(crate) mod sources;

use desktop_contract::{LibrariesView, LibraryEntry, LibraryKey, PreviewKind};
use read::Loaded;
use sources::{Places, Source, find_sources};
use std::collections::VecDeque;
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
        let (sources, notes) = find_sources(places);
        let mut state = self.lock();
        if state.readers > 0 {
            return;
        }
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

/// One reader thread: takes the next waiting library until none is left.
fn read_libraries(shared: &Shared, generation: u64) {
    loop {
        let (at, source, known) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let next = (state.generation == generation)
                .then(|| state.queue.pop_front())
                .flatten();
            let Some(at) = next else {
                state.readers = state.readers.saturating_sub(1);
                drop(state);
                shared.progress.notify_all();
                return;
            };
            let library = &state.libraries[at];
            (at, library.source.clone(), library.loaded.clone())
        };
        let result = read::load(&source, known.as_ref(), &shared.cache_dir);
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

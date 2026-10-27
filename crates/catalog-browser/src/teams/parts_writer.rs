//! Writes one team's parts file (see [`super::parts_file`]),
//! in the background, so a team whose server is down at the next start
//! still shows them.
//!
//! It writes only while the team's parts match its server: the session is
//! live and has caught up. It writes after each catch-up and after live
//! changes, once the parts have stayed the same for a few seconds, so a
//! burst of changes from a teammate makes one write, not one per change. Searching changes nothing, so it never writes. While live,
//! it also writes again now and then, so the "as of" time stays close.
//!
//! Signing out and removing the team delete the file. Each write and each
//! delete takes the team's turn, and a write happens only while the team
//! is still in step. So a write never brings back a file the person just
//! deleted.

use super::parts_file;
use crate::team::{PartsFile, TeamCatalog};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often the writer looks at the team.
const TICK: Duration = Duration::from_millis(500);
/// How long the parts must stay the same before they are written.
const QUIET: Duration = Duration::from_secs(3);
/// The longest a change waits while the parts keep changing.
const MAX_WAIT: Duration = Duration::from_secs(30);
/// While live, how often the file is written again with no change, so the
/// "as of" time it records stays within this of the truth.
const REFRESH: Duration = Duration::from_secs(10 * 60);

/// What one team's writer needs.
pub(super) struct WriterTask {
    pub(super) folder: PathBuf,
    pub(super) server_url: String,
    pub(super) catalog: Arc<TeamCatalog>,
}

/// The writer thread of one team. Dropping it stops the thread.
pub(super) struct PartsWriter {
    shared: Arc<Shared>,
}

struct Shared {
    task: WriterTask,
    turn: Mutex<()>,
    stop: AtomicBool,
}

impl PartsWriter {
    pub(super) fn start(task: WriterTask) -> Self {
        let shared = Arc::new(Shared {
            task,
            turn: Mutex::new(()),
            stop: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        std::thread::spawn(move || write_while_running(&thread_shared));
        Self { shared }
    }

    /// Deletes the team's parts file. Call it once the team's session has
    /// ended, so no later write can bring them back. An error says why the
    /// file could not be deleted.
    pub(super) fn delete(&self) -> Result<(), String> {
        let _turn = self.shared.turn();
        let task = &self.shared.task;
        parts_file::delete(&task.folder, &task.server_url)
    }
}

impl Drop for PartsWriter {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Shared {
    fn turn(&self) -> MutexGuard<'_, ()> {
        self.turn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes the team's parts, read at `version`, in the team's turn.
    fn write_now(&self, version: Version) -> Written {
        let task = &self.task;
        // Read before taking the turn: the category names may ask the
        // server, and signing out waits for the turn. The names first:
        // without them the parts are not worth copying.
        let category_names = task.catalog.category_names();
        let values = task.catalog.current_values();
        if category_names.is_empty() && !values.is_empty() {
            // The names could not be read. The catalog keeps that answer
            // until it moves on, so wait for its next version.
            return Written::NoCategoryNames;
        }
        let parts = PartsFile {
            server_url: task.server_url.clone(),
            as_of_ms: crate::team::now_ms(),
            values,
            category_names,
        };
        let _turn = self.turn();
        if self.stop.load(Ordering::SeqCst)
            || !still_worth_writing(version, task.catalog.version_in_step())
        {
            return Written::NotInStep;
        }
        match parts_file::write(&task.folder, &parts) {
            Ok(()) => Written::Yes,
            Err(problem) => {
                // Not shown: the app works on, and only an offline start
                // would miss these parts. The writer tries again soon.
                eprintln!(
                    "kicad-parts-desktop: could not write the parts file of {} ({problem}).",
                    task.server_url
                );
                Written::Failed
            }
        }
    }
}

/// How a write went.
enum Written {
    Yes,
    /// The disk refused it: try again soon.
    Failed,
    /// The category names could not be read: wait for the next version.
    NoCategoryNames,
    /// Signed out, signed in again, or offline meanwhile: nothing to do.
    NotInStep,
}

/// Whether parts read at version `read` may still be written, when the
/// session's parts are in step at `now`. Only the session must be the same:
/// a sign-out or a new sign-in meanwhile makes them not worth writing. New
/// Changes are not a reason to drop them, or a long burst would never be
/// written; the next write brings those.
fn still_worth_writing(read: Version, now: Option<Version>) -> bool {
    now.is_some_and(|now| now.0 == read.0)
}

fn write_while_running(shared: &Shared) {
    let mut debounce = Debounce::default();
    while !shared.stop.load(Ordering::SeqCst) {
        std::thread::sleep(TICK);
        let Some(version) = shared.task.catalog.version_in_step() else {
            continue;
        };
        let now = Instant::now();
        if !debounce.due(version, now) {
            continue;
        }
        // The version read before the parts: parts newer than it are
        // written again at the next look, never missed.
        match shared.write_now(version) {
            Written::Yes => debounce.wrote(version, now),
            Written::Failed => debounce.failed(now),
            Written::NoCategoryNames => debounce.skip(version),
            Written::NotInStep => {}
        }
    }
}

/// The version of a team's parts (see `KnownParts::version`).
type Version = (u64, u64, u64);

/// Decides when a team's parts are worth writing: once they have stayed
/// the same for [`QUIET`], or after [`MAX_WAIT`] while they keep changing,
/// and again [`REFRESH`] after the last write while nothing changes.
#[derive(Default)]
struct Debounce {
    /// The version last written, and when.
    written: Option<(Version, Instant)>,
    /// The version last seen, and since when it has stayed the same.
    seen: Option<(Version, Instant)>,
    /// When the parts first differed from the file.
    changed_since: Option<Instant>,
    /// After a failed write, when to try again.
    retry_at: Option<Instant>,
    /// A version not worth writing: wait for the next one.
    skipped: Option<Version>,
}

impl Debounce {
    /// Whether to write `version`, seen at `now`.
    fn due(&mut self, version: Version, now: Instant) -> bool {
        if self.retry_at.is_some_and(|at| now < at) || self.skipped == Some(version) {
            return false;
        }
        if let Some((written, at)) = self.written {
            if written == version {
                return now.duration_since(at) >= REFRESH;
            }
        }
        if self.seen.map(|(seen, _)| seen) != Some(version) {
            self.seen = Some((version, now));
        }
        let changed_since = *self.changed_since.get_or_insert(now);
        let quiet_since = self.seen.map_or(now, |(_, since)| since);
        now.duration_since(quiet_since) >= QUIET || now.duration_since(changed_since) >= MAX_WAIT
    }

    /// `version` was written at `now`.
    fn wrote(&mut self, version: Version, now: Instant) {
        self.written = Some((version, now));
        self.changed_since = None;
        self.retry_at = None;
    }

    /// `version` is not worth writing, such as with no category names:
    /// wait for the next version.
    fn skip(&mut self, version: Version) {
        self.skipped = Some(version);
    }

    /// A write failed at `now`: try again after
    /// [`QUIET`], not at every look.
    fn failed(&mut self, now: Instant) {
        self.retry_at = Some(now + QUIET);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `versions`, one per tick, and returns the ticks it writes at.
    fn writes(versions: impl IntoIterator<Item = Version>) -> Vec<usize> {
        let start = Instant::now();
        let mut debounce = Debounce::default();
        let mut written = Vec::new();
        for (tick, version) in versions.into_iter().enumerate() {
            let now = start + TICK * tick as u32;
            if debounce.due(version, now) {
                debounce.wrote(version, now);
                written.push(tick);
            }
        }
        written
    }

    /// Ticks in `duration`.
    fn ticks(duration: Duration) -> usize {
        (duration.as_millis() / TICK.as_millis()) as usize
    }

    #[test]
    fn a_catch_up_is_written_once_the_parts_stay_the_same() {
        let after_quiet = ticks(QUIET);
        let written = writes(std::iter::repeat_n((1, 7, 1), after_quiet + 10));
        assert_eq!(written, [after_quiet]);
    }

    #[test]
    fn a_burst_of_live_changes_makes_one_write_not_one_per_change() {
        // A change every tick for 20 s, then quiet.
        let burst = ticks(Duration::from_secs(20));
        let versions = (0..burst as u64)
            .map(|seq| (1, seq, 1))
            .chain(std::iter::repeat_n((1, burst as u64, 1), ticks(QUIET) + 2));
        let written = writes(versions);
        assert_eq!(written, [burst + ticks(QUIET)]);
    }

    #[test]
    fn changes_that_never_stop_are_still_written_now_and_then() {
        let two_minutes = ticks(Duration::from_secs(120));
        let written = writes((0..two_minutes as u64).map(|seq| (1, seq, 1)));
        // Once per MAX_WAIT, counted from the first change after a write.
        let wait = ticks(MAX_WAIT);
        assert_eq!(written, [wait, 2 * wait + 1, 3 * wait + 2]);
    }

    #[test]
    fn a_long_burst_is_written_within_max_wait_though_its_version_moves_on() {
        // The writer saw (1, 59, 1); by its turn, more Changes came.
        assert!(still_worth_writing((1, 59, 1), Some((1, 64, 1))));
        // Signed out, offline, or signed in again meanwhile: not written.
        assert!(!still_worth_writing((1, 59, 1), None));
        assert!(!still_worth_writing((1, 59, 1), Some((2, 64, 1))));
        // And the debounce asks for it within MAX_WAIT of the first change.
        let written = writes((0..ticks(MAX_WAIT) as u64 + 1).map(|seq| (1, seq, 1)));
        assert_eq!(written, [ticks(MAX_WAIT)]);
    }

    #[test]
    fn parts_with_no_category_names_wait_for_the_next_version() {
        let start = Instant::now();
        let mut debounce = Debounce::default();
        assert!(!debounce.due((1, 7, 1), start));
        assert!(debounce.due((1, 7, 1), start + QUIET));
        debounce.skip((1, 7, 1));
        assert!(!debounce.due((1, 7, 1), start + QUIET * 5));
        assert!(
            !debounce.due((1, 8, 1), start + QUIET * 5),
            "quiet again first"
        );
        assert!(debounce.due((1, 8, 1), start + QUIET * 6));
    }

    #[test]
    fn a_failed_write_is_tried_again_soon_but_not_at_every_look() {
        let start = Instant::now();
        let mut debounce = Debounce::default();
        let quiet = start + QUIET;
        assert!(!debounce.due((1, 7, 1), start) && debounce.due((1, 7, 1), quiet));
        debounce.failed(quiet);
        assert!(!debounce.due((1, 7, 1), quiet + TICK));
        assert!(debounce.due((1, 7, 1), quiet + QUIET));
    }

    #[test]
    fn unchanged_parts_are_written_again_only_to_refresh_the_time() {
        let quiet = ticks(QUIET);
        let written = writes(std::iter::repeat_n((1, 7, 1), quiet + ticks(REFRESH) + 1));
        assert_eq!(written, [quiet, quiet + ticks(REFRESH)]);
    }
}

//! One team's last-known parts: what the part list and the Libraries
//! browser show of that team.
//!
//! They read a team's parts only through [`TeamCatalog::last_known_parts`].
//! The parts come from one of two places:
//!
//! - **The session's copy** of the team catalog, kept in step with the
//!   server, once this run's first catch-up has finished. A team whose
//!   server then drops keeps these parts, marked offline.
//! - **The team's parts file** ([`PartsFile`], kicad-parts-ed46), which an
//!   earlier run wrote, until then. So a team whose server cannot be
//!   reached at start still shows its parts, marked offline and read-only.
//!   The list of teams reads and writes the files (`teams::parts_file`).

use super::TeamCatalog;
use crate::PartValues;
use desktop_contract::LiveUpdates;
use std::collections::BTreeMap;
use std::sync::Arc;

/// One team's parts, as they were last known.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KnownParts {
    /// The team's server address.
    pub team: String,
    /// How the person knows the team: its code, such as `MA`, or else its
    /// address without `https://`.
    pub label: String,
    /// Changes whenever the parts may have changed, so what is made from
    /// them is made again only then.
    pub version: (u64, u64, u64),
    /// Each part's current values, by IPN.
    pub values: BTreeMap<String, PartValues>,
    /// Every category's plain name, by code.
    pub category_names: BTreeMap<String, String>,
}

impl KnownParts {
    /// Parts given directly, as tests and tools give them.
    pub fn new(
        team: &str,
        label: &str,
        values: BTreeMap<String, PartValues>,
        category_names: BTreeMap<String, String>,
    ) -> Self {
        Self {
            team: team.to_owned(),
            label: label.to_owned(),
            version: (0, 0, 0),
            values,
            category_names,
        }
    }
}

/// What a team's parts file holds: its last-known parts as an earlier run
/// wrote them to disk. It holds no password or token.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartsFile {
    /// The team server the parts came from. Parts are only ever shown for
    /// this server (kicad-parts-092b).
    pub server_url: String,
    /// When the parts last matched the team server, in milliseconds since
    /// 1970.
    pub as_of_ms: u64,
    /// Each part's values, by IPN.
    pub values: BTreeMap<String, PartValues>,
    /// Every category's plain name, by code.
    pub category_names: BTreeMap<String, String>,
}

/// The parts file a team shows until its first catch-up, if any.
#[derive(Default)]
pub(super) struct FromFile {
    /// Counts each parts file shown, so each has its own version.
    shown: u64,
    parts: Option<Arc<PartsFile>>,
}

/// Where a team's last-known parts come from now.
enum Source {
    /// None to show: signed out, or asked to sign in again.
    Nothing,
    /// The team's parts file, the `shown`th one.
    File { shown: u64, parts: Arc<PartsFile> },
    /// The session's copy, at this version.
    Session((u64, u64, u64)),
}

impl TeamCatalog {
    /// The team's last-known parts, named `team` (its server address) and
    /// `label`; `None` when there are none to show. A team that must sign
    /// in again shows none, so the person never mistakes old parts for
    /// current ones.
    ///
    /// Searching asks for these on every keystroke, so they are made again
    /// only when the team catalog moved on (a new Change, a catch-up or
    /// another session), and reading them makes no request.
    pub fn last_known_parts(&self, team: &str, label: &str) -> Option<Arc<KnownParts>> {
        let source = self.source();
        // The version is read before the values: values newer than their
        // version are made again at the next change, never kept stale.
        // Session versions start at 1, so a file's never matches one.
        let version = match &source {
            Source::Nothing => return None,
            Source::File { shown, .. } => (0, *shown, 0),
            Source::Session(version) => {
                self.forget_parts_file_once_caught_up();
                *version
            }
        };
        let cached = self.known_parts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = cached
            .as_ref()
            .filter(|known| known.version == version && known.team == team && known.label == label)
        {
            return Some(known.clone());
        }
        drop(cached);
        let (values, category_names) = match source {
            Source::File { parts, .. } => (parts.values.clone(), parts.category_names.clone()),
            _ => (self.current_values(), self.category_names()),
        };
        let known = Arc::new(KnownParts {
            team: team.to_owned(),
            label: label.to_owned(),
            version,
            values,
            category_names,
        });
        *self.known_parts.lock().unwrap_or_else(|e| e.into_inner()) = Some(known.clone());
        Some(known)
    }

    /// Shows `parts`, from the team's parts file, until this run's first
    /// catch-up. The list of teams calls it only for a file from this
    /// team's own server.
    pub(crate) fn show_parts_file(&self, parts: PartsFile) {
        let mut from_file = self.lock_from_file();
        from_file.shown += 1;
        from_file.parts = Some(Arc::new(parts));
    }

    /// Stops showing the parts file, such as after signing out or when the
    /// team asks for its password again. The file itself is the list of
    /// teams' business.
    pub(crate) fn forget_parts_file(&self) {
        self.lock_from_file().parts = None;
    }

    /// When the parts shown last matched the team server, in milliseconds
    /// since 1970, while they may be out of date: they come from the parts
    /// file, or the server dropped. `None` while they are current, or while
    /// none are shown. `session` is the session's state, if one runs: its
    /// live updates, its count of catch-ups, and when it was
    /// last in step.
    pub(super) fn parts_as_of(&self, session: Option<(&LiveUpdates, u64, u64)>) -> Option<u64> {
        let file_at = || {
            self.lock_from_file()
                .parts
                .as_ref()
                .map(|file| file.as_of_ms)
        };
        match session {
            None => file_at(),
            Some((LiveUpdates::SignInAgain { .. }, _, _)) => None,
            // Not caught up yet: the parts file shows, if any.
            Some((_, 0, _)) => file_at(),
            Some((LiveUpdates::Reconnecting { .. }, _, in_step_at)) => Some(in_step_at),
            Some(_) => None,
        }
    }

    /// The version of the session's parts while they match the team server
    /// now: the session is live and has caught up. Only then are they worth
    /// writing to the parts file.
    pub(crate) fn version_in_step(&self) -> Option<(u64, u64, u64)> {
        let state = self.lock();
        let session = state.session.as_ref()?;
        let (live, checkpoint, catch_ups) = session.follower.snapshot();
        (live == LiveUpdates::Live && catch_ups > 0).then_some((
            state.number,
            checkpoint,
            catch_ups,
        ))
    }

    /// Where the team's last-known parts come from now. Changes nothing.
    fn source(&self) -> Source {
        let state = self.lock();
        let from_file = self.lock_from_file();
        let file = from_file.parts.clone().map(|parts| Source::File {
            shown: from_file.shown,
            parts,
        });
        let Some(session) = state.session.as_ref() else {
            return file.unwrap_or(Source::Nothing);
        };
        let (live, checkpoint, catch_ups) = session.follower.snapshot();
        if matches!(live, LiveUpdates::SignInAgain { .. }) {
            return Source::Nothing;
        }
        match file {
            // Not caught up yet: the parts file shows until then.
            Some(file) if catch_ups == 0 => file,
            _ => Source::Session((state.number, checkpoint, catch_ups)),
        }
    }

    /// Frees the parts file once the session has caught up: from then on
    /// the session's copy shows.
    fn forget_parts_file_once_caught_up(&self) {
        let state = self.lock();
        let caught_up = state
            .session
            .as_ref()
            .is_some_and(|session| session.follower.snapshot().2 > 0);
        if caught_up {
            self.lock_from_file().parts = None;
        }
    }

    fn lock_from_file(&self) -> std::sync::MutexGuard<'_, FromFile> {
        self.from_file.lock().unwrap_or_else(|e| e.into_inner())
    }
}

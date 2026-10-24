//! One team's last-known parts: what the part list and the Libraries
//! browser show of that team.
//!
//! They read a team's parts only through [`TeamCatalog::last_known_parts`].
//! The parts come from one of two places:
//!
//! - **The session's copy** of the team catalog, kept in step with the
//!   server, once this run's first catch-up has finished. A team whose
//!   server then drops keeps these parts, marked offline.
//! - **The parts an earlier run kept on disk** ([`KeptParts`],
//!   kicad-parts-ed46), until then. So a team whose server cannot be
//!   reached at start still shows its parts, marked offline and read-only.
//!   The list of teams reads and writes the files (`teams::kept_parts`).

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

/// A team's last-known parts as kept on disk between runs. It holds no
/// password or token.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeptParts {
    /// The team server the parts came from. Parts are only ever shown for
    /// this server (kicad-parts-092b).
    pub server_url: String,
    /// The team's code, such as `MA`; blank if it had none yet.
    pub code: String,
    /// When the parts last matched the team server, in milliseconds since
    /// 1970.
    pub as_of_ms: u64,
    /// Each part's values, by IPN.
    pub values: BTreeMap<String, PartValues>,
    /// Every category's plain name, by code.
    pub category_names: BTreeMap<String, String>,
}

/// The parts kept on disk that a team shows until its first catch-up.
#[derive(Default)]
pub(super) struct Kept {
    /// Counts each set of kept parts shown, so each has its own version.
    shown: u64,
    parts: Option<KeptParts>,
}

/// Where a team's last-known parts come from now.
enum Source {
    /// None to show: signed out, or asked to sign in again.
    Nothing,
    /// The parts kept on disk; the number tells one kept set from another.
    Kept(u64),
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
    /// only when the team catalog moved on (a new Change, a finished import
    /// or another session), and reading them makes no request.
    pub fn last_known_parts(&self, team: &str, label: &str) -> Option<Arc<KnownParts>> {
        // The version is read before the values: values newer than their
        // version are made again at the next change, never kept stale.
        let version = match self.source() {
            Source::Nothing => return None,
            Source::Kept(number) => (0, number, 0),
            Source::Session(version) => version,
        };
        let cached = self.known_parts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = cached
            .as_ref()
            .filter(|known| known.version == version && known.team == team && known.label == label)
        {
            return Some(known.clone());
        }
        drop(cached);
        let (values, category_names) = if version.0 == 0 {
            // Kept parts: they may have been forgotten since `source`.
            let kept = self.lock_kept();
            let kept = kept.parts.as_ref()?;
            (kept.values.clone(), kept.category_names.clone())
        } else {
            (self.current_values(), self.category_names())
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

    /// Shows `kept`, the parts an earlier run kept on disk, until this
    /// run's first catch-up. The list of teams calls it only for parts
    /// from this team's own server.
    pub(crate) fn show_kept(&self, kept: KeptParts) {
        let mut slot = self.lock_kept();
        slot.shown += 1;
        slot.parts = Some(kept);
    }

    /// Stops showing parts kept on disk, such as after signing out or when
    /// the team asks for its password again. The file is the list of
    /// teams' business.
    pub(crate) fn forget_kept(&self) {
        self.lock_kept().parts = None;
    }

    /// When the parts shown last matched the team server, in milliseconds
    /// since 1970, while they may be out of date: they are kept from an
    /// earlier run, or the server dropped. `None` while they are current,
    /// or while none are shown. `session` is the session's state, if one
    /// runs: its live updates, its count of catch-ups and imports, and
    /// when it was last in step.
    pub(super) fn parts_as_of(&self, session: Option<(&LiveUpdates, u64, u64)>) -> Option<u64> {
        let kept_at = || self.lock_kept().parts.as_ref().map(|kept| kept.as_of_ms);
        match session {
            None => kept_at(),
            Some((LiveUpdates::SignInAgain { .. }, _, _)) => None,
            // Not caught up yet: the kept parts show, if any.
            Some((_, 0, _)) => kept_at(),
            Some((LiveUpdates::Reconnecting { .. }, _, in_step_at)) => Some(in_step_at),
            Some(_) => None,
        }
    }

    /// The version of the session's parts while they match the team server
    /// now: the session is live and has caught up. Only then are they worth
    /// keeping on disk.
    pub(crate) fn version_in_step(&self) -> Option<(u64, u64, u64)> {
        let state = self.lock();
        let session = state.session.as_ref()?;
        let (live, checkpoint, imports) = session.follower.snapshot();
        (live == LiveUpdates::Live && imports > 0).then_some((state.number, checkpoint, imports))
    }

    fn source(&self) -> Source {
        let state = self.lock();
        let mut kept = self.lock_kept();
        let shown = kept.parts.as_ref().map(|_| Source::Kept(kept.shown));
        let Some(session) = state.session.as_ref() else {
            return shown.unwrap_or(Source::Nothing);
        };
        let (live, checkpoint, imports) = session.follower.snapshot();
        if matches!(live, LiveUpdates::SignInAgain { .. }) {
            return Source::Nothing;
        }
        if imports == 0 {
            // Not caught up yet: the kept parts, if any, show until then.
            if let Some(shown) = shown {
                return shown;
            }
        } else {
            // Caught up: the kept parts are no longer needed in memory.
            kept.parts = None;
        }
        Source::Session((state.number, checkpoint, imports))
    }

    fn lock_kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner())
    }
}

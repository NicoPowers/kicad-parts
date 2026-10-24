//! One team's last-known parts: what the part list and the Libraries
//! browser show of that team.
//!
//! They read a team's parts only through [`TeamCatalog::last_known_parts`].
//! Today the parts come from the copy of the team catalog that the session
//! keeps in step with the server. So a team whose server cannot be reached
//! keeps its parts, marked offline, until the app closes. Keeping them on
//! disk for offline starts (kicad-parts-ed46) only changes where this reads
//! them from; nothing that shows them changes.

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
        let version = {
            let state = self.lock();
            let session = state.session.as_ref()?;
            let (live, checkpoint, imports) = session.follower.snapshot();
            if matches!(live, LiveUpdates::SignInAgain { .. }) {
                return None;
            }
            (state.number, checkpoint, imports)
        };
        let cached = self.known_parts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = cached
            .as_ref()
            .filter(|known| known.version == version && known.team == team && known.label == label)
        {
            return Some(known.clone());
        }
        drop(cached);
        let known = Arc::new(KnownParts {
            team: team.to_owned(),
            label: label.to_owned(),
            version,
            values: self.current_values(),
            category_names: self.category_names(),
        });
        *self.known_parts.lock().unwrap_or_else(|e| e.into_inner()) = Some(known.clone());
        Some(known)
    }
}

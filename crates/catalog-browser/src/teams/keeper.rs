//! Keeps one team's sign-in alive in the background, so the person stays
//! signed in to every team, also after a restart:
//!
//! - At start, it looks for the team's kept sign-in. If there is one, it
//!   carries it on with no password, and keeps trying while the team
//!   server cannot be reached.
//! - While signed in, it keeps the newest token in the store and renews it
//!   (PocketBase's auth-refresh) when half its life has passed.
//! - When the team server no longer accepts the sign-in, it forgets the
//!   kept token and asks for this team's password again. Other teams carry
//!   on: each team has its own keeper.
//!
//! The person may sign in, sign out or remove the team while a request
//! runs, so an answer may be about a sign-in that is gone. Every write to
//! the store for this team takes the team's turn, and a judgement acts only
//! if, in that turn, the sign-in it judged is still the team's: the keeper
//! is not stopped, the person has not signed in or out since, and the
//! session is the one judged. A stopped keeper never writes to the store.

use super::sign_ins::SignInStore;
use crate::team::{Renewed, Resumed, TeamCatalog};
use base64::Engine;
use desktop_contract::{LiveUpdates, TeamSession};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the keeper looks at the team's session.
const TICK: Duration = Duration::from_millis(250);
/// The first wait before trying an unreachable server again; it doubles
/// after each failed try, up to [`MAX_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// How long to wait before renewing again after a renewal failed for a
/// reason other than a rejected token, such as a network problem.
const RENEW_RETRY: Duration = Duration::from_secs(60);
/// When a token's expiry cannot be read, renew it this often.
const RENEW_UNKNOWN: Duration = Duration::from_secs(60 * 60);
const RENEW_AT_LEAST_EVERY: Duration = Duration::from_secs(12 * 60 * 60);

/// What one team's keeper needs.
pub(super) struct KeeperTask {
    pub(super) server_url: String,
    pub(super) catalog: Arc<TeamCatalog>,
    pub(super) store: Arc<dyn SignInStore>,
    /// Told, in a plain sentence, when the store fails, so the app can say
    /// that sign-ins last only until it closes.
    pub(super) store_failed: Arc<dyn Fn(String) + Send + Sync>,
}

/// The keeper thread of one team. Dropping it stops the thread.
pub(super) struct Keeper {
    shared: Arc<Shared>,
}

struct Shared {
    task: KeeperTask,
    /// `Some` while the app holds a kept sign-in it has not carried on yet:
    /// `None` inside while the first try runs, then why the server could
    /// not be reached.
    reaching: Mutex<Option<Option<String>>>,
    /// Grows whenever the person signs in or out of this team, so an
    /// answer about an earlier sign-in is dropped.
    changes: AtomicU64,
    turn: Mutex<()>,
    stop: AtomicBool,
}

/// The sign-in a rejection is about.
enum Judged<'a> {
    /// The session that sends this token.
    Session(&'a str),
    /// The kept sign-in, while no session runs yet.
    Kept,
}

impl Keeper {
    /// Starts keeping the team's sign-in. With `look_for_kept`, it first
    /// carries on a sign-in kept from an earlier run, if there is one.
    pub(super) fn start(task: KeeperTask, look_for_kept: bool) -> Self {
        let shared = Arc::new(Shared {
            task,
            reaching: Mutex::new(look_for_kept.then_some(None)),
            changes: AtomicU64::new(0),
            turn: Mutex::new(()),
            stop: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        std::thread::spawn(move || keep(&thread_shared, look_for_kept));
        Self { shared }
    }

    /// `Some` while the app is still carrying on a kept sign-in: `None`
    /// inside during the first try, then why the server could not be
    /// reached.
    pub(super) fn reaching(&self) -> Option<Option<String>> {
        self.shared.reaching().clone()
    }

    /// The person signed in with a password: stop carrying on any kept
    /// sign-in, and drop any answer about an earlier one. The keeper keeps
    /// the new token itself.
    pub(super) fn signed_in(&self) {
        let _turn = self.shared.turn();
        self.shared.changes.fetch_add(1, Ordering::SeqCst);
        *self.shared.reaching() = None;
    }

    /// Signs out of the team and forgets its kept sign-in. An error says
    /// why the store could not forget it.
    pub(super) fn sign_out(&self) -> Result<(), String> {
        let _turn = self.shared.turn();
        self.shared.changes.fetch_add(1, Ordering::SeqCst);
        *self.shared.reaching() = None;
        self.shared.task.catalog.sign_out();
        self.shared.task.store.forget(&self.shared.task.server_url)
    }
}

impl Drop for Keeper {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Shared {
    fn reaching(&self) -> MutexGuard<'_, Option<Option<String>>> {
        self.reaching.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn turn(&self) -> MutexGuard<'_, ()> {
        self.turn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changes(&self) -> u64 {
        self.changes.load(Ordering::SeqCst)
    }

    /// Whether a judgement made when `changes` stood at `changes_at` may
    /// still act. Call it in the team's turn.
    fn still_current(&self, changes_at: u64) -> bool {
        !self.stop.load(Ordering::SeqCst) && self.changes() == changes_at
    }

    fn store_result(&self, result: Result<(), String>) {
        if let Err(problem) = result {
            (self.task.store_failed)(problem);
        }
    }

    /// The server no longer accepts the `judged` sign-in: forget it, and
    /// ask for this team's password again. Does nothing if, by now, that
    /// sign-in is gone.
    fn rejected(&self, judged: Judged<'_>, changes_at: u64) {
        let _turn = self.turn();
        if !self.still_current(changes_at) {
            return;
        }
        let token = match judged {
            Judged::Session(token) => Some(token),
            Judged::Kept => None,
        };
        if self.task.catalog.sign_in_again(token) {
            *self.reaching() = None;
            self.store_result(self.task.store.forget(&self.task.server_url));
        }
    }

    /// Keeps `token`, the session's token, unless that changed meanwhile.
    /// Says whether it did.
    fn keep(&self, token: &str) -> bool {
        let _turn = self.turn();
        if self.stop.load(Ordering::SeqCst) || self.task.catalog.token().as_deref() != Some(token) {
            return false;
        }
        self.store_result(self.task.store.keep(&self.task.server_url, token));
        true
    }
}

/// The keeper thread.
fn keep(shared: &Shared, look_for_kept: bool) {
    let task = &shared.task;
    let mut to_carry_on = None;
    if look_for_kept {
        match task.store.read(&task.server_url) {
            Ok(Some(token)) => to_carry_on = Some(token),
            Ok(None) => *shared.reaching() = None,
            Err(problem) => {
                *shared.reaching() = None;
                (task.store_failed)(problem);
            }
        }
    }
    let mut retry = FIRST_RETRY;
    let mut try_at = Instant::now();
    let mut kept: Option<String> = None;
    // Wall-clock time, so a computer that slept still renews on waking:
    // an Instant stands still during sleep on macOS and Linux.
    let mut renew_at = SystemTime::now();
    while !shared.stop.load(Ordering::SeqCst) {
        if let Some(token) = to_carry_on.clone() {
            if shared.reaching().is_none() {
                // The person signed in or out meanwhile.
                to_carry_on = None;
                continue;
            }
            if Instant::now() >= try_at {
                let changes_at = shared.changes();
                match task.catalog.resume(&task.server_url, &token) {
                    Resumed::SignedIn { token: carried_on } => {
                        to_carry_on = None;
                        let _turn = shared.turn();
                        *shared.reaching() = None;
                        if !shared.still_current(changes_at)
                            && task.catalog.token() == Some(carried_on)
                        {
                            // The person signed out while this ran: do not
                            // bring the session back. A password sign-in
                            // has its own token and stays.
                            task.catalog.sign_out();
                        }
                    }
                    // A password sign-in finished first.
                    Resumed::AlreadySignedIn => to_carry_on = None,
                    Resumed::Unreachable(problem) => {
                        *shared.reaching() = Some(Some(problem));
                        try_at = Instant::now() + retry;
                        retry = (retry * 2).min(MAX_RETRY);
                    }
                    Resumed::Rejected => {
                        to_carry_on = None;
                        shared.rejected(Judged::Kept, changes_at);
                    }
                }
            }
            std::thread::sleep(TICK);
            continue;
        }
        let changes_at = shared.changes();
        let token = task.catalog.token();
        match (task.catalog.status().session, token) {
            (
                TeamSession::SignedIn {
                    live: LiveUpdates::SignInAgain { .. },
                    ..
                },
                Some(token),
            ) => {
                kept = None;
                shared.rejected(Judged::Session(&token), changes_at);
            }
            (TeamSession::SignedIn { .. }, Some(token)) => {
                if kept.as_ref() != Some(&token) {
                    if shared.keep(&token) {
                        renew_at = SystemTime::now() + renewal_delay(&token, SystemTime::now());
                        kept = Some(token);
                    }
                } else if SystemTime::now() >= renew_at {
                    match task.catalog.renew() {
                        // The next look keeps the new token.
                        Renewed::Token(_) | Renewed::SessionChanged => {}
                        Renewed::Rejected => {
                            kept = None;
                            shared.rejected(Judged::Session(&token), changes_at);
                        }
                        Renewed::Failed(_) => renew_at = SystemTime::now() + RENEW_RETRY,
                    }
                }
            }
            _ => kept = None,
        }
        std::thread::sleep(TICK);
    }
}

/// How long to wait before renewing `token`: half the life it has left at
/// `now`, at most [`RENEW_AT_LEAST_EVERY`]. A token whose expiry cannot be
/// read is renewed every [`RENEW_UNKNOWN`].
fn renewal_delay(token: &str, now: SystemTime) -> Duration {
    let Some(expires) = expiry(token) else {
        return RENEW_UNKNOWN;
    };
    let now = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let left = Duration::from_secs(expires.saturating_sub(now));
    (left / 2).clamp(Duration::from_secs(1), RENEW_AT_LEAST_EVERY)
}

/// The `exp` claim of a PocketBase token (a JWT), in seconds since 1970.
/// Only read to plan the renewal; the server checks the token.
fn expiry(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims["exp"].as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_expiring_at(exp: u64) -> String {
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"id":"u1","exp":{exp}}}"#));
        format!("eyJhbGciOiJIUzI1NiJ9.{claims}.signature")
    }

    #[test]
    fn a_token_is_renewed_when_half_its_life_has_passed() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        // PocketBase's default: 7 days. Renewed after 12 hours at most.
        let week = token_expiring_at(1_000_000 + 7 * 24 * 3600);
        assert_eq!(renewal_delay(&week, now), RENEW_AT_LEAST_EVERY);
        let hour = token_expiring_at(1_000_000 + 3600);
        assert_eq!(renewal_delay(&hour, now), Duration::from_secs(1800));
        // An expired token is renewed at once, which tells whether the
        // server still accepts it.
        let expired = token_expiring_at(999_000);
        assert_eq!(renewal_delay(&expired, now), Duration::from_secs(1));
        assert_eq!(renewal_delay("not-a-token", now), RENEW_UNKNOWN);
    }
}

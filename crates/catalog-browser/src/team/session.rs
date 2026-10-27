//! One team's session: the signed-in client, who is signed in, and the
//! background thread that keeps the team catalog's replica in step with the
//! server.
//!
//! While signed in, the follower subscribes to realtime notifications of
//! new Changes, reads the current state of every part, and, after any
//! disconnect, reconnects and catches up from the last Change `seq` it
//! applied.

use super::CategoryNames;
use catalog::pocketbase::StoredChange;
use catalog::replica::CatalogReplica;
use collaboration::{
    CollabError, ConnectionProblem, PocketBaseClient, RealtimeConnection, ServerAddress,
};
use desktop_contract::{LiveUpdates, SignInRequest};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The realtime topic of new Changes.
const CHANGES_TOPIC: &str = "catalog_changes";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the follower waits for an event before checking whether it
/// should stop.
const POLL: Duration = Duration::from_millis(250);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// The signed-in client, shared with the follower. Renewing the sign-in
/// puts a client with the new token here; the follower takes it the next
/// time it connects.
type SharedClient = Arc<Mutex<PocketBaseClient>>;

pub(super) struct Session {
    client: SharedClient,
    pub(super) name: String,
    pub(super) email: String,
    pub(super) can_edit: bool,
    pub(super) follower: Follower,
    pub(super) category_names: Arc<Mutex<Option<NamesRead>>>,
}

impl Session {
    /// Starts following the team catalog as the signed-in `account` (the
    /// `users` record), with `client`, which sends that account's token.
    fn start(client: PocketBaseClient, account: &serde_json::Value) -> Self {
        let text = |name: &str| account[name].as_str().unwrap_or_default().to_owned();
        let name = Some(text("name"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| text("email"));
        let client = Arc::new(Mutex::new(client));
        Self {
            follower: Follower::start(client.clone()),
            client,
            name,
            email: text("email"),
            can_edit: account["can_edit_catalog"].as_bool().unwrap_or(false),
            category_names: Arc::default(),
        }
    }

    /// The signed-in client, with the newest token.
    pub(super) fn client(&self) -> PocketBaseClient {
        self.client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Uses `client`, which sends a renewed token, from now on.
    pub(super) fn renewed(&self, client: PocketBaseClient) {
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = client;
    }
}

/// The category names last read, and the position of the team catalog
/// (checkpoint and catch-ups) they were read at.
pub(super) type NamesRead = ((u64, u64), CategoryNames);

pub(super) struct Live {
    pub(super) updates: LiveUpdates,
    pub(super) replica: CatalogReplica,
    /// Counts finished catch-ups.
    pub(super) catch_ups: u64,
    /// When the replica last matched the server, in milliseconds since
    /// 1970: when a catch-up finished, or when live updates broke after
    /// one. While live, the replica matches the server now.
    pub(super) in_step_at_ms: u64,
}

/// The background thread that follows new Changes.
pub(super) struct Follower {
    pub(super) live: Arc<Mutex<Live>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Follower {
    fn start(client: SharedClient) -> Self {
        let live = Arc::new(Mutex::new(Live {
            updates: LiveUpdates::Connecting,
            replica: CatalogReplica::new(),
            catch_ups: 0,
            in_step_at_ms: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (live, stop) = (live.clone(), stop.clone());
            std::thread::spawn(move || follow_changes(&client, &live, &stop))
        };
        Self {
            live,
            stop,
            thread: Some(thread),
        }
    }

    pub(super) fn snapshot(&self) -> (LiveUpdates, u64, u64) {
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        (
            live.updates.clone(),
            live.replica.checkpoint(),
            live.catch_ups,
        )
    }

    /// When the replica last matched the server (see [`Live`]).
    pub(super) fn in_step_at(&self) -> u64 {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .in_step_at_ms
    }
}

impl Drop for Follower {
    /// Asks the thread to stop without waiting for it: it may be in the
    /// middle of a request to a server that does not answer, and it ends
    /// by itself once that request returns or times out.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        drop(self.thread.take());
    }
}

/// Checks the address, signs in, reads the account, and starts following.
pub(super) fn sign_in(request: &SignInRequest) -> Result<Session, String> {
    // https:// anywhere, http:// only on this computer: see ServerAddress.
    let server_url = ServerAddress::for_sign_in(&request.server_url)
        .map_err(|refusal| refusal.to_string())?
        .url();
    let email = request.email.trim();
    if email.is_empty() || request.password.is_empty() {
        return Err("Enter your email and password.".into());
    }
    let client = PocketBaseClient::new(&server_url)
        .authenticate("users", email, &request.password)
        .map_err(|error| match error {
            CollabError::Transport(problem) => problem.to_string(),
            CollabError::Http { status: 400, .. } | CollabError::Unauthorized(_) => {
                "The email or password is not right.".into()
            }
            other => other.to_string(),
        })?;
    let id = client.auth_id().unwrap_or_default().to_owned();
    let account: serde_json::Value = client
        .get(&format!("/api/collections/users/records/{id}"))
        .and_then(|reply| reply.json())
        .map_err(|error| error.to_string())?;
    Ok(Session::start(client, &account))
}

/// Carries on a sign-in kept from an earlier run: renews `token` with the
/// team server at `server_url`, then starts following. No password is
/// needed. A token the server no longer accepts is
/// [`CollabError::Unauthorized`]; a server that cannot be reached is
/// [`CollabError::Transport`].
pub(super) fn resume(server_url: &str, token: &str) -> Result<Session, CollabError> {
    let (client, account) = PocketBaseClient::new(server_url)
        .with_token(token)?
        .refresh_auth("users")?;
    Ok(Session::start(client, &account))
}

/// Follows new Changes until `stop` is set: subscribe, catch up, apply each
/// notification; on any break, wait and do it all again, waiting twice as
/// long after each failed try (up to [`MAX_RECONNECT_DELAY`]). The first
/// catch-up reads the current state of every part
/// ([`CatalogReplica::from_server`]); later ones replay only the Changes
/// after the checkpoint. Catching up after subscribing means a Change
/// published in between arrives twice at most, once in the catch-up and
/// once as a notification, and the replica ignores the second copy. If the
/// server rejects the sign-in, it stops and asks the person to sign in
/// again instead of retrying.
fn follow_changes(shared: &Mutex<PocketBaseClient>, live: &Mutex<Live>, stop: &AtomicBool) {
    let set = |updates: LiveUpdates| {
        live.lock().unwrap_or_else(|e| e.into_inner()).updates = updates;
    };
    let caught_up = || {
        live.lock().unwrap_or_else(|e| e.into_inner()).catch_ups += 1;
    };
    let in_step_now = || {
        live.lock().unwrap_or_else(|e| e.into_inner()).in_step_at_ms = now_ms();
    };
    // Network calls run on a copy so `status` never waits for them.
    let update_replica = |work: &dyn Fn(&mut CatalogReplica) -> Result<(), CollabError>| {
        let mut replica = live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replica
            .clone();
        work(&mut replica)?;
        live.lock().unwrap_or_else(|e| e.into_inner()).replica = replica;
        Ok::<(), CollabError>(())
    };
    let mut delay = RECONNECT_DELAY;
    while !stop.load(Ordering::SeqCst) {
        let went_live = std::cell::Cell::new(false);
        // The newest client, in case the sign-in was renewed meanwhile.
        let client = &shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let result = (|| -> Result<(), CollabError> {
            let mut connection = RealtimeConnection::connect(client, CONNECT_TIMEOUT)?;
            connection.subscribe(client, &[CHANGES_TOPIC])?;
            update_replica(&|replica| {
                // A replica that has applied nothing yet starts from the
                // current state, which is much quicker than replaying
                // every Change ever made.
                if replica.checkpoint() == 0 {
                    *replica = CatalogReplica::from_server(client)?;
                } else {
                    replica.catch_up(client)?;
                }
                Ok(())
            })?;
            caught_up();
            in_step_now();
            set(LiveUpdates::Live);
            went_live.set(true);
            while !stop.load(Ordering::SeqCst) {
                let Some(event) = connection.next_event(POLL)? else {
                    continue;
                };
                if event.data["action"] != "create" || event.topic != CHANGES_TOPIC {
                    continue;
                }
                let change: StoredChange = serde_json::from_value(event.data["record"].clone())
                    .map_err(|error| CollabError::Protocol(error.to_string()))?;
                update_replica(&|replica| replica.receive(&change, client).map(|_| ()))?;
            }
            Ok(())
        })();
        if went_live.get() {
            delay = RECONNECT_DELAY;
            // Live until now: the parts shown are as of this moment.
            in_step_now();
        }
        if let Err(error) = result {
            match after_break(&error, went_live.get()) {
                // Status stays Live: the catch-up after reconnecting brings
                // anything saved meanwhile. A failed reconnect shows.
                AfterBreak::ReconnectQuietly => {}
                AfterBreak::SignInAgain => {
                    set(LiveUpdates::SignInAgain {
                        message: error.to_string(),
                    });
                    return;
                }
                AfterBreak::Retry => set(LiveUpdates::Reconnecting {
                    message: error.to_string(),
                }),
            }
        }
        let mut waited = Duration::ZERO;
        while waited < delay && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(POLL);
            waited += POLL;
        }
        delay = next_delay(delay);
    }
}

#[derive(Debug, PartialEq)]
enum AfterBreak {
    /// Say "Reconnecting" with the problem, and try again.
    Retry,
    /// The server closed a live stream cleanly, as PocketBase does after
    /// 5 quiet minutes (proxies such as Cloudflare after about 100 s).
    /// That is normal, so reconnect without telling the person.
    ReconnectQuietly,
    /// The token was rejected; retrying cannot help.
    SignInAgain,
}

/// What to do after the stream broke with `error`. `went_live` says
/// whether this try got as far as live updates.
fn after_break(error: &CollabError, went_live: bool) -> AfterBreak {
    match error {
        CollabError::Unauthorized(_) | CollabError::Forbidden(_) => AfterBreak::SignInAgain,
        CollabError::Transport(ConnectionProblem::ClosedByServer) if went_live => {
            AfterBreak::ReconnectQuietly
        }
        _ => AfterBreak::Retry,
    }
}

/// The time now, in milliseconds since 1970.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

fn next_delay(delay: Duration) -> Duration {
    (delay * 2).min(MAX_RECONNECT_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_stream_the_server_closes_reconnects_quietly() {
        let closed = CollabError::Transport(ConnectionProblem::ClosedByServer);
        assert_eq!(after_break(&closed, true), AfterBreak::ReconnectQuietly);
        // Closed before it went live: something is wrong, so it shows.
        assert_eq!(after_break(&closed, false), AfterBreak::Retry);
    }

    #[test]
    fn a_rejected_sign_in_stops_retrying_and_other_breaks_back_off() {
        for rejected in [
            CollabError::Unauthorized("token".into()),
            CollabError::Forbidden("rules".into()),
        ] {
            assert_eq!(after_break(&rejected, true), AfterBreak::SignInAgain);
        }
        let refused = CollabError::Transport(ConnectionProblem::NotReachable("refused".into()));
        assert_eq!(after_break(&refused, true), AfterBreak::Retry);
        let mut delay = RECONNECT_DELAY;
        let mut delays = Vec::new();
        for _ in 0..7 {
            delays.push(delay.as_secs());
            delay = next_delay(delay);
        }
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30]);
    }
}

//! A stand-in team server on 127.0.0.1 that answers only sign-in, the
//! account and auth-refresh, the way PocketBase does. Everything else,
//! live updates included, gets 404, so a session's follower keeps
//! reconnecting. It is enough to test the list of teams and their kept
//! sign-ins without starting PocketBase; tooling/catalog-import's
//! desktop_teams test does the rest against real servers.

use base64::Engine;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const EMAIL: &str = "ada@example.test";
pub const PASSWORD: &str = "right-password";

pub struct FakeServer {
    pub url: String,
    state: Arc<State>,
}

#[derive(Default)]
struct State {
    /// Tokens the server accepts.
    tokens: Mutex<HashSet<String>>,
    issued: AtomicU64,
    password_sign_ins: AtomicU64,
    refreshes: AtomicU64,
    /// How long a new token lasts, in seconds.
    lifetime: AtomicU64,
    /// Drops every connection without an answer, as if the server were off.
    down: AtomicBool,
    stop: AtomicBool,
    /// While true, auth-refresh requests wait before they are answered, so
    /// a test can act while a renewal is in flight.
    hold_refreshes: Mutex<bool>,
    released: Condvar,
    /// How many auth-refresh requests are waiting.
    held: AtomicU64,
}

impl FakeServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = Arc::new(State::default());
        state.lifetime.store(7 * 24 * 3600, Ordering::SeqCst);
        let thread_state = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_state.stop.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let state = thread_state.clone();
                std::thread::spawn(move || answer(stream, &state));
            }
        });
        Self { url, state }
    }

    /// Ends every sign-in, as a changed password does.
    pub fn reject_all(&self) {
        self.state.tokens.lock().unwrap().clear();
    }

    /// Ends one sign-in.
    pub fn reject(&self, token: &str) {
        self.state.tokens.lock().unwrap().remove(token);
    }

    /// Makes auth-refresh requests wait until [`Self::release_refreshes`].
    pub fn hold_refreshes(&self) {
        *self.state.hold_refreshes.lock().unwrap() = true;
    }

    pub fn release_refreshes(&self) {
        *self.state.hold_refreshes.lock().unwrap() = false;
        self.state.released.notify_all();
    }

    /// How many auth-refresh requests are waiting now.
    pub fn held(&self) -> u64 {
        self.state.held.load(Ordering::SeqCst)
    }

    pub fn set_down(&self, down: bool) {
        self.state.down.store(down, Ordering::SeqCst);
    }

    pub fn set_token_lifetime(&self, seconds: u64) {
        self.state.lifetime.store(seconds, Ordering::SeqCst);
    }

    pub fn password_sign_ins(&self) -> u64 {
        self.state.password_sign_ins.load(Ordering::SeqCst)
    }

    pub fn refreshes(&self) -> u64 {
        self.state.refreshes.load(Ordering::SeqCst)
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.release_refreshes();
        self.state.stop.store(true, Ordering::SeqCst);
        // Wakes the accepting thread so it sees `stop`.
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
    }
}

fn answer(mut stream: TcpStream, state: &State) {
    if state.down.load(Ordering::SeqCst) {
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some((head, body)) = read_request(&mut stream) else {
        return;
    };
    let request_line = head.lines().next().unwrap_or_default().to_owned();
    if request_line.starts_with("POST /api/collections/users/auth-refresh ") {
        let mut hold = state.hold_refreshes.lock().unwrap();
        if *hold {
            state.held.fetch_add(1, Ordering::SeqCst);
            while *hold {
                hold = state.released.wait(hold).unwrap();
            }
            state.held.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let token = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        })
        .unwrap_or_default();
    let accepted = state.tokens.lock().unwrap().contains(&token);
    let (status, reply) = match request_line.split(' ').take(2).collect::<Vec<_>>()[..] {
        ["POST", "/api/collections/users/auth-with-password"] => {
            let sent: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            if sent["identity"] == EMAIL && sent["password"] == PASSWORD {
                state.password_sign_ins.fetch_add(1, Ordering::SeqCst);
                let token = issue(state);
                (
                    200,
                    serde_json::json!({ "token": token, "record": account() }),
                )
            } else {
                (
                    400,
                    serde_json::json!({ "message": "Failed to authenticate." }),
                )
            }
        }
        ["GET", "/api/collections/users/records/u1"] if accepted => (200, account()),
        ["POST", "/api/collections/users/auth-refresh"] if accepted => {
            state.refreshes.fetch_add(1, Ordering::SeqCst);
            let token = issue(state);
            (
                200,
                serde_json::json!({ "token": token, "record": account() }),
            )
        }
        ["GET", "/api/collections/users/records/u1"]
        | ["POST", "/api/collections/users/auth-refresh"] => (
            401,
            serde_json::json!({ "message": "The request requires valid record authorization token." }),
        ),
        _ => (404, serde_json::json!({ "message": "Not found." })),
    };
    let text = reply.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
}

fn account() -> serde_json::Value {
    serde_json::json!({
        "id": "u1",
        "email": EMAIL,
        "name": "Ada Editor",
        "can_edit_catalog": true,
    })
}

/// A new token that looks like PocketBase's: a JWT whose `exp` the app
/// reads to plan renewals.
///
/// Like PocketBase 0.38's, it holds only the account and an expiry in whole
/// seconds, so two tokens made in the same second are the same text.
fn issue(state: &State) -> String {
    state.issued.fetch_add(1, Ordering::SeqCst);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let exp = now + state.lifetime.load(Ordering::SeqCst);
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(
        r#"{{"collectionId":"_pb_users_auth_","exp":{exp},"id":"u1","refreshable":true,"type":"auth"}}"#
    ));
    let token = format!("eyJhbGciOiJIUzI1NiJ9.{claims}.fake-signature-{exp}");
    state.tokens.lock().unwrap().insert(token.clone());
    token
}

/// The request's head (as text) and body.
fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let end = loop {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    let mut body = bytes[end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some((head, body))
}

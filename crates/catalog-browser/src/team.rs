//! The team catalog use cases behind the desktop: sign in to the team's
//! PocketBase server, read one part's details and history, publish edits
//! and restores, and follow changes others make.
//!
//! The session (the auth token) lives only in memory: the app saves the
//! server address and email, never the password or the token, so signing in
//! again after a restart asks for the password.
//!
//! While signed in, a background thread keeps a [`CatalogReplica`] in step
//! with the server: it subscribes to realtime notifications of new Changes
//! and, after any disconnect, reconnects and catches up from the last
//! Change `seq` it applied. The UI polls [`TeamCatalog::status`], whose
//! `checkpoint` grows whenever anyone publishes a Change.

use crate::PartValues;
use catalog::pocketbase::{
    ConflictingRevision, EditCommand, PublishOutcome, PublishReply, RestoreCommand, StoredBlob,
    StoredCadItemRevision, StoredChange, StoredPart, StoredPartRevision, cad_item_revisions_by_id,
    changes_by_id, part_by_ipn, part_revisions, submit_edit, submit_restore,
};
use catalog::replica::CatalogReplica;
use catalog::{
    CadItemDetails, CadReference, CadReferenceRole, CadReferenceTarget, DetailField, PartDetails,
    compare_details,
};
use collaboration::{CollabError, CorrelationId, PocketBaseClient, RealtimeConnection};
use desktop_contract::{
    CadItemUse, ConflictRow, DetailChange, Field, LiveUpdates, ManufacturerPart, PreviewKind,
    PublishResult, RevisionDetails, RevisionSummary, SignInRequest, TeamConflict, TeamEditRequest,
    TeamPart, TeamRestoreRequest, TeamSession, TeamStatus,
};
use kicad_integration::{LibRef, StoredItem, footprint_choices};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// The realtime topic of new Changes.
const CHANGES_TOPIC: &str = "catalog_changes";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the follower waits for an event before checking whether it
/// should stop.
const POLL: Duration = Duration::from_millis(250);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// What the app remembers between runs. No password, no token.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct TeamSettings {
    #[serde(default)]
    server_url: String,
    #[serde(default)]
    email: String,
}

struct Session {
    client: PocketBaseClient,
    name: String,
    email: String,
    can_edit: bool,
    follower: Follower,
}

struct Live {
    updates: LiveUpdates,
    replica: CatalogReplica,
}

/// The background thread that follows new Changes.
struct Follower {
    live: Arc<Mutex<Live>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Follower {
    fn start(client: PocketBaseClient) -> Self {
        let live = Arc::new(Mutex::new(Live {
            updates: LiveUpdates::Connecting,
            replica: CatalogReplica::new(),
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

    fn snapshot(&self) -> (LiveUpdates, u64) {
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        (live.updates.clone(), live.replica.checkpoint())
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

struct State {
    saved: TeamSettings,
    problem: Option<String>,
    session: Option<Session>,
}

pub struct TeamCatalog {
    settings_file: PathBuf,
    /// From launch options; they fill in the sign-in form when nothing is
    /// saved, or replace what is saved.
    suggested: TeamSettings,
    state: Mutex<State>,
    /// Revisions never change, so what was fetched for previews is kept
    /// until the next sign-in or sign-out.
    known: Mutex<KnownRevisions>,
}

#[derive(Default)]
struct KnownRevisions {
    /// Part details by (IPN, revision number).
    parts: HashMap<(String, u32), PartDetails>,
    /// CAD item revisions by (CAD item, snapshot SHA-256).
    cad_items: HashMap<(String, String), StoredCadItemRevision>,
}

/// What one revision of a part uses for its symbol or footprint, for
/// drawing a preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeamCad {
    /// The revision assigns none.
    Unassigned,
    /// A library the team catalog does not store, such as KiCad's own, or
    /// a reference that did not resolve when it was imported. Draw it from
    /// the library on this computer.
    Library {
        reference: String,
        alternates: Vec<String>,
    },
    /// One exact CAD item revision, whose bytes the team catalog stores.
    Stored(StoredCad),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredCad {
    pub item: StoredItem,
    /// Other footprints the part lists (after `;`).
    pub alternates: Vec<String>,
    /// The CAD item revision number, as the part's details show it.
    pub number: u32,
    /// The ID of the blob record holding the bytes.
    blob: String,
}

/// Why [`TeamCatalog::cad_for_preview`] could not answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TeamCadError {
    SignedOut,
    /// The team catalog does not have that part or revision.
    NotFound(String),
    /// The team server could not be reached or refused the request.
    Unreachable(String),
}

impl TeamCatalog {
    /// Reads the saved server address and email from `settings_file`. A
    /// suggested server or email (from launch options) takes their place.
    pub fn open(
        settings_file: PathBuf,
        suggested_server: Option<String>,
        suggested_email: Option<String>,
    ) -> Self {
        let saved = fs::read(&settings_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            settings_file,
            suggested: TeamSettings {
                server_url: suggested_server.unwrap_or_default(),
                email: suggested_email.unwrap_or_default(),
            },
            state: Mutex::new(State {
                saved,
                problem: None,
                session: None,
            }),
            known: Mutex::new(KnownRevisions::default()),
        }
    }

    pub fn status(&self) -> TeamStatus {
        let state = self.lock();
        let pick = |suggested: &str, saved: &str| {
            if suggested.is_empty() {
                saved.to_owned()
            } else {
                suggested.to_owned()
            }
        };
        let session = match &state.session {
            Some(session) => {
                let (live, checkpoint) = session.follower.snapshot();
                TeamSession::SignedIn {
                    name: session.name.clone(),
                    email: session.email.clone(),
                    can_edit: session.can_edit,
                    live,
                    checkpoint,
                }
            }
            None => TeamSession::SignedOut {
                problem: state.problem.clone(),
            },
        };
        TeamStatus {
            server_url: pick(&self.suggested.server_url, &state.saved.server_url),
            email: pick(&self.suggested.email, &state.saved.email),
            session,
        }
    }

    /// Signs in and starts following changes. A failure is reported in the
    /// returned status, not as an error.
    pub fn sign_in(&self, request: SignInRequest) -> TeamStatus {
        let outcome = sign_in(&request);
        self.forget_known();
        let earlier = {
            let mut state = self.lock();
            let earlier = state.session.take();
            match outcome {
                Ok((session, server_url)) => {
                    state.problem = None;
                    state.saved = TeamSettings {
                        server_url,
                        email: session.email.clone(),
                    };
                    state.session = Some(session);
                    let _ = save_settings(&self.settings_file, &state.saved);
                }
                Err(problem) => state.problem = Some(problem),
            }
            earlier
        };
        // Ended outside the lock, so status polling never waits for it.
        drop(earlier);
        self.status()
    }

    pub fn sign_out(&self) -> TeamStatus {
        self.forget_known();
        let earlier = {
            let mut state = self.lock();
            state.problem = None;
            state.session.take()
        };
        drop(earlier);
        self.status()
    }

    /// The team catalog's current values for every part it has, by IPN, as
    /// the part header shows them (main MPN and manufacturer, description,
    /// value, symbol and footprint), for searching and listing parts. Uses
    /// only what the app already follows, so it makes no request; empty
    /// while signed out.
    pub fn current_values(&self) -> BTreeMap<String, PartValues> {
        let state = self.lock();
        let Some(session) = &state.session else {
            return BTreeMap::new();
        };
        let live = session
            .follower
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        live.replica
            .parts()
            .values()
            .filter_map(|part| part.details.as_ref())
            .map(|details| {
                let main = details.manufacturer_parts.first();
                let field = |name: &str| details.field(name).map(str::to_owned);
                let values = PartValues {
                    mpn: Some(main.map(|part| part.mpn.clone()).unwrap_or_default()),
                    manufacturer: Some(
                        main.map(|part| part.manufacturer.clone())
                            .unwrap_or_default(),
                    ),
                    description: field("Description"),
                    value: field("Value"),
                    symbol: field("Symbol"),
                    footprint: field("Footprint"),
                };
                (details.ipn.clone(), values)
            })
            .collect()
    }

    /// The team catalog's view of `ipn`, or `None` if the team catalog does
    /// not have it.
    pub fn part(&self, ipn: &str) -> Result<Option<TeamPart>, String> {
        let client = self.client()?;
        let Some(part) = part_by_ipn(&client, ipn).map_err(|error| error.to_string())? else {
            return Ok(None);
        };
        load_part(&client, &part).map(Some)
    }

    pub fn edit(&self, request: TeamEditRequest) -> Result<PublishResult, String> {
        let client = self.client()?;
        let command = EditCommand {
            correlation_id: attempt_id(request.correlation_id.as_deref())?,
            reason: request.reason.clone(),
            ipn: request.ipn.clone(),
            expected_revision: request.expected_revision,
            fields: request
                .fields
                .iter()
                .map(|field| DetailField {
                    name: field.name.clone(),
                    value: field.value.clone(),
                })
                .collect(),
            manufacturer_parts: request
                .manufacturer_parts
                .iter()
                .map(|part| catalog::ManufacturerPart {
                    manufacturer: part.manufacturer.clone(),
                    mpn: part.mpn.clone(),
                })
                .collect(),
        };
        let reply = submit_edit(&client, &command);
        self.published(&client, &request.ipn, reply, |current, part| {
            edit_conflict(&request, current, part)
        })
    }

    pub fn restore(&self, request: TeamRestoreRequest) -> Result<PublishResult, String> {
        let client = self.client()?;
        let command = RestoreCommand::new(
            &request.ipn,
            request.expected_revision,
            request.restore_revision,
            &request.reason,
        );
        let command = RestoreCommand {
            correlation_id: attempt_id(request.correlation_id.as_deref())?,
            ..command
        };
        let reply = submit_restore(&client, &command);
        self.published(&client, &request.ipn, reply, |current, part| {
            restore_conflict(&request, current, part)
        })
    }

    fn published(
        &self,
        client: &PocketBaseClient,
        ipn: &str,
        reply: Result<PublishReply, CollabError>,
        conflict: impl FnOnce(&ConflictingRevision, &LoadedPart) -> Result<TeamConflict, String>,
    ) -> Result<PublishResult, String> {
        let reload = || -> Result<LoadedPart, String> {
            let part = part_by_ipn(client, ipn)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{ipn} is no longer in the team catalog"))?;
            load(client, &part)
        };
        match reply {
            Ok(reply) => match reply.outcome {
                PublishOutcome::Applied { revision, .. } => Ok(PublishResult::Saved {
                    revision: revision.number,
                    part: reload()?.view,
                }),
                PublishOutcome::Noop { .. } => Ok(PublishResult::NothingToSave {
                    message: "Nothing changed, so nothing was saved.".into(),
                }),
                PublishOutcome::Conflict {
                    current_revision, ..
                } => Ok(PublishResult::Conflict(Box::new(conflict(
                    &current_revision,
                    &reload()?,
                )?))),
                PublishOutcome::CorrelationReused { message, .. } => {
                    Ok(PublishResult::Refused { message })
                }
            },
            Err(CollabError::Http {
                status: 400 | 404,
                message,
            }) => Ok(PublishResult::Refused { message }),
            Err(CollabError::Forbidden(message)) => Ok(PublishResult::Refused { message }),
            Err(CollabError::Unauthorized(_)) => Ok(PublishResult::Refused {
                message: "Your sign-in has ended. Sign in again, then save.".into(),
            }),
            Err(error) => Err(error.to_string()),
        }
    }

    /// What revision `part_revision` of `ipn` uses for its symbol or
    /// footprint. Uses the followed replica, or what was fetched before,
    /// when it can; otherwise asks the team server.
    pub fn cad_for_preview(
        &self,
        ipn: &str,
        part_revision: u32,
        kind: PreviewKind,
    ) -> Result<TeamCad, TeamCadError> {
        let client = self.client().map_err(|_| TeamCadError::SignedOut)?;
        let details = self.part_details(&client, ipn, part_revision)?;
        let (field, role) = match kind {
            PreviewKind::Symbol => ("Symbol", CadReferenceRole::Symbol),
            PreviewKind::Footprint => ("Footprint", CadReferenceRole::Footprint),
        };
        let field = details.field(field).unwrap_or_default().trim();
        // Footprint fields may list `;`-separated alternatives; symbol
        // fields hold exactly one reference.
        let choices = match kind {
            _ if field.is_empty() => Vec::new(),
            PreviewKind::Symbol => vec![field],
            PreviewKind::Footprint => footprint_choices(field),
        };
        let Some((&reference, alternates)) = choices.split_first() else {
            return Ok(TeamCad::Unassigned);
        };
        let alternates: Vec<String> = alternates
            .iter()
            .map(|choice| (*choice).to_owned())
            .collect();
        let target = details
            .cad_references
            .iter()
            .find(|cad| cad.role == role && cad.reference == reference)
            .map(|cad| &cad.target);
        let Some(CadReferenceTarget::CadItemRevision {
            cad_item,
            snapshot_sha256,
        }) = target
        else {
            return Ok(TeamCad::Library {
                reference: reference.to_owned(),
                alternates,
            });
        };
        let revision = self.cad_item_revision(&client, cad_item, snapshot_sha256)?;
        let snapshot: CadItemDetails =
            serde_json::from_str(&revision.snapshot).map_err(|error| {
                TeamCadError::Unreachable(format!(
                    "the stored revision of {reference} is unreadable: {error}"
                ))
            })?;
        let lib_ref =
            LibRef::parse(reference).map_err(|error| TeamCadError::NotFound(error.to_string()))?;
        Ok(TeamCad::Stored(StoredCad {
            item: StoredItem {
                lib_ref,
                blob_sha256: snapshot.blob_sha256,
                format_version: snapshot.format_version,
            },
            alternates,
            number: revision.number,
            blob: revision.blob,
        }))
    }

    /// Downloads a CAD item revision's stored bytes. The preview cache
    /// checks them against their SHA-256.
    pub fn stored_bytes(&self, cad: &StoredCad) -> Result<Vec<u8>, String> {
        let client = self.client()?;
        let fail = |error: CollabError| format!("the team server did not send them: {error}");
        let blob: StoredBlob = client
            .get(&format!(
                "/api/collections/catalog_blobs/records/{}",
                cad.blob
            ))
            .and_then(|reply| reply.json())
            .map_err(fail)?;
        let token = client.file_token().map_err(fail)?;
        client
            .download_file("catalog_blobs", &blob.id, &blob.file, Some(&token))
            .map_err(fail)
    }

    fn part_details(
        &self,
        client: &PocketBaseClient,
        ipn: &str,
        number: u32,
    ) -> Result<PartDetails, TeamCadError> {
        let key = (ipn.to_owned(), number);
        if let Some(details) = self.lock_known().parts.get(&key) {
            return Ok(details.clone());
        }
        let followed = self.lock().session.as_ref().and_then(|session| {
            let live = session
                .follower
                .live
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            live.replica
                .part_by_ipn(ipn)
                .filter(|part| part.number == number)
                .and_then(|part| part.details.clone())
        });
        let details = match followed {
            Some(details) => details,
            None => fetch_part_details(client, ipn, number)?,
        };
        self.lock_known().parts.insert(key, details.clone());
        Ok(details)
    }

    fn cad_item_revision(
        &self,
        client: &PocketBaseClient,
        cad_item: &str,
        snapshot_sha256: &str,
    ) -> Result<StoredCadItemRevision, TeamCadError> {
        let key = (cad_item.to_owned(), snapshot_sha256.to_owned());
        if let Some(revision) = self.lock_known().cad_items.get(&key) {
            return Ok(revision.clone());
        }
        let filter = format!(
            "cad_item = {} && snapshot_sha256 = {}",
            serde_json::Value::from(cad_item),
            serde_json::Value::from(snapshot_sha256)
        );
        let revision = client
            .list_records::<StoredCadItemRevision>("catalog_cad_item_revisions", &filter, "number")
            .map_err(unreachable)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                TeamCadError::NotFound(
                    "the team catalog does not have that CAD item revision".into(),
                )
            })?;
        self.lock_known().cad_items.insert(key, revision.clone());
        Ok(revision)
    }

    fn forget_known(&self) {
        *self.lock_known() = KnownRevisions::default();
    }

    fn lock_known(&self) -> std::sync::MutexGuard<'_, KnownRevisions> {
        self.known.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn client(&self) -> Result<PocketBaseClient, String> {
        self.lock()
            .session
            .as_ref()
            .map(|session| session.client.clone())
            .ok_or_else(|| "Sign in to the team catalog first.".to_owned())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Revision `number` of `ipn`'s details, from the team server.
fn fetch_part_details(
    client: &PocketBaseClient,
    ipn: &str,
    number: u32,
) -> Result<PartDetails, TeamCadError> {
    let part = part_by_ipn(client, ipn)
        .map_err(unreachable)?
        .ok_or_else(|| TeamCadError::NotFound(format!("{ipn} is not in the team catalog")))?;
    let filter = format!(
        "part = {} && number = {number}",
        serde_json::Value::from(part.id.as_str())
    );
    let revision = client
        .list_records::<StoredPartRevision>("catalog_part_revisions", &filter, "number")
        .map_err(unreachable)?
        .into_iter()
        .next()
        .ok_or_else(|| TeamCadError::NotFound(format!("{ipn} has no revision {number}")))?;
    serde_json::from_str(&revision.snapshot).map_err(|error| {
        TeamCadError::Unreachable(format!("revision {number} of {ipn} is unreadable: {error}"))
    })
}

fn unreachable(error: CollabError) -> TeamCadError {
    TeamCadError::Unreachable(error.to_string())
}

/// The correlation ID the UI made for this save attempt, so that retrying
/// the same save replays its answer; a new one if the UI sent none.
fn attempt_id(sent: Option<&str>) -> Result<CorrelationId, String> {
    match sent {
        Some(id) => CorrelationId::new(id).map_err(|error| error.to_string()),
        None => Ok(CorrelationId::generate()),
    }
}

/// Checks the address, signs in, reads the account, and starts following.
fn sign_in(request: &SignInRequest) -> Result<(Session, String), String> {
    let server_url = loopback_url(&request.server_url)?;
    let email = request.email.trim();
    if email.is_empty() || request.password.is_empty() {
        return Err("Enter your email and password.".into());
    }
    let client = PocketBaseClient::new(&server_url)
        .authenticate("users", email, &request.password)
        .map_err(|error| match error {
            CollabError::Transport(_) => format!("Cannot reach a team server at {server_url}."),
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
    let text = |name: &str| account[name].as_str().unwrap_or_default().to_owned();
    let name = Some(text("name"))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| text("email"));
    let session = Session {
        follower: Follower::start(client.clone()),
        client,
        name,
        email: text("email"),
        can_edit: account["can_edit_catalog"].as_bool().unwrap_or(false),
    };
    Ok((session, server_url))
}

/// `http://` addresses on this computer only. Realtime updates need plain
/// HTTP for now, and a password must not cross a network unencrypted.
fn loopback_url(text: &str) -> Result<String, String> {
    let url = text.trim().trim_end_matches('/');
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        "Enter the team server address, starting with http://, such as http://127.0.0.1:8090."
            .to_owned()
    })?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    if !matches!(name, "127.0.0.1" | "localhost" | "[::1]") || rest.len() != host.len() {
        return Err(
            "For now this app only connects to a team server on this computer, such as http://127.0.0.1:8090."
                .into(),
        );
    }
    Ok(url.to_owned())
}

/// Follows new Changes until `stop` is set: subscribe, catch up from the
/// checkpoint, apply each notification; on any break, wait and do it all
/// again, waiting twice as long after each failed try (up to
/// [`MAX_RECONNECT_DELAY`]). Catching up after subscribing means a Change
/// published in between arrives twice at most, and the replica ignores the
/// copy. If the server rejects the sign-in, it stops and asks the person to
/// sign in again instead of retrying.
fn follow_changes(client: &PocketBaseClient, live: &Mutex<Live>, stop: &AtomicBool) {
    let set = |updates: LiveUpdates| {
        live.lock().unwrap_or_else(|e| e.into_inner()).updates = updates;
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
        let result = (|| -> Result<(), CollabError> {
            let mut connection = RealtimeConnection::connect(client.base_url(), CONNECT_TIMEOUT)?;
            connection.subscribe(client, &[CHANGES_TOPIC])?;
            update_replica(&|replica| replica.catch_up(client).map(|_| ()))?;
            set(LiveUpdates::Live);
            went_live.set(true);
            while !stop.load(Ordering::SeqCst) {
                let Some(event) = connection.next_event(POLL)? else {
                    continue;
                };
                if event.topic != CHANGES_TOPIC || event.data["action"] != "create" {
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
        }
        if let Err(error) = result {
            match after_break(&error) {
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
    Retry,
    /// The token was rejected; retrying cannot help.
    SignInAgain,
}

fn after_break(error: &CollabError) -> AfterBreak {
    match error {
        CollabError::Unauthorized(_) | CollabError::Forbidden(_) => AfterBreak::SignInAgain,
        _ => AfterBreak::Retry,
    }
}

fn next_delay(delay: Duration) -> Duration {
    (delay * 2).min(MAX_RECONNECT_DELAY)
}

/// A part's revisions as the app shows them, with what it needs to build a
/// conflict.
struct LoadedPart {
    view: TeamPart,
    /// By revision number.
    details: BTreeMap<u32, PartDetails>,
}

fn load_part(client: &PocketBaseClient, part: &StoredPart) -> Result<TeamPart, String> {
    load(client, part).map(|loaded| loaded.view)
}

fn load(client: &PocketBaseClient, part: &StoredPart) -> Result<LoadedPart, String> {
    let fail = |error: CollabError| error.to_string();
    let revisions = part_revisions(client, &part.id).map_err(fail)?;
    let change_ids: Vec<&str> = revisions.iter().map(|r| r.change.as_str()).collect();
    let change_records: BTreeMap<String, StoredChange> = changes_by_id(client, &change_ids)
        .map_err(fail)?
        .into_iter()
        .map(|change| (change.id.clone(), change))
        .collect();
    let mut cad_ids: Vec<&str> = revisions
        .iter()
        .flat_map(|revision| revision.cad_item_revisions.iter().map(String::as_str))
        .collect();
    cad_ids.sort_unstable();
    cad_ids.dedup();
    // CAD item revision numbers by (CAD item, snapshot hash).
    let cad_numbers: BTreeMap<(String, String), u32> = cad_item_revisions_by_id(client, &cad_ids)
        .map_err(fail)?
        .into_iter()
        .map(|revision| {
            (
                (revision.cad_item, revision.snapshot_sha256),
                revision.number,
            )
        })
        .collect();
    let binding = |reference: &CadReference| match &reference.target {
        CadReferenceTarget::CadItemRevision {
            cad_item,
            snapshot_sha256,
        } => match cad_numbers.get(&(cad_item.clone(), snapshot_sha256.clone())) {
            Some(number) => format!("revision {number}"),
            None => format!(
                "exact revision {}",
                &snapshot_sha256[..8.min(snapshot_sha256.len())]
            ),
        },
        CadReferenceTarget::External => "KiCad library".to_owned(),
        CadReferenceTarget::Unresolved { status } => format!("not found ({status})"),
    };

    let mut details = BTreeMap::new();
    for revision in &revisions {
        let parsed: PartDetails = serde_json::from_str(&revision.snapshot)
            .map_err(|error| format!("revision {} of {}: {error}", revision.number, part.ipn))?;
        details.insert(revision.number, parsed);
    }
    let current_number = part.current_revision_number;
    let current = details
        .get(&current_number)
        .ok_or_else(|| format!("{} has no revision {current_number}", part.ipn))?;
    let describe =
        |reference: &CadReference| format!("{}, {}", reference.reference, binding(reference));
    let to_changes = |before: &PartDetails, after: &PartDetails| -> Vec<DetailChange> {
        compare_details(before, after, describe)
            .into_iter()
            .map(|difference| DetailChange {
                label: difference.label,
                before: difference.before,
                after: difference.after,
            })
            .collect()
    };
    let history = revisions
        .iter()
        .rev()
        .map(|revision| {
            let this = &details[&revision.number];
            let changes = match revision.number.checked_sub(1).and_then(|n| details.get(&n)) {
                Some(previous) => to_changes(previous, this),
                None => Vec::new(),
            };
            let restore_changes = if revision.number == current_number {
                Vec::new()
            } else {
                to_changes(current, this)
            };
            summary(
                revision,
                change_records.get(&revision.change),
                changes,
                restore_changes,
            )
        })
        .collect();
    let view = TeamPart {
        ipn: part.ipn.clone(),
        current: revision_details(current_number, current, &binding),
        history,
    };
    Ok(LoadedPart { view, details })
}

fn revision_details(
    number: u32,
    details: &PartDetails,
    binding: &dyn Fn(&CadReference) -> String,
) -> RevisionDetails {
    RevisionDetails {
        number,
        fields: details
            .fields
            .iter()
            .map(|field| Field {
                name: field.name.clone(),
                value: field.value.clone(),
            })
            .collect(),
        manufacturer_parts: details
            .manufacturer_parts
            .iter()
            .map(|part| ManufacturerPart {
                manufacturer: part.manufacturer.clone(),
                mpn: part.mpn.clone(),
            })
            .collect(),
        cad_items: details
            .cad_references
            .iter()
            .map(|reference| CadItemUse {
                role: match reference.role {
                    CadReferenceRole::Symbol => "Symbol".to_owned(),
                    CadReferenceRole::Footprint => "Footprint".to_owned(),
                },
                reference: reference.reference.clone(),
                binding: binding(reference),
            })
            .collect(),
    }
}

fn summary(
    revision: &StoredPartRevision,
    change: Option<&StoredChange>,
    changes: Vec<DetailChange>,
    restore_changes: Vec<DetailChange>,
) -> RevisionSummary {
    let kind = change
        .map(|change| change.kind.as_str())
        .unwrap_or_default();
    let restored_from = serde_json::from_str::<serde_json::Value>(&revision.source)
        .ok()
        .and_then(|source| source["restored_revision"]["number"].as_u64())
        .and_then(|number| u32::try_from(number).ok());
    RevisionSummary {
        number: revision.number,
        action: action(kind).to_owned(),
        actor_name: change.map_or_else(|| "someone".to_owned(), actor_name),
        saved_at: change
            .map(|change| saved_at(&change.created))
            .unwrap_or_default(),
        reason: change
            .map(|change| change.reason.clone())
            .unwrap_or_default(),
        restored_from,
        changes,
        restore_changes,
    }
}

/// Who made a Change, in words a team member understands. Administrator
/// accounts (such as the one that runs imports) are not team members, so
/// their email is not shown.
fn actor_name(change: &StoredChange) -> String {
    if change.actor.starts_with("_superusers:") {
        "an administrator".to_owned()
    } else if change.actor_name.is_empty() {
        "someone".to_owned()
    } else {
        change.actor_name.clone()
    }
}

/// The plain word for a Change kind.
fn action(kind: &str) -> &'static str {
    match kind {
        "csv-import" => "Imported",
        "edit" => "Edited",
        "restore" => "Restored",
        _ => "Saved",
    }
}

/// PocketBase's `2026-10-09 23:39:58.997Z` as `2026-10-09 23:39 UTC`.
fn saved_at(created: &str) -> String {
    match created.get(..16) {
        Some(minutes) if created.ends_with('Z') => format!("{minutes} UTC"),
        _ => created.to_owned(),
    }
}

/// One line per manufacturer part, such as `Yageo RC0603FR-0747RL`.
fn manufacturer_lines<'a>(parts: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    parts
        .map(|(manufacturer, mpn)| format!("{manufacturer} {mpn}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn stored_lines(details: &PartDetails) -> String {
    manufacturer_lines(
        details
            .manufacturer_parts
            .iter()
            .map(|part| (part.manufacturer.as_str(), part.mpn.as_str())),
    )
}

fn manufacturer_parts_of(details: &PartDetails) -> Vec<ManufacturerPart> {
    details
        .manufacturer_parts
        .iter()
        .map(|part| ManufacturerPart {
            manufacturer: part.manufacturer.clone(),
            mpn: part.mpn.clone(),
        })
        .collect()
}

fn fields_of(details: &PartDetails) -> Vec<Field> {
    details
        .fields
        .iter()
        .map(|field| Field {
            name: field.name.clone(),
            value: field.value.clone(),
        })
        .collect()
}

/// Every field or list that the edit or the other person changed since the
/// revision the edit started from, and their revision with only the edit
/// changes applied on top.
fn edit_conflict(
    request: &TeamEditRequest,
    current: &ConflictingRevision,
    part: &LoadedPart,
) -> Result<TeamConflict, String> {
    let base = part
        .details
        .get(&request.expected_revision)
        .ok_or_else(|| format!("revision {} is missing", request.expected_revision))?;
    let theirs = current.details().map_err(|error| error.to_string())?;
    let yours = |name: &str| {
        request
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
    };
    let mut rows = Vec::new();
    let mut merged_fields = Vec::new();
    for field in &theirs.fields {
        let started = base.field(&field.name).unwrap_or_default().to_owned();
        let mine = yours(&field.name).unwrap_or_else(|| started.clone());
        let you_changed = mine != started;
        if you_changed || field.value != started {
            rows.push(ConflictRow {
                label: field.name.clone(),
                started_from: started.clone(),
                theirs: field.value.clone(),
                yours: mine.clone(),
                both_changed: you_changed && field.value != started && field.value != mine,
                you_changed,
            });
        }
        merged_fields.push(Field {
            name: field.name.clone(),
            value: if you_changed {
                mine
            } else {
                field.value.clone()
            },
        });
    }
    let started_parts = stored_lines(base);
    let their_parts = stored_lines(&theirs);
    let your_parts = manufacturer_lines(
        request
            .manufacturer_parts
            .iter()
            .map(|part| (part.manufacturer.as_str(), part.mpn.as_str())),
    );
    let you_changed_parts = your_parts != started_parts;
    if you_changed_parts || their_parts != started_parts {
        rows.push(ConflictRow {
            label: "Manufacturer parts".into(),
            started_from: started_parts.clone(),
            theirs: their_parts.clone(),
            yours: your_parts.clone(),
            both_changed: you_changed_parts
                && their_parts != started_parts
                && their_parts != your_parts,
            you_changed: you_changed_parts,
        });
    }
    let merged = TeamEditRequest {
        correlation_id: None,
        ipn: request.ipn.clone(),
        expected_revision: current.number,
        reason: request.reason.clone(),
        fields: merged_fields,
        manufacturer_parts: if you_changed_parts {
            request.manufacturer_parts.clone()
        } else {
            manufacturer_parts_of(&theirs)
        },
    };
    Ok(TeamConflict {
        started_from: request.expected_revision,
        theirs: their_summary(current, part),
        rows,
        merged,
        restore_revision: None,
        part: part.view.clone(),
    })
}

/// A restore that someone else beat: what the restore would bring back
/// next to what they saved.
fn restore_conflict(
    request: &TeamRestoreRequest,
    current: &ConflictingRevision,
    part: &LoadedPart,
) -> Result<TeamConflict, String> {
    let missing = |number: u32| format!("revision {number} is missing");
    let base = part
        .details
        .get(&request.expected_revision)
        .ok_or_else(|| missing(request.expected_revision))?;
    let restored = part
        .details
        .get(&request.restore_revision)
        .ok_or_else(|| missing(request.restore_revision))?;
    let theirs = current.details().map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for field in &theirs.fields {
        let started = base.field(&field.name).unwrap_or_default();
        let mine = restored.field(&field.name).unwrap_or_default();
        if mine != started || field.value != started {
            rows.push(ConflictRow {
                label: field.name.clone(),
                started_from: started.to_owned(),
                theirs: field.value.clone(),
                yours: mine.to_owned(),
                both_changed: mine != started && field.value != started && field.value != mine,
                you_changed: mine != started,
            });
        }
    }
    let merged = TeamEditRequest {
        correlation_id: None,
        ipn: request.ipn.clone(),
        expected_revision: current.number,
        reason: request.reason.clone(),
        fields: fields_of(&theirs),
        manufacturer_parts: manufacturer_parts_of(&theirs),
    };
    Ok(TeamConflict {
        started_from: request.expected_revision,
        theirs: their_summary(current, part),
        rows,
        merged,
        restore_revision: Some(request.restore_revision),
        part: part.view.clone(),
    })
}

/// Their revision from the part's history, or from the conflict answer if
/// the history is behind.
fn their_summary(current: &ConflictingRevision, part: &LoadedPart) -> RevisionSummary {
    part.view
        .history
        .iter()
        .find(|revision| revision.number == current.number)
        .cloned()
        .unwrap_or_else(|| RevisionSummary {
            number: current.number,
            action: action(&current.change_kind).to_owned(),
            actor_name: if current.actor.starts_with("_superusers:") {
                "an administrator".to_owned()
            } else {
                current.actor_name.clone()
            },
            saved_at: saved_at(&current.created),
            reason: current.reason.clone(),
            restored_from: None,
            changes: Vec::new(),
            restore_changes: Vec::new(),
        })
}

fn save_settings(path: &Path, settings: &TeamSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, json).map_err(|e| e.to_string())?;
    fs::rename(&temp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_http_addresses_are_accepted() {
        assert_eq!(
            loopback_url(" http://127.0.0.1:8090/ ").unwrap(),
            "http://127.0.0.1:8090"
        );
        assert!(loopback_url("http://localhost:8090").is_ok());
        assert!(loopback_url("http://[::1]:8090").is_ok());
        for refused in [
            "https://127.0.0.1:8090",
            "http://team.example.com:8090",
            "http://127.0.0.1.example.com",
            "http://127.0.0.1:8090/api",
            "127.0.0.1:8090",
            "",
        ] {
            assert!(loopback_url(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_rejected_sign_in_stops_retrying_and_other_breaks_back_off() {
        for rejected in [
            CollabError::Unauthorized("token".into()),
            CollabError::Forbidden("rules".into()),
        ] {
            assert_eq!(after_break(&rejected), AfterBreak::SignInAgain);
        }
        assert_eq!(
            after_break(&CollabError::Transport("refused".into())),
            AfterBreak::Retry
        );
        let mut delay = RECONNECT_DELAY;
        let mut delays = Vec::new();
        for _ in 0..7 {
            delays.push(delay.as_secs());
            delay = next_delay(delay);
        }
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30]);
    }
    #[test]
    fn a_sent_correlation_id_is_kept_and_checked() {
        assert_eq!(
            attempt_id(Some("desktop-abc")).unwrap().as_str(),
            "desktop-abc"
        );
        assert!(attempt_id(Some("has space")).is_err());
        assert!(attempt_id(None).unwrap().as_str().starts_with("cmd-"));
    }
    #[test]
    fn dates_are_shown_to_the_minute() {
        assert_eq!(saved_at("2026-10-09 23:39:58.997Z"), "2026-10-09 23:39 UTC");
        assert_eq!(saved_at(""), "");
    }

    #[test]
    fn settings_never_hold_a_password() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.json");
        let team = TeamCatalog::open(file.clone(), None, None);
        let status = team.sign_in(SignInRequest {
            server_url: "http://127.0.0.1:9".into(),
            email: "a@example.test".into(),
            password: "secret-password".into(),
        });
        assert!(matches!(
            status.session,
            TeamSession::SignedOut { problem: Some(_) }
        ));
        // A failed sign-in saves nothing at all.
        assert!(!file.exists());
        save_settings(
            &file,
            &TeamSettings {
                server_url: "http://127.0.0.1:9".into(),
                email: "a@example.test".into(),
            },
        )
        .unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("password") && !text.contains("token"),
            "{text}"
        );
        let reopened = TeamCatalog::open(file, Some("http://127.0.0.1:1".into()), None);
        let status = reopened.status();
        assert_eq!(
            (status.server_url.as_str(), status.email.as_str()),
            ("http://127.0.0.1:1", "a@example.test")
        );
    }
}

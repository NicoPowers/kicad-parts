//! The team catalog use cases behind the desktop: sign in to the team's
//! PocketBase server, read one part's details and history, publish edits,
//! and follow changes others make.
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
    ConflictingRecord, EditCommand, PublishOutcome, PublishReply, StoredBlob,
    StoredCadItemRevision, StoredChange, StoredPart, StoredPartRevision, changes_by_id,
    part_by_ipn, part_details_of, part_revisions, submit_edit,
};
use catalog::replica::CatalogReplica;
use catalog::{
    CadItemDetails, CadItemId, CadItemKind, CadItemRevisionId, CadReference, CadReferenceRole,
    CadReferenceTarget, DetailField, PartDetails, PartRevision, compare_details, compare_revisions,
};
use collaboration::{CollabError, CorrelationId, PocketBaseClient, RealtimeConnection};
use desktop_contract::{
    CadItemUse, ConflictRow, CurrentDetails, CurrentRevision, DetailChange, Field, HistoryEntry,
    LiveUpdates, ManufacturerPart, PreviewKind, PublishResult, SignInRequest, TeamConflict,
    TeamEditRequest, TeamPart, TeamSession, TeamStatus,
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
    /// Part revisions by (IPN, revision number).
    parts: HashMap<(String, u32), PartRevision>,
    /// CAD item revisions by (CAD item, revision number).
    cad_items: HashMap<(String, u32), StoredCadItemRevision>,
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
    /// The CAD item revision record the server sent is not the one the
    /// part's details name. `why` finishes the sentence "... because ...".
    Mismatch {
        reference: String,
        why: String,
    },
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
            .filter_map(|part| Some((part.details.as_ref()?, part.revision.as_ref()?)))
            .map(|(details, revision)| {
                let main = details.manufacturer_parts.first();
                let field = |name: &str| details.field(name).map(str::to_owned);
                let cad = |name: &str| revision.field(name).map(str::to_owned);
                let values = PartValues {
                    mpn: Some(main.map(|part| part.mpn.clone()).unwrap_or_default()),
                    manufacturer: Some(
                        main.map(|part| part.manufacturer.clone())
                            .unwrap_or_default(),
                    ),
                    description: field("Description"),
                    value: field("Value"),
                    symbol: cad("Symbol"),
                    footprint: cad("Footprint"),
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

    /// Saves what the edit changed: the details when any details field or
    /// manufacturer part differs from the details it started from, and the
    /// Symbol and Footprint when they differ from the revision it started
    /// from. Each is checked only against its own starting number.
    pub fn edit(&self, request: TeamEditRequest) -> Result<PublishResult, String> {
        let client = self.client()?;
        let part = part_by_ipn(&client, &request.ipn)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{} is no longer in the team catalog", request.ipn))?;
        let started = load(&client, &part)?;
        let base_details = started
            .details
            .get(&request.expected_details)
            .ok_or_else(|| format!("details {} are missing", request.expected_details))?;
        let base_revision = started
            .revisions
            .get(&request.expected_revision)
            .ok_or_else(|| format!("revision {} is missing", request.expected_revision))?;
        let fields = detail_fields(&request.fields);
        let parts = catalog_parts(&request.manufacturer_parts);
        let cad_fields = detail_fields(&request.cad_fields);
        let mut command = EditCommand::new(&request.ipn, &request.reason);
        command.correlation_id = attempt_id(request.correlation_id.as_deref())?;
        if fields != base_details.fields || parts != base_details.manufacturer_parts {
            command = command.editing_details(base_details, request.expected_details);
            if let Some(details) = &mut command.details {
                details.fields = fields;
                details.manufacturer_parts = parts;
            }
        }
        if cad_fields != base_revision.cad_fields {
            command = command.editing_cad(base_revision, request.expected_revision);
            if let Some(cad) = &mut command.cad {
                cad.fields = cad_fields;
            }
        }
        if command.details.is_none() && command.cad.is_none() {
            return Ok(PublishResult::NothingToSave {
                message: "Nothing changed, so nothing was saved.".into(),
            });
        }
        let reply = submit_edit(&client, &command);
        self.published(&client, &request.ipn, reply, |details, revision, part| {
            edit_conflict(&request, details, revision, part)
        })
    }

    fn published(
        &self,
        client: &PocketBaseClient,
        ipn: &str,
        reply: Result<PublishReply, CollabError>,
        conflict: impl FnOnce(
            Option<&ConflictingRecord>,
            Option<&ConflictingRecord>,
            &LoadedPart,
        ) -> Result<TeamConflict, String>,
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
                    revision: revision.map(|revision| revision.number),
                    part: reload()?.view,
                }),
                PublishOutcome::Noop { .. } => Ok(PublishResult::NothingToSave {
                    message: "Nothing changed, so nothing was saved.".into(),
                }),
                PublishOutcome::Conflict {
                    current_details,
                    current_revision,
                    ..
                } => Ok(PublishResult::Conflict(Box::new(conflict(
                    current_details.as_deref(),
                    current_revision.as_deref(),
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
        let details = self.part_revision(&client, ipn, part_revision)?;
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
            number,
            snapshot_sha256,
        }) = target
        else {
            return Ok(TeamCad::Library {
                reference: reference.to_owned(),
                alternates,
            });
        };
        let revision = self.cad_item_revision(&client, cad_item, *number)?;
        let named = NamedRevision {
            cad_item,
            number: *number,
            snapshot_sha256,
        };
        let snapshot = checked_snapshot(&revision, &named, reference, kind).map_err(|why| {
            TeamCadError::Mismatch {
                reference: reference.to_owned(),
                why,
            }
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
        let fail = |error: CollabError| plain_reason(&error);
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

    fn part_revision(
        &self,
        client: &PocketBaseClient,
        ipn: &str,
        number: u32,
    ) -> Result<PartRevision, TeamCadError> {
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
                .and_then(|part| part.revision.clone())
        });
        let details = match followed {
            Some(details) => details,
            None => fetch_part_revision(client, ipn, number)?,
        };
        self.lock_known().parts.insert(key, details.clone());
        Ok(details)
    }

    /// Revision `number` of `cad_item`, found by the ID those two give.
    fn cad_item_revision(
        &self,
        client: &PocketBaseClient,
        cad_item: &str,
        number: u32,
    ) -> Result<StoredCadItemRevision, TeamCadError> {
        let key = (cad_item.to_owned(), number);
        if let Some(revision) = self.lock_known().cad_items.get(&key) {
            return Ok(revision.clone());
        }
        let id = CadItemRevisionId::new(&CadItemId::from_stored(cad_item), number);
        let filter = format!("id = {}", serde_json::Value::from(id.as_str()));
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

/// The CAD item revision a part's details name.
struct NamedRevision<'a> {
    cad_item: &'a str,
    number: u32,
    snapshot_sha256: &'a str,
}

/// The details of a CAD item revision record, once it is shown to be the
/// revision a part's details name: its snapshot hashes to the named
/// SHA-256, it is the named revision of the named CAD item, and it is the
/// right kind of item, in the format the preview reads, for `reference`.
/// Otherwise says why not, finishing the sentence "... because ...".
fn checked_snapshot(
    revision: &StoredCadItemRevision,
    named: &NamedRevision<'_>,
    reference: &str,
    kind: PreviewKind,
) -> Result<CadItemDetails, String> {
    if catalog::sha256_hex(revision.snapshot.as_bytes()) != named.snapshot_sha256 {
        return Err("its record does not match its SHA-256".into());
    }
    if revision.cad_item != named.cad_item {
        return Err("its record belongs to another CAD item".into());
    }
    if revision.number != named.number {
        return Err("its record is another revision of that CAD item".into());
    }
    let snapshot: CadItemDetails = serde_json::from_str(&revision.snapshot)
        .map_err(|_| "its record cannot be read".to_owned())?;
    let (expected_kind, expected_format) = match kind {
        PreviewKind::Symbol => (CadItemKind::Symbol, "kicad-symbol-entry"),
        PreviewKind::Footprint => (CadItemKind::Footprint, "kicad-footprint"),
    };
    if snapshot.kind != expected_kind || snapshot.format != expected_format {
        return Err(format!(
            "its record is a {} in the {} format, not a {}",
            snapshot.kind.as_str(),
            snapshot.format,
            expected_kind.as_str()
        ));
    }
    if snapshot.key != reference {
        return Err("its record is for another reference".into());
    }
    Ok(snapshot)
}

/// Revision `number` of `ipn`, from the team server.
fn fetch_part_revision(
    client: &PocketBaseClient,
    ipn: &str,
    number: u32,
) -> Result<PartRevision, TeamCadError> {
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
    TeamCadError::Unreachable(plain_reason(&error))
}

/// Why a request to the team server failed, in words that finish the
/// sentence "... because ...".
fn plain_reason(error: &CollabError) -> String {
    match error {
        CollabError::Transport(_) => "the team server could not be reached".into(),
        CollabError::Unauthorized(_) | CollabError::Forbidden(_) => {
            "the team server no longer accepts your sign-in".into()
        }
        other => format!("the team server answered: {other}"),
    }
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

/// A part as the app shows it, with every saved details and revision, which
/// edits and conflicts compare with.
struct LoadedPart {
    view: TeamPart,
    /// By number.
    details: BTreeMap<u32, PartDetails>,
    /// By number.
    revisions: BTreeMap<u32, PartRevision>,
}

fn load_part(client: &PocketBaseClient, part: &StoredPart) -> Result<TeamPart, String> {
    load(client, part).map(|loaded| loaded.view)
}

/// How a CAD reference is bound, in plain words, naming the CAD item's own
/// revision (`symbol revision 1`) so it is never mistaken for the part's.
fn binding(reference: &CadReference) -> String {
    let role = match reference.role {
        CadReferenceRole::Symbol => "symbol",
        CadReferenceRole::Footprint => "footprint",
    };
    match &reference.target {
        CadReferenceTarget::CadItemRevision { number, .. } => format!("{role} revision {number}"),
        CadReferenceTarget::External => "KiCad library".to_owned(),
        CadReferenceTarget::Unresolved { status } => format!("not found ({status})"),
    }
}

fn load(client: &PocketBaseClient, part: &StoredPart) -> Result<LoadedPart, String> {
    let fail = |error: CollabError| error.to_string();
    let saved_details = part_details_of(client, &part.id).map_err(fail)?;
    let revisions = part_revisions(client, &part.id).map_err(fail)?;
    let change_ids: Vec<&str> = saved_details
        .iter()
        .map(|d| d.change.as_str())
        .chain(revisions.iter().map(|r| r.change.as_str()))
        .collect();
    let change_records: BTreeMap<String, StoredChange> = changes_by_id(client, &change_ids)
        .map_err(fail)?
        .into_iter()
        .map(|change| (change.id.clone(), change))
        .collect();

    let mut details = BTreeMap::new();
    for saved in &saved_details {
        let parsed: PartDetails = serde_json::from_str(&saved.snapshot)
            .map_err(|error| format!("details {} of {}: {error}", saved.number, part.ipn))?;
        details.insert(saved.number, parsed);
    }
    let mut cad = BTreeMap::new();
    for revision in &revisions {
        let parsed: PartRevision = serde_json::from_str(&revision.snapshot)
            .map_err(|error| format!("revision {} of {}: {error}", revision.number, part.ipn))?;
        cad.insert(revision.number, parsed);
    }
    let current_details = details.get(&part.current_details_number).ok_or_else(|| {
        format!(
            "{} has no details {}",
            part.ipn, part.current_details_number
        )
    })?;
    let current_revision = cad.get(&part.current_revision_number).ok_or_else(|| {
        format!(
            "{} has no revision {}",
            part.ipn, part.current_revision_number
        )
    })?;
    let describe =
        |reference: &CadReference| format!("{}, {}", reference.reference, binding(reference));

    // One history entry per Change, newest first.
    let mut entries: BTreeMap<u64, HistoryEntry> = BTreeMap::new();
    for saved in &saved_details {
        let previous = saved.number.checked_sub(1).and_then(|n| details.get(&n));
        let changes = previous
            .map(|previous| to_changes(compare_details(previous, &details[&saved.number])))
            .unwrap_or_default();
        if let Some(change) = change_records.get(&saved.change) {
            let entry = history_entry(&mut entries, change);
            entry.details = Some(saved.number);
            // Details rows come first.
            let mut rows = changes;
            rows.append(&mut entry.changes);
            entry.changes = rows;
        }
    }
    for revision in &revisions {
        let previous = revision.number.checked_sub(1).and_then(|n| cad.get(&n));
        let changes = previous
            .map(|previous| {
                to_changes(compare_revisions(
                    previous,
                    &cad[&revision.number],
                    describe,
                ))
            })
            .unwrap_or_default();
        if let Some(change) = change_records.get(&revision.change) {
            let entry = history_entry(&mut entries, change);
            entry.revision = Some(revision.number);
            entry.changes.extend(changes);
        }
    }
    let view = TeamPart {
        ipn: part.ipn.clone(),
        details: current_details_view(part.current_details_number, current_details),
        revision: current_revision_view(part.current_revision_number, current_revision),
        history: entries.into_values().rev().collect(),
    };
    Ok(LoadedPart {
        view,
        details,
        revisions: cad,
    })
}

/// The history entry of `change`, made when first needed.
fn history_entry<'a>(
    entries: &'a mut BTreeMap<u64, HistoryEntry>,
    change: &StoredChange,
) -> &'a mut HistoryEntry {
    entries.entry(change.seq).or_insert_with(|| HistoryEntry {
        seq: change.seq,
        revision: None,
        details: None,
        action: action(&change.kind).to_owned(),
        actor_name: actor_name(change),
        saved_at: saved_at(&change.created),
        reason: change.reason.clone(),
        changes: Vec::new(),
    })
}

fn to_changes(differences: Vec<catalog::DetailDifference>) -> Vec<DetailChange> {
    differences
        .into_iter()
        .map(|difference| DetailChange {
            label: difference.label,
            before: difference.before,
            after: difference.after,
        })
        .collect()
}

fn current_details_view(number: u32, details: &PartDetails) -> CurrentDetails {
    CurrentDetails {
        number,
        fields: fields_of(&details.fields),
        manufacturer_parts: manufacturer_parts_of(details),
    }
}

fn current_revision_view(number: u32, revision: &PartRevision) -> CurrentRevision {
    CurrentRevision {
        number,
        cad_fields: fields_of(&revision.cad_fields),
        cad_items: revision
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

/// Who made a Change, in words a team member understands. Administrator
/// accounts (such as the one that runs imports) are not team members, so
/// their email is not shown.
fn actor_name(change: &StoredChange) -> String {
    plain_actor(&change.actor, &change.actor_name)
}

fn plain_actor(actor: &str, name: &str) -> String {
    if actor.starts_with("_superusers:") {
        "an administrator".to_owned()
    } else if name.is_empty() {
        "someone".to_owned()
    } else {
        name.to_owned()
    }
}

/// The plain word for a Change kind.
fn action(kind: &str) -> &'static str {
    match kind {
        "csv-import" => "Imported",
        "edit" => "Edited",
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

fn fields_of(fields: &[DetailField]) -> Vec<Field> {
    fields
        .iter()
        .map(|field| Field {
            name: field.name.clone(),
            value: field.value.clone(),
        })
        .collect()
}

fn detail_fields(fields: &[Field]) -> Vec<DetailField> {
    fields
        .iter()
        .map(|field| DetailField {
            name: field.name.clone(),
            value: field.value.clone(),
        })
        .collect()
}

fn catalog_parts(parts: &[ManufacturerPart]) -> Vec<catalog::ManufacturerPart> {
    parts
        .iter()
        .map(|part| catalog::ManufacturerPart {
            manufacturer: part.manufacturer.clone(),
            mpn: part.mpn.clone(),
        })
        .collect()
}

/// Rows for the fields either side changed since `started`, and the merged
/// fields: theirs, with yours where you changed something.
fn field_rows(
    started: &[DetailField],
    theirs: &[DetailField],
    yours: &[Field],
    rows: &mut Vec<ConflictRow>,
) -> Vec<Field> {
    let started_value = |name: &str| {
        started
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
            .unwrap_or_default()
    };
    let mut merged = Vec::new();
    for field in theirs {
        let started = started_value(&field.name);
        let mine = yours
            .iter()
            .find(|yours| yours.name == field.name)
            .map_or_else(|| started.clone(), |yours| yours.value.clone());
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
        merged.push(Field {
            name: field.name.clone(),
            value: if you_changed {
                mine
            } else {
                field.value.clone()
            },
        });
    }
    merged
}

/// Every field or list that the edit or the other person changed since the
/// details and revision the edit started from, and what they saved with
/// only the edit's changes applied on top. `details` and `revision` are the
/// current ones the server gave for whatever moved.
fn edit_conflict(
    request: &TeamEditRequest,
    details: Option<&ConflictingRecord>,
    revision: Option<&ConflictingRecord>,
    part: &LoadedPart,
) -> Result<TeamConflict, String> {
    let base_details = part
        .details
        .get(&request.expected_details)
        .ok_or_else(|| format!("details {} are missing", request.expected_details))?;
    let base_revision = part
        .revisions
        .get(&request.expected_revision)
        .ok_or_else(|| format!("revision {} is missing", request.expected_revision))?;
    // What the server gave, or the part as it is now.
    let (their_details_number, their_details) = match details {
        Some(record) => (record.number, record.details().map_err(|e| e.to_string())?),
        None => (
            part.view.details.number,
            part.details[&part.view.details.number].clone(),
        ),
    };
    let (their_revision_number, their_revision) = match revision {
        Some(record) => (record.number, record.revision().map_err(|e| e.to_string())?),
        None => (
            part.view.revision.number,
            part.revisions[&part.view.revision.number].clone(),
        ),
    };
    let mut rows = Vec::new();
    let merged_fields = field_rows(
        &base_details.fields,
        &their_details.fields,
        &request.fields,
        &mut rows,
    );
    let started_parts = stored_lines(base_details);
    let their_parts = stored_lines(&their_details);
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
    let merged_cad = field_rows(
        &base_revision.cad_fields,
        &their_revision.cad_fields,
        &request.cad_fields,
        &mut rows,
    );
    let merged = TeamEditRequest {
        correlation_id: None,
        ipn: request.ipn.clone(),
        expected_details: their_details_number,
        expected_revision: their_revision_number,
        reason: request.reason.clone(),
        fields: merged_fields,
        manufacturer_parts: if you_changed_parts {
            request.manufacturer_parts.clone()
        } else {
            manufacturer_parts_of(&their_details)
        },
        cad_fields: merged_cad,
    };
    // The newest of the changes that moved what the edit started from.
    let newest = [details, revision]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.created.cmp(&b.created));
    let theirs = part
        .view
        .history
        .iter()
        .find(|entry| {
            details.is_some_and(|d| entry.details == Some(d.number))
                || revision.is_some_and(|r| entry.revision == Some(r.number))
        })
        .cloned()
        .or_else(|| {
            newest.map(|record| HistoryEntry {
                seq: 0,
                revision: None,
                details: None,
                action: action(&record.change_kind).to_owned(),
                actor_name: plain_actor(&record.actor, &record.actor_name),
                saved_at: saved_at(&record.created),
                reason: record.reason.clone(),
                changes: Vec::new(),
            })
        })
        .ok_or_else(|| "the conflict names nothing that moved".to_owned())?;
    Ok(TeamConflict {
        theirs,
        rows,
        merged,
        part: part.view.clone(),
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
    fn stored_revision_records_are_checked_against_the_part_details() {
        let details = |kind: CadItemKind, format: &str, key: &str| CadItemDetails {
            schema: "kicad-parts/cad-item-details/1".into(),
            kind,
            key: key.into(),
            format: format.into(),
            format_version: Some("20241209".into()),
            blob_sha256: "0".repeat(64),
            blob_size: 1,
            model_references: Vec::new(),
            license: catalog::License::UnknownNeedsReview,
        };
        let record = |details: &CadItemDetails| StoredCadItemRevision {
            id: "r1".into(),
            cad_item: "item1".into(),
            number: 1,
            predecessor: String::new(),
            change: "c1".into(),
            blob: "b1".into(),
            snapshot_sha256: details.sha256(),
            snapshot: details.canonical_json(),
            license: "unknown-needs-review".into(),
            source: String::new(),
        };
        let symbol = details(CadItemKind::Symbol, "kicad-symbol-entry", "g-pas:R_US");
        let good = record(&symbol);
        let symbol_sha256 = symbol.sha256();
        let check = |revision: &StoredCadItemRevision, cad_item: &str, kind: PreviewKind| {
            let named = NamedRevision {
                cad_item,
                number: 1,
                snapshot_sha256: &symbol_sha256,
            };
            checked_snapshot(revision, &named, "g-pas:R_US", kind)
        };
        assert_eq!(
            check(&good, "item1", PreviewKind::Symbol),
            Ok(symbol.clone())
        );

        // A snapshot changed after it was hashed.
        let mut tampered = good.clone();
        tampered.snapshot = tampered.snapshot.replace("R_US", "R_EU");
        assert_eq!(
            check(&tampered, "item1", PreviewKind::Symbol),
            Err("its record does not match its SHA-256".into())
        );
        // The right snapshot under another CAD item.
        assert_eq!(
            check(&good, "item2", PreviewKind::Symbol),
            Err("its record belongs to another CAD item".into())
        );
        // Another revision of the same CAD item, even with the same content
        // (going back to earlier content adds a revision).
        let mut later = good.clone();
        later.number = 3;
        assert_eq!(
            check(&later, "item1", PreviewKind::Symbol),
            Err("its record is another revision of that CAD item".into())
        );
        // A symbol asked for as a footprint.
        assert!(
            check(&good, "item1", PreviewKind::Footprint)
                .unwrap_err()
                .starts_with("its record is a symbol")
        );
        // A footprint record whose hash the part's details name for a symbol.
        let footprint = details(CadItemKind::Footprint, "kicad-footprint", "g-pas:R_US");
        let wrong_kind = record(&footprint);
        assert!(
            checked_snapshot(
                &wrong_kind,
                &NamedRevision {
                    cad_item: "item1",
                    number: 1,
                    snapshot_sha256: &footprint.sha256(),
                },
                "g-pas:R_US",
                PreviewKind::Symbol
            )
            .is_err()
        );
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

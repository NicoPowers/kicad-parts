//! The team catalog use cases behind the desktop: sign in to the team's
//! PocketBase server, read one part's details and history, publish edits,
//! archive and unarchive revisions, review CSV changes that were not
//! applied, and follow changes others make.
//!
//! A [`TeamCatalog`] is one team: one team server and, while signed in, one
//! session with it. The person's list of teams is in [`crate::teams`].
//!
//! While signed in, a background thread keeps a replica of the team catalog
//! in step with the server (see [`session`]).
//!
//! The UI polls [`TeamCatalog::status`], whose `checkpoint` grows whenever
//! anyone publishes a Change and whose `imports` grows when a CSV import
//! run finishes (a run may find CSV changes without publishing a Change) or
//! after catching up.

mod session;

use crate::PartValues;
use crate::csv_ipns::IpnsInCsv;
use catalog::library_check::{chosen_footprint_problems, judge_model_link};
use catalog::pocketbase::UploadRole;
use catalog::pocketbase::{
    ArchiveCommand, ArchiveOutcome, CSV_CAD_DIFFERS, CSV_CAD_ITEM_KEPT, CSV_DETAILS_DIFFER,
    CadUpload, ConflictingRecord, CreateCommand, CreateOutcome, CsvReviewCommand, CsvReviewOutcome,
    EDIT_BODY_LIMIT, EditCommand, IPN_GIVEN_TO_ANOTHER_PART, MANUFACTURER_PARTS_FIELD,
    PublishOutcome, PublishReply, StoredArchiveEntry, StoredBlob, StoredCadItemRevision,
    StoredChange, StoredCsvReview, StoredImportProblem, StoredPart, StoredPartRevision,
    archive_entries_of, archived_numbers, cad_item_revisions_by_id, cad_item_revisions_in_changes,
    changes_by_id, csv_reviews_of, current_categories, fields_for_category, has_cad_item,
    latest_import_run, part_by_ipn, part_details_of, part_revisions, problems_of,
    recorded_csv_document, remembered_matches, submit_archive, submit_create, submit_csv_review,
    submit_edit,
};
use catalog::{
    CATEGORY_LABEL, CadItemDetails, CadItemId, CadItemKind, CadItemRevisionId, CadReference,
    CadReferenceRole, CadReferenceTarget, CategoryDetails, DetailField, DifferenceKind,
    ModelTarget, PartDetails, PartRevision, compare_details, compare_revisions,
};
use collaboration::{CollabError, CorrelationId, PocketBaseClient};
use desktop_contract::{
    ArchiveNote, CadItemUse, ConflictRow, CsvChangeRow, CsvChanges, CsvRowForAnotherPart,
    CurrentDetails, CurrentRevision, DetailChange, Field, FileKind, HistoryEntry, ManufacturerPart,
    OlderUse, PreviewKind, PublishResult, RevisionStatus, SignInRequest, StoredFootprint,
    TeamArchiveRequest, TeamCategory, TeamConflict, TeamCsvReviewRequest, TeamEditRequest,
    TeamFile, TeamFileReading, TeamFileRequest, TeamIpnCheck, TeamNewPartRequest, TeamPart,
    TeamSession, TeamStatus,
};
use kicad_integration::library_file::{
    footprint_says_excluded_from_bom, symbol_entries_say_not_in_bom,
};
use kicad_integration::{LibRef, StoredItem, footprint_choices};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use session::{Session, sign_in};

/// What the status says when the team server no longer accepts a sign-in,
/// for example after the password changed.
pub const SIGN_IN_AGAIN: &str =
    "The team server no longer accepts your sign-in. Enter your password to sign in again.";

/// How renewing a session's sign-in went.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Renewed {
    /// The session sends this new token from now on.
    Token(String),
    /// The team server no longer accepts the session's sign-in.
    Rejected,
    /// The person signed in or out while it ran, so the answer is about a
    /// sign-in that is gone. Nothing to do.
    SessionChanged,
    /// Anything else, such as a network problem, in plain words. Try
    /// again later.
    Failed(String),
}

/// How carrying on a kept sign-in went.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resumed {
    /// Signed in again; the session sends `token`, a renewed one.
    SignedIn { token: String },
    /// A session was already running, such as from a password sign-in
    /// that finished first; it was kept.
    AlreadySignedIn,
    /// The team server could not be reached, for the plain reason given.
    /// The kept sign-in may still be good, so try again later.
    Unreachable(String),
    /// The team server no longer accepts the kept sign-in.
    Rejected,
}

struct State {
    /// Why the last sign-in failed, if it did.
    problem: Option<String>,
    session: Option<Session>,
}

/// One team's catalog: the session with one team server, if signed in,
/// and every use case the desktop runs against that team. [`crate::teams`]
/// holds one for each team in the person's list.
pub struct TeamCatalog {
    state: Mutex<State>,
    /// The team's code, such as `MA`, once read from the team's settings;
    /// blank before. It names the team in the list of teams.
    code: Mutex<String>,
    /// Revisions never change, so what was fetched for previews is kept
    /// until the next sign-in or sign-out.
    known: Mutex<KnownRevisions>,
}

impl Default for TeamCatalog {
    fn default() -> Self {
        Self::new()
    }
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
    /// The CAD item's ID.
    pub cad_item: String,
    /// The SHA-256 of the revision's record.
    pub snapshot_sha256: String,
    /// The ID of the blob record holding the bytes.
    blob: String,
}

impl StoredCad {
    /// The revision, as a footprint's 3D view asks for its stored models.
    pub fn footprint(&self) -> StoredFootprint {
        StoredFootprint {
            cad_item: self.cad_item.clone(),
            number: self.number,
            snapshot_sha256: self.snapshot_sha256.clone(),
        }
    }
}

/// A footprint's 3D model link as the footprint writes it, and the stored
/// model the Catalog binds it to, or why it cannot be used.
pub type FootprintModel = (String, Result<StoredModel, StoredModelError>);

/// Why a footprint revision's stored 3D model cannot be used. Each reason
/// finishes the sentence "... because ...".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoredModelError {
    /// Its record failed the checks previews use.
    Check(String),
    /// The team server could not give it now, such as when signed out.
    Unavailable(String),
}

/// One 3D model revision a footprint revision uses, as the Catalog stores
/// it, after its record is checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredModel {
    /// Where the Catalog keeps it, such as
    /// `3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step`.
    pub key: String,
    /// The SHA-256 of its bytes, which are checked against it.
    pub blob_sha256: String,
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
    /// A team catalog that is not signed in yet.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                problem: None,
                session: None,
            }),
            known: Mutex::new(KnownRevisions::default()),
            code: Mutex::new(String::new()),
        }
    }

    /// The team's code, such as `MA`; blank until the team's settings are
    /// read, or while the team has none.
    pub fn code(&self) -> String {
        self.code.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Notes the team's code, as read from its settings. The code never
    /// changes once set, so it is kept after signing out.
    pub(crate) fn remember_code(&self, code: &str) {
        if !code.is_empty() {
            *self.code.lock().unwrap_or_else(|e| e.into_inner()) = code.to_owned();
        }
    }

    /// Who is signed in, and how far the app has followed the team
    /// catalog. The server address and email are blank while signed out:
    /// the list of teams keeps those.
    pub fn status(&self) -> TeamStatus {
        let state = self.lock();
        let Some(session) = &state.session else {
            return TeamStatus {
                server_url: String::new(),
                email: String::new(),
                code: self.code(),
                note: None,
                session: TeamSession::SignedOut {
                    problem: state.problem.clone(),
                },
            };
        };
        let (live, checkpoint, imports) = session.follower.snapshot();
        TeamStatus {
            server_url: session.client().base_url().to_owned(),
            email: session.email.clone(),
            code: self.code(),
            note: None,
            session: TeamSession::SignedIn {
                name: session.name.clone(),
                email: session.email.clone(),
                can_edit: session.can_edit,
                live,
                checkpoint,
                imports,
            },
        }
    }

    /// Why the last sign-in failed or ended, while signed out.
    pub fn problem(&self) -> Option<String> {
        let state = self.lock();
        state.session.is_none().then(|| state.problem.clone())?
    }

    /// Whether a session with the team server is running.
    pub fn is_signed_in(&self) -> bool {
        self.lock().session.is_some()
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
                Ok(session) => {
                    state.problem = None;
                    state.session = Some(session);
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

    /// Carries on a sign-in kept from an earlier run, with no password:
    /// renews `token` with the team server at `server_url`, then follows
    /// the team catalog as when signing in.
    pub fn resume(&self, server_url: &str, token: &str) -> Resumed {
        let session = match session::resume(server_url, token) {
            Ok(session) => session,
            Err(
                CollabError::Unauthorized(_) | CollabError::Forbidden(_) | CollabError::Address(_),
            ) => return Resumed::Rejected,
            Err(CollabError::Transport(problem)) => {
                return Resumed::Unreachable(problem.to_string());
            }
            Err(other) => return Resumed::Unreachable(other.to_string()),
        };
        let mut state = self.lock();
        if state.session.is_some() {
            // Never replaces a session: a password sign-in finished first.
            drop(state);
            drop(session);
            return Resumed::AlreadySignedIn;
        }
        let token = session.client().token().unwrap_or_default().to_owned();
        state.problem = None;
        state.session = Some(session);
        drop(state);
        self.forget_known();
        Resumed::SignedIn { token }
    }

    /// The token the session sends now, to keep for the next run; `None`
    /// while signed out.
    pub fn token(&self) -> Option<String> {
        let state = self.lock();
        let client = state.session.as_ref()?.client();
        client.token().map(str::to_owned)
    }

    /// Renews the session's sign-in with the team server. Only the session
    /// that was renewed is judged: if the person signed in or out while the
    /// request ran, the answer is [`Renewed::SessionChanged`], whatever
    /// the server said.
    pub fn renew(&self) -> Renewed {
        let Some(client) = self.lock().session.as_ref().map(|session| session.client()) else {
            return Renewed::SessionChanged;
        };
        let answer = client.refresh_auth("users");
        let state = self.lock();
        let Some(session) = state
            .session
            .as_ref()
            .filter(|session| session.client().token() == client.token())
        else {
            return Renewed::SessionChanged;
        };
        match answer {
            Ok((renewed, _)) => {
                let token = renewed.token().unwrap_or_default().to_owned();
                session.renewed(renewed);
                Renewed::Token(token)
            }
            Err(CollabError::Unauthorized(_) | CollabError::Forbidden(_)) => Renewed::Rejected,
            Err(other) => Renewed::Failed(other.to_string()),
        }
    }

    /// Ends the session because the team server no longer accepts its
    /// sign-in, and asks the person to sign in again. It acts only on the
    /// sign-in that was judged: the session whose token is `judged`, or,
    /// with `None`, a kept sign-in not carried on yet, while no session
    /// runs. Says whether it acted.
    pub fn sign_in_again(&self, judged: Option<&str>) -> bool {
        let earlier = {
            let mut state = self.lock();
            let token = state
                .session
                .as_ref()
                .map(|session| session.client().token().map(str::to_owned));
            let same = match (judged, token) {
                (None, None) => true,
                (Some(judged), Some(Some(token))) => judged == token,
                _ => false,
            };
            if !same {
                return false;
            }
            state.problem = Some(SIGN_IN_AGAIN.into());
            state.session.take()
        };
        drop(earlier);
        self.forget_known();
        true
    }

    /// Every category's plain name, by code, for grouping the part list;
    /// empty while signed out. Read again only when the team catalog has
    /// moved on (a new Change or import), so searching makes no request.
    /// If reading fails, the names read before are kept until it moves on.
    pub fn category_names(&self) -> BTreeMap<String, String> {
        let (client, cache, position) = {
            let state = self.lock();
            let Some(session) = &state.session else {
                return BTreeMap::new();
            };
            let (_, checkpoint, imports) = session.follower.snapshot();
            (
                session.client(),
                session.category_names.clone(),
                (checkpoint, imports),
            )
        };
        let earlier = cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some((at, names)) = &earlier {
            if *at == position {
                return names.clone();
            }
        }
        // Read outside the session lock, so status polling never waits.
        let names = match current_categories(&client) {
            Ok(categories) => category_names(&categories),
            Err(_) => earlier.map(|(_, names)| names).unwrap_or_default(),
        };
        *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((position, names.clone()));
        names
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
                    category: Some(details.category.clone()),
                    fields: Some(
                        details
                            .fields
                            .iter()
                            .map(|field| (field.name.clone(), field.value.clone()))
                            .collect(),
                    ),
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
        self.edit_with(request, |_| Ok(Vec::new()))
    }

    /// [`Self::edit`], with `more` giving uploads for the symbols and
    /// footprints the edit newly names that no file is sent for, such as
    /// our libraries' items the Catalog lacks (kicad-parts-bc6f); what it
    /// refuses is said and nothing is saved.
    pub(crate) fn edit_with(
        &self,
        request: TeamEditRequest,
        more: impl FnOnce(&[(CadItemKind, String)]) -> Result<Vec<CadUpload>, String>,
    ) -> Result<PublishResult, String> {
        let client = self.client()?;
        let part = match part_by_ipn(&client, &request.ipn) {
            Ok(Some(part)) => part,
            Ok(None) => return Err(format!("{} is no longer in the team catalog", request.ipn)),
            Err(error) => return save_failed(error),
        };
        let started = match load(&client, &part) {
            Ok(started) => started,
            Err(ReadFailure::Unreachable) => {
                return Ok(PublishResult::Refused {
                    message: SAVE_UNREACHABLE.into(),
                });
            }
            Err(ReadFailure::Other(message)) => return Err(message),
        };
        let base_details = started
            .details
            .get(&request.expected_details)
            .ok_or_else(|| format!("details {} are missing", request.expected_details))?;
        let base_revision = started
            .revisions
            .get(&request.expected_revision)
            .ok_or_else(|| format!("revision {} is missing", request.expected_revision))?;
        let request = tidied(request, base_details, base_revision);
        let fields = detail_fields(&request.fields);
        let parts = catalog_parts(&request.manufacturer_parts);
        let cad_fields = detail_fields(&request.cad_fields);
        let mut uploads = match uploads_of(&request.files, &request.cad_fields) {
            Ok(uploads) => uploads,
            Err(message) => return Ok(PublishResult::Refused { message }),
        };
        let named = newly_named(&request.cad_fields, base_revision, &uploads);
        match more(&named) {
            Ok(more) => uploads.extend(more),
            Err(message) => return Ok(PublishResult::Refused { message }),
        }
        if let Some(message) = model_link_refusal(&client, &uploads)? {
            return Ok(PublishResult::Refused { message });
        }
        // A new category: the part keeps every field it had and gains the
        // new category's fields it lacks (blank unless typed in the form).
        let category = match request.category.as_str() {
            "" => None,
            code if code == base_details.category => None,
            code => match started
                .categories
                .iter()
                .find(|category| category.code == code)
            {
                Some(category) => Some(category),
                None => {
                    return Ok(PublishResult::Refused {
                        message: format!("The category {code} is not in the team catalog."),
                    });
                }
            },
        };
        let fields = match category {
            Some(category) => fields_for_category(&base_details.fields, &category.fields)
                .into_iter()
                .map(|field| {
                    let typed = fields.iter().find(|typed| typed.name == field.name);
                    typed.cloned().unwrap_or(field)
                })
                .collect(),
            None => fields,
        };
        let moved = category.is_some();
        let mut command = EditCommand::new(&request.ipn, &request.reason);
        command.correlation_id = attempt_id(request.correlation_id.as_deref())?;
        if fields != base_details.fields
            || parts != base_details.manufacturer_parts
            || category.is_some()
        {
            command = command.editing_details(base_details, request.expected_details);
            if let Some(details) = &mut command.details {
                if let Some(category) = category {
                    details.category.clone_from(&category.code);
                }
                details.fields = fields;
                details.manufacturer_parts = parts;
            }
        }
        if cad_fields != base_revision.cad_fields || !uploads.is_empty() {
            command = command.editing_cad(base_revision, request.expected_revision);
            if let Some(cad) = &mut command.cad {
                cad.fields = cad_fields;
                cad.uploads = uploads;
            }
        }
        if command.details.is_none() && command.cad.is_none() {
            return Ok(PublishResult::NothingToSave {
                message: "Nothing changed, so nothing was saved.".into(),
            });
        }
        if command.body().to_string().len() > EDIT_BODY_LIMIT {
            return Ok(PublishResult::Refused {
                message: format!(
                    "The files are too big to save together: one save can send at most {} MB. Nothing was saved.",
                    EDIT_BODY_LIMIT / (1024 * 1024)
                ),
            });
        }
        let reply = submit_edit(&client, &command);
        let result = self.published(&client, &request.ipn, reply, |details, revision, part| {
            edit_conflict(&request, details, revision, part)
        })?;
        // Every change you made is already saved (by someone else, or by
        // you in another window): nothing of yours is left to compare or
        // to save on top.
        Ok(match result {
            PublishResult::Conflict(conflict)
                if !conflict.rows.iter().any(|row| row.you_changed) =>
            {
                PublishResult::AlreadySaved {
                    message: ALREADY_SAVED.into(),
                    part: conflict.part,
                }
            }
            PublishResult::Refused { message } => PublishResult::Refused {
                message: plain_refusal(message, moved),
            },
            other => other,
        })
    }

    /// Archives or unarchives one revision, with a reason.
    pub fn archive(&self, request: TeamArchiveRequest) -> Result<PublishResult, String> {
        let client = self.client()?;
        let mut command = ArchiveCommand::new(
            &request.ipn,
            request.revision,
            request.archived,
            &request.reason,
        );
        command.correlation_id = attempt_id(request.correlation_id.as_deref())?;
        match submit_archive(&client, &command) {
            Ok(reply) => match reply.outcome {
                ArchiveOutcome::Applied {
                    revision,
                    archived,
                    current_revision,
                    ..
                } => saved_then(reload(&client, &request.ipn), |part| {
                    PublishResult::Archived {
                        revision,
                        archived,
                        current: current_revision.number,
                        part,
                    }
                }),
                ArchiveOutcome::Noop { .. } => Ok(PublishResult::NothingToSave {
                    message: if request.archived {
                        format!("Revision {} is already archived.", request.revision)
                    } else {
                        format!("Revision {} is not archived.", request.revision)
                    },
                }),
                ArchiveOutcome::CorrelationReused { message, .. } => {
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
            Err(error) => save_failed(error),
        }
    }

    /// Reviews CSV changes that were not applied to an edited part: takes
    /// the CSV's values for the fields named, keeps the part's for the rest.
    pub fn csv_review(&self, request: TeamCsvReviewRequest) -> Result<PublishResult, String> {
        let client = self.client()?;
        let mut command = CsvReviewCommand::new(
            &request.ipn,
            &request.problem,
            request.accept.clone(),
            request.expected,
            &request.reason,
        );
        command.correlation_id = attempt_id(request.correlation_id.as_deref())?;
        match submit_csv_review(&client, &command) {
            Ok(reply) => match reply.outcome {
                CsvReviewOutcome::Applied {
                    accepted,
                    details,
                    revision,
                    ..
                } => saved_then(reload(&client, &request.ipn), |part| {
                    PublishResult::CsvReviewed {
                        accepted,
                        details_saved: details.is_some(),
                        revision: revision.map(|revision| revision.number),
                        part,
                    }
                }),
                CsvReviewOutcome::Noop { .. } => Ok(PublishResult::NothingToSave {
                    message: "Someone already reviewed these CSV values, so nothing was saved."
                        .into(),
                }),
                CsvReviewOutcome::Conflict { .. } => Ok(PublishResult::Refused {
                    message: format!(
                        "Someone else changed {} after you opened these CSV values. Nothing was saved. Look at the part again, then review the CSV values again.",
                        request.ipn
                    ),
                }),
                CsvReviewOutcome::OutOfDate { .. } => Ok(PublishResult::Refused {
                    message: "A newer import has run since you opened these CSV values. Nothing was saved. Open the part again to see what differs now.".into(),
                }),
                CsvReviewOutcome::CorrelationReused { message, .. } => {
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
            Err(error) => save_failed(error),
        }
    }

    /// Every part's current details and current revision, from what the app
    /// follows; no request. Empty while signed out.
    pub(crate) fn current_parts(&self) -> Vec<(PartDetails, PartRevision)> {
        let state = self.lock();
        let Some(session) = &state.session else {
            return Vec::new();
        };
        let live = session
            .follower
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        live.replica
            .parts()
            .values()
            .filter_map(|part| Some((part.details.clone()?, part.revision.clone()?)))
            .collect()
    }

    /// Who is signed in, to which server, and how far the app has followed
    /// the team catalog (its checkpoint and finished imports), when they
    /// can edit: the library check is worked out again only when this
    /// changes. Two teams may share a checkpoint, so the server is part of
    /// it. No request; `None` while signed out or for someone who can only
    /// view.
    pub(crate) fn editor_position(&self) -> Option<(String, u64, u64)> {
        let state = self.lock();
        let session = state.session.as_ref().filter(|session| session.can_edit)?;
        let (_, checkpoint, imports) = session.follower.snapshot();
        let who = format!(
            "{}
{}",
            session.client().base_url(),
            session.email
        );
        Some((who, checkpoint, imports))
    }

    /// What an importer reads from the team catalog: each CAD item's
    /// current revision and the bytes of its newest imported one, and the
    /// latest import runs. For the library check.
    pub(crate) fn import_state(&self) -> Result<catalog::pocketbase::ImportState, String> {
        let client = self.client()?;
        catalog::pocketbase::read_import_state(&client).map_err(|error| {
            format!(
                "The team catalog could not be read because {}.",
                plain_reason(&error)
            )
        })
    }

    /// Whether the Catalog has the CAD item `key` of `kind`: one request.
    pub(crate) fn has_cad_item(&self, kind: CadItemKind, key: &str) -> Result<bool, String> {
        let client = self.client()?;
        has_cad_item(&client, kind, key).map_err(|error| {
            format!(
                "The team catalog could not be read because {}.",
                plain_reason(&error)
            )
        })
    }

    /// Every category's current revision and every remembered match, for
    /// Add part's category guess: two requests.
    pub(crate) fn categories_and_matches(
        &self,
    ) -> Result<(Vec<CategoryDetails>, Vec<catalog::guess::RememberedMatch>), String> {
        let client = self.client()?;
        let fail = |error: CollabError| {
            format!(
                "The team catalog could not be read because {}.",
                plain_reason(&error)
            )
        };
        Ok((
            current_categories(&client).map_err(fail)?,
            remembered_matches(&client).map_err(fail)?,
        ))
    }

    /// The parts that already have `mpn` (without regard to case), from what
    /// the app follows; no request.
    pub fn same_mpn(&self, mpn: &str, manufacturer: &str) -> Vec<desktop_contract::TeamSameMpn> {
        crate::add_part::same_mpn(mpn, manufacturer, &self.current_parts())
    }

    /// Whether `ipn` can be a new part's IPN: written as an IPN, and not
    /// one the team catalog or the CSV catalog (`csv`) has.
    pub fn check_ipn(&self, ipn: &str, prefix: &str, csv: &IpnsInCsv) -> TeamIpnCheck {
        let problem = if catalog::Ipn::parse(ipn).is_err() {
            Some("Write the IPN as three capital letters, four digits and four letters or digits, such as RES-0001-0001.".to_owned())
        } else if !prefix.is_empty() && !ipn.starts_with(&format!("{prefix}-")) {
            Some(format!(
                "A part in this category has an IPN starting with {prefix}-."
            ))
        } else if self
            .current_parts()
            .iter()
            .any(|(details, _)| details.ipn == ipn)
        {
            Some(format!(
                "{ipn} is already in the Catalog. An IPN is never reused."
            ))
        } else {
            // A CSV row the team catalog has not imported yet.
            csv.file_of(ipn)
                .map(|file| format!("{ipn} is already in {file}. An IPN is never reused."))
        };
        TeamIpnCheck { problem }
    }

    /// Saves a new part: part details 1 in the confirmed category, revision
    /// 1 with its symbol and footprint, and the files they use. `csv` are
    /// the CSV catalog's IPNs, read just before saving: the team catalog
    /// may not have them yet, so an IPN there is refused, and none of them
    /// is offered as the next free IPN.
    pub fn add_part(
        &self,
        request: TeamNewPartRequest,
        csv: &IpnsInCsv,
    ) -> Result<PublishResult, String> {
        let files = || {
            let uploads = crate::add_part::uploads(&request)?;
            Ok((uploads, request.downloads.clone()))
        };
        self.add_part_with(request.clone(), csv, files)
    }

    /// The next free IPN after `ipn` was found taken: for its prefix, after
    /// every IPN the team catalog and the CSV catalog (`csv`) have, and
    /// `ipn` itself.
    pub(crate) fn next_free_after(&self, ipn: &str, csv: &IpnsInCsv) -> String {
        let parts = self.current_parts();
        let taken = csv
            .ipns()
            .chain(parts.iter().map(|(details, _)| details.ipn.as_str()))
            .chain(std::iter::once(ipn));
        catalog::download::next_free_ipn(&ipn[..3], taken).unwrap_or_default()
    }

    /// [`Self::add_part`] with the files to send, and the names of the files
    /// the part came from, made by `files` once the form's own checks pass.
    pub(crate) fn add_part_with(
        &self,
        request: TeamNewPartRequest,
        csv: &IpnsInCsv,
        files: impl FnOnce() -> Result<(Vec<CadUpload>, Vec<String>), String>,
    ) -> Result<PublishResult, String> {
        let client = self.client()?;
        let refused = |message: String| Ok(PublishResult::Refused { message });
        if request.reason.trim().is_empty() {
            return refused("Say why you are adding the part before you save.".into());
        }
        for part in &request.manufacturer_parts {
            match (
                part.manufacturer.trim().is_empty(),
                part.mpn.trim().is_empty(),
            ) {
                (false, true) => return refused("Type the MPN, or clear Manufacturer.".into()),
                (true, false) => return refused("Type the manufacturer, or clear MPN.".into()),
                _ => {}
            }
        }
        // A taken IPN is the server's to say: someone may take it any time.
        let ipn = request.ipn.trim().to_owned();
        if catalog::Ipn::parse(&ipn).is_err() {
            if let Some(problem) = self.check_ipn(&ipn, "", csv).problem {
                return refused(problem);
            }
        }
        // Only this app sees the CSV catalog folder, so it checks it here: a
        // row saved or pulled since Add part offered the IPN has it now. A
        // part the team catalog has is the server's to answer, because it
        // knows whether you added it yourself.
        let in_team_catalog = self
            .current_parts()
            .iter()
            .any(|(details, _)| details.ipn == ipn);
        if let Some(file) = csv.file_of(&ipn).filter(|_| !in_team_catalog) {
            let next_free = self.next_free_after(&ipn, csv);
            return Ok(PublishResult::IpnTaken {
                message: format!(
                    "Someone gave the IPN {ipn} to a row of {file}, so nothing was saved. The next free IPN is {next_free}."
                ),
                next_free,
                ipn,
                yours: false,
            });
        }
        let (uploads, sources) = match files() {
            Ok(files) => files,
            Err(message) => return refused(message),
        };
        if let Some(message) = model_link_refusal(&client, &uploads)? {
            return refused(message);
        }
        let mut command = CreateCommand::new(&ipn, &request.category, request.reason.trim());
        command.correlation_id = attempt_id(request.correlation_id.as_deref())?;
        command.fields = request
            .fields
            .iter()
            .map(|field| DetailField {
                name: field.name.clone(),
                value: field.value.trim().to_owned(),
            })
            .collect();
        command.manufacturer_parts = request
            .manufacturer_parts
            .iter()
            .map(|part| (part.manufacturer.trim(), part.mpn.trim()))
            .filter(|(manufacturer, mpn)| !manufacturer.is_empty() || !mpn.is_empty())
            .map(|(manufacturer, mpn)| catalog::ManufacturerPart {
                manufacturer: manufacturer.to_owned(),
                mpn: mpn.to_owned(),
            })
            .collect();
        command.cad_fields[0].value = request.symbol.trim().to_owned();
        command.cad_fields[1].value = request.footprint.trim().to_owned();
        command.uploads = uploads;
        command.from = request.from.trim().to_owned();
        command.files = sources;
        if command.body().to_string().len() > EDIT_BODY_LIMIT {
            return refused(format!(
                "The files are too big to save together: one save can send at most {} MB. Choose No 3D model, or a smaller model. Nothing was saved.",
                EDIT_BODY_LIMIT / (1024 * 1024)
            ));
        }
        match submit_create(&client, &command) {
            Ok(reply) => match reply.outcome {
                CreateOutcome::Applied { seq, .. } => {
                    // The new part opens and is found at once: wait (a
                    // little) until the followed replica has it.
                    self.wait_for_change(seq);
                    saved_then(reload(&client, &ipn), |part| PublishResult::Added { part })
                }
                CreateOutcome::IpnTaken {
                    added_by_you,
                    csv_file,
                    ..
                } => {
                    let next_free = self.next_free_after(&ipn, csv);
                    // A part you added yourself (a save whose answer was
                    // lost) is offered to open; another part with this IPN
                    // is someone else's.
                    let message = if added_by_you {
                        format!(
                            "You already added {ipn}, so nothing more was saved. Open it to check it, or, for another part, use the next free IPN, {next_free}."
                        )
                    } else if !csv_file.is_empty() {
                        // The team server knows a row the import could not
                        // make a part from.
                        format!(
                            "A row of {csv_file} has the IPN {ipn}, but the import could not make a part from it, so nothing was saved. The next free IPN is {next_free}."
                        )
                    } else {
                        format!(
                            "Someone gave the IPN {ipn} to another part first, so nothing was saved. The next free IPN is {next_free}."
                        )
                    };
                    Ok(PublishResult::IpnTaken {
                        message,
                        next_free,
                        ipn: ipn.clone(),
                        yours: added_by_you,
                    })
                }
                CreateOutcome::CorrelationReused { message, .. } => refused(message),
            },
            Err(CollabError::Http {
                status: 400,
                message,
            }) => {
                refused(crate::add_part::reference_for_file(&request, &message).unwrap_or(message))
            }
            Err(CollabError::Forbidden(message)) => refused(message),
            Err(CollabError::Unauthorized(_)) => {
                refused("Your sign-in has ended. Sign in again, then save.".into())
            }
            Err(error) => save_failed(error),
        }
    }

    /// Waits up to five seconds for the followed replica to apply Change
    /// `seq`.
    fn wait_for_change(&self, seq: u64) {
        for _ in 0..100 {
            let reached = self
                .lock()
                .session
                .as_ref()
                .is_none_or(|session| session.follower.snapshot().1 >= seq);
            if reached {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Reads a symbol library or footprint file chosen in Edit part: the
    /// symbols it holds (or the footprint's name), the one to pick first,
    /// the library to suggest, and the Catalog 3D models a footprint names
    /// that the Catalog does not have yet.
    pub fn read_file(&self, request: TeamFileRequest) -> Result<TeamFileReading, String> {
        let bytes = decoded(&request.base64, &request.file_name)?;
        let current = LibRef::parse(request.current_reference.trim()).ok();
        let current_library = current
            .as_ref()
            .map(|reference| reference.library.clone())
            .filter(|library| library.starts_with("g-"));
        let current_name = current.map(|reference| reference.item);
        match request.kind {
            FileKind::Symbol => {
                let file = catalog::cad_file::read_symbol_file(&bytes)?;
                let chosen = current_name
                    .filter(|name| file.names.contains(name))
                    .unwrap_or_else(|| file.names[0].clone());
                let library = match current_library {
                    Some(library) => library,
                    None => self.csv_file_of(&request.ipn)?.unwrap_or_default(),
                };
                Ok(TeamFileReading {
                    names: file.names,
                    chosen,
                    library,
                    missing_models: Vec::new(),
                })
            }
            FileKind::Footprint => {
                let file = catalog::cad_file::read_footprint_file(&bytes)?;
                let library = current_library.unwrap_or_else(|| {
                    catalog::cad_file::suggested_footprint_library(&file.name)
                        .unwrap_or_default()
                        .to_owned()
                });
                let client = self.client()?;
                let mut missing_models = Vec::new();
                for key in file.models.iter().filter_map(|model| model.key.as_ref()) {
                    let known =
                        has_cad_item(&client, CadItemKind::Model3d, key).map_err(|error| {
                            format!(
                                "The team catalog could not be checked because {}.",
                                plain_reason(&error)
                            )
                        })?;
                    if !known && !missing_models.contains(key) {
                        missing_models.push(key.clone());
                    }
                }
                Ok(TeamFileReading {
                    names: vec![file.name.clone()],
                    chosen: file.name,
                    library,
                    missing_models,
                })
            }
            FileKind::Model3d => Err("Choose a 3D model for a footprint's 3D model.".into()),
        }
    }

    /// The CSV file `ipn` was first imported from, such as `g-res`, if it
    /// came from the CSV catalog.
    fn csv_file_of(&self, ipn: &str) -> Result<Option<String>, String> {
        let client = self.client()?;
        let fail = |error: CollabError| {
            format!(
                "The team catalog could not be checked because {}.",
                plain_reason(&error)
            )
        };
        let Some(part) = part_by_ipn(&client, ipn).map_err(fail)? else {
            return Ok(None);
        };
        let revisions = part_revisions(&client, &part.id).map_err(fail)?;
        Ok(revisions.first().and_then(|revision| {
            serde_json::from_str::<serde_json::Value>(&revision.source)
                .ok()?
                .get("csv_file")?
                .as_str()
                .map(str::to_owned)
        }))
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
        match reply {
            Ok(reply) => match reply.outcome {
                PublishOutcome::Applied {
                    revision,
                    new_cad_item_revisions,
                    ..
                } => {
                    let older_uses = new_cad_item_revisions
                        .into_iter()
                        .filter_map(|new| {
                            let other_parts = new.other_parts.filter(|count| *count > 0)?;
                            Some(OlderUse {
                                kind: new.kind,
                                reference: new.key,
                                number: new.number,
                                other_parts,
                            })
                        })
                        .collect();
                    saved_then(reload(client, ipn), |part| PublishResult::Saved {
                        revision: revision.map(|revision| revision.number),
                        part,
                        older_uses,
                    })
                }
                PublishOutcome::Noop { .. } => Ok(PublishResult::NothingToSave {
                    message: "Nothing changed, so nothing was saved.".into(),
                }),
                PublishOutcome::Conflict {
                    current_details,
                    current_revision,
                    ..
                } => match reload(client, ipn) {
                    Ok(loaded) => Ok(PublishResult::Conflict(Box::new(conflict(
                        current_details.as_deref(),
                        current_revision.as_deref(),
                        &loaded,
                    )?))),
                    Err(ReadFailure::Unreachable) => Ok(PublishResult::Refused {
                        message: CONFLICT_NOT_SHOWN.into(),
                    }),
                    Err(ReadFailure::Other(message)) => Err(message),
                },
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
            Err(error) => save_failed(error),
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
            cad_item: cad_item.clone(),
            snapshot_sha256: snapshot_sha256.clone(),
            blob: revision.blob,
        }))
    }

    /// The Catalog's newest revision of the symbol or footprint
    /// `reference`, to draw it as the Catalog has it; `None` when the
    /// Catalog does not have it.
    pub fn current_cad(
        &self,
        kind: PreviewKind,
        reference: &str,
    ) -> Result<Option<StoredCad>, TeamCadError> {
        let client = self.client().map_err(|_| TeamCadError::SignedOut)?;
        let item_kind = match kind {
            PreviewKind::Symbol => CadItemKind::Symbol,
            PreviewKind::Footprint => CadItemKind::Footprint,
        };
        let id = CadItemId::new(item_kind, reference);
        let filter = format!("cad_item = {}", serde_json::Value::from(id.as_str()));
        let Some(revision) = client
            .list_records::<StoredCadItemRevision>("catalog_cad_item_revisions", &filter, "-number")
            .map_err(unreachable)?
            .into_iter()
            .max_by_key(|revision| revision.number)
        else {
            return Ok(None);
        };
        let snapshot: CadItemDetails =
            serde_json::from_str(&revision.snapshot).map_err(|_| TeamCadError::Mismatch {
                reference: reference.to_owned(),
                why: "its record cannot be read".into(),
            })?;
        let lib_ref =
            LibRef::parse(reference).map_err(|error| TeamCadError::NotFound(error.to_string()))?;
        Ok(Some(StoredCad {
            item: StoredItem {
                lib_ref,
                blob_sha256: snapshot.blob_sha256,
                format_version: snapshot.format_version,
            },
            alternates: Vec::new(),
            number: revision.number,
            // Worked out here, so the 3D view checks the record as read.
            snapshot_sha256: catalog::sha256_hex(revision.snapshot.as_bytes()),
            cad_item: revision.cad_item,
            blob: revision.blob,
        }))
    }

    /// Downloads a CAD item revision's stored bytes. The preview cache
    /// checks them against their SHA-256.
    pub fn stored_bytes(&self, cad: &StoredCad) -> Result<Vec<u8>, String> {
        self.blob_bytes(&cad.blob)
    }

    /// Downloads a 3D model revision's stored bytes. The preview cache
    /// checks them against their SHA-256.
    pub fn stored_model_bytes(&self, model: &StoredModel) -> Result<Vec<u8>, String> {
        self.blob_bytes(&model.blob)
    }

    /// The 3D models the footprint revision uses, by their links as the
    /// footprint writes them, once each record is checked. A link the
    /// Catalog does not bind to a stored model is left out. A model whose
    /// record fails its checks says why, finishing "... because ...".
    pub fn footprint_models(
        &self,
        footprint: &StoredFootprint,
    ) -> Result<Vec<FootprintModel>, TeamCadError> {
        let client = self.client().map_err(|_| TeamCadError::SignedOut)?;
        let record = self.cad_item_revision(&client, &footprint.cad_item, footprint.number)?;
        let named = NamedRevision {
            cad_item: &footprint.cad_item,
            number: footprint.number,
            snapshot_sha256: &footprint.snapshot_sha256,
        };
        let snapshot = checked_record(&record, &named, CadItemKind::Footprint, "kicad-footprint")
            .map_err(|why| TeamCadError::Mismatch {
            reference: footprint.cad_item.clone(),
            why,
        })?;
        let mut models = Vec::new();
        for model in snapshot.model_references {
            let ModelTarget::CadItemRevision {
                cad_item,
                number,
                snapshot_sha256,
            } = &model.target
            else {
                continue;
            };
            let checked = match self.cad_item_revision(&client, cad_item, *number) {
                Ok(record) => {
                    let named = NamedRevision {
                        cad_item,
                        number: *number,
                        snapshot_sha256,
                    };
                    checked_record(&record, &named, CadItemKind::Model3d, "3d-model")
                        .map(|details| StoredModel {
                            key: details.key,
                            blob_sha256: details.blob_sha256,
                            blob: record.blob,
                        })
                        .map_err(StoredModelError::Check)
                }
                Err(TeamCadError::Unreachable(why) | TeamCadError::NotFound(why)) => {
                    Err(StoredModelError::Unavailable(why))
                }
                Err(TeamCadError::SignedOut) => {
                    Err(StoredModelError::Unavailable("you are signed out".into()))
                }
                Err(TeamCadError::Mismatch { why, .. }) => Err(StoredModelError::Check(why)),
            };
            models.push((model.reference, checked));
        }
        Ok(models)
    }

    fn blob_bytes(&self, blob: &str) -> Result<Vec<u8>, String> {
        let client = self.client()?;
        let fail = |error: CollabError| plain_reason(&error);
        let blob: StoredBlob = client
            .get(&format!("/api/collections/catalog_blobs/records/{blob}"))
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

    /// The signed-in client, for updating the KiCad libraries.
    pub(crate) fn client_for_libraries(&self) -> Result<PocketBaseClient, String> {
        self.client()
    }

    fn client(&self) -> Result<PocketBaseClient, String> {
        self.lock()
            .session
            .as_ref()
            .map(|session| session.client())
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
    let (expected_kind, expected_format) = match kind {
        PreviewKind::Symbol => (CadItemKind::Symbol, "kicad-symbol-entry"),
        PreviewKind::Footprint => (CadItemKind::Footprint, "kicad-footprint"),
    };
    let snapshot = checked_record(revision, named, expected_kind, expected_format)?;
    if snapshot.key != reference {
        return Err("its record is for another reference".into());
    }
    Ok(snapshot)
}

/// The details of a CAD item revision record, once it is shown to be the
/// named revision: its snapshot hashes to the named SHA-256, it is the
/// named revision of the named CAD item, and it is a `kind` in `format`.
/// Otherwise says why not, finishing the sentence "... because ...".
fn checked_record(
    revision: &StoredCadItemRevision,
    named: &NamedRevision<'_>,
    expected_kind: CadItemKind,
    expected_format: &str,
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
    if snapshot.kind != expected_kind || snapshot.format != expected_format {
        return Err(format!(
            "its record is a {} in the {} format, not a {}",
            snapshot.kind.as_str(),
            snapshot.format,
            expected_kind.as_str()
        ));
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

/// What to say when every change an edit makes is already saved.
const ALREADY_SAVED: &str = "These changes are already saved, so nothing more was saved.";

/// What to say when a save could not reach the team server. It does not
/// claim that nothing was saved: the answer to a request that arrived may
/// be what was lost. Saving again sends the same correlation ID, so the
/// server replays its answer instead of saving twice.
const SAVE_UNREACHABLE: &str =
    "Could not reach the team server. Check that it is running, then save again.";

/// What to say when a save worked but the part could not be read again.
/// The app catches up by itself once live updates reconnect.
const SAVED_NOT_SHOWN: &str = "Saved. The app could not reach the team server to show the latest version; it will update when the server is back.";

/// What to say when someone else saved first and their change could not be
/// read. Nothing was saved, so saving again is safe.
const CONFLICT_NOT_SHOWN: &str = "Someone else changed this part first, so nothing was saved. The app could not reach the team server to show their change. Check that it is running, then save again.";

/// A save that failed before the server answered it.
fn save_failed(error: CollabError) -> Result<PublishResult, String> {
    match error {
        CollabError::Transport(_) => Ok(PublishResult::Refused {
            message: SAVE_UNREACHABLE.into(),
        }),
        other => Err(other.to_string()),
    }
}

/// The edit as the person meant it. Spaces typed or pasted around a value
/// they changed are left out, so adding only spaces changes nothing and
/// an MPN pasted with a space after it is still the same MPN. A value they
/// did not change stays exactly as it is, spaces and all. Manufacturer
/// parts are tidied row by row: a row exactly as it was stays as it is, a
/// changed row has its manufacturer and MPN trimmed, and a row left blank
/// is left out.
fn tidied(
    mut request: TeamEditRequest,
    details: &PartDetails,
    revision: &PartRevision,
) -> TeamEditRequest {
    let tidy = |fields: &mut Vec<Field>, before: &[DetailField]| {
        for field in fields {
            let unchanged = before
                .iter()
                .any(|old| old.name == field.name && old.value == field.value);
            if !unchanged {
                field.value = field.value.trim().to_owned();
            }
        }
    };
    tidy(&mut request.fields, &details.fields);
    tidy(&mut request.cad_fields, &revision.cad_fields);
    request.manufacturer_parts = request
        .manufacturer_parts
        .into_iter()
        .filter_map(|part| {
            let unchanged = details
                .manufacturer_parts
                .iter()
                .any(|old| old.manufacturer == part.manufacturer && old.mpn == part.mpn);
            if unchanged {
                return Some(part);
            }
            let manufacturer = part.manufacturer.trim();
            let mpn = part.mpn.trim();
            (!manufacturer.is_empty() || !mpn.is_empty()).then(|| ManufacturerPart {
                manufacturer: manufacturer.to_owned(),
                mpn: mpn.to_owned(),
            })
        })
        .collect();
    request
}

/// The correlation ID the UI made for this save attempt, so that retrying
/// the same save replays its answer; a new one if the UI sent none.
fn attempt_id(sent: Option<&str>) -> Result<CorrelationId, String> {
    match sent {
        Some(id) => CorrelationId::new(id).map_err(|error| error.to_string()),
        None => Ok(CorrelationId::generate()),
    }
}

/// A chosen file's bytes from the base64 the UI sends.
pub(crate) fn decoded(base64: &str, file_name: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(base64)
        .map_err(|_| format!("The file {file_name} could not be read."))
}

/// The symbols and footprints `cad_fields` names that `revision` does not
/// and that no upload is for, by kind and reference.
pub(crate) fn newly_named(
    cad_fields: &[Field],
    revision: &PartRevision,
    uploads: &[CadUpload],
) -> Vec<(CadItemKind, String)> {
    let mut named = Vec::new();
    for (kind, role, column) in [
        (CadItemKind::Symbol, UploadRole::Symbol, "Symbol"),
        (CadItemKind::Footprint, UploadRole::Footprint, "Footprint"),
    ] {
        let now = cad_fields
            .iter()
            .find(|field| field.name == column)
            .map(|field| field.value.clone())
            .unwrap_or_default();
        let before = revision.field(column).unwrap_or_default();
        let before = footprint_choices(before);
        for reference in footprint_choices(&now) {
            let sent = uploads
                .iter()
                .any(|upload| upload.role == role && upload.reference == reference);
            if !sent && !before.contains(&reference) {
                named.push((kind, reference.to_owned()));
            }
        }
    }
    named
}

/// The uploads for the files of an edit: a symbol or footprint file is for
/// the reference its field names (a footprint's first choice), and is read
/// the way the import reads the CSV catalog's files.
fn uploads_of(files: &[TeamFile], cad_fields: &[Field]) -> Result<Vec<CadUpload>, String> {
    let field = |name: &str| {
        cad_fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.trim().to_owned())
            .unwrap_or_default()
    };
    // A footprint copied from an installed library goes to the choice named
    // like it (kicad-parts-bc6f); any other footprint file to the first.
    for kind in [FileKind::Symbol, FileKind::Footprint] {
        let count = files
            .iter()
            .filter(|file| {
                file.kind == kind && (kind == FileKind::Symbol || file.installed.is_none())
            })
            .count();
        if count > 1 {
            return Err("Choose one file for each of the symbol and the footprint.".into());
        }
    }
    let footprint = field("Footprint");
    let choices: Vec<String> = footprint_choices(&footprint)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut uploads = crate::add_part::copied_footprints(files, &choices)?;
    for file in files {
        // Copies of an installed library's footprints and their 3D models
        // are sent above.
        if file.kind != FileKind::Symbol && file.installed.is_some() {
            continue;
        }
        let bytes = decoded(&file.base64, &file.file_name)?;
        let source = file.label.clone().unwrap_or_else(|| file.file_name.clone());
        let upload = match file.kind {
            FileKind::Symbol => {
                let reference = field("Symbol");
                let name = file
                    .symbol
                    .as_deref()
                    .ok_or_else(|| "Choose which symbol of the file to use.".to_owned())?;
                let mut upload =
                    catalog::cad_file::symbol_upload(&bytes, name, &reference, &source)?;
                upload.installed = crate::add_part::installed_source(file);
                upload
            }
            FileKind::Footprint => {
                let footprint = field("Footprint");
                let reference = footprint_choices(&footprint)
                    .first()
                    .map(|choice| (*choice).to_owned())
                    .unwrap_or_default();
                catalog::cad_file::footprint_upload(&bytes, &reference, &file.file_name)?
            }
            FileKind::Model3d => {
                let key = file.model_key.as_deref().ok_or_else(|| {
                    format!("Say which 3D model of the footprint {} is.", file.file_name)
                })?;
                catalog::cad_file::model_upload(&bytes, key, &file.file_name)?
            }
        };
        uploads.push(upload);
    }
    Ok(uploads)
}

/// Why a footprint file being saved cannot be, by the library check's rule
/// for 3D model links (kicad-parts-da65), or `None`: each link must stay in
/// the Catalog's library, and a 3D model it links there must be in the
/// Catalog or sent with it. Nothing is sent before this says so.
fn model_link_refusal(
    client: &PocketBaseClient,
    uploads: &[CadUpload],
) -> Result<Option<String>, String> {
    let sent: BTreeSet<&str> = uploads
        .iter()
        .filter(|upload| upload.role == UploadRole::Model3d)
        .map(|upload| upload.reference.as_str())
        .collect();
    let mut problems = Vec::new();
    for upload in uploads
        .iter()
        .filter(|upload| upload.role == UploadRole::Footprint)
    {
        if catalog::cad_file::read_footprint_file(&upload.bytes).is_err() {
            continue;
        }
        let (library, name) = upload.reference.split_once(':').unwrap_or_default();
        // KiCad 5 paths written without quotes count too.
        let links = catalog::download::model_links(&String::from_utf8_lossy(&upload.bytes));
        // The Catalog's 3D models the links name and the edit does not send.
        let mut in_catalog = BTreeSet::new();
        for link in &links {
            let judged = judge_model_link(library, name, link);
            let Some(path) = judged.path().filter(|path| !sent.contains(path)) else {
                continue;
            };
            let known = has_cad_item(client, CadItemKind::Model3d, path).map_err(|error| {
                format!(
                    "The team catalog could not be checked because {}.",
                    plain_reason(&error)
                )
            })?;
            if known {
                in_catalog.insert(path.to_owned());
            }
        }
        problems.extend(chosen_footprint_problems(library, name, &links, |path| {
            sent.contains(path) || in_catalog.contains(path)
        }));
    }
    Ok((!problems.is_empty()).then(|| format!("{} Nothing was saved.", problems.join(" "))))
}

/// A part as the app shows it, with every saved details and revision, which
/// edits and conflicts compare with.
struct LoadedPart {
    view: TeamPart,
    /// By number.
    details: BTreeMap<u32, PartDetails>,
    /// By number.
    revisions: BTreeMap<u32, PartRevision>,
    /// Every category's current revision.
    categories: Vec<CategoryDetails>,
}

/// The plain name of each category, by code.
type CategoryNames = BTreeMap<String, String>;

fn category_names(categories: &[CategoryDetails]) -> CategoryNames {
    categories
        .iter()
        .map(|category| (category.code.clone(), category.name.clone()))
        .collect()
}

/// A category's plain name, or its code when the Catalog has no such
/// category.
fn category_name(names: &CategoryNames, code: &str) -> String {
    names.get(code).cloned().unwrap_or_else(|| code.to_owned())
}

fn load_part(client: &PocketBaseClient, part: &StoredPart) -> Result<TeamPart, String> {
    load(client, part)
        .map(|loaded| loaded.view)
        .map_err(String::from)
}

/// Why reading a part from the team server failed: it could not be
/// reached, or something else, said in words.
enum ReadFailure {
    Unreachable,
    Other(String),
}

impl From<CollabError> for ReadFailure {
    fn from(error: CollabError) -> Self {
        match error {
            CollabError::Transport(_) => Self::Unreachable,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<String> for ReadFailure {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<ReadFailure> for String {
    fn from(failure: ReadFailure) -> Self {
        match failure {
            ReadFailure::Unreachable => "Could not reach the team server.".into(),
            ReadFailure::Other(message) => message,
        }
    }
}

/// `ipn` as it is now, read again after a save.
fn reload(client: &PocketBaseClient, ipn: &str) -> Result<LoadedPart, ReadFailure> {
    let part = part_by_ipn(client, ipn)?
        .ok_or_else(|| format!("{ipn} is no longer in the team catalog"))?;
    load(client, &part)
}

/// What a save that worked shows: `saved` with the part as it is now, or,
/// if the part could not be read again for any reason, that the save
/// worked anyway (it is done either way).
fn saved_then(
    reloaded: Result<LoadedPart, ReadFailure>,
    saved: impl FnOnce(TeamPart) -> PublishResult,
) -> Result<PublishResult, String> {
    Ok(match reloaded {
        Ok(loaded) => saved(loaded.view),
        Err(_) => PublishResult::SavedNotShown {
            message: SAVED_NOT_SHOWN.into(),
        },
    })
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

fn load(client: &PocketBaseClient, part: &StoredPart) -> Result<LoadedPart, ReadFailure> {
    let fail = ReadFailure::from;
    let saved_details = part_details_of(client, &part.id).map_err(fail)?;
    let revisions = part_revisions(client, &part.id).map_err(fail)?;
    let archive_entries = archive_entries_of(client, &part.id).map_err(fail)?;
    let reviews = csv_reviews_of(client, &part.id).map_err(fail)?;
    let categories = current_categories(client).map_err(fail)?;
    let names = category_names(&categories);
    let change_ids: Vec<&str> = saved_details
        .iter()
        .map(|d| d.change.as_str())
        .chain(revisions.iter().map(|r| r.change.as_str()))
        .chain(archive_entries.iter().map(|a| a.change.as_str()))
        .chain(reviews.iter().map(|r| r.change.as_str()))
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
            .map(|previous| to_changes(compare_details(previous, &details[&saved.number]), &names))
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
                to_changes(
                    compare_revisions(previous, &cad[&revision.number], describe),
                    &names,
                )
            })
            .unwrap_or_default();
        if let Some(change) = change_records.get(&revision.change) {
            let entry = history_entry(&mut entries, change);
            entry.revision = Some(revision.number);
            entry.changes.extend(changes);
        }
    }
    // The CAD item revisions an edit's files made, each with the file it
    // came from.
    let edits: Vec<&str> = revisions
        .iter()
        .map(|revision| revision.change.as_str())
        .filter(|change| {
            change_records
                .get(*change)
                .is_some_and(|c| c.kind == "edit" || c.kind == "create")
        })
        .collect();
    if !edits.is_empty() {
        let made = cad_item_revisions_in_changes(client, &edits).map_err(fail)?;
        // The revision each one followed, to say what file it replaced.
        let before_ids: Vec<&str> = made
            .iter()
            .map(|revision| revision.predecessor.as_str())
            .filter(|id| !id.is_empty())
            .collect();
        let before = if before_ids.is_empty() {
            Vec::new()
        } else {
            cad_item_revisions_by_id(client, &before_ids).map_err(fail)?
        };
        for revision in &made {
            let replaced = before.iter().find(|old| old.id == revision.predecessor);
            let (Some(change), Some(row)) = (
                change_records.get(&revision.change),
                file_row(revision, replaced),
            ) else {
                continue;
            };
            history_entry(&mut entries, change).changes.push(row);
        }
    }
    for entry in &archive_entries {
        if let Some(change) = change_records.get(&entry.change) {
            history_entry(&mut entries, change).archive = Some(archive_note(entry));
        }
    }
    // Where a part added in the app came from.
    for change in change_records
        .values()
        .filter(|change| change.kind == "create")
    {
        if let Some(row) = downloaded_from(change) {
            history_entry(&mut entries, change).changes.insert(0, row);
        }
    }
    for review in &reviews {
        if let Some(change) = change_records.get(&review.change) {
            let took = serde_json::from_str::<Vec<String>>(&review.accepted)
                .is_ok_and(|accepted| !accepted.is_empty());
            history_entry(&mut entries, change).action = match (took, review.code.as_str()) {
                (true, _) => ACCEPTED,
                (false, CSV_CAD_DIFFERS) => "Kept current symbol and footprint",
                (false, _) => "Kept current part details",
            }
            .to_owned();
        }
    }
    // What the CSV said the last time an import applied to the part, for
    // each kind: the newest details and the newest revision a CSV import
    // saved, with the seq of the Change that saved them.
    let imported = |change: &str| {
        change_records
            .get(change)
            .filter(|change| change.kind == "csv-import")
            .map(|change| change.seq)
    };
    let last_imported = LastImported {
        details: saved_details
            .iter()
            .filter_map(|saved| Some((imported(&saved.change)?, saved.snapshot.as_str())))
            .max_by_key(|(seq, _)| *seq),
        revision: revisions
            .iter()
            .filter_map(|revision| Some((imported(&revision.change)?, revision.snapshot.as_str())))
            .max_by_key(|(seq, _)| *seq),
    };
    // Every import problem the part view shows, read in one request.
    let problems = problems_of(
        client,
        &part.ipn,
        &[
            CSV_DETAILS_DIFFER,
            CSV_CAD_DIFFERS,
            CSV_CAD_ITEM_KEPT,
            IPN_GIVEN_TO_ANOTHER_PART,
        ],
    )
    .map_err(|error| error.to_string())?;
    let mut runs = LatestRuns::new(client);
    let csv_rows_for_other_parts = rows_for_other_parts(&problems, &mut runs)?;
    // A CSV file whose row with this IPN may be another part says nothing
    // sure about this part, so its values are never offered for review.
    let other_part_files: BTreeSet<&str> = csv_rows_for_other_parts
        .iter()
        .map(|row| row.csv_file.as_str())
        .collect();
    let csv_changes = open_csv_changes(
        &problems,
        part,
        current_details,
        current_revision,
        &reviews,
        &last_imported,
        &names,
        describe,
        &mut runs,
        &other_part_files,
    )?;
    let archived = archived_numbers(&archive_entries.iter().collect::<Vec<_>>());
    let view = TeamPart {
        ipn: part.ipn.clone(),
        details: current_details_view(part.current_details_number, current_details, &names),
        revision: current_revision_view(
            part.current_revision_number,
            current_revision,
            &bom_marks(client, current_revision),
        ),
        revisions: revisions
            .iter()
            .map(|revision| RevisionStatus {
                number: revision.number,
                archived: archived.contains(&revision.number),
            })
            .collect(),
        history: entries.into_values().rev().collect(),
        csv_changes,
        csv_rows_for_other_parts,
        categories: categories
            .iter()
            .map(|category| TeamCategory {
                code: category.code.clone(),
                name: category.name.clone(),
                fields: category.fields.clone(),
            })
            .collect(),
    };
    Ok(LoadedPart {
        view,
        details,
        revisions: cad,
        categories,
    })
}

/// The history row of a new part's download: the site and the files, such
/// as `SnapEDA: LM317.zip`.
fn downloaded_from(change: &StoredChange) -> Option<DetailChange> {
    let sources: serde_json::Value = serde_json::from_str(&change.sources).ok()?;
    let from = sources["from"].as_str().unwrap_or_default();
    let files: Vec<&str> = sources["files"]
        .as_array()
        .map(|files| files.iter().filter_map(|file| file.as_str()).collect())
        .unwrap_or_default();
    let after = match (from.is_empty(), files.is_empty()) {
        (true, true) => return None,
        (false, true) => from.to_owned(),
        (true, false) => files.join(", "),
        (false, false) => format!("{from}: {}", files.join(", ")),
    };
    Some(DetailChange {
        // A part from our libraries, KiCad's or an installed library was
        // not downloaded.
        label: if from == crate::library_part::FROM_OUR_LIBRARIES
            || from == crate::pick::FROM_KICAD
            || from.starts_with(&crate::pick::from_installed(""))
        {
            "Added from"
        } else {
            "Downloaded from"
        }
        .into(),
        before: String::new(),
        after,
    })
}

/// The history action of a review that accepted CSV values.
const ACCEPTED: &str = "Accepted CSV values";

/// The history row of a CAD item revision made from a file, such as
/// `Footprint file`, from the file of the revision it `replaced` (blank for
/// a new CAD item) to `R_0603.kicad_mod (footprint revision 2 of ...)`;
/// `None` for one that came from anywhere else.
fn file_row(
    made: &StoredCadItemRevision,
    replaced: Option<&StoredCadItemRevision>,
) -> Option<DetailChange> {
    let snapshot: CadItemDetails = serde_json::from_str(&made.snapshot).ok()?;
    let (label, what) = match snapshot.kind {
        CadItemKind::Symbol => ("Symbol file", "symbol"),
        CadItemKind::Footprint => ("Footprint file", "footprint"),
        CadItemKind::Model3d => ("3D model file", "3D model"),
    };
    // A file chosen in the app keeps its name; one an import read, its
    // path in the CSV catalog.
    let file = |revision: &StoredCadItemRevision| -> Option<String> {
        let source: serde_json::Value = serde_json::from_str(&revision.source).ok()?;
        let name = source
            .get("file_name")
            .or_else(|| source.get("path"))?
            .as_str()?;
        Some(format!(
            "{name} ({what} revision {} of {})",
            revision.number, snapshot.key
        ))
    };
    let source: serde_json::Value = serde_json::from_str(&made.source).ok()?;
    source.get("file_name")?;
    Some(DetailChange {
        label: label.to_owned(),
        before: replaced.and_then(file).unwrap_or_default(),
        after: file(made)?,
    })
}

/// The snapshots of a part's newest details and newest revision that a CSV
/// import saved, with the seq of the Change that saved each: what the CSV
/// said the last time an import applied to it.
struct LastImported<'a> {
    details: Option<(u64, &'a str)>,
    revision: Option<(u64, &'a str)>,
}

/// What the CSV said the last time the app applied or reviewed it, for one
/// kind: the SHA-256 of that CSV document, and the document if it can be
/// read.
struct LastSeen {
    sha256: String,
    document: Option<String>,
}

/// Which import problems the latest import run of their CSV file found, so
/// what the CSV says now. Problems that only older runs found stay recorded
/// for the zero-loss report, but the CSV has moved on since. Each CSV
/// file's latest run is read once.
struct LatestRuns<'c> {
    client: &'c PocketBaseClient,
    /// The problem IDs each CSV file's latest run found, such as `g-res`.
    found: BTreeMap<String, Vec<String>>,
}

impl<'c> LatestRuns<'c> {
    fn new(client: &'c PocketBaseClient) -> Self {
        Self {
            client,
            found: BTreeMap::new(),
        }
    }

    /// Whether the latest run of the CSV file `problem` is about found it.
    fn found_now(&mut self, problem: &StoredImportProblem) -> Result<bool, String> {
        let Some(csv_file) = csv_file_of(&problem.path) else {
            return Ok(false);
        };
        if !self.found.contains_key(csv_file) {
            let found = latest_import_run(self.client, csv_file)
                .map_err(|error| error.to_string())?
                .and_then(|entry| entry.import_run())
                .map(|run| run.problems)
                .unwrap_or_default();
            self.found.insert(csv_file.to_owned(), found);
        }
        Ok(self.found[csv_file].contains(&problem.id))
    }
}

/// The CSV file a repository path names, such as `g-res` for
/// `database/g-res.csv`.
fn csv_file_of(path: &str) -> Option<&str> {
    path.strip_prefix("database/")?.strip_suffix(".csv")
}

/// The CSV rows with the part's IPN that may be a different part
/// ([`IPN_GIVEN_TO_ANOTHER_PART`] among the part's `problems`), as the
/// latest import of their CSV file found them, in file order. The next free
/// IPN is left blank: only the desktop sees the CSV catalog folder, so
/// `CatalogBrowser::team_part` fills it in. A problem whose detail cannot be
/// read is skipped, so the part still opens.
fn rows_for_other_parts(
    problems: &[StoredImportProblem],
    runs: &mut LatestRuns,
) -> Result<Vec<CsvRowForAnotherPart>, String> {
    let mut rows = Vec::new();
    for problem in problems {
        if problem.code != IPN_GIVEN_TO_ANOTHER_PART || csv_file_of(&problem.path).is_none() {
            continue;
        }
        if !runs.found_now(problem)? {
            continue;
        }
        let Ok(detail) = serde_json::from_str::<serde_json::Value>(&problem.detail) else {
            continue;
        };
        let mpns = detail["csv_manufacturer_parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|made| made["mpn"].as_str())
            .map(str::trim)
            .filter(|mpn| !mpn.is_empty())
            .map(str::to_owned)
            .collect();
        rows.push(CsvRowForAnotherPart {
            csv_file: problem.path.clone(),
            mpns,
            next_free_ipn: String::new(),
        });
    }
    rows.sort_by(|a, b| a.csv_file.cmp(&b.csv_file));
    Ok(rows)
}

/// The new CSV values nobody has reviewed yet: each [`CSV_DETAILS_DIFFER`]
/// or [`CSV_CAD_DIFFERS`] import problem of the part that the latest import
/// run of its CSV file found, unless a review already covers what the CSV
/// says (as the server's no-op does), and only if a field still differs.
///
/// The app asks only when the CSV changed since it was last applied or
/// reviewed: for each kind, a problem whose recorded CSV document is the
/// newer (by seq) of what the last import saved and what the last review
/// covered is left out, because only the part was edited in this app. The
/// problem itself stays recorded for the zero-loss report. Each row says
/// whether the CSV changed that field since then.
///
/// Nothing is offered from a CSV file in `other_part_files` (such as
/// `database/g-res.csv`): its row with this IPN may be another part.
///
/// A recorded document that cannot be read is skipped, so the part still
/// opens.
#[allow(clippy::too_many_arguments)]
fn open_csv_changes(
    problems: &[StoredImportProblem],
    part: &StoredPart,
    details: &PartDetails,
    revision: &PartRevision,
    reviews: &[StoredCsvReview],
    last_imported: &LastImported,
    names: &CategoryNames,
    describe: impl Fn(&CadReference) -> String,
    runs: &mut LatestRuns,
    other_part_files: &BTreeSet<&str>,
) -> Result<Vec<CsvChanges>, String> {
    let last_seen = |cad: bool| -> Option<LastSeen> {
        let code = if cad {
            CSV_CAD_DIFFERS
        } else {
            CSV_DETAILS_DIFFER
        };
        let imported = if cad {
            last_imported.revision
        } else {
            last_imported.details
        };
        let reviewed = reviews
            .iter()
            .filter(|review| review.code == code)
            .max_by_key(|review| review.seq);
        match (imported, reviewed) {
            (Some((seq, _)), Some(review)) if review.seq > seq => Some(LastSeen {
                sha256: review.csv_sha256.clone(),
                document: problems
                    .iter()
                    .find(|problem| problem.id == review.problem)
                    .and_then(recorded_csv_document),
            }),
            (None, Some(review)) => Some(LastSeen {
                sha256: review.csv_sha256.clone(),
                document: problems
                    .iter()
                    .find(|problem| problem.id == review.problem)
                    .and_then(recorded_csv_document),
            }),
            (Some((_, snapshot)), _) => Some(LastSeen {
                sha256: catalog::sha256_hex(snapshot),
                document: Some(snapshot.to_owned()),
            }),
            (None, None) => None,
        }
    };
    let seen = [last_seen(false), last_seen(true)];
    let mut open: Vec<(String, CsvChanges)> = Vec::new();
    for problem in problems {
        // Only the problems a person can review have a CSV document.
        let Some(document) = recorded_csv_document(problem) else {
            continue;
        };
        // A CSV file for a CAD item made in the app: the import applied
        // nothing of it, so there is no "last seen" to compare with; only a
        // review covers it.
        let kept_cad_item = problem.code == CSV_CAD_ITEM_KEPT;
        let cad = problem.code == CSV_CAD_DIFFERS || kept_cad_item;
        let csv_sha256 = catalog::sha256_hex(&document);
        let last = seen[usize::from(cad)].as_ref().filter(|_| !kept_cad_item);
        if last.is_some_and(|last| last.sha256 == csv_sha256) {
            continue;
        }
        let last_document = last.and_then(|last| last.document.as_deref());
        if reviews
            .iter()
            .any(|review| review.code == problem.code && review.csv_sha256 == csv_sha256)
        {
            continue;
        }
        let Some(csv_file) = csv_file_of(&problem.path) else {
            continue;
        };
        if other_part_files.contains(problem.path.as_str()) {
            continue;
        }
        if !runs.found_now(problem)? {
            continue;
        }
        let mut rows = if kept_cad_item {
            let Some(row) = kept_cad_item_row(problem, revision, &describe) else {
                continue;
            };
            vec![row]
        } else if cad {
            let Ok(csv) = serde_json::from_str::<PartRevision>(&document) else {
                continue;
            };
            let before = last_document.and_then(|text| serde_json::from_str(text).ok());
            cad_rows(revision, &csv, before.as_ref(), &describe)
        } else {
            let Ok(csv) = serde_json::from_str::<PartDetails>(&document) else {
                continue;
            };
            let before = last_document.and_then(|text| serde_json::from_str(text).ok());
            details_rows(details, &csv, before.as_ref(), names)
        };
        // Values the CSV changed first; the rest keep their order.
        rows.sort_by_key(|row| !row.changed_in_csv);
        if rows.is_empty() {
            continue;
        }
        open.push((
            problem.code.clone(),
            CsvChanges {
                problem: problem.id.clone(),
                csv_file: format!("{csv_file}.csv"),
                cad,
                expected: if cad {
                    part.current_revision_number
                } else {
                    part.current_details_number
                },
                rows,
            },
        ));
    }
    Ok(in_order(open))
}

/// Open CSV changes, details first, each with its import problem's code.
/// One import run finds at most one problem of each of the first two kinds,
/// so a second one of those is left out; a CAD item made in the app gives
/// one problem per symbol or footprint choice, so each of those shows.
fn in_order(mut open: Vec<(String, CsvChanges)>) -> Vec<CsvChanges> {
    open.sort_by(|(a_code, a), (b_code, b)| {
        (a.cad, a_code, &a.rows[0].field, &a.rows[0].now).cmp(&(
            b.cad,
            b_code,
            &b.rows[0].field,
            &b.rows[0].now,
        ))
    });
    open.dedup_by(|(a_code, a), (b_code, b)| {
        a_code == b_code && (a_code != CSV_CAD_ITEM_KEPT || a.problem == b.problem)
    });
    open.into_iter().map(|(_, changes)| changes).collect()
}

/// The row of a [`CSV_CAD_ITEM_KEPT`] problem: the Symbol or Footprint, the
/// exact revision the part uses now, and that the CSV has a changed file for
/// it, which can only be kept.
fn kept_cad_item_row(
    problem: &catalog::pocketbase::StoredImportProblem,
    revision: &PartRevision,
    describe: &impl Fn(&CadReference) -> String,
) -> Option<CsvChangeRow> {
    let detail: serde_json::Value = serde_json::from_str(&problem.detail).ok()?;
    let reference = detail.get("reference")?.as_str()?;
    let (field, role, what) = match detail.get("role")?.as_str()? {
        "symbol" => ("Symbol", CadReferenceRole::Symbol, "symbol"),
        _ => ("Footprint", CadReferenceRole::Footprint, "footprint"),
    };
    let now = revision
        .cad_references
        .iter()
        .find(|item| item.role == role && item.reference == reference)
        .map_or_else(|| reference.to_owned(), describe);
    Some(CsvChangeRow {
        field: field.to_owned(),
        now,
        csv: format!(
            "A changed file for {reference}. It cannot be accepted here, because this {what} was changed in this app."
        ),
        changed_in_csv: true,
        keep_only: true,
    })
}

/// The details fields both have whose values differ, in the part's order,
/// then the manufacturer parts if they differ. A row is changed in the CSV
/// unless `before` (what the CSV said when last applied or reviewed) has the
/// same value; with no `before`, every row counts as changed.
fn details_rows(
    now: &PartDetails,
    csv: &PartDetails,
    before: Option<&PartDetails>,
    names: &CategoryNames,
) -> Vec<CsvChangeRow> {
    let mut rows = Vec::new();
    // The CSV file the row is in makes its category, so a row moved to
    // another file has another category.
    if now.category != csv.category {
        rows.push(CsvChangeRow {
            field: CATEGORY_LABEL.to_owned(),
            now: category_name(names, &now.category),
            csv: category_name(names, &csv.category),
            changed_in_csv: before.is_none_or(|before| before.category != csv.category),
            keep_only: false,
        });
    }
    rows.extend(now.fields.iter().filter_map(|field| {
        let theirs = csv.field(&field.name)?;
        (theirs != field.value).then(|| CsvChangeRow {
            field: field.name.clone(),
            now: field.value.clone(),
            csv: theirs.to_owned(),
            changed_in_csv: before.is_none_or(|before| before.field(&field.name) != Some(theirs)),
            keep_only: false,
        })
    }));
    if now.manufacturer_parts != csv.manufacturer_parts {
        rows.push(CsvChangeRow {
            field: MANUFACTURER_PARTS_FIELD.to_owned(),
            now: stored_lines(now),
            csv: stored_lines(csv),
            changed_in_csv: before
                .is_none_or(|before| before.manufacturer_parts != csv.manufacturer_parts),
            keep_only: false,
        });
    }
    rows
}

/// Symbol and Footprint, where the field or the exact CAD it uses differs.
/// When only the exact CAD differs, each side lists its references.
fn cad_rows(
    now: &PartRevision,
    csv: &PartRevision,
    before: Option<&PartRevision>,
    describe: &impl Fn(&CadReference) -> String,
) -> Vec<CsvChangeRow> {
    let mut rows = Vec::new();
    for (name, role) in [
        ("Symbol", CadReferenceRole::Symbol),
        ("Footprint", CadReferenceRole::Footprint),
    ] {
        let (Some(ours), Some(theirs)) = (now.field(name), csv.field(name)) else {
            continue;
        };
        let references = |revision: &PartRevision| -> Vec<CadReference> {
            revision
                .cad_references
                .iter()
                .filter(|reference| reference.role == role)
                .cloned()
                .collect()
        };
        let (our_references, their_references) = (references(now), references(csv));
        if ours == theirs && our_references == their_references {
            continue;
        }
        let lines = |references: &[CadReference]| {
            references
                .iter()
                .map(describe)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let (now_text, csv_text) = if ours == theirs {
            (lines(&our_references), lines(&their_references))
        } else {
            (ours.to_owned(), theirs.to_owned())
        };
        let changed_in_csv = before.is_none_or(|before| {
            before.field(name) != Some(theirs) || references(before) != their_references
        });
        // A reference the CSV catalog cannot find, which the Catalog has
        // (such as a symbol added in this app): accepting it would lose it,
        // so it can only be kept.
        let lost = their_references.iter().find(|theirs| {
            matches!(theirs.target, CadReferenceTarget::Unresolved { .. })
                && our_references.iter().any(|ours| {
                    ours.reference == theirs.reference
                        && matches!(ours.target, CadReferenceTarget::CadItemRevision { .. })
                })
        });
        if let Some(lost) = lost {
            rows.push(CsvChangeRow {
                field: name.to_owned(),
                now: now_text,
                csv: format!(
                    "The CSV catalog does not have {}. It cannot be accepted here, because the Catalog has it.",
                    lost.reference
                ),
                changed_in_csv,
                keep_only: true,
            });
            continue;
        }
        rows.push(CsvChangeRow {
            field: name.to_owned(),
            now: now_text,
            csv: csv_text,
            changed_in_csv,
            keep_only: false,
        });
    }
    rows
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
        archive: None,
        action: action(&change.kind).to_owned(),
        actor_name: actor_name(change),
        saved_at: saved_at(&change.created),
        reason: change.reason.clone(),
        changes: Vec::new(),
    })
}

fn archive_note(entry: &StoredArchiveEntry) -> ArchiveNote {
    ArchiveNote {
        revision: entry.revision_number,
        archived: entry.archived,
        current_after: entry.current_revision_number_after,
    }
}

/// Says plainly why the server refused a category change because the new
/// category's fields changed while the person was editing (an import
/// changed them), instead of the server's "field 7 must be X".
fn plain_refusal(message: String, moved: bool) -> String {
    let fields_differ = message.contains("an edit cannot add, remove, rename or reorder fields")
        || message.contains("fields as now, in the same order");
    if moved && fields_differ {
        CATEGORY_FIELDS_CHANGED.to_owned()
    } else {
        message
    }
}

const CATEGORY_FIELDS_CHANGED: &str =
    "The category's fields changed while you were editing. Open Edit part again.";

/// History rows, with categories by their plain names.
fn to_changes(
    differences: Vec<catalog::DetailDifference>,
    names: &CategoryNames,
) -> Vec<DetailChange> {
    // A field a category change added blank needs no row of its own: the
    // category row says why it is there. Without a category change (a CSV
    // header change, say) every row is shown.
    let moved = differences
        .iter()
        .any(|difference| difference.what == DifferenceKind::Category);
    differences
        .into_iter()
        .filter(|difference| {
            !moved
                || difference.what != DifferenceKind::Field
                || difference.before != difference.after
        })
        .map(|difference| {
            let shown = |code: String| match difference.what {
                DifferenceKind::Category => category_name(names, &code),
                _ => code,
            };
            DetailChange {
                label: difference.label,
                before: shown(difference.before),
                after: shown(difference.after),
            }
        })
        .collect()
}

fn current_details_view(
    number: u32,
    details: &PartDetails,
    names: &CategoryNames,
) -> CurrentDetails {
    CurrentDetails {
        number,
        category: details.category.clone(),
        category_name: category_name(names, &details.category),
        fields: fields_of(&details.fields),
        manufacturer_parts: manufacturer_parts_of(details),
    }
}

fn current_revision_view(
    number: u32,
    revision: &PartRevision,
    marks: &[(CadReferenceRole, BomMark)],
) -> CurrentRevision {
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
        excluded_from_bom: excluded_from_bom_words(revision.excluded_from_bom, marks),
    }
}

/// What one symbol or footprint reference of a revision says about the
/// BOM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BomMark {
    /// True if it says it is not in the BOM: read from its stored bytes, or
    /// certain from the revision's own setting.
    Says(bool),
    /// From KiCad's own library, which the app does not read yet.
    KiCadLibrary,
    /// The Catalog does not have it.
    Missing,
    /// Stored, but its bytes could not be fetched or were not needed.
    Unread,
}

/// What each symbol and footprint reference of `revision` says about the
/// BOM. The server set the revision's setting from these same marks, so
/// most follow from it without reading anything: a revision that is not
/// excluded has no stored symbol that says so, nor a single stored
/// footprint that does. Stored bytes are read only for the symbols of an
/// excluded revision (to say which reason applies) and for footprint
/// choices of a revision that is not (to tell disagreeing choices apart).
fn bom_marks(
    client: &PocketBaseClient,
    revision: &PartRevision,
) -> Vec<(CadReferenceRole, BomMark)> {
    let footprints = revision
        .cad_references
        .iter()
        .filter(|reference| reference.role == CadReferenceRole::Footprint)
        .count();
    revision
        .cad_references
        .iter()
        .map(|reference| {
            let read = |cad_item: &str, number: u32| {
                stored_cad_bytes(client, cad_item, number).map_or(BomMark::Unread, |bytes| {
                    BomMark::Says(match reference.role {
                        CadReferenceRole::Symbol => symbol_entries_say_not_in_bom(&bytes),
                        CadReferenceRole::Footprint => footprint_says_excluded_from_bom(&bytes),
                    })
                })
            };
            let mark = match (
                &reference.target,
                reference.role,
                revision.excluded_from_bom,
            ) {
                (CadReferenceTarget::External, ..) => BomMark::KiCadLibrary,
                (CadReferenceTarget::Unresolved { .. }, ..) => BomMark::Missing,
                (
                    CadReferenceTarget::CadItemRevision {
                        cad_item, number, ..
                    },
                    role,
                    excluded,
                ) => match (role, excluded) {
                    (CadReferenceRole::Symbol, true) => read(cad_item, *number),
                    (CadReferenceRole::Symbol, false) => BomMark::Says(false),
                    (CadReferenceRole::Footprint, true) => BomMark::Unread,
                    (CadReferenceRole::Footprint, false) if footprints == 1 => BomMark::Says(false),
                    (CadReferenceRole::Footprint, false) => read(cad_item, *number),
                },
            };
            (reference.role, mark)
        })
        .collect()
}

/// The stored bytes of revision `number` of `cad_item`, if they can be
/// fetched.
fn stored_cad_bytes(client: &PocketBaseClient, cad_item: &str, number: u32) -> Option<Vec<u8>> {
    let id = CadItemRevisionId::new(&CadItemId::from_stored(cad_item), number);
    let revision: StoredCadItemRevision = client
        .first_record(
            "catalog_cad_item_revisions",
            &format!("id = {}", serde_json::Value::from(id.as_str())),
            "number",
        )
        .ok()??;
    let blob: StoredBlob = client
        .get(&format!(
            "/api/collections/catalog_blobs/records/{}",
            revision.blob
        ))
        .and_then(|reply| reply.json())
        .ok()?;
    let token = client.file_token().ok()?;
    client
        .download_file("catalog_blobs", &blob.id, &blob.file, Some(&token))
        .ok()
}

/// Excluded from BOM in plain words, from the revision's setting and what
/// each reference says. A symbol or footprint the server could not read
/// gave no evidence, so the words say "Not known" unless the rules decide
/// without it.
fn excluded_from_bom_words(excluded: bool, marks: &[(CadReferenceRole, BomMark)]) -> String {
    let of = |role: CadReferenceRole| -> Vec<BomMark> {
        marks
            .iter()
            .filter(|(of_role, _)| *of_role == role)
            .map(|(_, mark)| *mark)
            .collect()
    };
    let (symbols, footprints) = (
        of(CadReferenceRole::Symbol),
        of(CadReferenceRole::Footprint),
    );
    if excluded {
        return if symbols.contains(&BomMark::Says(true)) {
            "Yes: its symbol says it is not in the BOM"
        } else {
            "Yes: every footprint choice says it is not in the BOM"
        }
        .to_owned();
    }
    let known = |mark: &BomMark| matches!(mark, BomMark::Says(_));
    // An unknown symbol could say it is not in the BOM. Unknown footprint
    // choices matter only while no known choice is in the BOM: then all of
    // them could say it is not.
    let symbol_unknown = !symbols.iter().all(known);
    let footprint_unknown =
        !footprints.iter().all(known) && !footprints.contains(&BomMark::Says(false));
    if symbol_unknown || footprint_unknown {
        let reasons: Vec<String> = [
            (&symbols, "symbol", symbol_unknown),
            (&footprints, "footprint", footprint_unknown),
        ]
        .into_iter()
        .filter(|(_, _, unknown)| *unknown)
        .filter_map(|(marks, what, _)| {
            let reason = marks.iter().find_map(|mark| match mark {
                BomMark::KiCadLibrary => {
                    Some("comes from KiCad's own library, which this app does not read yet")
                }
                BomMark::Missing => Some("is not in the Catalog"),
                BomMark::Unread => Some("could not be read"),
                BomMark::Says(_) => None,
            })?;
            Some(format!("the {what} {reason}"))
        })
        .collect();
        return format!("Not known: {}", reasons.join(", and "));
    }
    if footprints.contains(&BomMark::Says(true)) {
        "No: only some footprint choices say it is not in the BOM".to_owned()
    } else {
        "No".to_owned()
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
        "archive" => "Archived",
        "unarchive" => "Unarchived",
        "csv-review" => ACCEPTED,
        "create" => "Added",
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
        // A field you set just as they did is not a difference.
        let differs = if you_changed {
            mine != field.value
        } else {
            field.value != started
        };
        if differs {
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
    let names = category_names(&part.categories);
    let your_category = match request.category.as_str() {
        "" => base_details.category.clone(),
        code => code.to_owned(),
    };
    let you_changed_category = your_category != base_details.category;
    let category_differs = if you_changed_category {
        your_category != their_details.category
    } else {
        their_details.category != base_details.category
    };
    if category_differs {
        rows.push(ConflictRow {
            label: CATEGORY_LABEL.into(),
            started_from: category_name(&names, &base_details.category),
            theirs: category_name(&names, &their_details.category),
            yours: category_name(&names, &your_category),
            both_changed: you_changed_category
                && their_details.category != base_details.category
                && their_details.category != your_category,
            you_changed: you_changed_category,
        });
    }
    let mut merged_fields = field_rows(
        &base_details.fields,
        &their_details.fields,
        &request.fields,
        &mut rows,
    );
    // Your new category's fields stay yours to save on top.
    if you_changed_category {
        for field in &request.fields {
            if !merged_fields.iter().any(|merged| merged.name == field.name) {
                merged_fields.push(field.clone());
            }
        }
    }
    let started_parts = stored_lines(base_details);
    let their_parts = stored_lines(&their_details);
    let your_parts = manufacturer_lines(
        request
            .manufacturer_parts
            .iter()
            .map(|part| (part.manufacturer.as_str(), part.mpn.as_str())),
    );
    let you_changed_parts = your_parts != started_parts;
    let parts_differ = if you_changed_parts {
        your_parts != their_parts
    } else {
        their_parts != started_parts
    };
    if parts_differ {
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
    // A file you chose is a change of yours even when the reference stays
    // the same (a new revision of the same symbol or footprint), so the
    // edit is never taken for already saved.
    for file in &request.files {
        let (label, role) = match file.kind {
            FileKind::Symbol => ("Symbol file", CadReferenceRole::Symbol),
            FileKind::Footprint => ("Footprint file", CadReferenceRole::Footprint),
            FileKind::Model3d => continue,
        };
        let bound = |revision: &PartRevision| {
            revision
                .cad_references
                .iter()
                .filter(|reference| reference.role == role)
                .map(|reference| format!("{}, {}", reference.reference, binding(reference)))
                .collect::<Vec<_>>()
                .join(
                    "
",
                )
        };
        let (started, theirs) = (bound(base_revision), bound(&their_revision));
        rows.push(ConflictRow {
            label: label.into(),
            both_changed: theirs != started,
            started_from: started,
            theirs,
            yours: file.label.clone().unwrap_or_else(|| file.file_name.clone()),
            you_changed: true,
        });
    }
    let merged = TeamEditRequest {
        correlation_id: None,
        ipn: request.ipn.clone(),
        expected_details: their_details_number,
        expected_revision: their_revision_number,
        reason: request.reason.clone(),
        category: if you_changed_category {
            your_category
        } else {
            their_details.category.clone()
        },
        fields: merged_fields,
        manufacturer_parts: if you_changed_parts {
            request.manufacturer_parts.clone()
        } else {
            manufacturer_parts_of(&their_details)
        },
        cad_fields: merged_cad,
        // Files you chose are still yours to save on top.
        files: request.files.clone(),
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
                archive: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_category_whose_fields_changed_while_editing_is_said_plainly() {
        let server =
            "Field 7 must be Voltage; an edit cannot add, remove, rename or reorder fields.";
        assert_eq!(plain_refusal(server.into(), true), CATEGORY_FIELDS_CHANGED);
        let count = "Fields must list the same 6 fields as now, in the same order.";
        assert_eq!(plain_refusal(count.into(), true), CATEGORY_FIELDS_CHANGED);
        // Without a category change, the server's words stay.
        assert_eq!(plain_refusal(server.into(), false), server);
        assert_eq!(
            plain_refusal("No edit access.".into(), true),
            "No edit access."
        );
    }

    #[test]
    fn only_a_category_change_hides_the_fields_it_added_blank() {
        let difference = |what, label: &str, before: &str, after: &str| catalog::DetailDifference {
            what,
            label: label.into(),
            before: before.into(),
            after: after.into(),
        };
        let names: CategoryNames = [("res".to_owned(), "Resistors".to_owned())].into();
        let labels = |rows: Vec<DetailChange>| -> Vec<String> {
            rows.into_iter().map(|row| row.label).collect()
        };
        // A blank column a CSV header change added is shown.
        let header = vec![difference(DifferenceKind::Field, "Notes", "", "")];
        assert_eq!(labels(to_changes(header, &names)), ["Notes"]);
        // With a category change, the category row explains it.
        let moved = vec![
            difference(DifferenceKind::Category, "Category", "cap", "res"),
            difference(DifferenceKind::Field, "Tolerance", "", ""),
        ];
        assert_eq!(labels(to_changes(moved, &names)), ["Category"]);
    }

    #[test]
    fn a_row_moved_to_another_csv_file_shows_its_new_category_by_name() {
        let details = |category: &str| PartDetails {
            schema: catalog::PART_DETAILS_SCHEMA.into(),
            ipn: "RES-0000-0470".into(),
            category: category.into(),
            fields: vec![DetailField {
                name: "Value".into(),
                value: "47".into(),
            }],
            manufacturer_parts: Vec::new(),
        };
        let names: CategoryNames = [("res", "Resistors"), ("cap", "Capacitors")]
            .into_iter()
            .map(|(code, name)| (code.to_owned(), name.to_owned()))
            .collect();
        let rows = details_rows(&details("res"), &details("cap"), None, &names);
        assert_eq!(
            rows,
            [CsvChangeRow {
                field: "Category".into(),
                now: "Resistors".into(),
                csv: "Capacitors".into(),
                changed_in_csv: true,
                keep_only: false,
            }]
        );
        // Already moved when last applied: not a CSV change, accepting it
        // undoes the edit. An unknown code shows as it is.
        let rows = details_rows(
            &details("res"),
            &details("xyz"),
            Some(&details("xyz")),
            &names,
        );
        assert_eq!(
            (rows[0].csv.as_str(), rows[0].changed_in_csv),
            ("xyz", false)
        );
        assert!(details_rows(&details("res"), &details("res"), None, &names).is_empty());
    }

    #[test]
    fn each_kept_cad_item_choice_shows_its_own_csv_change() {
        let changes = |problem: &str, now: &str| CsvChanges {
            problem: problem.into(),
            csv_file: "g-res.csv".into(),
            cad: true,
            expected: 2,
            rows: vec![CsvChangeRow {
                field: "Footprint".into(),
                now: now.into(),
                csv: "A changed file".into(),
                changed_in_csv: true,
                keep_only: true,
            }],
        };
        let open = in_order(vec![
            (
                CSV_CAD_ITEM_KEPT.to_owned(),
                changes("b", "g-a:Second, footprint revision 1"),
            ),
            (
                CSV_CAD_ITEM_KEPT.to_owned(),
                changes("a", "g-a:First, footprint revision 2"),
            ),
        ]);
        let problems: Vec<&str> = open
            .iter()
            .map(|changes| changes.problem.as_str())
            .collect();
        assert_eq!(problems, ["a", "b"]);
    }

    #[test]
    fn excluded_from_bom_is_said_plainly_and_honestly() {
        use BomMark::*;
        use CadReferenceRole::{Footprint as F, Symbol as S};
        let words = excluded_from_bom_words;
        assert_eq!(
            words(true, &[(S, Says(true)), (F, Unread)]),
            "Yes: its symbol says it is not in the BOM"
        );
        assert_eq!(
            words(true, &[(S, KiCadLibrary), (F, Unread), (F, Unread)]),
            "Yes: every footprint choice says it is not in the BOM"
        );
        assert_eq!(words(false, &[(S, Says(false)), (F, Says(false))]), "No");
        assert_eq!(words(false, &[]), "No");
        assert_eq!(
            words(
                false,
                &[(S, Says(false)), (F, Says(true)), (F, Says(false))]
            ),
            "No: only some footprint choices say it is not in the BOM"
        );
        // A KiCad-library footprint beside a stored one that is in the BOM
        // cannot make it excluded.
        assert_eq!(
            words(
                false,
                &[(S, Says(false)), (F, Says(false)), (F, KiCadLibrary)]
            ),
            "No"
        );
        assert_eq!(
            words(false, &[(S, KiCadLibrary), (F, Says(false))]),
            "Not known: the symbol comes from KiCad's own library, which this app does not read yet"
        );
        assert_eq!(
            words(
                false,
                &[(S, Says(false)), (F, Says(true)), (F, KiCadLibrary)]
            ),
            "Not known: the footprint comes from KiCad's own library, which this app does not read yet"
        );
        assert_eq!(
            words(false, &[(S, Missing), (F, KiCadLibrary)]),
            "Not known: the symbol is not in the Catalog, and the footprint comes from KiCad's own library, which this app does not read yet"
        );
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
        let symbol = details(CadItemKind::Symbol, "kicad-symbol-entry", "g-res:R_US");
        let good = record(&symbol);
        let symbol_sha256 = symbol.sha256();
        let check = |revision: &StoredCadItemRevision, cad_item: &str, kind: PreviewKind| {
            let named = NamedRevision {
                cad_item,
                number: 1,
                snapshot_sha256: &symbol_sha256,
            };
            checked_snapshot(revision, &named, "g-res:R_US", kind)
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
        let footprint = details(CadItemKind::Footprint, "kicad-footprint", "g-res:R_US");
        let wrong_kind = record(&footprint);
        assert!(
            checked_snapshot(
                &wrong_kind,
                &NamedRevision {
                    cad_item: "item1",
                    number: 1,
                    snapshot_sha256: &footprint.sha256(),
                },
                "g-res:R_US",
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
}

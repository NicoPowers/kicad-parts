//! The team catalog in the desktop UI: the sign-in badge in the header,
//! and, for a part the team catalog has, its details and current revision,
//! history, edits and conflicts in the right-hand column of the part panel.
//!
//! Part details are shared by every revision, so saving them adds no
//! revision; only a new symbol or footprint does. A bad revision is
//! archived, never deleted: the current revision is the newest one that is
//! not archived. When the CSV has new values for a part that an import did
//! not apply, because the part was edited in this app, a notice offers to
//! review them field by field.

use crate::backend;
use desktop_contract::{
    CsvChangeRow, CsvChanges, DetailChange, Field, HistoryEntry, LiveUpdates, ManufacturerPart,
    PublishResult, SignInRequest, TeamArchiveRequest, TeamConflict, TeamCsvReviewRequest,
    TeamEditRequest, TeamPart, TeamSession, TeamStatus,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

// ---- Header badge and sign-in -------------------------------------------

/// The team catalog's state in the header, with sign-in and sign-out.
///
/// `team` is the status without its checkpoint, so new Changes do not
/// re-render (and close) the menu. What the user typed and whether the menu
/// is open live here, outside the part that re-renders.
#[component]
pub fn TeamBadge(
    team: Memo<Option<TeamStatus>>,
    set_team: RwSignal<Option<TeamStatus>>,
) -> impl IntoView {
    let server_url = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    let busy = RwSignal::new(false);
    // Fill the form once from the saved or suggested values.
    Effect::new(move |filled: Option<bool>| {
        if filled == Some(true) {
            return true;
        }
        let Some(status) = team.get() else {
            return false;
        };
        server_url.set(status.server_url.clone());
        email.set(status.email.clone());
        true
    });
    let toggled = move |event: leptos::ev::Event| {
        let is_open = event
            .target()
            .and_then(|target| js_sys::Reflect::get(&target, &"open".into()).ok())
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        open.set(is_open);
    };
    let sign_in = move || {
        busy.set(true);
        let request = SignInRequest {
            server_url: server_url.get_untracked(),
            email: email.get_untracked(),
            password: password.get_untracked(),
        };
        password.set(String::new());
        spawn_local(async move {
            let result = backend::team_sign_in(request).await;
            busy.set(false);
            if let Ok(status) = result {
                if status.session.is_signed_in() {
                    open.set(false);
                }
                set_team.set(Some(status));
            }
        });
    };
    move || {
        let Some(status) = team.get() else {
            return ().into_any();
        };
        match &status.session {
            TeamSession::SignedOut { problem } => {
                let problem = problem.clone();
                view! {
                    <details class="team-settings" open=open.get_untracked() on:toggle=toggled>
                        <summary class="kicad-badge team-badge-off">"Team catalog: not signed in"</summary>
                        <div class="kicad-settings-body team-settings-body">
                            <form
                                class="team-sign-in"
                                aria-label="Sign in to the team catalog"
                                on:submit=move |event| {
                                    event.prevent_default();
                                    sign_in();
                                }
                            >
                                <p>"Sign in to see each part's history and to edit parts with your team."</p>
                                <label for="team-server">"Team server address"</label>
                                <input
                                    id="team-server"
                                    type="url"
                                    spellcheck="false"
                                    placeholder="http://127.0.0.1:8090"
                                    prop:value=move || server_url.get()
                                    on:input:target=move |event| server_url.set(event.target().value())
                                />
                                <label for="team-email">"Email"</label>
                                <input
                                    id="team-email"
                                    type="email"
                                    autocomplete="username"
                                    spellcheck="false"
                                    prop:value=move || email.get()
                                    on:input:target=move |event| email.set(event.target().value())
                                />
                                <label for="team-password">"Password"</label>
                                <input
                                    id="team-password"
                                    type="password"
                                    autocomplete="current-password"
                                    prop:value=move || password.get()
                                    on:input:target=move |event| password.set(event.target().value())
                                />
                                <p class="hint">"The app saves the address and email, never your password."</p>
                                {problem.map(|message| view! { <p class="problem" role="alert">{message}</p> })}
                                <button type="submit" class="primary" disabled=move || busy.get()>
                                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                                </button>
                            </form>
                        </div>
                    </details>
                }
                .into_any()
            }
            TeamSession::SignedIn {
                name,
                email: signed_in_email,
                can_edit,
                live,
                ..
            } => {
                let (badge, live_text) = match live {
                    LiveUpdates::Live => (
                        "kicad-badge kicad-badge-ready",
                        "Changes others make appear here by themselves.".to_owned(),
                    ),
                    LiveUpdates::Connecting => (
                        "kicad-badge team-badge-off",
                        "Connecting so that changes others make appear here…".to_owned(),
                    ),
                    LiveUpdates::Reconnecting { message } => (
                        "kicad-badge kicad-badge-problem",
                        format!(
                            "Reconnecting so that changes others make appear here. Last problem: {message}"
                        ),
                    ),
                    LiveUpdates::SignInAgain { .. } => (
                        "kicad-badge kicad-badge-problem",
                        "The team server no longer accepts this sign-in, so changes others make no longer appear. Sign out, then sign in again.".to_owned(),
                    ),
                };
                let summary = if matches!(live, LiveUpdates::SignInAgain { .. }) {
                    "Team catalog: sign in again".to_owned()
                } else {
                    format!("Team catalog · {name}")
                };
                let access = if *can_edit {
                    "You can edit parts."
                } else {
                    "You can view parts but not edit them."
                };
                view! {
                    <details class="team-settings" open=open.get_untracked() on:toggle=toggled>
                        <summary class=badge>{summary}</summary>
                        <div class="kicad-settings-body team-settings-body">
                            <p>
                                {format!("Signed in to {} as {name} ({signed_in_email}). {access}", status.server_url)}
                            </p>
                            <p class="hint">{live_text}</p>
                            <button
                                type="button"
                                on:click=move |_| {
                                    spawn_local(async move {
                                        if let Ok(status) = backend::team_sign_out().await {
                                            open.set(false);
                                            set_team.set(Some(status));
                                        }
                                    });
                                }
                            >
                                "Sign out"
                            </button>
                        </div>
                    </details>
                }
                .into_any()
            }
        }
    }
}

// ---- The part's team view ------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Details,
    History,
}

/// A message after an action: saved (quiet) or refused (one alert).
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Done(String),
    Problem(String),
}

/// What is being edited. Inputs write here without re-rendering the form.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub ipn: String,
    /// The number of the part details the edit starts from.
    pub base_details: u32,
    /// The revision the edit starts from.
    pub base_revision: u32,
    pub fields: Vec<Field>,
    pub parts: Vec<ManufacturerPart>,
    /// The Symbol and Footprint fields.
    pub cad_fields: Vec<Field>,
    pub reason: String,
}

/// The state of the team column for the selected part. Reset when another
/// part is selected.
#[derive(Clone, Copy)]
pub struct TeamUi {
    pub tab: RwSignal<Tab>,
    /// Some while the edit form is open; a new number re-renders it.
    pub editing: RwSignal<Option<u32>>,
    pub draft: StoredValue<Draft>,
    /// Bumped when manufacturer parts are added or removed.
    pub parts_version: RwSignal<u32>,
    pub conflict: RwSignal<Option<TeamConflict>>,
    pub notice: RwSignal<Option<Notice>>,
    /// The history entry selected, by its Change's number.
    pub selected: RwSignal<Option<u64>>,
    /// The open archive form: the revision, true to archive it or false to
    /// unarchive it, and the part's current revision when the form opened.
    pub archiving: RwSignal<Option<(u32, bool, u32)>>,
    pub archive_reason: RwSignal<String>,
    /// The CSV values still to review in the open form (a group leaves once
    /// its review is saved); Some while it is open.
    pub reviewing: RwSignal<Option<Vec<CsvChanges>>>,
    /// The fields ticked in the review form, as (problem, field).
    pub review_picks: RwSignal<Vec<(String, String)>>,
    pub review_reason: RwSignal<String>,
    /// Each CSV review request sent and its correlation ID, so sending the
    /// same one again replays the server's answer.
    pub review_attempts: StoredValue<Vec<(TeamCsvReviewRequest, String)>>,
    pub busy: RwSignal<bool>,
    /// Edits and archives each keep their own retry slot, so opening or
    /// cancelling one form never changes what the other's retry sends.
    pub edit_attempt: RetrySlot<TeamEditRequest>,
    pub archive_attempt: RetrySlot<TeamArchiveRequest>,
}

/// One kind of save's retry slot: the last request sent and its
/// correlation ID, and the ID made when its form opened. Sending the same
/// request again (after a lost answer) reuses the ID, so the server replays
/// its answer instead of reporting a conflict with yourself.
pub struct RetrySlot<T: Send + Sync + 'static> {
    last: StoredValue<Option<(T, String)>>,
    opened_id: StoredValue<String>,
}

impl<T: Send + Sync + 'static> Clone for RetrySlot<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for RetrySlot<T> {}

impl<T: Clone + PartialEq + Send + Sync + 'static> RetrySlot<T> {
    fn new() -> Self {
        Self {
            last: StoredValue::new(None),
            opened_id: StoredValue::new(new_correlation_id()),
        }
    }

    /// Starts afresh, as when its form opens: the next save gets a new ID.
    fn open(&self) {
        self.last.set_value(None);
        self.opened_id.set_value(new_correlation_id());
    }

    /// The correlation ID for `request`: the previous one if this is the
    /// same save sent again, the form's own ID for its first save,
    /// otherwise new.
    fn id_for(&self, request: T) -> String {
        let repeated = self.last.with_value(|last| match last {
            Some((previous, id)) if *previous == request => Some(id.clone()),
            _ => None,
        });
        if let Some(id) = repeated {
            return id;
        }
        let id = if self.last.with_value(Option::is_none) {
            self.opened_id.get_value()
        } else {
            new_correlation_id()
        };
        self.last.set_value(Some((request, id.clone())));
        id
    }
}

impl Default for TeamUi {
    fn default() -> Self {
        Self::new()
    }
}

impl TeamUi {
    pub fn new() -> Self {
        Self {
            tab: RwSignal::new(Tab::Details),
            editing: RwSignal::new(None),
            draft: StoredValue::new(Draft::default()),
            parts_version: RwSignal::new(0),
            conflict: RwSignal::new(None),
            notice: RwSignal::new(None),
            selected: RwSignal::new(None),
            archiving: RwSignal::new(None),
            archive_reason: RwSignal::new(String::new()),
            reviewing: RwSignal::new(None),
            review_picks: RwSignal::new(Vec::new()),
            review_reason: RwSignal::new(String::new()),
            review_attempts: StoredValue::new(Vec::new()),
            busy: RwSignal::new(false),
            edit_attempt: RetrySlot::new(),
            archive_attempt: RetrySlot::new(),
        }
    }

    pub fn reset(&self) {
        self.edit_attempt.open();
        self.archive_attempt.open();
        self.tab.set(Tab::Details);
        self.editing.set(None);
        self.conflict.set(None);
        self.notice.set(None);
        self.selected.set(None);
        self.archiving.set(None);
        self.archive_reason.set(String::new());
        self.close_review();
        self.busy.set(false);
    }

    fn close_review(&self) {
        self.reviewing.set(None);
        self.review_picks.set(Vec::new());
        self.review_reason.set(String::new());
        self.review_attempts.set_value(Vec::new());
    }

    fn start_review(&self, changes: Vec<CsvChanges>) {
        self.notice.set(None);
        self.review_picks.set(Vec::new());
        self.review_attempts.set_value(Vec::new());
        self.review_reason.set(accept_reason(&csv_files(&changes)));
        self.reviewing.set(Some(changes));
        focus_later("csv-review-heading");
    }

    /// Opens the archive form. Its first save gets a new correlation ID, so
    /// archiving the same revision for the same reason as an earlier save
    /// (after someone else unarchived it) is not taken for that save.
    fn start_archiving(&self, number: u32, archived: bool, current: u32) {
        self.archive_attempt.open();
        self.notice.set(None);
        self.archive_reason.set(String::new());
        self.archiving.set(Some((number, archived, current)));
    }

    fn start_editing(&self, draft: Draft) {
        self.edit_attempt.open();
        self.draft.set_value(draft);
        self.close_review();
        self.conflict.set(None);
        self.notice.set(None);
        self.tab.set(Tab::Details);
        self.editing
            .update(|editing| *editing = Some(editing.map_or(1, |n| n + 1)));
    }
}

fn draft_of(part: &TeamPart) -> Draft {
    Draft {
        ipn: part.ipn.clone(),
        base_details: part.details.number,
        base_revision: part.revision.number,
        fields: part.details.fields.clone(),
        parts: part.details.manufacturer_parts.clone(),
        cad_fields: part.revision.cad_fields.clone(),
        reason: String::new(),
    }
}

fn draft_from_request(request: &TeamEditRequest) -> Draft {
    Draft {
        ipn: request.ipn.clone(),
        base_details: request.expected_details,
        base_revision: request.expected_revision,
        fields: request.fields.clone(),
        parts: request.manufacturer_parts.clone(),
        cad_fields: request.cad_fields.clone(),
        reason: request.reason.clone(),
    }
}

fn request_of(draft: Draft) -> TeamEditRequest {
    TeamEditRequest {
        correlation_id: None,
        ipn: draft.ipn,
        expected_details: draft.base_details,
        expected_revision: draft.base_revision,
        reason: draft.reason,
        fields: draft.fields,
        manufacturer_parts: draft.parts,
        cad_fields: draft.cad_fields,
    }
}

/// Scrolls the right-hand column to its top, where the notice or the
/// conflict is.
fn scroll_column_to_top() {
    if let Ok(Some(column)) = document().query_selector(".part-fields") {
        let _ = js_sys::Reflect::set(&column, &"scrollTop".into(), &0.into());
    }
}

/// Applies what the server answered to an edit.
fn published(ui: TeamUi, part: RwSignal<Option<TeamPart>>, result: Result<PublishResult, String>) {
    ui.busy.set(false);
    match result {
        Ok(PublishResult::Saved {
            revision,
            part: saved,
        }) => {
            part.set(Some(saved));
            ui.editing.set(None);
            ui.conflict.set(None);
            ui.selected.set(None);
            let message = match revision {
                Some(revision) => format!("Saved as revision {revision}."),
                None => "Saved. The part details changed; no revision was added.".to_owned(),
            };
            ui.notice.set(Some(Notice::Done(message)));
            scroll_column_to_top();
        }
        Ok(PublishResult::Archived {
            revision,
            archived,
            current,
            part: saved,
        }) => {
            // The current revision when the form opened: a live update may
            // already have refreshed the part while the request was out.
            let before = ui.archiving.get_untracked().map(|(_, _, current)| current);
            part.set(Some(saved));
            ui.archiving.set(None);
            ui.archive_reason.set(String::new());
            ui.selected.set(None);
            let then = if before == Some(current) {
                format!("The current revision stays revision {current}.")
            } else {
                format!("The current revision is now revision {current}.")
            };
            let message = if archived {
                format!("Revision {revision} is archived. {then}")
            } else {
                format!("Revision {revision} is no longer archived. {then}")
            };
            ui.notice.set(Some(Notice::Done(message)));
            scroll_column_to_top();
        }
        Ok(PublishResult::CsvReviewed { part: saved, .. }) => {
            // Several review requests are summed up by save_csv_review.
            part.set(Some(saved));
        }
        Ok(PublishResult::AlreadySaved {
            message,
            part: saved,
        }) => {
            part.set(Some(saved));
            ui.editing.set(None);
            ui.conflict.set(None);
            ui.selected.set(None);
            ui.notice.set(Some(Notice::Done(message)));
            scroll_column_to_top();
        }
        // Saving on top of their changes added nothing: theirs already
        // hold yours, so the conflict closes instead of staying open.
        Ok(PublishResult::NothingToSave { .. }) if ui.conflict.with_untracked(Option::is_some) => {
            ui.editing.set(None);
            ui.conflict.set(None);
            ui.notice.set(Some(Notice::Done(
                "Someone else already saved the same changes, so nothing more was saved.".into(),
            )));
            scroll_column_to_top();
        }
        Ok(PublishResult::NothingToSave { message }) => ui.notice.set(Some(Notice::Done(message))),
        Ok(PublishResult::Conflict(conflict)) => {
            part.set(Some(conflict.part.clone()));
            ui.editing.set(None);
            ui.notice.set(None);
            ui.conflict.set(Some(*conflict));
            scroll_column_to_top();
        }
        Ok(PublishResult::Refused { message }) => ui.notice.set(Some(Notice::Problem(message))),
        Err(message) => ui.notice.set(Some(Notice::Problem(message))),
    }
}

/// A new correlation ID: letters, digits and dashes, as the server needs.
fn new_correlation_id() -> String {
    let now = js_sys::Date::now() as u64;
    let random = (js_sys::Math::random() * 1e15) as u64;
    format!("desktop-{now:x}-{random:x}")
}

fn save_edit(ui: TeamUi, part: RwSignal<Option<TeamPart>>, request: TeamEditRequest) {
    if request.reason.trim().is_empty() {
        ui.notice.set(Some(Notice::Problem(
            "Say why you are changing the part before you save.".into(),
        )));
        return;
    }
    let mut request = request;
    request.correlation_id = None;
    request.correlation_id = Some(ui.edit_attempt.id_for(request.clone()));
    ui.busy.set(true);
    spawn_local(async move {
        let result = backend::team_edit(request).await;
        published(ui, part, result);
    });
}

/// The right-hand column for a part the team catalog has.
#[component]
pub fn TeamColumn(part: RwSignal<Option<TeamPart>>, can_edit: bool, ui: TeamUi) -> impl IntoView {
    let tab_button = move |tab: Tab, id: &'static str, panel: &'static str| {
        let selected = move || ui.tab.get() == tab;
        view! {
            <button
                type="button"
                role="tab"
                id=id
                class="team-tab"
                aria-selected=move || if selected() { "true" } else { "false" }
                aria-controls=panel
                tabindex=move || if selected() { "0" } else { "-1" }
                on:click=move |_| ui.tab.set(tab)
                on:keydown=move |event| {
                    let key = event.key();
                    if key == "ArrowRight" || key == "ArrowLeft" {
                        event.prevent_default();
                        let next = if ui.tab.get_untracked() == Tab::Details {
                            Tab::History
                        } else {
                            Tab::Details
                        };
                        ui.tab.set(next);
                        let target = if next == Tab::Details { "team-tab-details" } else { "team-tab-history" };
                        if let Some(element) = document().get_element_by_id(target) {
                            use wasm_bindgen::JsCast;
                            if let Some(element) = element.dyn_ref::<leptos::web_sys::HtmlElement>() {
                                let _ = element.focus();
                            }
                        }
                    }
                }
            >
                {move || match tab {
                    Tab::Details => "Details".to_owned(),
                    Tab::History => {
                        let count = part.with(|part| part.as_ref().map_or(0, |part| part.history.len()));
                        format!("History ({count})")
                    }
                }}
            </button>
        }
    };
    // The review form is drawn once when it opens; its rows follow
    // ui.reviewing, so a saved group leaves without redrawing the form.
    let reviewing_open = Memo::new(move |_| ui.reviewing.with(Option::is_some));
    // If the CSV values whose form is open are no longer open on the part
    // (someone else reviewed them, or a newer import found different ones),
    // close the form and say which, unless this app's own request is out.
    Effect::new(move |_| {
        let Some(open) = ui.reviewing.get() else {
            return;
        };
        if ui.busy.get() {
            return;
        }
        let gone = part.with(|part| {
            let part = part.as_ref()?;
            let missing: Vec<bool> = open
                .iter()
                .filter(|changes| {
                    !part
                        .csv_changes
                        .iter()
                        .any(|now| now.problem == changes.problem)
                })
                .map(|changes| changes.cad)
                .collect();
            if missing.is_empty() {
                return None;
            }
            // Values of the same kind are still open: a newer import found
            // different ones.
            Some(
                missing
                    .iter()
                    .any(|cad| part.csv_changes.iter().any(|now| now.cad == *cad)),
            )
        });
        if let Some(newer) = gone {
            ui.close_review();
            let message = if newer {
                "A newer import found different CSV values, so nothing was saved. Review them again."
            } else {
                "These CSV values are no longer open, so nothing was saved."
            };
            ui.notice.set(Some(Notice::Problem(message.into())));
        }
    });
    // If someone else archives or unarchives the revision whose form is
    // open, close the form and say so plainly, rather than turning it into
    // the opposite action with the reason still in it.
    // While this app's own request is out, a live update may already show
    // its result; that is not someone else's doing.
    Effect::new(move |_| {
        let Some((number, wanted, _)) = ui.archiving.get() else {
            return;
        };
        if ui.busy.get() {
            return;
        }
        let done = part.with(|part| {
            part.as_ref()
                .is_some_and(|part| is_archived(part, number) == wanted)
        });
        if done {
            ui.archiving.set(None);
            ui.archive_reason.set(String::new());
            let what = if wanted { "archived" } else { "unarchived" };
            ui.notice.set(Some(Notice::Problem(format!(
                "Someone else already {what} revision {number}, so nothing was saved."
            ))));
        }
    });
    view! {
        <div class="team-column">
            {move || {
                ui.conflict
                    .get()
                    .map(|conflict| view! { <ConflictView conflict=conflict part=part ui=ui /> })
            }}
            <div
                class="team-tabs"
                role="tablist"
                aria-label="Team catalog"
                hidden=move || ui.conflict.with(Option::is_some)
            >
                {tab_button(Tab::Details, "team-tab-details", "team-panel-details")}
                {tab_button(Tab::History, "team-tab-history", "team-panel-history")}
            </div>
            <NoticeView ui=ui />
            <div
                id="team-panel-details"
                role="tabpanel"
                aria-labelledby="team-tab-details"
                hidden=move || ui.tab.get() != Tab::Details || ui.conflict.with(Option::is_some)
            >
                {move || match (ui.editing.get(), reviewing_open.get()) {
                    (Some(_), _) => view! { <EditForm part=part ui=ui /> }.into_any(),
                    (None, true) => view! { <CsvReviewForm part=part ui=ui /> }.into_any(),
                    (None, false) => view! { <DetailsView part=part can_edit=can_edit ui=ui /> }.into_any(),
                }}
            </div>
            <div
                id="team-panel-history"
                role="tabpanel"
                aria-labelledby="team-tab-history"
                hidden=move || ui.tab.get() != Tab::History || ui.conflict.with(Option::is_some)
            >
                <HistoryView part=part can_edit=can_edit ui=ui />
            </div>
        </div>
    }
}

#[component]
fn NoticeView(ui: TeamUi) -> impl IntoView {
    move || match ui.notice.get() {
        Some(Notice::Done(message)) => view! {
            <p class="team-notice" role="status">{message}</p>
        }
        .into_any(),
        Some(Notice::Problem(message)) => view! {
            <p class="problem team-problem" role="alert">{message}</p>
        }
        .into_any(),
        None => ().into_any(),
    }
}

/// `(blank)` for an empty value, so a blank field is visible.
fn shown(value: &str) -> String {
    if value.is_empty() {
        "(blank)".to_owned()
    } else {
        value.to_owned()
    }
}

/// A field's value as [`shown`] gives it, with a symbol or footprint
/// reference kept whole up to its colon ([`crate::cad_reference`]).
fn value_view(field: &str, value: &str) -> AnyView {
    if crate::is_cad_field(field) && !value.is_empty() {
        crate::cad_reference(value)
    } else {
        shown(value).into_any()
    }
}

fn is_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// One row per field: its name and its value, blanks shown as `(blank)` and
/// web addresses on one line.
fn field_rows(fields: Vec<Field>) -> impl IntoView {
    fields
        .into_iter()
        .map(|field| {
            let url = is_url(&field.value);
            let title = url.then(|| field.value.clone());
            let value = value_view(&field.name, &field.value);
            view! {
                <tr>
                    <th scope="row">{field.name}</th>
                    <td class:field-url=url class:blank=field.value.is_empty() title=title>
                        {value}
                    </td>
                </tr>
            }
        })
        .collect_view()
}

#[component]
fn DetailsView(part: RwSignal<Option<TeamPart>>, can_edit: bool, ui: TeamUi) -> impl IntoView {
    move || {
        let Some(team_part) = part.get() else {
            return ().into_any();
        };
        let details = team_part.details.clone();
        let revision = team_part.revision.clone();
        let count = details.fields.len();
        let for_edit = team_part.clone();
        let csv_notice = (!team_part.csv_changes.is_empty()).then(|| {
            let changes = team_part.csv_changes.clone();
            let files = csv_files(&changes).join(" and ");
            view! {
                <section class="csv-notice" role="status" aria-labelledby="csv-notice-heading">
                    <h3 id="csv-notice-heading">"New values in the CSV"</h3>
                    <p>
                        {format!(
                            "{files} has new values for this part. They were not applied, because the part was edited in this app."
                        )}
                    </p>
                    {if can_edit {
                        view! {
                            <button
                                type="button"
                                id="csv-review-open"
                                on:click=move |_| ui.start_review(changes.clone())
                            >
                                "Review CSV values"
                            </button>
                        }
                        .into_any()
                    } else {
                        view! { <p class="hint">"Someone who can edit parts can review them."</p> }.into_any()
                    }}
                </section>
            }
        });
        view! {
            {csv_notice}
            <div class="team-toolbar">
                <h3 id="fields-heading" tabindex="-1">{format!("Part details ({count})")}</h3>
                {if can_edit {
                    view! {
                        <button
                            type="button"
                            class="primary"
                            on:click=move |_| ui.start_editing(draft_of(&for_edit))
                        >
                            "Edit part"
                        </button>
                    }
                    .into_any()
                } else {
                    view! { <p class="hint read-only">"You can view this part but not edit it."</p> }
                        .into_any()
                }}
            </div>
            <p class="hint">"Every revision of the part shares these details."</p>
            <table aria-labelledby="fields-heading">
                <thead>
                    <tr><th scope="col">"Field"</th><th scope="col">"Value"</th></tr>
                </thead>
                <tbody>{field_rows(details.fields.clone())}</tbody>
            </table>
            <h3 id="manufacturer-parts-heading">"Manufacturer parts"</h3>
            {if details.manufacturer_parts.is_empty() {
                view! { <p class="hint">"None listed."</p> }.into_any()
            } else {
                view! {
                    <ol class="manufacturer-parts" aria-labelledby="manufacturer-parts-heading">
                        {details
                            .manufacturer_parts
                            .iter()
                            .enumerate()
                            .map(|(index, part)| {
                                let main = if index == 0 { " (main)" } else { "" };
                                view! { <li>{format!("{} {}{main}", part.manufacturer, part.mpn)}</li> }
                            })
                            .collect_view()}
                    </ol>
                }
                .into_any()
            }}
            <h3 id="cad-items-heading">{format!("Symbol and footprint (revision {} of this part)", revision.number)}</h3>
            {if revision.cad_items.is_empty() {
                view! { <p class="hint">"This part uses no symbol or footprint."</p> }.into_any()
            } else {
                view! {
                    <table aria-labelledby="cad-items-heading">
                        <tbody>
                            {revision
                                .cad_items
                                .iter()
                                .map(|item| view! {
                                    <tr>
                                        <th scope="row">{item.role.clone()}</th>
                                        <td><code>{crate::cad_reference(&item.reference)}</code>", "<span class="binding">{item.binding.clone()}</span></td>
                                    </tr>
                                })
                                .collect_view()}
                            <tr>
                                <th scope="row">"Excluded from BOM"</th>
                                <td>{revision.excluded_from_bom.clone()}</td>
                            </tr>
                        </tbody>
                    </table>
                }
                .into_any()
            }}
            {other_revisions(&team_part).map(|text| view! { <p class="hint other-revisions">{text}</p> })}
        }
        .into_any()
    }
}

/// `Other revisions: revision 1 (older), revision 3 (archived).`, or None
/// when the part has only its current revision.
fn other_revisions(part: &TeamPart) -> Option<String> {
    let others: Vec<String> = part
        .revisions
        .iter()
        .filter(|revision| revision.number != part.revision.number)
        .map(|revision| {
            let state = if revision.archived {
                "archived"
            } else {
                "older"
            };
            format!("revision {} ({state})", revision.number)
        })
        .collect();
    (!others.is_empty()).then(|| format!("Other revisions: {}.", others.join(", ")))
}

/// Text inputs for `fields`, which write into the draft through `set`.
fn field_inputs(
    fields: Vec<Field>,
    prefix: &'static str,
    ui: TeamUi,
    set: fn(&mut Draft, usize, String),
) -> impl IntoView {
    fields
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            let id = format!("{prefix}-{index}");
            let value = field.value;
            view! {
                <div class="edit-row">
                    <label for=id.clone()>{field.name}</label>
                    <input
                        id=id
                        type="text"
                        spellcheck="false"
                        prop:value=value
                        on:input:target=move |event| {
                            let value = event.target().value();
                            ui.draft.update_value(|draft| set(draft, index, value));
                        }
                    />
                </div>
            }
        })
        .collect_view()
}

#[component]
fn EditForm(part: RwSignal<Option<TeamPart>>, ui: TeamUi) -> impl IntoView {
    let draft = ui.draft.get_value();
    let (base_details, base_revision) = (draft.base_details, draft.base_revision);
    let fields = field_inputs(
        draft.fields.clone(),
        "edit-field",
        ui,
        |draft, index, value| {
            draft.fields[index].value = value;
        },
    );
    let cad_fields = field_inputs(
        draft.cad_fields.clone(),
        "edit-cad",
        ui,
        |draft, index, value| {
            draft.cad_fields[index].value = value;
        },
    );
    let has_cad = !draft.cad_fields.is_empty();
    let save = move || save_edit(ui, part, request_of(ui.draft.get_value()));
    view! {
        <form
            class="edit-form"
            aria-labelledby="edit-heading"
            on:submit=move |event| {
                event.prevent_default();
                save();
            }
        >
            <h3 id="edit-heading">"Edit part"</h3>
            <p class="hint">
                "Change details only to correct them. A part with a different value, package or pinout needs a new IPN."
            </p>
            {move || {
                part.with(|part| {
                    part.as_ref()
                        .filter(|part| {
                            part.details.number > base_details || part.revision.number > base_revision
                        })
                        .map(|_| view! {
                            <p class="hint">
                                "Someone else saved a change to this part while you were editing. When you save, you can compare your changes with theirs."
                            </p>
                        })
                })
            }}
            <fieldset>
                <legend>"Part details"</legend>
                {fields}
            </fieldset>
            <fieldset>
                <legend>"Manufacturer parts (the first is the main one)"</legend>
                {move || {
                    ui.parts_version.track();
                    let parts = ui.draft.with_value(|draft| draft.parts.clone());
                    parts
                        .into_iter()
                        .enumerate()
                        .map(|(index, entry)| {
                            let number = index + 1;
                            view! {
                                <div class="edit-part">
                                    <input
                                        type="text"
                                        spellcheck="false"
                                        aria-label=format!("Manufacturer {number}")
                                        placeholder="Manufacturer"
                                        prop:value=entry.manufacturer
                                        on:input:target=move |event| {
                                            let value = event.target().value();
                                            ui.draft.update_value(|draft| draft.parts[index].manufacturer = value);
                                        }
                                    />
                                    <input
                                        type="text"
                                        spellcheck="false"
                                        aria-label=format!("MPN {number}")
                                        placeholder="MPN"
                                        prop:value=entry.mpn
                                        on:input:target=move |event| {
                                            let value = event.target().value();
                                            ui.draft.update_value(|draft| draft.parts[index].mpn = value);
                                        }
                                    />
                                    <button
                                        type="button"
                                        aria-label=format!("Remove manufacturer part {number}")
                                        on:click=move |_| {
                                            ui.draft.update_value(|draft| {
                                                draft.parts.remove(index);
                                            });
                                            ui.parts_version.update(|version| *version += 1);
                                        }
                                    >
                                        "Remove"
                                    </button>
                                </div>
                            }
                        })
                        .collect_view()
                }}
                <button
                    type="button"
                    on:click=move |_| {
                        ui.draft.update_value(|draft| {
                            draft.parts.push(ManufacturerPart {
                                manufacturer: String::new(),
                                mpn: String::new(),
                            })
                        });
                        ui.parts_version.update(|version| *version += 1);
                    }
                >
                    "Add a manufacturer part"
                </button>
            </fieldset>
            {has_cad.then(|| view! {
                <fieldset>
                    <legend>"Symbol and footprint"</legend>
                    <p class="hint">
                        {format!("Changing these adds a new revision of this part. Designs that use revision {base_revision} keep it.")}
                    </p>
                    {cad_fields}
                </fieldset>
            })}
            <label for="edit-reason" class="reason-label">"Why are you changing it?"</label>
            <textarea
                id="edit-reason"
                rows="2"
                prop:value=draft.reason
                on:input:target=move |event| {
                    let value = event.target().value();
                    ui.draft.update_value(|draft| draft.reason = value);
                }
            ></textarea>
            <div class="form-actions">
                <button type="submit" class="primary" disabled=move || ui.busy.get()>
                    {move || if ui.busy.get() { "Saving…" } else { "Save" }}
                </button>
                <button
                    type="button"
                    on:click=move |_| {
                        ui.editing.set(None);
                        ui.notice.set(None);
                    }
                >
                    "Cancel"
                </button>
            </div>
        </form>
    }
}

/// A table of differences, with column names for the two sides.
fn changes_table(
    label: String,
    before: &'static str,
    after: &'static str,
    changes: Vec<DetailChange>,
) -> impl IntoView {
    view! {
        <table class="changes" aria-label=label>
            <thead>
                <tr>
                    <th scope="col">"Field"</th>
                    <th scope="col">{before}</th>
                    <th scope="col">{after}</th>
                </tr>
            </thead>
            <tbody>
                {changes
                    .into_iter()
                    .map(|change| {
                        let before = value_view(&change.label, &change.before);
                        let after = value_view(&change.label, &change.after);
                        view! {
                            <tr>
                                <th scope="row">{change.label}</th>
                                <td class="before" class:blank=change.before.is_empty()>{before}</td>
                                <td class="after" class:blank=change.after.is_empty()>{after}</td>
                            </tr>
                        }
                    })
                    .collect_view()}
            </tbody>
        </table>
    }
}

/// A save time such as `2026-10-09 18:02 UTC`, marked up as a `<time>`.
fn saved_at_view(shown: String) -> impl IntoView {
    let datetime = machine_time(&shown);
    view! { <time datetime=datetime>{shown}</time> }
}

/// `2026-10-09 18:02 UTC` as `2026-10-09T18:02Z`, the form `<time
/// datetime>` needs; `None` for any other text.
fn machine_time(shown: &str) -> Option<String> {
    let (date, time) = shown.strip_suffix(" UTC")?.split_once(' ')?;
    Some(format!("{date}T{time}Z"))
}

/// `Revision 1 (current) · Imported by an administrator` for an entry that
/// added a revision (with the revision's state, `current` or `archived`,
/// next to it), `Revision 2 · Archived by Ada` for an archive, and `Part
/// details · Edited by Ada` for one that only saved the details.
fn entry_title(entry: &HistoryEntry, part: &TeamPart) -> String {
    let what = match (entry.revision, &entry.archive) {
        (Some(number), _) if number == part.revision.number => {
            format!("Revision {number} (current)")
        }
        (Some(number), _) if is_archived(part, number) => format!("Revision {number} (archived)"),
        (Some(number), _) => format!("Revision {number}"),
        (None, Some(archive)) => format!("Revision {}", archive.revision),
        (None, None) if is_kept(entry) => "CSV values".to_owned(),
        (None, None) => "Part details".to_owned(),
    };
    format!("{what} · {} by {}", entry.action, entry.actor_name)
}

/// A review that kept the current values ("Kept current part details" or
/// "Kept current symbol and footprint").
fn is_kept(entry: &HistoryEntry) -> bool {
    entry.action.starts_with("Kept current")
}

/// The CSV files the values come from, each once.
fn csv_files(changes: &[CsvChanges]) -> Vec<String> {
    changes.iter().fold(Vec::new(), |mut files, change| {
        if !files.contains(&change.csv_file) {
            files.push(change.csv_file.clone());
        }
        files
    })
}

/// The reason the review form starts with.
fn accept_reason(files: &[String]) -> String {
    format!("Accept CSV values from {}", files.join(" and "))
}

fn is_archived(part: &TeamPart, number: u32) -> bool {
    part.revisions
        .iter()
        .any(|revision| revision.number == number && revision.archived)
}

#[component]
fn HistoryView(part: RwSignal<Option<TeamPart>>, can_edit: bool, ui: TeamUi) -> impl IntoView {
    move || {
        let Some(team_part) = part.get() else {
            return ().into_any();
        };
        let newest = team_part.history.first().map(|entry| entry.seq);
        let chosen = ui.selected.get().or(newest);
        let selected = team_part
            .history
            .iter()
            .find(|entry| Some(entry.seq) == chosen)
            .or(team_part.history.first())
            .cloned();
        view! {
            <h3 id="history-heading">"History"</h3>
            <ol class="history" aria-labelledby="history-heading">
                {team_part
                    .history
                    .iter()
                    .map(|entry| {
                        let seq = entry.seq;
                        let is_selected = Some(seq) == chosen;
                        let reason = if entry.reason.is_empty() {
                            String::new()
                        } else {
                            format!("“{}”", entry.reason)
                        };
                        view! {
                            <li>
                                <button
                                    type="button"
                                    class="history-entry"
                                    aria-pressed=if is_selected { "true" } else { "false" }
                                    on:click=move |_| {
                                        ui.selected.set(Some(seq));
                                        ui.archiving.set(None);
                                    }
                                >
                                    <span class="history-title">{entry_title(entry, &team_part)}</span>
                                    <span class="history-when">{saved_at_view(entry.saved_at.clone())}</span>
                                    <span class="history-reason">{reason}</span>
                                </button>
                            </li>
                        }
                    })
                    .collect_view()}
            </ol>
            {selected.map(|entry| view! { <EntryView entry=entry part=team_part.clone() can_edit=can_edit team=part ui=ui /> })}
        }
        .into_any()
    }
}

#[component]
fn EntryView(
    entry: HistoryEntry,
    part: TeamPart,
    can_edit: bool,
    team: RwSignal<Option<TeamPart>>,
    ui: TeamUi,
) -> impl IntoView {
    let heading = match (entry.revision, &entry.archive) {
        (Some(number), _) => format!("What changed in revision {number}"),
        (None, Some(archive)) if archive.archived => {
            format!("Revision {} archived", archive.revision)
        }
        (None, Some(archive)) => format!("Revision {} unarchived", archive.revision),
        (None, None) if is_kept(&entry) => "CSV values not accepted".to_owned(),
        (None, None) => "What changed in the part details".to_owned(),
    };
    let what_changed = if let Some(archive) = &entry.archive {
        let done = if archive.archived {
            "archived"
        } else {
            "unarchived"
        };
        view! {
            <p class="hint">
                {format!(
                    "Revision {} was {done}. The current revision became revision {}.",
                    archive.revision, archive.current_after
                )}
            </p>
        }
        .into_any()
    } else if is_kept(&entry) {
        view! {
            <p class="hint">
                "The current values were kept instead of the CSV values. You are asked again only if the CSV changes."
            </p>
        }
        .into_any()
    } else if entry.revision == Some(1) {
        view! { <p class="hint">"The first revision of this part."</p> }.into_any()
    } else if entry.changes.is_empty() {
        view! { <p class="hint">"Nothing changed."</p> }.into_any()
    } else {
        changes_table(heading.clone(), "Before", "After", entry.changes.clone()).into_any()
    };
    // A revision entry offers to archive or unarchive its revision; an
    // archive entry offers to undo it while the revision is still archived.
    let offered = entry.revision.or_else(|| {
        entry
            .archive
            .as_ref()
            .filter(|archive| archive.archived && is_archived(&part, archive.revision))
            .map(|archive| archive.revision)
    });
    let archive_section = offered
        .filter(|_| can_edit)
        .map(|number| view! { <ArchiveSection number=number part=part.clone() team=team ui=ui /> });
    view! {
        <section class="revision" aria-labelledby="revision-heading">
            <h4 id="revision-heading">{heading}</h4>
            <p class="history-when">
                {format!("{} by {} on ", entry.action, entry.actor_name)}
                {saved_at_view(entry.saved_at.clone())}
            </p>
            {what_changed}
            {archive_section}
        </section>
    }
}

/// Archive or unarchive revision `number`: a button that opens a small form
/// with a plain explanation and a reason, or a note saying why the only
/// revision that is not archived cannot be archived.
#[component]
fn ArchiveSection(
    number: u32,
    part: TeamPart,
    team: RwSignal<Option<TeamPart>>,
    ui: TeamUi,
) -> impl IntoView {
    let archived = is_archived(&part, number);
    let current = part.revision.number;
    // The newest revision not archived once this one is (un)archived.
    let after = part
        .revisions
        .iter()
        .filter(|revision| {
            if revision.number == number {
                archived
            } else {
                !revision.archived
            }
        })
        .map(|revision| revision.number)
        .max();
    let ipn = part.ipn.clone();
    move || {
        let Some(after) = after else {
            return view! {
                <p class="hint">
                    {format!(
                        "Revision {number} is the only revision that is not archived, so it cannot be archived. Add a corrected revision first."
                    )}
                </p>
            }
            .into_any();
        };
        let verb = if archived { "Unarchive" } else { "Archive" };
        let opener = format!("archive-open-{number}");
        let open = ui
            .archiving
            .get()
            .is_some_and(|(open, wanted, _)| open == number && wanted != archived);
        if !open {
            return view! {
                <button
                    type="button"
                    class="archive-open"
                    id=opener
                    on:click=move |_| {
                        ui.start_archiving(number, !archived, current);
                        focus_later("archive-reason");
                    }
                >
                    {format!("{verb} revision {number}…")}
                </button>
            }
            .into_any();
        }
        let then = if after == current {
            format!("The current revision stays revision {after}.")
        } else {
            format!("The current revision becomes revision {after}.")
        };
        let explanation = if archived {
            format!("KiCad and this app offer revision {number} again. {then}")
        } else {
            format!(
                "KiCad and this app stop offering revision {number}, so nobody new picks it. Designs that already use it keep it. Nothing is deleted. {then}"
            )
        };
        let question = if archived {
            "Why are you unarchiving it?"
        } else {
            "Why are you archiving it?"
        };
        let ipn = ipn.clone();
        view! {
            <form
                class="archive-form"
                aria-labelledby="archive-heading"
                on:submit=move |event| {
                    event.prevent_default();
                    save_archive(
                        ui,
                        team,
                        TeamArchiveRequest {
                            correlation_id: None,
                            ipn: ipn.clone(),
                            revision: number,
                            archived: !archived,
                            reason: ui.archive_reason.get_untracked(),
                        },
                    );
                }
            >
                <h4 id="archive-heading">{format!("{verb} revision {number}")}</h4>
                <p>{explanation}</p>
                <label for="archive-reason" class="reason-label">{question}</label>
                <textarea
                    id="archive-reason"
                    rows="2"
                    prop:value=move || ui.archive_reason.get()
                    on:input:target=move |event| ui.archive_reason.set(event.target().value())
                ></textarea>
                <div class="form-actions">
                    <button type="submit" class="primary" disabled=move || ui.busy.get()>
                        {format!("{verb} revision {number}")}
                    </button>
                    <button
                        type="button"
                        on:click=move |_| {
                            ui.archiving.set(None);
                            focus_later(&format!("archive-open-{number}"));
                        }
                    >
                        "Cancel"
                    </button>
                </div>
            </form>
        }
        .into_any()
    }
}

/// The review form: each field whose CSV value differs, with the part's
/// value and the CSV's, and a box to tick to accept the CSV's. Fields not
/// ticked stay as they are. Values the CSV did not change since it was last
/// applied or reviewed come last, marked, since accepting them undoes an
/// edit. The rows and both buttons follow ui.reviewing, which a group leaves
/// once its review is saved.
#[component]
fn CsvReviewForm(part: RwSignal<Option<TeamPart>>, ui: TeamUi) -> impl IntoView {
    let changes = ui.reviewing.get_untracked().unwrap_or_default();
    let open = move || ui.reviewing.get_untracked().unwrap_or_default();
    let files = csv_files(&changes);
    let intro = format!(
        "{} has new values for this part. Tick the ones to accept. The others stay as they are, and you are not asked about these CSV values again unless the CSV changes.",
        files.join(" and ")
    );
    let prefilled = accept_reason(&files);
    let keep_reason = format!(
        "Keep the values edited in this app, not the CSV values from {}",
        files.join(" and ")
    );
    let rows = move || {
        ui.reviewing
            .get()
            .unwrap_or_default()
            .iter()
            .flat_map(|changes| {
                changes
                    .rows
                    .iter()
                    .map(move |row| (changes.problem.clone(), changes.cad, row.clone()))
            })
            .enumerate()
            .map(|(index, (problem, cad, row))| review_row(ui, index, problem, cad, row))
            .collect_view()
    };
    // The hint about a new revision, while symbol or footprint values are
    // open.
    let cad_hint = move || {
        let has_cad = ui.reviewing.with(|open| {
            open.as_ref()
                .is_some_and(|open| open.iter().any(|changes| changes.cad))
        });
        has_cad.then(|| {
            let (newest, current) = part.with_untracked(|part| {
                part.as_ref().map_or((0, 0), |part| {
                    let newest = part.revisions.iter().map(|revision| revision.number).max();
                    (newest.unwrap_or(0), part.revision.number)
                })
            });
            view! {
                <p class="hint">
                    {format!(
                        "Accepting the Symbol or Footprint adds revision {} of this part. Designs that use revision {current} keep it.",
                        newest + 1
                    )}
                </p>
            }
        })
    };
    view! {
        <form
            class="csv-review-form"
            aria-labelledby="csv-review-heading"
            on:submit=move |event| {
                event.prevent_default();
                let picks = ui.review_picks.get_untracked();
                save_csv_review(ui, part, open(), picks, ui.review_reason.get_untracked());
            }
        >
            <h3 id="csv-review-heading" tabindex="-1">"Review CSV values"</h3>
            <p>{intro}</p>
            <table class="changes" aria-label="New values in the CSV">
                <thead>
                    <tr>
                        <th scope="col" class="pick">"Accept"</th>
                        <th scope="col">"Field"</th>
                        <th scope="col">"Now"</th>
                        <th scope="col">"In the CSV"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
            {cad_hint}
            <label for="csv-review-reason" class="reason-label">"Reason (shown in the history)"</label>
            <textarea
                id="csv-review-reason"
                rows="2"
                prop:value=move || ui.review_reason.get()
                on:input:target=move |event| ui.review_reason.set(event.target().value())
            ></textarea>
            <div class="form-actions">
                <button
                    type="submit"
                    class="primary"
                    disabled=move || ui.busy.get() || ui.review_picks.with(Vec::is_empty)
                >
                    "Accept the ticked values"
                </button>
                <button
                    type="button"
                    disabled=move || ui.busy.get()
                    on:click=move |_| {
                        // The prefilled reason is about accepting values;
                        // keeping them gets its own unless the person wrote
                        // one.
                        let typed = ui.review_reason.get_untracked();
                        let reason = if typed == prefilled { keep_reason.clone() } else { typed };
                        save_csv_review(ui, part, open(), Vec::new(), reason);
                    }
                >
                    "Keep current values"
                </button>
                <button
                    type="button"
                    on:click=move |_| {
                        ui.close_review();
                        focus_later("csv-review-open");
                    }
                >
                    "Cancel"
                </button>
            </div>
        </form>
    }
}

/// One row of the review form: a box to tick, the field, its value now and
/// in the CSV, and a mark when the CSV did not change it.
fn review_row(
    ui: TeamUi,
    index: usize,
    problem: String,
    cad: bool,
    row: CsvChangeRow,
) -> impl IntoView {
    let id = format!("csv-pick-{index}");
    let key = (problem, row.field.clone());
    let checked_key = key.clone();
    let notes: Vec<&str> = [
        cad.then_some("adds a revision"),
        (!row.changed_in_csv).then_some("not changed in the CSV"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let label = if notes.is_empty() {
        format!("Accept the CSV's {}", row.field)
    } else {
        format!("Accept the CSV's {} ({})", row.field, notes.join(", "))
    };
    view! {
        <tr>
            <td class="pick">
                <input
                    type="checkbox"
                    id=id
                    aria-label=label
                    prop:checked=move || ui.review_picks.with(|picks| picks.contains(&checked_key))
                    on:change:target=move |event| {
                        let on = event.target().checked();
                        let key = key.clone();
                        ui.review_picks.update(|picks| {
                            picks.retain(|pick| *pick != key);
                            if on {
                                picks.push(key);
                            }
                        });
                    }
                />
            </td>
            <th scope="row">{row.field.clone()}</th>
            <td class:blank=row.now.is_empty()>{value_view(&row.field, &row.now)}</td>
            <td>
                <span class="csv-value" class:blank=row.csv.is_empty()>{value_view(&row.field, &row.csv)}</span>
                {(!row.changed_in_csv).then(|| view! {
                    <span class="csv-same">"(not changed in the CSV)"</span>
                })}
            </td>
        </tr>
    }
}

/// Sends one review per group of CSV values (details, then symbol and
/// footprint), accepting the ticked fields of each and keeping the rest, and
/// sums up what happened. A group whose review is saved, or was already
/// reviewed, leaves the form; a refusal stops there and says what was saved
/// before it.
fn save_csv_review(
    ui: TeamUi,
    part: RwSignal<Option<TeamPart>>,
    changes: Vec<CsvChanges>,
    picks: Vec<(String, String)>,
    reason: String,
) {
    if reason.trim().is_empty() {
        ui.notice
            .set(Some(Notice::Problem("Say why before you save.".into())));
        return;
    }
    let Some(ipn) = part.with_untracked(|part| part.as_ref().map(|part| part.ipn.clone())) else {
        return;
    };
    let requests: Vec<TeamCsvReviewRequest> = changes
        .iter()
        .map(|changes| {
            let mut request = TeamCsvReviewRequest {
                correlation_id: None,
                ipn: ipn.clone(),
                problem: changes.problem.clone(),
                // In the order the rows show them.
                accept: changes
                    .rows
                    .iter()
                    .filter(|row| picks.contains(&(changes.problem.clone(), row.field.clone())))
                    .map(|row| row.field.clone())
                    .collect(),
                expected: changes.expected,
                reason: reason.clone(),
            };
            let sent = ui.review_attempts.with_value(|attempts| {
                attempts
                    .iter()
                    .find(|(previous, _)| *previous == request)
                    .map(|(_, id)| id.clone())
            });
            let id = sent.unwrap_or_else(|| {
                let id = new_correlation_id();
                ui.review_attempts
                    .update_value(|attempts| attempts.push((request.clone(), id.clone())));
                id
            });
            request.correlation_id = Some(id);
            request
        })
        .collect();
    // From the requests actually sent, so a tick left on a group that is no
    // longer open counts for nothing.
    let took = requests.iter().any(|request| !request.accept.is_empty());
    let kinds: Vec<bool> = changes.iter().map(|changes| changes.cad).collect();
    ui.busy.set(true);
    spawn_local(async move {
        let mut details_saved = false;
        let mut revision = None;
        let mut answered: Vec<bool> = Vec::new();
        let mut applied = false;
        for (request, cad) in requests.into_iter().zip(kinds) {
            let problem = request.problem.clone();
            let result = backend::team_csv_review(request).await;
            // The group leaves the form, with its ticks.
            let done = |ui: TeamUi| {
                ui.reviewing.update(|open| {
                    if let Some(open) = open {
                        open.retain(|changes| changes.problem != problem);
                    }
                });
                ui.review_picks
                    .update(|picks| picks.retain(|(picked, _)| *picked != problem));
            };
            match result {
                Ok(PublishResult::CsvReviewed {
                    details_saved: saved,
                    revision: added,
                    part: latest,
                    ..
                }) => {
                    details_saved |= saved;
                    revision = revision.or(added);
                    applied = true;
                    answered.push(cad);
                    done(ui);
                    part.set(Some(latest));
                }
                // Already reviewed: nothing left to do for this group.
                Ok(PublishResult::NothingToSave { .. }) => done(ui),
                Ok(PublishResult::Refused { message }) | Err(message) => {
                    ui.busy.set(false);
                    let message = if answered.is_empty() {
                        message
                    } else {
                        format!(
                            "Your choice for the {} was saved. Your choice for the {} was not: {message}",
                            kind_words(answered[0]),
                            kind_words(cad)
                        )
                    };
                    ui.notice.set(Some(Notice::Problem(message)));
                    return;
                }
                other => {
                    published(ui, part, other);
                    return;
                }
            }
        }
        // Closed before busy ends, so the form is not taken for one someone
        // else answered.
        ui.close_review();
        ui.busy.set(false);
        ui.selected.set(None);
        let message = match (applied, took, details_saved, revision) {
            (false, _, _, _) => {
                "These CSV values were already reviewed, so nothing was saved.".to_owned()
            }
            (true, false, _, _) => {
                "Kept the current values. You are asked about these CSV values again only if the CSV changes."
                    .to_owned()
            }
            (true, true, true, Some(revision)) => format!(
                "Saved. The part details changed, and the new symbol or footprint is revision {revision}."
            ),
            (true, true, false, Some(revision)) => format!("Saved as revision {revision}."),
            (true, true, _, None) => {
                "Saved. The part details changed; no revision was added.".to_owned()
            }
        };
        ui.notice.set(Some(Notice::Done(message)));
        scroll_column_to_top();
        focus_later("fields-heading");
    });
}

/// "part details" or "symbol and footprint".
fn kind_words(cad: bool) -> &'static str {
    if cad {
        "symbol and footprint"
    } else {
        "part details"
    }
}

/// Moves keyboard focus to the element with `id` once the view has been
/// redrawn.
fn focus_later(id: &str) {
    let id = id.to_owned();
    request_animation_frame(move || {
        use wasm_bindgen::JsCast;
        if let Some(element) = document()
            .get_element_by_id(&id)
            .and_then(|element| element.dyn_into::<leptos::web_sys::HtmlElement>().ok())
        {
            let _ = element.focus();
        }
    });
}

fn save_archive(ui: TeamUi, part: RwSignal<Option<TeamPart>>, request: TeamArchiveRequest) {
    if request.reason.trim().is_empty() {
        let doing = if request.archived {
            "archiving"
        } else {
            "unarchiving"
        };
        ui.notice.set(Some(Notice::Problem(format!(
            "Say why you are {doing} revision {} before you save.",
            request.revision
        ))));
        return;
    }
    let mut request = request;
    request.correlation_id = None;
    request.correlation_id = Some(ui.archive_attempt.id_for(request.clone()));
    ui.busy.set(true);
    spawn_local(async move {
        let result = backend::team_archive(request).await;
        published(ui, part, result);
    });
}

#[component]
fn ConflictView(
    conflict: TeamConflict,
    part: RwSignal<Option<TeamPart>>,
    ui: TeamUi,
) -> impl IntoView {
    let theirs = conflict.theirs.clone();
    let reason = if theirs.reason.is_empty() {
        String::new()
    } else {
        format!(" (“{}”)", theirs.reason)
    };
    // The sentence is split around the save time, which is a <time>.
    let summary_start = format!("{} saved a change on ", theirs.actor_name);
    let summary_end =
        format!("{reason} after you started editing. Your changes are not saved yet.");
    let rows = conflict.rows.clone();
    let save_merged = conflict.merged.clone();
    let edit_merged = conflict.merged.clone();
    let actions = view! {
        <div class="form-actions">
            <button
                type="button"
                class="primary"
                disabled=move || ui.busy.get()
                on:click=move |_| save_edit(ui, part, save_merged.clone())
            >
                "Save my changes on top of theirs"
            </button>
            <button
                type="button"
                on:click=move |_| ui.start_editing(draft_from_request(&edit_merged))
            >
                "Edit again"
            </button>
            <button
                type="button"
                on:click=move |_| {
                    ui.conflict.set(None);
                    ui.notice.set(Some(Notice::Done("Your changes were discarded.".into())));
                }
            >
                "Discard my changes"
            </button>
        </div>
    };
    view! {
        <section class="conflict" role="alert" aria-labelledby="conflict-heading">
            <h3 id="conflict-heading">"Someone else changed this part first"</h3>
            <p>{summary_start}{saved_at_view(theirs.saved_at.clone())}{summary_end}</p>
            <table class="changes" aria-label="Your changes and theirs">
                <thead>
                    <tr>
                        <th scope="col">"Field"</th>
                        <th scope="col">"You started from"</th>
                        <th scope="col">"Theirs"</th>
                        <th scope="col">"Yours"</th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| {
                            let started_from = value_view(&row.label, &row.started_from);
                            let theirs = value_view(&row.label, &row.theirs);
                            let yours = value_view(&row.label, &row.yours);
                            view! {
                                <tr class:both-changed=row.both_changed>
                                    <th scope="row">{row.label}</th>
                                    <td class:blank=row.started_from.is_empty()>{started_from}</td>
                                    <td class:blank=row.theirs.is_empty()>{theirs}</td>
                                    {if row.you_changed {
                                        view! { <td class:blank=row.yours.is_empty()>{yours}</td> }.into_any()
                                    } else {
                                        view! { <td class="blank">"not changed"</td> }.into_any()
                                    }}
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
            <p class="hint">
                "Saving on top keeps their changes and adds yours. Where you both changed a field, yours is kept."
            </p>
            {actions}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::machine_time;

    #[test]
    fn save_times_get_a_machine_readable_form() {
        assert_eq!(
            machine_time("2026-10-09 18:02 UTC").as_deref(),
            Some("2026-10-09T18:02Z")
        );
        assert_eq!(machine_time("yesterday"), None);
    }
}

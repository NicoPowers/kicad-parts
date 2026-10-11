//! The team catalog in the desktop UI: the sign-in badge in the header,
//! and, for a part the team catalog has, its details, history, edits and
//! conflicts in the right-hand column of the part panel.

use crate::backend;
use desktop_contract::{
    DetailChange, Field, LiveUpdates, ManufacturerPart, PublishResult, RevisionSummary,
    SignInRequest, TeamConflict, TeamEditRequest, TeamPart, TeamSession, TeamStatus,
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
    pub base: u32,
    pub fields: Vec<Field>,
    pub parts: Vec<ManufacturerPart>,
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
    /// The revision selected in the history.
    pub selected: RwSignal<Option<u32>>,
    pub busy: RwSignal<bool>,
    /// The last save sent and its correlation ID: sending the same save
    /// again (after a lost answer) reuses the ID, so the server replays its
    /// answer instead of reporting a conflict with yourself.
    pub attempt: StoredValue<Option<(Attempt, String)>>,
    /// The ID made when the edit form opened, for its first save.
    pub opened_id: StoredValue<String>,
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
            busy: RwSignal::new(false),
            attempt: StoredValue::new(None),
            opened_id: StoredValue::new(new_correlation_id()),
        }
    }

    pub fn reset(&self) {
        self.attempt.set_value(None);
        self.opened_id.set_value(new_correlation_id());
        self.tab.set(Tab::Details);
        self.editing.set(None);
        self.conflict.set(None);
        self.notice.set(None);
        self.selected.set(None);
        self.busy.set(false);
    }

    fn start_editing(&self, draft: Draft) {
        self.attempt.set_value(None);
        self.opened_id.set_value(new_correlation_id());
        self.draft.set_value(draft);
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
        base: part.current.number,
        fields: part.current.fields.clone(),
        parts: part.current.manufacturer_parts.clone(),
        reason: String::new(),
    }
}

fn draft_from_request(request: &TeamEditRequest) -> Draft {
    Draft {
        ipn: request.ipn.clone(),
        base: request.expected_revision,
        fields: request.fields.clone(),
        parts: request.manufacturer_parts.clone(),
        reason: request.reason.clone(),
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
fn published(
    ui: TeamUi,
    part: RwSignal<Option<TeamPart>>,
    result: Result<PublishResult, String>,
    done: impl FnOnce(u32) -> String,
) {
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
            ui.notice.set(Some(Notice::Done(done(revision))));
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

/// One save as sent, without its correlation ID.
#[derive(Clone, Debug, PartialEq)]
pub enum Attempt {
    Edit(TeamEditRequest),
}

/// A new correlation ID: letters, digits and dashes, as the server needs.
fn new_correlation_id() -> String {
    let now = js_sys::Date::now() as u64;
    let random = (js_sys::Math::random() * 1e15) as u64;
    format!("desktop-{now:x}-{random:x}")
}

/// The correlation ID for `attempt`: the previous one if this is the same
/// save sent again, the form's own ID for its first save, otherwise new.
fn attempt_id(ui: TeamUi, attempt: Attempt) -> String {
    let repeated = ui.attempt.with_value(|last| match last {
        Some((previous, id)) if *previous == attempt => Some(id.clone()),
        _ => None,
    });
    if let Some(id) = repeated {
        return id;
    }
    let id = if ui.attempt.with_value(Option::is_none) {
        ui.opened_id.get_value()
    } else {
        new_correlation_id()
    };
    ui.attempt.set_value(Some((attempt, id.clone())));
    id
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
    request.correlation_id = Some(attempt_id(ui, Attempt::Edit(request.clone())));
    ui.busy.set(true);
    spawn_local(async move {
        let result = backend::team_edit(request).await;
        published(ui, part, result, |revision| {
            format!("Saved as revision {revision}.")
        });
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
                {move || match ui.editing.get() {
                    Some(_) => view! { <EditForm part=part ui=ui /> }.into_any(),
                    None => view! { <DetailsView part=part can_edit=can_edit ui=ui /> }.into_any(),
                }}
            </div>
            <div
                id="team-panel-history"
                role="tabpanel"
                aria-labelledby="team-tab-history"
                hidden=move || ui.tab.get() != Tab::History || ui.conflict.with(Option::is_some)
            >
                <HistoryView part=part ui=ui />
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

fn is_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

#[component]
fn DetailsView(part: RwSignal<Option<TeamPart>>, can_edit: bool, ui: TeamUi) -> impl IntoView {
    move || {
        let Some(team_part) = part.get() else {
            return ().into_any();
        };
        let current = team_part.current.clone();
        let count = current.fields.len();
        let for_edit = team_part.clone();
        view! {
            <div class="team-toolbar">
                <h3 id="fields-heading">{format!("Part details ({count})")}</h3>
                {if can_edit {
                    view! {
                        <button
                            type="button"
                            class="primary"
                            on:click=move |_| ui.start_editing(draft_of(&for_edit))
                        >
                            "Edit details"
                        </button>
                    }
                    .into_any()
                } else {
                    view! { <p class="hint read-only">"You can view this part but not edit it."</p> }
                        .into_any()
                }}
            </div>
            <table aria-labelledby="fields-heading">
                <thead>
                    <tr><th scope="col">"Field"</th><th scope="col">"Value"</th></tr>
                </thead>
                <tbody>
                    {current
                        .fields
                        .iter()
                        .map(|field| {
                            let url = is_url(&field.value);
                            let title = url.then(|| field.value.clone());
                            view! {
                                <tr>
                                    <th scope="row">{field.name.clone()}</th>
                                    <td class:field-url=url class:blank=field.value.is_empty() title=title>
                                        {shown(&field.value)}
                                    </td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
            <h3 id="manufacturer-parts-heading">"Manufacturer parts"</h3>
            {if current.manufacturer_parts.is_empty() {
                view! { <p class="hint">"None listed."</p> }.into_any()
            } else {
                view! {
                    <ol class="manufacturer-parts" aria-labelledby="manufacturer-parts-heading">
                        {current
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
            <h3 id="cad-items-heading">"Symbol and footprint used"</h3>
            <table aria-labelledby="cad-items-heading">
                <tbody>
                    {current
                        .cad_items
                        .iter()
                        .map(|item| view! {
                            <tr>
                                <th scope="row">{item.role.clone()}</th>
                                <td><code>{item.reference.clone()}</code>{format!(", {}", item.binding)}</td>
                            </tr>
                        })
                        .collect_view()}
                </tbody>
            </table>
        }
        .into_any()
    }
}

#[component]
fn EditForm(part: RwSignal<Option<TeamPart>>, ui: TeamUi) -> impl IntoView {
    let draft = ui.draft.get_value();
    let base = draft.base;
    let fields = draft
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let id = format!("edit-field-{index}");
            let value = field.value.clone();
            view! {
                <div class="edit-row">
                    <label for=id.clone()>{field.name.clone()}</label>
                    <input
                        id=id
                        type="text"
                        spellcheck="false"
                        prop:value=value
                        on:input:target=move |event| {
                            let value = event.target().value();
                            ui.draft.update_value(|draft| draft.fields[index].value = value);
                        }
                    />
                </div>
            }
        })
        .collect_view();
    let save = move || {
        let draft = ui.draft.get_value();
        save_edit(
            ui,
            part,
            TeamEditRequest {
                correlation_id: None,
                ipn: draft.ipn,
                expected_revision: draft.base,
                reason: draft.reason,
                fields: draft.fields,
                manufacturer_parts: draft.parts,
            },
        );
    };
    view! {
        <form
            class="edit-form"
            aria-labelledby="edit-heading"
            on:submit=move |event| {
                event.prevent_default();
                save();
            }
        >
            <h3 id="edit-heading">{format!("Edit details (starting from revision {base})")}</h3>
            {move || {
                part.with(|part| {
                    part.as_ref()
                        .filter(|part| part.current.number > base)
                        .map(|part| view! {
                            <p class="hint">
                                {format!(
                                    "Someone else saved revision {} while you were editing. When you save, you can compare both versions.",
                                    part.current.number,
                                )}
                            </p>
                        })
                })
            }}
            <fieldset>
                <legend>"Fields"</legend>
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
                    .map(|change| view! {
                        <tr>
                            <th scope="row">{change.label}</th>
                            <td class="before" class:blank=change.before.is_empty()>{shown(&change.before)}</td>
                            <td class="after" class:blank=change.after.is_empty()>{shown(&change.after)}</td>
                        </tr>
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

fn revision_title(revision: &RevisionSummary) -> String {
    format!(
        "Revision {} · {} by {}",
        revision.number, revision.action, revision.actor_name
    )
}

#[component]
fn HistoryView(part: RwSignal<Option<TeamPart>>, ui: TeamUi) -> impl IntoView {
    move || {
        let Some(team_part) = part.get() else {
            return ().into_any();
        };
        let current = team_part.current.number;
        let chosen = ui.selected.get().unwrap_or(current);
        let selected = team_part
            .history
            .iter()
            .find(|revision| revision.number == chosen)
            .or(team_part.history.first())
            .cloned();
        view! {
            <h3 id="history-heading">"History"</h3>
            <ol class="history" aria-labelledby="history-heading">
                {team_part
                    .history
                    .iter()
                    .map(|revision| {
                        let number = revision.number;
                        let is_selected = number == chosen;
                        let reason = if revision.reason.is_empty() {
                            String::new()
                        } else {
                            format!("“{}”", revision.reason)
                        };
                        view! {
                            <li>
                                <button
                                    type="button"
                                    class="history-entry"
                                    aria-pressed=if is_selected { "true" } else { "false" }
                                    on:click=move |_| ui.selected.set(Some(number))
                                >
                                    <span class="history-title">
                                        {revision_title(revision)}
                                        {(number == current).then_some(" (current)")}
                                    </span>
                                    <span class="history-when">{saved_at_view(revision.saved_at.clone())}</span>
                                    <span class="history-reason">{reason}</span>
                                </button>
                            </li>
                        }
                    })
                    .collect_view()}
            </ol>
            {selected.map(|revision| view! {
                <RevisionView revision=revision />
            })}
        }
        .into_any()
    }
}

#[component]
fn RevisionView(revision: RevisionSummary) -> impl IntoView {
    let number = revision.number;
    let what_changed = if number == 1 {
        view! { <p class="hint">"The first revision of this part."</p> }.into_any()
    } else if revision.changes.is_empty() {
        view! { <p class="hint">"Nothing changed in this revision."</p> }.into_any()
    } else {
        changes_table(
            format!("What changed in revision {number}"),
            "Before",
            "After",
            revision.changes.clone(),
        )
        .into_any()
    };
    view! {
        <section class="revision" aria-labelledby="revision-heading">
            <h4 id="revision-heading">{format!("What changed in revision {number}")}</h4>
            <p class="history-when">
                {format!("{} by {} on ", revision.action, revision.actor_name)}
                {saved_at_view(revision.saved_at.clone())}
            </p>
            {what_changed}
        </section>
    }
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
    let summary_start = format!("{} saved revision {} on ", theirs.actor_name, theirs.number);
    let summary_end = format!(
        "{reason} after you started editing revision {}. Your changes are not saved yet.",
        conflict.started_from,
    );
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
            <table class="changes" aria-label="Your version and theirs">
                <thead>
                    <tr>
                        <th scope="col">"Field"</th>
                        <th scope="col">{format!("Revision {} (you started here)", conflict.started_from)}</th>
                        <th scope="col">{format!("Theirs (revision {})", theirs.number)}</th>
                        <th scope="col">"Yours"</th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! {
                            <tr class:both-changed=row.both_changed>
                                <th scope="row">{row.label}</th>
                                <td class:blank=row.started_from.is_empty()>{shown(&row.started_from)}</td>
                                <td class:blank=row.theirs.is_empty()>{shown(&row.theirs)}</td>
                                {if row.you_changed {
                                    view! { <td class:blank=row.yours.is_empty()>{shown(&row.yours)}</td> }.into_any()
                                } else {
                                    view! { <td class="blank">"not changed"</td> }.into_any()
                                }}
                            </tr>
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

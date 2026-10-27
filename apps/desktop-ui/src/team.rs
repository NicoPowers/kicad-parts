//! The team catalog in the desktop UI: for a part a team's catalog has,
//! its details and current revision, history, edits and conflicts in
//! the right-hand column of the part panel. The Teams menu in the header is
//! in `teams.rs`.
//!
//! Part details are shared by every revision, so saving them adds no
//! revision; only a new symbol or footprint does. A bad revision is
//! archived, never deleted: the current revision is the newest one that is
//! not archived.

use crate::backend;
use crate::edit_cad::CadFields;
use crate::focus::focus_later;
use crate::viewer::{CloseUp, CloseUpDialog, Picture};
use desktop_contract::{
    DetailChange, Field, FileKind, HistoryEntry, ManufacturerPart, OlderUse, PreviewKind,
    PreviewOutcome, PublishResult, TeamArchiveRequest, TeamCategory, TeamConflict, TeamEditRequest,
    TeamFile, TeamPart,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

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
    /// The code of the category chosen.
    pub category: String,
    /// The category and fields of the details the edit starts from, so
    /// choosing another category can add its fields to them.
    pub start_category: String,
    pub start_fields: Vec<Field>,
    pub fields: Vec<Field>,
    pub parts: Vec<ManufacturerPart>,
    /// The Symbol and Footprint fields.
    pub cad_fields: Vec<Field>,
    pub reason: String,
    /// Files chosen for the Symbol, the Footprint and its 3D models.
    pub files: Vec<TeamFile>,
    /// Said when saving while a chosen file has no library chosen yet.
    pub unchosen_library: Option<String>,
}

/// The state of the team column for the selected part. Reset when another
/// part is selected.
#[derive(Clone, Copy)]
pub struct TeamUi {
    /// The server address of the team whose part the column shows. Every
    /// save names it, so it reaches that part's own team.
    pub team: RwSignal<String>,
    /// The team's server cannot be reached now, which a note above the
    /// column explains; the column then offers no edits.
    pub offline: RwSignal<bool>,
    pub tab: RwSignal<Tab>,
    /// Some while the edit form is open; a new number re-renders it.
    pub editing: RwSignal<Option<u32>>,
    pub draft: StoredValue<Draft>,
    /// Bumped when manufacturer parts are added or removed.
    pub parts_version: RwSignal<u32>,
    /// How many symbols, footprints or files Edit part is reading.
    pub reading_files: RwSignal<u32>,
    /// Something said under Symbol or Footprint stops the save, so Save is
    /// off.
    pub cad_blocked: RwSignal<bool>,
    /// Why choosing a symbol or footprint did not work: its role, its row
    /// and the plain words.
    pub cad_problem: RwSignal<Option<(PreviewKind, u32, String)>>,
    /// Bumped when another category is chosen, which changes the fields.
    pub category_version: RwSignal<u32>,
    pub conflict: RwSignal<Option<TeamConflict>>,
    pub notice: RwSignal<Option<Notice>>,
    /// The history entry selected, by its Change's number.
    pub selected: RwSignal<Option<u64>>,
    /// The open archive form: the revision, true to archive it or false to
    /// unarchive it, and the part's current revision when the form opened.
    pub archiving: RwSignal<Option<(u32, bool, u32)>>,
    pub archive_reason: RwSignal<String>,
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
    pub(crate) fn new() -> Self {
        Self {
            last: StoredValue::new(None),
            opened_id: StoredValue::new(new_correlation_id()),
        }
    }

    /// Starts afresh, as when its form opens: the next save gets a new ID.
    pub(crate) fn open(&self) {
        self.last.set_value(None);
        self.opened_id.set_value(new_correlation_id());
    }

    /// The correlation ID for `request`: the previous one if this is the
    /// same save sent again, the form's own ID for its first save,
    /// otherwise new.
    pub(crate) fn id_for(&self, request: T) -> String {
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
            team: RwSignal::new(String::new()),
            offline: RwSignal::new(false),
            tab: RwSignal::new(Tab::Details),
            editing: RwSignal::new(None),
            draft: StoredValue::new(Draft::default()),
            parts_version: RwSignal::new(0),
            reading_files: RwSignal::new(0),
            cad_problem: RwSignal::new(None),
            cad_blocked: RwSignal::new(false),
            category_version: RwSignal::new(0),
            conflict: RwSignal::new(None),
            notice: RwSignal::new(None),
            selected: RwSignal::new(None),
            archiving: RwSignal::new(None),
            archive_reason: RwSignal::new(String::new()),
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
        self.busy.set(false);
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

    pub(crate) fn start_editing(&self, draft: Draft) {
        self.edit_attempt.open();
        self.cad_problem.set(None);
        self.cad_blocked.set(false);
        self.draft.set_value(draft);
        self.conflict.set(None);
        self.notice.set(None);
        self.tab.set(Tab::Details);
        self.editing
            .update(|editing| *editing = Some(editing.map_or(1, |n| n + 1)));
    }
}

pub(crate) fn draft_of(part: &TeamPart) -> Draft {
    Draft {
        ipn: part.ipn.clone(),
        base_details: part.details.number,
        base_revision: part.revision.number,
        category: part.details.category.clone(),
        start_category: part.details.category.clone(),
        start_fields: part.details.fields.clone(),
        fields: part.details.fields.clone(),
        parts: part.details.manufacturer_parts.clone(),
        cad_fields: part.revision.cad_fields.clone(),
        reason: String::new(),
        files: Vec::new(),
        unchosen_library: None,
    }
}

/// The draft of `request`, an edit that starts from `part` as it is now.
fn draft_from_request(request: &TeamEditRequest, part: &TeamPart) -> Draft {
    Draft {
        ipn: request.ipn.clone(),
        base_details: request.expected_details,
        base_revision: request.expected_revision,
        category: if request.category.is_empty() {
            part.details.category.clone()
        } else {
            request.category.clone()
        },
        start_category: part.details.category.clone(),
        start_fields: part.details.fields.clone(),
        fields: request.fields.clone(),
        parts: request.manufacturer_parts.clone(),
        cad_fields: request.cad_fields.clone(),
        reason: request.reason.clone(),
        files: request.files.clone(),
        unchosen_library: None,
    }
}

fn request_of(draft: Draft) -> TeamEditRequest {
    TeamEditRequest {
        correlation_id: None,
        ipn: draft.ipn,
        expected_details: draft.base_details,
        expected_revision: draft.base_revision,
        reason: draft.reason,
        category: draft.category,
        fields: draft.fields,
        manufacturer_parts: draft.parts,
        cad_fields: draft.cad_fields,
        files: draft.files,
    }
}

/// Moves the draft to `category`: the fields the edit started from, then,
/// blank, the fields of the new category they lack. Values already typed
/// are kept. Nothing is dropped, and the IPN never changes.
fn choose_category(draft: &mut Draft, category: &TeamCategory) {
    let mut wanted = draft.start_fields.clone();
    if category.code != draft.start_category {
        for name in &category.fields {
            if !wanted.iter().any(|field| &field.name == name) {
                wanted.push(Field {
                    name: name.clone(),
                    value: String::new(),
                });
            }
        }
    }
    draft.fields = wanted
        .into_iter()
        .map(|field| {
            let typed = draft.fields.iter().find(|typed| typed.name == field.name);
            typed.cloned().unwrap_or(field)
        })
        .collect();
    draft.category.clone_from(&category.code);
}

/// Scrolls the right-hand column to its top, where the notice or the
/// conflict is.
pub(crate) fn scroll_column_to_top() {
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
            older_uses,
        }) => {
            part.set(Some(saved));
            ui.editing.set(None);
            ui.conflict.set(None);
            ui.selected.set(None);
            let mut message = match revision {
                Some(revision) => format!("Saved as revision {revision}."),
                None => "Saved. The part details changed; no revision was added.".to_owned(),
            };
            for older in &older_uses {
                message.push(' ');
                message.push_str(&older_use_words(older));
            }
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
        // The save worked; the part shown updates by itself once the app
        // reaches the team server again.
        Ok(PublishResult::SavedNotShown { message }) => {
            ui.editing.set(None);
            ui.conflict.set(None);
            ui.archiving.set(None);
            ui.archive_reason.set(String::new());
            ui.notice.set(Some(Notice::Done(message)));
            scroll_column_to_top();
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
        Ok(PublishResult::NothingToSave { message }) => ui.notice.set(Some(Notice::Done(message))),
        Ok(PublishResult::Conflict(conflict)) => {
            part.set(Some(conflict.part.clone()));
            ui.editing.set(None);
            ui.notice.set(None);
            ui.conflict.set(Some(*conflict));
            scroll_column_to_top();
        }
        Ok(PublishResult::Refused { message }) => ui.notice.set(Some(Notice::Problem(message))),
        // Only Add part answers these; see AddPartForm.
        Ok(PublishResult::Added { part: saved }) => part.set(Some(saved)),
        Ok(PublishResult::IpnTaken { message, .. }) => {
            ui.notice.set(Some(Notice::Problem(message)))
        }
        Err(message) => ui.notice.set(Some(Notice::Problem(message))),
    }
}

/// `28 other parts still use an older revision of the footprint X.`
fn older_use_words(older: &OlderUse) -> String {
    let (count, verb) = if older.other_parts == 1 {
        ("1 other part".to_owned(), "uses")
    } else {
        (format!("{} other parts", older.other_parts), "use")
    };
    format!(
        "{count} still {verb} an older revision of the {} {}.",
        older.kind, older.reference
    )
}

/// A new correlation ID: letters, digits and dashes, as the server needs.
fn new_correlation_id() -> String {
    let now = js_sys::Date::now() as u64;
    let random = (js_sys::Math::random() * 1e15) as u64;
    format!("desktop-{now:x}-{random:x}")
}

fn save_edit(ui: TeamUi, part: RwSignal<Option<TeamPart>>, request: TeamEditRequest) {
    if let Some(message) = ui.draft.with_value(|draft| draft.unchosen_library.clone()) {
        ui.notice.set(Some(Notice::Problem(message)));
        return;
    }
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
    let team = ui.team.get_untracked();
    spawn_local(async move {
        let result = backend::team_edit(&team, request).await;
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
    // If the revision whose form is open is archived or unarchived
    // elsewhere (by someone else, or in another window), close the form
    // and say so plainly, rather than turning it into the opposite action
    // with the reason still in it.
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
            let state = if wanted {
                "already archived"
            } else {
                "already in use again"
            };
            ui.notice
                .set(Some(Notice::Done(format!("Revision {number} is {state}."))));
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
pub(crate) fn shown(value: &str) -> String {
    if value.is_empty() {
        "(blank)".to_owned()
    } else {
        value.to_owned()
    }
}

/// A field's value as [`shown`] gives it, with a symbol or footprint
/// reference kept whole up to its colon ([`crate::cad_reference`]).
fn value_view(field: &str, value: &str) -> AnyView {
    if field.ends_with(" file") && !value.is_empty() {
        // A file and the CAD item revision it made, such as
        // `R.kicad_mod (footprint revision 2 of g-Resistor_SMD:R)`: the
        // reference after "of" is kept whole up to its colon.
        let reference = value
            .rsplit_once(" of ")
            .map(|(_, reference)| reference.trim_end_matches(')'));
        crate::text_with_reference(value, reference)
    } else if crate::is_cad_field(field) && !value.is_empty() {
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
        view! {
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
                } else if ui.offline.get() {
                    // The note about the offline team says why.
                    ().into_any()
                } else {
                    view! { <p class="hint read-only">"You can view this part but not edit it."</p> }
                        .into_any()
                }}
            </div>
            {(!details.category_name.is_empty()).then(|| view! {
                <p class="part-category" id="part-category">
                    "Category: "<strong>{details.category_name.clone()}</strong>
                </p>
            })}
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
    // The fields follow the category chosen.
    let fields = move || {
        ui.category_version.track();
        let fields = ui.draft.with_value(|draft| draft.fields.clone());
        field_inputs(fields, "edit-field", ui, |draft, index, value| {
            draft.fields[index].value = value;
        })
    };
    let categories = part.with_untracked(|part| {
        part.as_ref()
            .map(|part| part.categories.clone())
            .unwrap_or_default()
    });
    let category_choice = (!categories.is_empty()).then(|| {
        let chosen = draft.category.clone();
        let start = draft.start_category.clone();
        // A category the list does not have (renamed or gone) shows as its
        // code, rather than silently choosing another.
        let missing = (!start.is_empty() && !categories.iter().any(|category| category.code == start))
            .then(|| {
                view! {
                    <option value=start.clone() selected=chosen == start>
                        {format!("{start} (not in the list)")}
                    </option>
                }
            });
        let options = categories
            .iter()
            .map(|category| {
                view! {
                    <option value=category.code.clone() selected=category.code == chosen>
                        {category.name.clone()}
                    </option>
                }
            })
            .collect_view();
        // What the hint says, or nothing while the category is the one the
        // edit started from.
        let hint = move || {
            ui.category_version.track();
            ui.draft.with_value(|draft| {
                (draft.category != draft.start_category).then_some({
                    if draft.fields.len() > draft.start_fields.len() {
                        "The IPN stays the same. Fields of the new category are added below."
                    } else {
                        "The IPN stays the same."
                    }
                })
            })
        };
        view! {
            <div class="edit-row">
                <label for="edit-category">"Category"</label>
                <select
                    id="edit-category"
                    aria-describedby=move || hint().map(|_| "edit-category-hint")
                    on:change:target=move |event| {
                        let code = event.target().value();
                        let category = categories
                            .iter()
                            .find(|category| category.code == code)
                            .cloned()
                            .or_else(|| {
                                (code == start).then(|| TeamCategory {
                                    code: code.clone(),
                                    name: code.clone(),
                                    fields: Vec::new(),
                                })
                            });
                        if let Some(category) = category {
                            ui.draft.update_value(|draft| choose_category(draft, &category));
                            ui.category_version.update(|version| *version += 1);
                        }
                    }
                >
                    {missing}
                    {options}
                </select>
            </div>
            {move || hint().map(|text| view! { <p class="hint" id="edit-category-hint">{text}</p> })}
        }
    });
    let ipn = draft.ipn.clone();
    // The part's Symbol and Footprint as they are now, which Undo puts back.
    let before: Vec<(String, String)> = part.with_untracked(|part| {
        part.as_ref()
            .map(|part| {
                part.revision
                    .cad_fields
                    .iter()
                    .map(|field| (field.name.clone(), field.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    });
    let cad_fields = view! { <CadFields ui=ui ipn=ipn.clone() before=before /> };
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
                {category_choice}
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
                    {move || (ui.reading_files.get() > 0).then(|| view! { <p class="hint" aria-busy="true">"Reading…"</p> })}
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
            {move || ui.cad_blocked.get().then(|| view! {
                <p class="problem cad-blocked" id="edit-blocked">
                    "Save is off until what is said under Symbol or Footprint is fixed."
                </p>
            })}
            <div class="form-actions">
                <button type="submit" class="primary" disabled=move || ui.busy.get() || ui.cad_blocked.get()>
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

/// The largest file the app sends: a save sends at most 32 MB, and a file
/// that is not text grows by a third on the way.
pub(crate) const MAX_FILE_BYTES: f64 = 24.0 * 1024.0 * 1024.0;

/// The extensions a file for `kind` may have, and what to say otherwise.
pub(crate) fn accepted(kind: FileKind) -> (&'static [&'static str], &'static str) {
    match kind {
        FileKind::Symbol => (
            &[".kicad_sym"],
            "Choose a symbol library file (.kicad_sym) for the symbol.",
        ),
        FileKind::Footprint => (
            &[".kicad_mod"],
            "Choose a footprint file (.kicad_mod) for the footprint.",
        ),
        FileKind::Model3d => (
            &[".step", ".stp", ".wrl"],
            "Choose a 3D model file (.step, .stp or .wrl).",
        ),
    }
}

/// The file chosen in a file input, read: its name and bytes, or `None`
/// when none is chosen. Refuses, plainly, a file of the wrong kind or one
/// too big to send.
pub(crate) async fn read_chosen(
    input: leptos::web_sys::HtmlInputElement,
    kind: FileKind,
) -> Result<Option<(String, Vec<u8>)>, String> {
    use wasm_bindgen::JsCast;
    let unreadable = || "The file could not be read.".to_owned();
    let files = js_sys::Reflect::get(&input, &"files".into()).map_err(|_| unreadable())?;
    let file = js_sys::Reflect::get(&files, &0u32.into()).map_err(|_| unreadable())?;
    if file.is_undefined() || file.is_null() {
        return Ok(None);
    }
    let name = js_sys::Reflect::get(&file, &"name".into())
        .ok()
        .and_then(|name| name.as_string())
        .unwrap_or_default();
    let (extensions, wrong) = accepted(kind);
    let lower = name.to_lowercase();
    if !extensions
        .iter()
        .any(|extension| lower.ends_with(extension))
    {
        return Err(wrong.to_owned());
    }
    let size = js_sys::Reflect::get(&file, &"size".into())
        .ok()
        .and_then(|size| size.as_f64())
        .unwrap_or(0.0);
    if size > MAX_FILE_BYTES {
        return Err(format!(
            "{name} is too big to send: a file can be at most 24 MB."
        ));
    }
    let method = js_sys::Reflect::get(&file, &"arrayBuffer".into())
        .ok()
        .and_then(|method| method.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(unreadable)?;
    let promise = method
        .call0(&file)
        .ok()
        .and_then(|promise| promise.dyn_into::<js_sys::Promise>().ok())
        .ok_or_else(unreadable)?;
    let buffer = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|_| unreadable())?;
    Ok(Some((name, js_sys::Uint8Array::new(&buffer).to_vec())))
}

/// Standard base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for position in 0..4 {
            if position <= chunk.len() {
                text.push(char::from(
                    ALPHABET[((triple >> (18 - 6 * position)) & 63) as usize],
                ));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// The drawing of a chosen symbol or footprint file. A click on it opens
/// the close-up.
#[component]
pub(crate) fn FilePreview(
    kind: FileKind,
    preview: RwSignal<Option<Result<PreviewOutcome, String>>>,
    previewing: RwSignal<bool>,
    /// Drawn from a library item chosen in the browser, not from a file.
    #[prop(optional)]
    chosen: bool,
    /// 3D model files chosen with the footprint, for its 3D view.
    #[prop(optional, into)]
    models: Option<Signal<Vec<TeamFile>>>,
    /// Add part: the download's 3D model, used as saving links it.
    #[prop(optional, into)]
    download_model: Option<Signal<Option<TeamFile>>>,
    /// Add part: the footprint is from a download, so its 3D view draws
    /// only the download's own model.
    #[prop(optional, into)]
    from_download: Option<Signal<bool>>,
) -> impl IntoView {
    let (what, canvas) = if kind == FileKind::Symbol {
        ("symbol", "canvas canvas-symbol file-canvas")
    } else {
        ("footprint", "canvas canvas-footprint file-canvas")
    };
    // A click on a drawing opens it in a close-up that zooms.
    let close_up = CloseUp::new();
    let drawings = move || {
        if previewing.get() {
            return view! {
                <p class="loading" aria-busy="true">{format!("Drawing the {what} with kicad-cli…")}</p>
            }
            .into_any();
        }
        match preview.get() {
            Some(Ok(PreviewOutcome::Ready(set))) => {
                let alt = if chosen {
                    format!("Preview of the {what} {}", set.reference)
                } else {
                    format!("Preview of the {what} {} from the file", set.reference)
                };
                let footprint = kind == FileKind::Footprint;
                view! {
                    <div class="file-preview">
                        {set
                            .images
                            .into_iter()
                            .map(|image| {
                                let picture = Picture::new(&image, alt.clone(), footprint);
                                view! {
                                    <button
                                        type="button"
                                        class=canvas
                                        title="Enlarge"
                                        on:click=move |_| {
                                            // The 3D models chosen by now.
                                            let files = models.map(|models| models.get_untracked()).unwrap_or_default();
                                            let download = download_model.and_then(|model| model.get_untracked());
                                            let downloaded = from_download.is_some_and(|from| from.get_untracked());
                                            close_up.open(picture.clone().with_models(files, download, downloaded));
                                        }
                                    >
                                        <img src=image.data_uri alt=alt.clone() />
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                }
                .into_any()
            }
            Some(Ok(PreviewOutcome::Problem(problem))) => {
                view! { <p class="problem file-problem" role="alert">{problem.message}</p> }
                    .into_any()
            }
            Some(Err(message)) => {
                view! { <p class="problem file-problem" role="alert">{message}</p> }.into_any()
            }
            None => ().into_any(),
        }
    };
    view! {
        {drawings}
        <CloseUpDialog close_up=close_up />
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

/// `Revision 1 (current) · Added by Ada` for an entry that
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
        (None, None) => "Part details".to_owned(),
    };
    format!("{what} · {} by {}", entry.action, entry.actor_name)
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
    } else if entry.revision == Some(1) && !entry.changes.is_empty() {
        // A part added in the app: where it came from and the files it
        // was made from.
        let rows = entry.changes.clone();
        view! {
            <p class="hint">"The first revision of this part, made from these files:"</p>
            <table class="changes" aria-label="Where the part came from">
                <tbody>
                    {rows
                        .into_iter()
                        .map(|change| {
                            let value = value_view(&change.label, &change.after);
                            view! {
                                <tr>
                                    <th scope="row">{change.label}</th>
                                    <td>{value}</td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
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
            format!(
                "This app offers revision {number} again, and KiCad no longer lists it under Archived revisions. {then}"
            )
        } else {
            format!(
                "This app stops offering revision {number}, and KiCad lists it under Archived revisions, so nobody new picks it by mistake. Designs that already use it keep it. Nothing is deleted. {then}"
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
    let team = ui.team.get_untracked();
    spawn_local(async move {
        let result = backend::team_archive(&team, request).await;
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
    let conflict_part = conflict.part.clone();
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
                on:click=move |_| ui.start_editing(draft_from_request(&edit_merged, &conflict_part))
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

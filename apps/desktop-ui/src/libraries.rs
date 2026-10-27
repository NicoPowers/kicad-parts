//! The Libraries browser (kicad-parts-d152): a dialog, opened from the
//! header, over our libraries, each linked team's library repo (as its
//! library layout was reviewed, kicad-parts-e9a3), KiCad's own and the
//! person's installed ones. Libraries on the left in three groups, a searchable and filtered
//! list of symbols or footprints in the middle, and the chosen item's
//! drawing and details on the right.
//!
//! Opened from the header it only browses: an item of ours not in the
//! Catalog offers Add part from it, an item the Catalog uses offers Show
//! the parts that use it, and a symbol offers its matching footprints.
//! Edit part and Add part (kicad-parts-bc6f) open it with [`BrowseFor`]:
//! one role, the current item selected, and a callback for the choice.

use crate::focus::focus_later;
use crate::team_marks::{ListTeams, TeamChip, TeamMark};
use crate::{Load, PreviewSection, backend, cad_reference, counted, grouped, into_load};
use desktop_contract::{
    LibrariesView, LibraryCheckView, LibraryEntry, LibraryFilters, LibraryGroup, LibraryItemDetail,
    LibraryItemRequest, LibraryItemRow, LibraryItems, LibraryItemsRequest, LibraryKey, LibraryTeam,
    MountFilter, PartKey, PreviewKind, PreviewOutcome, PreviewProblemReason, TeamLibraryChoice,
    TeamPickRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Rows the list shows; the count says how many match in all.
const ROWS: usize = 200;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// How often the dialog asks how far the background reading is.
const PROGRESS_POLL: Duration = Duration::from_millis(400);

/// What the browser is opened for. From the header: everything, browse
/// only (the default).
#[derive(Clone, Default)]
pub(crate) struct BrowseFor {
    /// Only symbols or only footprints.
    pub role: Option<PreviewKind>,
    /// A reference such as `Device:R` to select when the browser opens.
    pub current: Option<String>,
    /// Footprints for this symbol, with KiCad's two filters on.
    pub for_symbol: Option<String>,
    /// Called with the chosen item; the browser then closes.
    pub choose: Option<Callback<TeamPickRequest>>,
    /// The element that gets keyboard focus back when the browser closes,
    /// by its id, such as the Choose... button that opened it.
    pub return_focus: Option<String>,
}

/// What the browser can do outside itself.
#[derive(Clone, Copy)]
pub(crate) struct BrowserActions {
    /// The team chosen in the part list, by server address: its parts mark
    /// the items they use. `None` for every team's parts.
    pub team: Signal<Option<String>>,
    /// Whether the person may start Add part.
    pub can_add: Memo<bool>,
    pub add_from: Callback<TeamLibraryChoice>,
    /// Searches the part list for a reference.
    pub show_parts: Callback<String>,
    /// Opens a part, named by its team and IPN.
    pub open_part: Callback<PartKey>,
}

fn kind_noun(kind: PreviewKind, count: usize) -> &'static str {
    match (kind, count == 1) {
        (PreviewKind::Symbol, true) => "symbol",
        (PreviewKind::Symbol, false) => "symbols",
        (PreviewKind::Footprint, true) => "footprint",
        (PreviewKind::Footprint, false) => "footprints",
    }
}

/// The count line under the filters.
pub(crate) fn count_text(kind: PreviewKind, items: &LibraryItems) -> String {
    let mut text = if items.rows.len() < items.total {
        format!(
            "Showing {} of {} {}. Search to narrow the list.",
            grouped(items.rows.len()),
            grouped(items.total),
            kind_noun(kind, items.total)
        )
    } else {
        counted(items.total, kind_noun(kind, 1), kind_noun(kind, 2))
    };
    if items.reading {
        if !text.ends_with('.') {
            text.push('.');
        }
        text.push_str(" More may appear while libraries are read.");
    }
    text
}

/// The progress line, such as `Reading libraries… 12 of 377`; `None`
/// once every library is read.
pub(crate) fn progress_text(view: &LibrariesView) -> Option<String> {
    (!view.is_ready()).then(|| {
        format!(
            "Reading libraries… {} of {}",
            grouped(view.done),
            grouped(view.total)
        )
    })
}

/// The library a reference's nickname names, as KiCad would find it: ours
/// first, then the person's installed libraries, then KiCad's.
pub(crate) fn library_of(
    entries: &[LibraryEntry],
    reference: &str,
) -> Option<(LibraryKey, String)> {
    let (nickname, name) = reference.trim().split_once(':')?;
    [
        LibraryGroup::Ours,
        LibraryGroup::Installed,
        LibraryGroup::Kicad,
    ]
    .into_iter()
    .find_map(|group| {
        entries
            .iter()
            .find(|entry| entry.key.group == group && entry.key.nickname == nickname)
            .map(|entry| (entry.key.clone(), name.to_owned()))
    })
}

/// A whole number typed in a filter box; blank or not a number filters
/// nothing.
fn number(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

#[component]
pub(crate) fn LibrariesDialog(
    open: RwSignal<Option<BrowseFor>>,
    actions: BrowserActions,
    library_check: RwSignal<Option<LibraryCheckView>>,
) -> impl IntoView {
    let browse = open.get_untracked().unwrap_or_default();
    let dialog = NodeRef::<leptos::html::Dialog>::new();
    let view = RwSignal::new(None::<LibrariesView>);
    let tab = RwSignal::new(browse.role.unwrap_or(PreviewKind::Symbol));
    let library = RwSignal::new(None::<LibraryKey>);
    let query = RwSignal::new(String::new());
    let filters = RwSignal::new(LibraryFilters {
        match_footprint_filters: browse.for_symbol.is_some(),
        match_pin_count: browse.for_symbol.is_some(),
        ..LibraryFilters::default()
    });
    let for_symbol = RwSignal::new(browse.for_symbol.clone());
    let items = RwSignal::new(Load::<LibraryItems>::Loading);
    // True from a change of what the list shows until its answer.
    let busy = RwSignal::new(true);
    let selected = RwSignal::new(None::<LibraryItemRow>);
    let detail = RwSignal::new(None::<Load<Option<LibraryItemDetail>>>);
    let preview = RwSignal::new(Load::<PreviewOutcome>::Loading);
    let alive = Arc::new(AtomicBool::new(true));
    let choose = browse.choose;
    let return_focus = browse.return_focus.clone();
    let pending_current = StoredValue::new(browse.current.clone());
    // Opened on a current item, the list has the focus with that item
    // selected, so Enter chooses it; otherwise the search box has it.
    let first_focus = if browse.current.is_some() {
        "library-items"
    } else {
        "library-search"
    };
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::SeqCst));
    }

    // Open as a modal dialog, put the cursor in the search box, and start
    // (or refresh) the background reading.
    Effect::new(move |_| {
        if let Some(dialog) = dialog.get() {
            crate::show_modal(&dialog);
            focus_later(first_focus);
        }
    });
    {
        let alive = alive.clone();
        spawn_local(async move {
            if let Ok(found) = backend::libraries(true).await {
                view.set(Some(found));
            }
            poll_progress(view, alive);
        });
    }

    // Opened with a current item: open its library once it is listed, and
    // select the item once the library is read and has it. A reference only
    // an unsaved file has (or one gone from its library) opens the library
    // with nothing selected.
    let awaiting = StoredValue::new(None::<(LibraryKey, String)>);
    Effect::new(move |_| {
        if let Some(reference) = pending_current.get_value() {
            let found = view.with(|view| {
                let view = view.as_ref()?;
                let entries = match tab.get_untracked() {
                    PreviewKind::Symbol => &view.symbols,
                    PreviewKind::Footprint => &view.footprints,
                };
                library_of(entries, &reference)
            });
            if let Some((key, name)) = found {
                pending_current.set_value(None);
                library.set(Some(key.clone()));
                awaiting.set_value(Some((key, name)));
            }
        }
        let Some((key, name)) = awaiting.get_value() else {
            return;
        };
        // Whether the library is read yet; asked again as each one is.
        let read = view.with(|view| {
            view.as_ref().is_some_and(|view| {
                let entries = match tab.get_untracked() {
                    PreviewKind::Symbol => &view.symbols,
                    PreviewKind::Footprint => &view.footprints,
                };
                entries.iter().any(|entry| {
                    entry.key == key && (entry.items.is_some() || entry.problem.is_some())
                })
            })
        });
        if !read {
            return;
        }
        awaiting.set_value(None);
        let request = LibraryItemRequest {
            kind: tab.get_untracked(),
            library: key.clone(),
            name: name.clone(),
            selection: None,
        };
        spawn_local(async move {
            let team = actions.team.get_untracked();
            if let Ok(Some(_)) = backend::library_item(team.as_deref(), request).await {
                if selected.try_get_untracked().flatten().is_none() {
                    selected.set(Some(LibraryItemRow {
                        library: key,
                        name,
                        description: String::new(),
                        in_catalog: false,
                    }));
                }
            }
        });
    });

    // The list: asked again when what it shows changes, and while
    // libraries are read, as each one is ready.
    let progress = Memo::new(move |_| view.with(|view| view.as_ref().map(|view| view.done)));
    let items_seq = StoredValue::new(0u64);
    Effect::new(move |_| {
        let request = LibraryItemsRequest {
            kind: tab.get(),
            library: library.get(),
            query: query.get(),
            filters: filters.get(),
            for_symbol: for_symbol.get(),
            limit: ROWS,
        };
        let _ = progress.get();
        let seq = items_seq.get_value() + 1;
        items_seq.set_value(seq);
        busy.set(true);
        set_timeout(
            move || {
                if items_seq.try_get_value() != Some(seq) {
                    return;
                }
                let team = actions.team.get_untracked();
                spawn_local(async move {
                    let result = backend::library_items(team.as_deref(), request).await;
                    if items_seq.try_get_value() == Some(seq) {
                        items.set(into_load(result));
                        busy.set(false);
                    }
                });
            },
            SEARCH_DEBOUNCE,
        );
    });

    // The selected item's details and drawing; only the newest answer is
    // shown, and older drawings still waiting for kicad-cli are dropped.
    let detail_seq = StoredValue::new(0u64);
    let selection = StoredValue::new(0u64);
    Effect::new(move |_| {
        let Some(row) = selected.get() else {
            detail.set(None);
            return;
        };
        let seq = detail_seq.get_value() + 1;
        detail_seq.set_value(seq);
        let this_selection = (js_sys::Date::now() as u64).max(selection.get_value() + 1);
        selection.set_value(this_selection);
        detail.set(Some(Load::Loading));
        preview.set(Load::Loading);
        let request = LibraryItemRequest {
            kind: tab.get_untracked(),
            library: row.library.clone(),
            name: row.name.clone(),
            selection: Some(this_selection),
        };
        let for_detail = request.clone();
        let team = actions.team.get_untracked();
        spawn_local(async move {
            let result = backend::library_item(team.as_deref(), for_detail).await;
            if detail_seq.try_get_value() == Some(seq) {
                detail.set(Some(into_load(result)));
            }
        });
        spawn_local(async move {
            let mut result = backend::library_preview(request.clone()).await;
            if detail_seq.try_get_value() != Some(seq) {
                return;
            }
            let replaced = matches!(
                &result,
                Ok(PreviewOutcome::Problem(problem)) if problem.reason == PreviewProblemReason::Replaced
            );
            if replaced {
                result = backend::library_preview(LibraryItemRequest {
                    selection: None,
                    ..request
                })
                .await;
            }
            if detail_seq.try_get_value() == Some(seq) {
                preview.set(into_load(result));
            }
        });
    });

    // Library check findings about our items, by reference.
    let findings = Memo::new(move |_| {
        library_check.with(|check| {
            let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for group in check.iter().flat_map(|check| &check.groups) {
                for item in group.items.iter().filter(|item| !item.library.is_empty()) {
                    found
                        .entry(format!("{}:{}", item.library, item.name))
                        .or_default()
                        .push(item.message.clone());
                }
            }
            found
        })
    });

    let close = move || {
        if let Some(dialog) = dialog.get_untracked() {
            dialog.close();
        }
    };
    // A click outside the dialog (on its backdrop) closes it, like the
    // drawing close-up: both press and release outside, on the dialog
    // itself, so dragging out does not close it and the close-up's own
    // backdrop is left to the close-up.
    let pressed_outside = StoredValue::new(false);
    let outside = move |event: &leptos::ev::MouseEvent| {
        use wasm_bindgen::JsCast;
        let on_dialog = event.target().is_some_and(|target| {
            dialog.get_untracked().is_some_and(|dialog| {
                let dialog: &wasm_bindgen::JsValue = dialog.as_ref();
                target.unchecked_ref::<wasm_bindgen::JsValue>() == dialog
            })
        });
        on_dialog
            && dialog.get_untracked().is_some_and(|dialog| {
                let Some([left, right, top, bottom]) = crate::bounds(&dialog) else {
                    return false;
                };
                let (x, y) = (f64::from(event.client_x()), f64::from(event.client_y()));
                x < left || x > right || y < top || y > bottom
            })
    };
    let switch_tab = move |kind: PreviewKind| {
        if tab.get_untracked() != kind {
            tab.set(kind);
            library.set(None);
            selected.set(None);
        }
    };
    let matching_footprints = Callback::new(move |reference: String| {
        tab.set(PreviewKind::Footprint);
        library.set(None);
        query.set(String::new());
        selected.set(None);
        filters.update(|filters| {
            filters.match_footprint_filters = true;
            filters.match_pin_count = true;
        });
        for_symbol.set(Some(reference));
        focus_later("library-search");
    });
    // An action that leaves the browser closes the dialog itself, so focus
    // goes back to the Libraries button, and then runs; an action that
    // moves focus (to the part search, say) does so after.
    let done_with = Callback::new(move |after: Box<dyn FnOnce() + Send + Sync>| {
        match dialog.get_untracked() {
            Some(dialog) => dialog.close(),
            None => open.set(None),
        }
        after();
    });
    let only_role = browse.role;
    let tab_button = move |kind: PreviewKind, label: &'static str, id: &'static str| {
        let other = match kind {
            PreviewKind::Symbol => "library-tab-footprints",
            PreviewKind::Footprint => "library-tab-symbols",
        };
        let is = move || tab.get() == kind;
        view! {
            <button
                type="button"
                role="tab"
                id=id
                class="team-tab"
                aria-selected=move || if is() { "true" } else { "false" }
                aria-controls="library-panel"
                tabindex=move || if is() { "0" } else { "-1" }
                hidden=only_role.is_some_and(|role| role != kind)
                on:click=move |_| switch_tab(kind)
                on:keydown=move |event| {
                    let key = event.key();
                    if (key == "ArrowRight" || key == "ArrowLeft") && only_role.is_none() {
                        event.prevent_default();
                        let next = match kind {
                            PreviewKind::Symbol => PreviewKind::Footprint,
                            PreviewKind::Footprint => PreviewKind::Symbol,
                        };
                        switch_tab(next);
                        focus_later(other);
                    }
                }
            >
                {label}
            </button>
        }
    };

    view! {
        <dialog
            node_ref=dialog
            class="libraries-dialog"
            aria-labelledby="libraries-title"
            on:close=move |_| {
                open.set(None);
                if let Some(id) = &return_focus {
                    focus_later(id);
                }
            }
            on:pointerdown=move |event| pressed_outside.set_value(outside(&event))
            on:click=move |event| {
                if pressed_outside.get_value() && outside(&event) {
                    close();
                }
                pressed_outside.set_value(false);
            }
        >
            <header class="libraries-header">
                <h2 id="libraries-title">"Libraries"</h2>
                <p
                    class="libraries-progress"
                    role="status"
                    data-state=move || view.with(|view| match view {
                        None => "loading",
                        Some(view) if view.is_ready() => "ready",
                        Some(_) => "reading",
                    })
                >
                    {move || view.with(|view| view.as_ref().and_then(progress_text))}
                </p>
                <button type="button" id="libraries-close" on:click=move |_| close()>"Close"</button>
            </header>
            <div class="libraries-body">
                <nav class="libraries-list" aria-label="Libraries">
                    <div class="team-tabs" role="tablist" aria-label="Symbols or footprints">
                        {tab_button(PreviewKind::Symbol, "Symbols", "library-tab-symbols")}
                        {tab_button(PreviewKind::Footprint, "Footprints", "library-tab-footprints")}
                    </div>
                    <LibraryList
                        view=view
                        tab=tab
                        library=library
                        selected=selected
                        with_teams=choose.is_none()
                    />
                </nav>
                <section
                    class="libraries-items"
                    id="library-panel"
                    role="tabpanel"
                    aria-labelledby=move || match tab.get() {
                        PreviewKind::Symbol => "library-tab-symbols",
                        PreviewKind::Footprint => "library-tab-footprints",
                    }
                >
                    <label class="library-search-label" for="library-search">
                        {move || match tab.get() {
                            PreviewKind::Symbol => "Search symbols",
                            PreviewKind::Footprint => "Search footprints",
                        }}
                    </label>
                    <input
                        id="library-search"
                        class="search-input"
                        type="search"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="Name, description or keywords"
                        aria-describedby="library-count"
                        prop:value=move || query.get()
                        on:input:target=move |event| query.set(event.target().value())
                    />
                    <Filters tab=tab filters=filters for_symbol=for_symbol items=items />
                    <ItemList
                        tab=tab
                        items=items
                        busy=busy
                        selected=selected
                        findings=findings
                        library=library
                        view=view
                        choose=choose
                        done_with=done_with
                    />
                </section>
                <section class="libraries-detail" aria-label="Selected item" tabindex="0">
                    <Detail
                        tab=tab
                        selected=selected
                        detail=detail
                        preview=preview
                        findings=findings
                        actions=actions
                        choose=choose
                        done_with=done_with
                        matching_footprints=matching_footprints
                    />
                </section>
            </div>
        </dialog>
    }
}

/// Asks how far the reading is until every library is read, or the dialog
/// closes.
fn poll_progress(view: RwSignal<Option<LibrariesView>>, alive: Arc<AtomicBool>) {
    let ready = view.try_with_untracked(|view| view.as_ref().is_some_and(LibrariesView::is_ready));
    if ready != Some(false) || !alive.load(Ordering::SeqCst) {
        return;
    }
    set_timeout(
        move || {
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            spawn_local(async move {
                if let Ok(found) = backend::libraries(false).await {
                    if alive.load(Ordering::SeqCst) {
                        view.set(Some(found));
                    }
                }
                poll_progress(view, alive);
            });
        },
        PROGRESS_POLL,
    );
}

/// What the Ours and Installed groups say when they list nothing.
#[derive(Clone, Default)]
struct GroupNotes {
    ours: Option<String>,
    installed: Option<String>,
}

/// One group of the library list: its heading and its libraries.
struct ListGroup {
    /// The heading's element ID.
    id: String,
    title: String,
    /// The team's mark, for a team's library repo.
    mark: Option<TeamMark>,
    entries: Vec<LibraryEntry>,
    note: Option<String>,
}

/// The groups of `entries` in the order shown: ours, each team's repo,
/// KiCad's, then installed ones. Empty groups are left out, except Ours
/// with `ours_note` and Installed with `installed_note`, which say why
/// they are empty. Teams' repos are left out without `with_teams`, such as
/// while choosing an item for a part.
fn list_groups(
    entries: &[LibraryEntry],
    teams: &[LibraryTeam],
    notes: &GroupNotes,
    with_teams: bool,
) -> Vec<ListGroup> {
    let mut groups = Vec::new();
    let mut push = |id: String,
                    title: String,
                    mark: Option<TeamMark>,
                    entries: Vec<LibraryEntry>,
                    note: Option<String>| {
        if !entries.is_empty() || note.is_some() {
            groups.push(ListGroup {
                id,
                title,
                mark,
                entries,
                note,
            });
        }
    };
    let of = |group: LibraryGroup, team: &str| -> Vec<LibraryEntry> {
        entries
            .iter()
            .filter(|entry| entry.key.group == group && entry.key.team == team)
            .cloned()
            .collect()
    };
    for group in LibraryGroup::ALL {
        let in_group = of(group, "");
        let note = match group {
            LibraryGroup::Ours => notes.ours.clone(),
            LibraryGroup::Installed => notes.installed.clone(),
            _ => None,
        }
        .filter(|_| in_group.is_empty());
        let count = in_group.len();
        push(
            format!("library-group-{group:?}").to_lowercase(),
            format!("{} ({count})", group.title()),
            None,
            in_group,
            note,
        );
        if group == LibraryGroup::Ours && with_teams {
            for (at, team) in teams.iter().enumerate() {
                let in_team = of(LibraryGroup::Team, &team.server_url);
                let count = in_team.len();
                push(
                    format!("library-group-team-{at}"),
                    format!("{} ({count})", team.repo_name),
                    Some(TeamMark {
                        label: team.code.clone(),
                        colour: team.colour.clone(),
                        offline: false,
                    }),
                    in_team,
                    team.note.clone(),
                );
            }
        }
    }
    groups
}

/// The libraries of the tab, by group, with All libraries first.
#[component]
fn LibraryList(
    view: RwSignal<Option<LibrariesView>>,
    tab: RwSignal<PreviewKind>,
    library: RwSignal<Option<LibraryKey>>,
    selected: RwSignal<Option<LibraryItemRow>>,
    /// Lists the linked teams' library repos too.
    with_teams: bool,
) -> impl IntoView {
    let entries = Memo::new(move |_| {
        view.with(|view| {
            view.as_ref()
                .map(|view| match tab.get() {
                    PreviewKind::Symbol => view.symbols.clone(),
                    PreviewKind::Footprint => view.footprints.clone(),
                })
                .unwrap_or_default()
        })
    });
    let notes = move || {
        view.with(|view| {
            view.as_ref()
                .map(|view| view.notes.clone())
                .unwrap_or_default()
        })
    };
    let group_notes = move || {
        view.with(|view| GroupNotes {
            ours: view.as_ref().and_then(|view| view.ours_note.clone()),
            installed: view.as_ref().and_then(|view| view.installed_note.clone()),
        })
    };
    let teams = move || {
        view.with(|view| {
            view.as_ref()
                .map(|view| view.teams.clone())
                .unwrap_or_default()
        })
    };
    let pick = move |key: Option<LibraryKey>| {
        if library.get_untracked() != key {
            library.set(key);
            selected.set(None);
        }
    };
    view! {
        <div class="library-groups">
            <button
                type="button"
                class="library-button"
                aria-current=move || library.with(Option::is_none).then_some("true")
                on:click=move |_| pick(None)
            >
                <span class="library-name">"All libraries"</span>
            </button>
            {move || {
                let entries = entries.get();
                list_groups(&entries, &teams(), &group_notes(), with_teams)
                    .into_iter()
                    .map(|group| {
                        let ListGroup { id, title, mark, entries: in_group, note: group_note } = group;
                        let labelled = id.clone();
                        let is_team = mark.is_some();
                        {
                            view! {
                                <h3 class="library-group" class:library-group-team=is_team id=id>
                                    {mark.map(|mark| view! { <TeamChip mark=mark /> " " })}
                                    {title}
                                </h3>
                                {group_note.map(|note| view! { <p class="hint library-note library-group-note">{note}</p> })}
                                <ul class="library-entries" aria-labelledby=labelled>
                                    {in_group
                                        .into_iter()
                                        .map(|entry| {
                                            let key = entry.key.clone();
                                            let current = key.clone();
                                            let count = match (&entry.problem, entry.items) {
                                                (Some(_), _) => "not read".to_owned(),
                                                (None, Some(count)) => grouped(count),
                                                (None, None) => "…".to_owned(),
                                            };
                                            let title = (!entry.description.is_empty()).then(|| entry.description.clone());
                                            // A library that could not be read, or only in
                                            // part, says so under its name.
                                            let said = entry.problem.clone().or_else(|| entry.note.clone());
                                            view! {
                                                <li>
                                                    <button
                                                        type="button"
                                                        class="library-button"
                                                        class:library-problem=entry.problem.is_some()
                                                        title=title
                                                        aria-current=move || library.with(|chosen| (chosen.as_ref() == Some(&current)).then_some("true"))
                                                        on:click=move |_| pick(Some(key.clone()))
                                                    >
                                                        <span class="library-name">{entry.key.nickname.clone()}</span>
                                                        <span class="library-count">{count}</span>
                                                    </button>
                                                    {said.map(|said| view! { <p class="library-entry-note">{said}</p> })}
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            }
                        }
                    })
                    .collect_view()
            }}
            {move || notes().into_iter().map(|note| view! { <p class="hint library-note">{note}</p> }).collect_view()}
        </div>
    }
}

/// The filters of the tab.
#[component]
fn Filters(
    tab: RwSignal<PreviewKind>,
    filters: RwSignal<LibraryFilters>,
    for_symbol: RwSignal<Option<String>>,
    items: RwSignal<Load<LibraryItems>>,
) -> impl IntoView {
    let number_box = move |id: &'static str,
                           label: &'static str,
                           get: fn(&LibraryFilters) -> Option<u32>,
                           set: fn(&mut LibraryFilters, Option<u32>)| {
        view! {
            <input
                id=id
                class="filter-number"
                type="number"
                min="0"
                inputmode="numeric"
                aria-label=label
                prop:value=move || filters.with(|filters| get(filters).map(|n| n.to_string()).unwrap_or_default())
                on:input:target=move |event| {
                    let value = number(&event.target().value());
                    filters.update(|filters| set(filters, value));
                }
            />
        }
    };
    let check = move |id: &'static str,
                      label: String,
                      get: fn(&LibraryFilters) -> bool,
                      set: fn(&mut LibraryFilters, bool)| {
        view! {
            <label class="filter-check" for=id>
                <input
                    id=id
                    type="checkbox"
                    prop:checked=move || filters.with(get)
                    on:change:target=move |event| {
                        let checked = event.target().checked();
                        filters.update(|filters| set(filters, checked));
                    }
                />
                {label}
            </label>
        }
    };
    view! {
        <div class="library-filters" role="group" aria-label="Filters">
            {move || match tab.get() {
                PreviewKind::Symbol => view! {
                    <span class="filter-range">
                        <span aria-hidden="true">"Pins"</span>
                        {number_box("filter-pins-min", "Pins, at least", |f| f.pins_min, |f, v| f.pins_min = v)}
                        <span aria-hidden="true">"to"</span>
                        {number_box("filter-pins-max", "Pins, at most", |f| f.pins_max, |f, v| f.pins_max = v)}
                    </span>
                    <label class="filter-text" for="filter-reference">
                        "Reference"
                        <input
                            id="filter-reference"
                            type="text"
                            size="4"
                            spellcheck="false"
                            placeholder="R, C, U"
                            prop:value=move || filters.with(|filters| filters.reference.clone())
                            on:input:target=move |event| {
                                let value = event.target().value();
                                filters.update(|filters| filters.reference = value);
                            }
                        />
                    </label>
                    {check("filter-hide-power", "Hide power symbols".into(), |f| f.hide_power, |f, v| f.hide_power = v)}
                    {check("filter-with-footprint", "Has a default footprint".into(), |f| f.with_footprint, |f, v| f.with_footprint = v)}
                }
                .into_any(),
                PreviewKind::Footprint => view! {
                    <span class="filter-range">
                        <span aria-hidden="true">"Pads"</span>
                        {number_box("filter-pads-min", "Pads, at least", |f| f.pads_min, |f, v| f.pads_min = v)}
                        <span aria-hidden="true">"to"</span>
                        {number_box("filter-pads-max", "Pads, at most", |f| f.pads_max, |f, v| f.pads_max = v)}
                    </span>
                    <label class="filter-text" for="filter-mount">
                        "Mounting"
                        <select
                            id="filter-mount"
                            on:change:target=move |event| {
                                let mount = match event.target().value().as_str() {
                                    "smd" => MountFilter::Smd,
                                    "tht" => MountFilter::ThroughHole,
                                    _ => MountFilter::Any,
                                };
                                filters.update(|filters| filters.mount = mount);
                            }
                        >
                            <option value="any" selected=move || filters.with(|f| f.mount == MountFilter::Any)>"SMD or through-hole"</option>
                            <option value="smd" selected=move || filters.with(|f| f.mount == MountFilter::Smd)>"SMD"</option>
                            <option value="tht" selected=move || filters.with(|f| f.mount == MountFilter::ThroughHole)>"Through-hole"</option>
                        </select>
                    </label>
                    {move || for_symbol.get().map(|reference| {
                        // What the symbol's filters are, once the list says.
                        let known = items.with(|items| match items {
                            Load::Ready(items) => items.for_symbol.clone(),
                            _ => None,
                        });
                        let (patterns, pins) = match &known {
                            Some(symbol) if symbol.found => (
                                if symbol.footprint_filters.is_empty() {
                                    "it has none".to_owned()
                                } else {
                                    symbol.footprint_filters.join(" ")
                                },
                                symbol.pin_count.to_string(),
                            ),
                            _ => ("…".to_owned(), "…".to_owned()),
                        };
                        let not_found = known.as_ref().is_some_and(|symbol| !symbol.found);
                        view! {
                            <div class="for-symbol">
                                <p class="for-symbol-title">
                                    "For the symbol "<code>{cad_reference(&reference)}</code>
                                    <button
                                        type="button"
                                        class="link-button"
                                        id="for-symbol-clear"
                                        on:click=move |_| for_symbol.set(None)
                                    >
                                        "Show all footprints"
                                    </button>
                                </p>
                                {not_found.then(|| view! {
                                    <p class="hint">"This symbol is not in the libraries, so its filters cannot be used."</p>
                                })}
                                {check("filter-match-filters", format!("Match its footprint filters ({patterns})"), |f| f.match_footprint_filters, |f, v| f.match_footprint_filters = v)}
                                {check("filter-match-pins", format!("Match its pin count ({pins})"), |f| f.match_pin_count, |f, v| f.match_pin_count = v)}
                            </div>
                        }
                    })}
                }
                .into_any(),
            }}
        </div>
    }
}

/// The item list: a list box the arrow keys move through.
#[component]
#[allow(clippy::too_many_arguments)]
fn ItemList(
    tab: RwSignal<PreviewKind>,
    items: RwSignal<Load<LibraryItems>>,
    busy: RwSignal<bool>,
    selected: RwSignal<Option<LibraryItemRow>>,
    findings: Memo<BTreeMap<String, Vec<String>>>,
    library: RwSignal<Option<LibraryKey>>,
    view: RwSignal<Option<LibrariesView>>,
    choose: Option<Callback<TeamPickRequest>>,
    done_with: Callback<Box<dyn FnOnce() + Send + Sync>>,
) -> impl IntoView {
    let list = NodeRef::<leptos::html::Ul>::new();
    let rows = Memo::new(move |_| {
        items.with(|items| match items {
            Load::Ready(items) => items.rows.clone(),
            _ => Vec::new(),
        })
    });
    let selected_at = Memo::new(move |_| {
        selected.with(|chosen| {
            let chosen = chosen.as_ref()?;
            rows.with(|rows| {
                rows.iter()
                    .position(|row| row.library == chosen.library && row.name == chosen.name)
            })
        })
    });
    // Keep the selected row in view as the arrow keys move it.
    Effect::new(move |_| {
        if let Some(at) = selected_at.get() {
            request_animation_frame(move || {
                if let Some(row) = document().get_element_by_id(&format!("library-item-{at}")) {
                    let options = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(&options, &"block".into(), &"nearest".into());
                    if let Ok(method) = js_sys::Reflect::get(&row, &"scrollIntoView".into()) {
                        use wasm_bindgen::JsCast;
                        if let Some(method) = method.dyn_ref::<js_sys::Function>() {
                            let _ = method.call1(&row, &options);
                        }
                    }
                }
            });
        }
    });
    let run_choose = move || {
        let (Some(choose), Some(row)) = (choose, selected.get_untracked()) else {
            return false;
        };
        let pick = TeamPickRequest {
            kind: tab.get_untracked(),
            library: row.library,
            name: row.name,
        };
        done_with.run(Box::new(move || choose.run(pick)));
        true
    };
    let on_key = move |event: leptos::ev::KeyboardEvent| {
        let all = rows.get_untracked();
        if all.is_empty() {
            return;
        }
        let at = selected_at.get_untracked();
        let last = all.len() - 1;
        let next = match event.key().as_str() {
            "ArrowDown" => at.map_or(0, |at| (at + 1).min(last)),
            "ArrowUp" => at.map_or(0, |at| at.saturating_sub(1)),
            "PageDown" => at.map_or(0, |at| (at + 10).min(last)),
            "PageUp" => at.map_or(0, |at| at.saturating_sub(10)),
            "Home" => 0,
            "End" => last,
            "Enter" => {
                event.prevent_default();
                // Choosing for Edit part or Add part; when only browsing,
                // Enter goes to the item's first action.
                if !run_choose() {
                    if let Ok(Some(button)) = document().query_selector(".libraries-actions button")
                    {
                        use wasm_bindgen::JsCast;
                        if let Ok(button) = button.dyn_into::<leptos::web_sys::HtmlElement>() {
                            let _ = button.focus();
                        }
                    }
                }
                return;
            }
            _ => return,
        };
        event.prevent_default();
        selected.set(all.get(next).cloned());
    };
    // What the chosen library could not read, said above its items.
    let problem = move || {
        let chosen = library.get()?;
        let kind = tab.get();
        view.with(|view| {
            let view = view.as_ref()?;
            let entries = match kind {
                PreviewKind::Symbol => &view.symbols,
                PreviewKind::Footprint => &view.footprints,
            };
            let entry = entries.iter().find(|entry| entry.key == chosen)?;
            match (&entry.problem, &entry.note) {
                (Some(why), _) => Some(format!("This library could not be read. {why}")),
                (None, Some(note)) => Some(format!("Some of its files were skipped. {note}")),
                (None, None) => None,
            }
        })
    };
    view! {
        <p id="library-count" class="result-count" aria-live="polite">
            {move || match items.get() {
                Load::Ready(items) => count_text(tab.get(), &items),
                Load::Loading => "Loading…".to_owned(),
                Load::Failed(error) => format!("Could not list them: {error}"),
            }}
        </p>
        {move || problem().map(|why| view! {
            <p class="problem quiet library-unreadable">{why}</p>
        })}
        <ul
            node_ref=list
            id="library-items"
            class="library-items"
            role="listbox"
            tabindex="0"
            aria-label=move || match tab.get() {
                PreviewKind::Symbol => "Symbols",
                PreviewKind::Footprint => "Footprints",
            }
            aria-activedescendant=move || selected_at.get().map(|at| format!("library-item-{at}"))
            aria-busy=move || if busy.get() { "true" } else { "false" }
            on:keydown=on_key
        >
            {move || {
                let all_libraries = library.with(Option::is_none);
                let found = findings.get();
                rows.get()
                    .into_iter()
                    .enumerate()
                    .map(|(at, row)| {
                        let reference = row.reference();
                        let checked = row.library.group == LibraryGroup::Ours && found.contains_key(&reference);
                        let is_selected = move || selected_at.get() == Some(at);
                        let pick = row.clone();
                        let twice = row.clone();
                        view! {
                            <li
                                id=format!("library-item-{at}")
                                role="option"
                                class="library-item"
                                aria-selected=move || if is_selected() { "true" } else { "false" }
                                on:click=move |_| {
                                    selected.set(Some(pick.clone()));
                                    if let Some(list) = list.get_untracked() {
                                        let _ = list.focus();
                                    }
                                }
                                on:dblclick=move |_| {
                                    selected.set(Some(twice.clone()));
                                    run_choose();
                                }
                            >
                                <span class="library-item-name">{row.name.clone()}</span>
                                {all_libraries.then(|| view! { <span class="library-item-library">{row.library.nickname.clone()}</span> })}
                                <span class="library-item-description">{row.description.clone()}</span>
                                {(row.in_catalog || checked).then(|| view! {
                                    <span class="library-item-marks">
                                        {row.in_catalog.then(|| view! { <span class="mark mark-catalog">"In the Catalog"</span> })}
                                        {checked.then(|| view! { <span class="mark mark-check">"Library check"</span> })}
                                    </span>
                                })}
                            </li>
                        }
                    })
                    .collect_view()
            }}
        </ul>
    }
}

/// The selected item: its drawing, details and what can be done with it.
#[component]
#[allow(clippy::too_many_arguments)]
fn Detail(
    tab: RwSignal<PreviewKind>,
    selected: RwSignal<Option<LibraryItemRow>>,
    detail: RwSignal<Option<Load<Option<LibraryItemDetail>>>>,
    preview: RwSignal<Load<PreviewOutcome>>,
    findings: Memo<BTreeMap<String, Vec<String>>>,
    actions: BrowserActions,
    choose: Option<Callback<TeamPickRequest>>,
    done_with: Callback<Box<dyn FnOnce() + Send + Sync>>,
    matching_footprints: Callback<String>,
) -> impl IntoView {
    move || {
        let Some(row) = selected.get() else {
            return view! {
                <div class="empty-state library-empty">
                    <h3>"Nothing selected"</h3>
                    <p>"Choose a symbol or footprint to see its drawing and details."</p>
                </div>
            }
            .into_any();
        };
        let kind = tab.get_untracked();
        let reference = row.reference();
        view! {
            <h3 id="library-detail-title" class="library-detail-title">
                <code>{cad_reference(&reference)}</code>
            </h3>
            <div class="library-preview">
                <PreviewSection kind=kind state=preview id_prefix="library-" interactive=true />
            </div>
            {move || match detail.get() {
                None | Some(Load::Loading) => view! { <p class="loading" aria-busy="true">"Loading details…"</p> }.into_any(),
                Some(Load::Failed(error)) => view! { <p class="problem" role="alert">{format!("Could not load the details: {error}")}</p> }.into_any(),
                Some(Load::Ready(None)) => view! { <p class="hint">"This item is not in its library any more."</p> }.into_any(),
                Some(Load::Ready(Some(detail))) => {
                    let found = (row.library.group == LibraryGroup::Ours)
                        .then(|| findings.with(|found| found.get(&detail.reference).cloned()))
                        .flatten()
                        .unwrap_or_default();
                    view! {
                        <Actions
                            kind=kind
                            row=row.clone()
                            detail=detail.clone()
                            actions=actions
                            choose=choose
                            done_with=done_with
                            matching_footprints=matching_footprints
                        />
                        {(!found.is_empty()).then(|| view! {
                            <div class="library-findings">
                                <p class="library-findings-title">"The library check found:"</p>
                                <ul>{found.into_iter().map(|message| view! { <li>{message}</li> }).collect_view()}</ul>
                            </div>
                        })}
                        <DetailFacts kind=kind row=row.clone() detail=detail.clone() />
                        <UsedBy detail=detail actions=actions done_with=done_with />
                    }
                    .into_any()
                }
            }}
        }
        .into_any()
    }
}

#[component]
fn DetailFacts(kind: PreviewKind, row: LibraryItemRow, detail: LibraryItemDetail) -> impl IntoView {
    let text = |value: String| value.into_any();
    let mut facts: Vec<(&str, AnyView)> = vec![(
        "Library",
        text(format!(
            "{} ({})",
            row.library.nickname,
            row.library.group.title()
        )),
    )];
    if !detail.description.is_empty() {
        facts.push(("Description", text(detail.description.clone())));
    }
    if !detail.keywords.is_empty() {
        facts.push(("Keywords", text(detail.keywords.clone())));
    }
    match kind {
        PreviewKind::Symbol => {
            facts.push(("Pins", text(detail.count.to_string())));
            if !detail.reference_letter.is_empty() {
                facts.push(("Reference", text(detail.reference_letter.clone())));
            }
            if detail.power {
                facts.push(("Power symbol", text("Yes".into())));
            }
            if !detail.default_footprint.is_empty() {
                facts.push((
                    "Default footprint",
                    cad_reference(&detail.default_footprint),
                ));
            }
            if !detail.footprint_filters.is_empty() {
                facts.push((
                    "Footprint filters",
                    text(detail.footprint_filters.join(" ")),
                ));
            }
        }
        PreviewKind::Footprint => {
            facts.push(("Pads", text(detail.count.to_string())));
            if !detail.mount.is_empty() {
                facts.push(("Mounting", text(detail.mount.clone())));
            }
            if !detail.model.is_empty() {
                facts.push(("3D model", text(detail.model.clone())));
            }
        }
    }
    facts.push(("File", text(detail.file.clone())));
    view! {
        <dl class="facts library-facts">
            {facts
                .into_iter()
                .map(|(name, value)| view! {
                    <div class="fact">
                        <dt>{name}</dt>
                        <dd>{value}</dd>
                    </div>
                })
                .collect_view()}
        </dl>
    }
}

#[component]
fn Actions(
    kind: PreviewKind,
    row: LibraryItemRow,
    detail: LibraryItemDetail,
    actions: BrowserActions,
    choose: Option<Callback<TeamPickRequest>>,
    done_with: Callback<Box<dyn FnOnce() + Send + Sync>>,
    matching_footprints: Callback<String>,
) -> impl IntoView {
    let reference = detail.reference.clone();
    let used = !detail.used_by.is_empty();
    let choose_button = choose.map(|choose| {
        let pick = TeamPickRequest {
            kind,
            library: row.library.clone(),
            name: row.name.clone(),
        };
        view! {
            <button
                type="button"
                class="primary"
                id="library-choose"
                on:click=move |_| {
                    let pick = pick.clone();
                    done_with.run(Box::new(move || choose.run(pick)));
                }
            >
                "Choose"
            </button>
        }
    });
    // Only our items not in the Catalog can start Add part here; KiCad's
    // and installed items come with Add part's From a library
    // (kicad-parts-bc6f).
    let add = (choose.is_none()
        && row.library.group == LibraryGroup::Ours
        && !used
        && actions.can_add.get_untracked())
    .then(|| {
        let choice = match kind {
            PreviewKind::Symbol => TeamLibraryChoice {
                symbol: Some(reference.clone()),
                footprint: None,
            },
            PreviewKind::Footprint => TeamLibraryChoice {
                symbol: None,
                footprint: Some(reference.clone()),
            },
        };
        view! {
            <button
                type="button"
                id="library-add-part"
                on:click=move |_| {
                    let choice = choice.clone();
                    done_with.run(Box::new(move || actions.add_from.run(choice)));
                }
            >
                "Add part from it"
            </button>
        }
    });
    let show = (choose.is_none() && used).then(|| {
        let reference = reference.clone();
        view! {
            <button
                type="button"
                id="library-show-parts"
                on:click=move |_| {
                    let reference = reference.clone();
                    done_with.run(Box::new(move || actions.show_parts.run(reference)));
                }
            >
                "Show the parts that use it"
            </button>
        }
    });
    let footprints = (choose.is_none() && kind == PreviewKind::Symbol).then(|| {
        let reference = reference.clone();
        view! {
            <button
                type="button"
                id="library-matching-footprints"
                on:click=move |_| matching_footprints.run(reference.clone())
            >
                "Show matching footprints"
            </button>
        }
    });
    view! {
        <div class="libraries-actions">
            {choose_button}
            {add}
            {show}
            {footprints}
        </div>
    }
}

/// The Catalog parts that use the item, each opening its part.
#[component]
fn UsedBy(
    detail: LibraryItemDetail,
    actions: BrowserActions,
    done_with: Callback<Box<dyn FnOnce() + Send + Sync>>,
) -> impl IntoView {
    let used = !detail.used_by.is_empty();
    // Each part's team mark, while two teams or more show parts, as in the
    // part list: the same IPN may be in two teams.
    let marks = use_context::<ListTeams>()
        .map(|list| list.marks.get_untracked())
        .unwrap_or_default();
    used.then(|| {
        let count = detail.used_by.len();
        view! {
            <h4 class="library-used-title">
                {format!("Used by {}", crate::counted(count, "Catalog part", "Catalog parts"))}
            </h4>
            <ul class="library-used">
                {detail
                    .used_by
                    .clone()
                    .into_iter()
                    .map(|part| {
                        let key = PartKey {
                            team: part.team.clone(),
                            ipn: part.ipn.clone(),
                        };
                        let chip = marks
                            .mark(&part.team)
                            .map(|mark| view! { <TeamChip mark=mark.clone() /> });
                        view! {
                            <li>
                                {chip}
                                <button
                                    type="button"
                                    class="link-button"
                                    on:click=move |_| {
                                        let key = key.clone();
                                        done_with.run(Box::new(move || actions.open_part.run(key)));
                                    }
                                >
                                    {part.ipn.clone()}
                                </button>
                                " "
                                <span class="library-used-description">{part.description}</span>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(group: LibraryGroup, nickname: &str) -> LibraryEntry {
        LibraryEntry {
            key: LibraryKey {
                group,
                nickname: nickname.into(),
                team: String::new(),
            },
            description: String::new(),
            items: Some(1),
            problem: None,
            note: None,
        }
    }

    #[test]
    fn a_reference_finds_its_library_as_kicad_would() {
        let entries = [
            entry(LibraryGroup::Kicad, "Device"),
            entry(LibraryGroup::Installed, "Device"),
            entry(LibraryGroup::Ours, "g-pas"),
        ];
        let (key, name) = library_of(&entries, "Device:R").unwrap();
        assert_eq!((key.group, name.as_str()), (LibraryGroup::Installed, "R"));
        assert_eq!(
            library_of(&entries, "g-pas:R_US").unwrap().0.group,
            LibraryGroup::Ours
        );
        assert_eq!(library_of(&entries, "Nope:R"), None);
        assert_eq!(library_of(&entries, "no colon"), None);
    }

    #[test]
    fn counts_say_how_many_and_whether_more_may_come() {
        let items = |total: usize, shown: usize, reading: bool| LibraryItems {
            total,
            rows: (0..shown)
                .map(|n| LibraryItemRow {
                    library: LibraryKey {
                        group: LibraryGroup::Kicad,
                        nickname: "Device".into(),
                        team: String::new(),
                    },
                    name: n.to_string(),
                    description: String::new(),
                    in_catalog: false,
                })
                .collect(),
            for_symbol: None,
            reading,
        };
        assert_eq!(
            count_text(PreviewKind::Symbol, &items(1, 1, false)),
            "1 symbol"
        );
        assert_eq!(
            count_text(PreviewKind::Footprint, &items(4321, 200, false)),
            "Showing 200 of 4,321 footprints. Search to narrow the list."
        );
        assert_eq!(
            count_text(PreviewKind::Symbol, &items(0, 0, true)),
            "0 symbols. More may appear while libraries are read."
        );
        let view = LibrariesView {
            done: 12,
            total: 377,
            ..LibrariesView::default()
        };
        assert_eq!(
            progress_text(&view).as_deref(),
            Some("Reading libraries… 12 of 377")
        );
        assert_eq!(progress_text(&LibrariesView::default()), None);
        assert_eq!(number(" 8 "), Some(8));
        assert_eq!(number("x"), None);
    }
}

//! The part list on the left: the search results, one section per category
//! that opens and closes (an accordion).
//!
//! With an empty search every section starts closed and keeps what the
//! person opened and closed; a section's parts are loaded when it opens.
//! While searching, the sections with matches are open, each showing its
//! first matches with "Show all", and the others are not shown; clearing
//! the search goes back to what the person had open. Selecting a part
//! opens its section and scrolls the part into view.
//!
//! A new answer, such as one after a teammate's save, updates sections and
//! rows in place. So keyboard focus, the open sections and the scroll
//! position stay where the person left them.

use crate::{Load, backend, grouped};
use desktop_contract::{PartSummary, SearchResponse};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::{BTreeMap, BTreeSet};

/// Parts each section shows while searching, before "Show all".
pub(crate) const SEARCH_ROWS: usize = 25;
/// Most parts a section shows: when opened without a search, or after
/// "Show all". Each section still counts all its matches.
pub(crate) const SECTION_ROWS: usize = 200;

/// One answer of the search behind the list, with what it was asked for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Searched {
    pub(crate) response: SearchResponse,
    /// The part that was selected when the search was asked for.
    pub(crate) selected: Option<String>,
    /// The team catalog's position then. After a change in the same
    /// search, open sections keep their parts and load again.
    pub(crate) version: Option<u64>,
}

/// Whether `query` is a search, rather than the whole catalog.
fn searching(query: &str) -> bool {
    !query.trim().is_empty()
}

/// What the list's open sections and loaded parts are kept for: the
/// search, and the reference the parts are limited to, if any. Not empty
/// whenever either is given, so a limited list counts as a search.
pub(crate) fn search_key(response: &SearchResponse) -> String {
    let query = response.query.trim();
    match response.uses.as_deref() {
        Some(reference) => format!("{query}\u{1f}uses {reference}"),
        None => query.to_owned(),
    }
}

/// Which sections are open.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OpenSections {
    /// The sections the person has open while not searching, by category
    /// code. Kept for the session.
    opened: BTreeSet<String>,
    /// While searching, the sections the person opened or closed by hand,
    /// and the search (trimmed) they did it in. A new search forgets them.
    during_search: Option<(String, BTreeMap<String, bool>)>,
}

impl OpenSections {
    /// Whether `category`'s section is open in the results for `query`.
    pub(crate) fn is_open(&self, category: &str, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return self.opened.contains(category);
        }
        match &self.during_search {
            Some((search, chosen)) if search == query => {
                chosen.get(category).copied().unwrap_or(true)
            }
            _ => true,
        }
    }

    /// Opens or closes `category`'s section by hand.
    pub(crate) fn set(&mut self, category: &str, query: &str, open: bool) {
        let query = query.trim();
        if query.is_empty() {
            if open {
                self.opened.insert(category.to_owned());
            } else {
                self.opened.remove(category);
            }
            return;
        }
        let chosen = match &mut self.during_search {
            Some((search, chosen)) if search == query => chosen,
            during_search => &mut during_search.insert((query.to_owned(), BTreeMap::new())).1,
        };
        chosen.insert(category.to_owned(), open);
    }

    pub(crate) fn toggle(&mut self, category: &str, query: &str) {
        let open = self.is_open(category, query);
        self.set(category, query, !open);
    }

    /// Opens the section of a part that was selected, in this search and
    /// once the search is cleared, so the part stays in view.
    pub(crate) fn reveal(&mut self, category: &str, query: &str) {
        self.opened.insert(category.to_owned());
        if searching(query) {
            self.set(category, query, true);
        }
    }
}

/// When to open the selected part's section.
#[derive(Clone, Debug, Default, PartialEq)]
struct Reveals {
    /// A part just selected, and the search (trimmed) shown then, until a
    /// result set holds it; typing another search forgets it, but clearing
    /// the search does not.
    pending: Option<(String, String)>,
    /// The part last revealed and the section it was in.
    revealed: Option<(String, String)>,
}

impl Reveals {
    fn select(&mut self, ipn: Option<String>, shown_query: &str) {
        self.pending = ipn.map(|ipn| (ipn, shown_query.trim().to_owned()));
    }

    /// For results of `query` asked for while `selected` was selected,
    /// whose section `category` holds it: the section to open, if any.
    /// It opens once per selection, and again if the part moves to another
    /// section (a team catalog change).
    fn results(
        &mut self,
        selected: Option<&str>,
        category: Option<&str>,
        query: &str,
    ) -> Option<String> {
        let query = query.trim();
        let selected = selected?;
        if let Some((ipn, asked)) = &self.pending {
            if ipn == selected {
                if !query.is_empty() && asked != query {
                    self.pending = None;
                    return None;
                }
                let category = category?;
                self.revealed = Some((ipn.clone(), category.to_owned()));
                self.pending = None;
                return Some(category.to_owned());
            }
        }
        match (&self.revealed, category) {
            (Some((ipn, was)), Some(now)) if ipn == selected && was != now => {
                self.revealed = Some((ipn.clone(), now.to_owned()));
                Some(now.to_owned())
            }
            _ => None,
        }
    }
}

/// How the list scrolls after a new result set.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Scroll {
    Stay,
    /// A new search: to the top of the list.
    Top,
    /// The search was cleared: to the selected part, if it is shown.
    Selected,
}

fn scroll_after(previous: Option<&str>, query: &str) -> Scroll {
    let query = query.trim();
    match previous {
        Some(previous) if previous.trim() == query => Scroll::Stay,
        None => Scroll::Stay,
        Some(_) if query.is_empty() => Scroll::Selected,
        Some(_) => Scroll::Top,
    }
}

/// How a section ends below its parts.
#[derive(Clone, Debug, PartialEq)]
enum SectionEnd {
    Nothing,
    /// More match than are shown: "Show all N".
    ShowAll(usize),
    Loading,
    Failed(String),
    /// The most a section shows, of how many.
    Capped {
        shown: usize,
        total: usize,
    },
}

/// The parts a section shows and how it ends: the loaded parts when it has
/// them, otherwise the first matches the search sent. The selected part is
/// among `hits` whenever it is in this section, so it is always shown.
/// While `reloading`, the loaded parts are from before a team catalog
/// change and may not match the new count, so nothing ends them.
fn section_rows(
    searching: bool,
    loaded: Option<&Load<Vec<PartSummary>>>,
    reloading: bool,
    hits: &[PartSummary],
    total: usize,
) -> (Vec<PartSummary>, SectionEnd) {
    let (mut rows, end) = match loaded {
        Some(Load::Ready(rows)) if reloading => (rows.clone(), Some(SectionEnd::Nothing)),
        Some(Load::Ready(rows)) => (rows.clone(), None),
        Some(Load::Failed(error)) => (Vec::new(), Some(SectionEnd::Failed(error.clone()))),
        Some(Load::Loading) => (Vec::new(), Some(SectionEnd::Loading)),
        None if searching => (Vec::new(), None),
        None => (Vec::new(), Some(SectionEnd::Loading)),
    };
    let loaded_all = matches!(loaded, Some(Load::Ready(_)));
    for hit in hits {
        if !rows.iter().any(|row| row.ipn == hit.ipn) {
            rows.push(hit.clone());
        }
    }
    let end = end.unwrap_or(if total <= rows.len() {
        SectionEnd::Nothing
    } else if loaded_all {
        SectionEnd::Capped {
            shown: rows.len(),
            total,
        }
    } else {
        SectionEnd::ShowAll(total)
    });
    (rows, end)
}

/// The count line above the list.
fn count_text(response: &SearchResponse) -> String {
    match response.total {
        0 => "No parts match.".to_owned(),
        1 => "1 matching part".to_owned(),
        total => format!("{} matching parts", grouped(total)),
    }
}

/// An HTML ID for a section's list of parts.
fn section_id(category: &str) -> String {
    let safe: String = category
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("parts-in-{safe}")
}

/// The IPNs of `rows`, in order. A section draws one row per IPN and keeps
/// it while the part is listed, so a changed part updates in place.
fn row_ipns(rows: &[PartSummary]) -> Vec<String> {
    rows.iter().map(|row| row.ipn.clone()).collect()
}

/// A row's title and the text beside it. Parts without an MPN are titled
/// by IPN, with the CSV file beside it instead of the IPN again.
fn row_labels(part: &PartSummary) -> (String, String) {
    if part.mpn.is_empty() {
        (part.ipn.clone(), part.csv_file.clone())
    } else {
        (part.mpn.clone(), part.ipn.clone())
    }
}

/// The parts loaded for open sections, for one search and team catalog
/// position. After a team catalog change in the same search, open
/// sections keep their parts while they load again.
#[derive(Clone, Debug, Default, PartialEq)]
struct Loaded {
    key: (String, Option<u64>),
    sections: BTreeMap<String, Load<Vec<PartSummary>>>,
    /// The sections whose Show all was chosen in this search, loaded again
    /// after a team catalog change.
    shown_all: BTreeSet<String>,
    /// The sections showing their earlier parts while they load again.
    reloading: BTreeSet<String>,
}

impl Loaded {
    /// What is kept for a new answer `key`, and the sections to load again.
    /// The same search after a team catalog change keeps Show all. Its open
    /// sections keep their parts while they load again, so the list does
    /// not shrink and move the selected part out of view. The part
    /// `selected` stays only in `selected_in`, the section that holds it
    /// now, so it is never shown twice.
    fn next(
        &self,
        key: (String, Option<u64>),
        is_open: impl Fn(&str) -> bool,
        selected: Option<&str>,
        selected_in: Option<&str>,
    ) -> (Loaded, BTreeSet<String>) {
        if self.key.0 != key.0 {
            return (
                Loaded {
                    key,
                    ..Loaded::default()
                },
                BTreeSet::new(),
            );
        }
        let sections: BTreeMap<_, _> = self
            .sections
            .iter()
            .filter(|(category, rows)| matches!(rows, Load::Ready(_)) && is_open(category))
            .map(|(category, rows)| {
                let mut rows = rows.clone();
                if let (Some(ipn), Load::Ready(rows)) = (selected, &mut rows) {
                    if selected_in != Some(category.as_str()) {
                        rows.retain(|row| row.ipn != ipn);
                    }
                }
                (category.clone(), rows)
            })
            .collect();
        let reload = sections.keys().chain(&self.shown_all).cloned().collect();
        let next = Loaded {
            key,
            reloading: sections.keys().cloned().collect(),
            sections,
            shown_all: self.shown_all.clone(),
        };
        (next, reload)
    }

    /// Marks `category` as loading. Parts already shown stay until the new
    /// ones arrive, and the section counts as loading meanwhile.
    fn start_load(&mut self, category: &str) {
        if matches!(self.sections.get(category), Some(Load::Ready(_))) {
            self.reloading.insert(category.to_owned());
        } else {
            self.sections.insert(category.to_owned(), Load::Loading);
        }
    }

    /// Takes in the parts loaded for `category`.
    fn finish_load(&mut self, category: String, rows: Load<Vec<PartSummary>>) {
        self.reloading.remove(&category);
        self.sections.insert(category, rows);
    }
}

#[component]
pub(crate) fn ResultList(
    results: RwSignal<Load<Searched>>,
    selected: RwSignal<Option<String>>,
    adding: RwSignal<bool>,
    checking: RwSignal<bool>,
) -> impl IntoView {
    let open = RwSignal::new(OpenSections::default());
    let loaded = RwSignal::new(Loaded::default());
    let reveals = StoredValue::new(Reveals::default());
    // The search (trimmed) the shown results are for.
    let shown_query = StoredValue::new(None::<String>);
    // Set when the selected part should be scrolled into view once drawn,
    // which may be after its section's parts load.
    let want_scroll = StoredValue::new(false);
    let try_scroll = move || {
        if want_scroll.get_value() {
            request_animation_frame(move || {
                // Kept only while the part's section is still loading, which
                // moves it; a part not shown is not scrolled to later.
                if scroll_selected_into_view() != Scrolled::WhileLoading {
                    want_scroll.set_value(false);
                }
            });
        }
    };

    // The section whose Show all was just chosen: once its parts load,
    // focus moves to the first newly shown part, as the button is gone.
    let focus_after_load = StoredValue::new(None::<String>);
    // Loads up to SECTION_ROWS parts of `category` for the search shown.
    let load_section = move |category: String| {
        let Load::Ready(searched) = results.get_untracked() else {
            return;
        };
        let key = loaded.with_untracked(|loaded| loaded.key.clone());
        loaded.update(|loaded| loaded.start_load(&category));
        spawn_local(async move {
            let answer = backend::search(
                searched.response.query.clone(),
                SECTION_ROWS,
                selected.get_untracked(),
                Some(category.clone()),
                searched.response.uses.clone(),
            )
            .await;
            let rows = match answer {
                Ok(response) => Load::Ready(
                    response
                        .sections
                        .into_iter()
                        .next()
                        .map(|section| section.hits)
                        .unwrap_or_default(),
                ),
                Err(error) => Load::Failed(error),
            };
            // Kept only if the search and team catalog are still the same.
            let current = loaded.with_untracked(|loaded| loaded.key == key);
            if current {
                let focus =
                    focus_after_load.with_value(|wanted| wanted.as_ref() == Some(&category));
                if focus {
                    focus_after_load.set_value(None);
                    let first_new = format!(
                        "#{} > li:nth-child({}) button.result",
                        section_id(&category),
                        SEARCH_ROWS + 1
                    );
                    focus_when_drawn(first_new);
                }
                loaded.update(|loaded| loaded.finish_load(category, rows));
                try_scroll();
            }
        });
    };
    Effect::new(move |previous: Option<Option<String>>| {
        let now = selected.get();
        if previous.as_ref() != Some(&now) {
            let shown = shown_query.get_value().unwrap_or_default();
            reveals.update_value(|reveals| reveals.select(now.clone(), &shown));
            // A part already in the shown results (clicked in the list) is
            // revealed at once, without waiting for the next answer.
            let found = now.as_deref().and_then(|ipn| {
                results.with_untracked(|results| match results {
                    Load::Ready(searched) => searched
                        .response
                        .sections
                        .iter()
                        .find(|section| section.hits.iter().any(|hit| hit.ipn == ipn))
                        .map(|section| (section.category.clone(), search_key(&searched.response))),
                    _ => None,
                })
            });
            if let Some((category, query)) = found {
                let reveal = reveals
                    .try_update_value(|reveals| {
                        reveals.results(now.as_deref(), Some(&category), &query)
                    })
                    .flatten();
                if let Some(category) = reveal {
                    open.update(|open| open.reveal(&category, &query));
                    want_scroll.set_value(true);
                    try_scroll();
                }
            }
        }
        now
    });
    Effect::new(move |_| {
        let Some((query, asked_for, category, version)) = results.with(|results| match results {
            Load::Ready(searched) => Some((
                search_key(&searched.response),
                searched.selected.clone(),
                searched.response.selected.clone(),
                searched.version,
            )),
            _ => None,
        }) else {
            return;
        };
        let key = (query.clone(), version);
        if loaded.with_untracked(|loaded| loaded.key != key) {
            let (next, reload) = loaded.with_untracked(|loaded| {
                open.with_untracked(|open| {
                    loaded.next(
                        key,
                        |category| open.is_open(category, &query),
                        asked_for.as_deref(),
                        category.as_deref(),
                    )
                })
            });
            loaded.set(next);
            for category in reload {
                load_section(category);
            }
        }
        // Only an answer asked for with the part selected now says where
        // that part is.
        let reveal = if asked_for == selected.get_untracked() {
            reveals
                .try_update_value(|reveals| {
                    reveals.results(asked_for.as_deref(), category.as_deref(), &query)
                })
                .flatten()
        } else {
            None
        };
        let previous = shown_query.get_value();
        shown_query.set_value(Some(query.clone()));
        if let Some(category) = reveal {
            open.update(|open| open.reveal(&category, &query));
            want_scroll.set_value(true);
        } else {
            match scroll_after(previous.as_deref(), &query) {
                Scroll::Stay => {}
                Scroll::Top => {
                    want_scroll.set_value(false);
                    scroll_list_to_top();
                }
                Scroll::Selected => want_scroll.set_value(true),
            }
        }
        try_scroll();
    });

    let choose = move |ipn: String| {
        adding.set(false);
        checking.set(false);
        selected.set(Some(ipn));
    };
    let toggled = move || want_scroll.set_value(false);
    view! {
        <p id="result-count" class="result-count" aria-live="polite">
            {move || results.with(|results| match results {
                Load::Loading => "Searching…".to_owned(),
                Load::Failed(error) => format!("Search failed: {error}"),
                Load::Ready(searched) => count_text(&searched.response),
            })}
        </p>
        <ul
            class="results"
            aria-label="Search results"
            data-query=move || results.with(|results| match results {
                Load::Ready(searched) => Some(searched.response.query.clone()),
                _ => None,
            })
        >
            // One section per category, kept while the search stays the
            // same. A new answer, such as one after a teammate's save,
            // updates its contents in place, so focus, open sections and
            // scroll stay put. The team catalog's position is not part of
            // the key: after a team change, Loaded::next loads the open
            // sections again without drawing them anew.
            // A section's parts are drawn only while it is open.
            <For
                each=move || results.with(|results| match results {
                    Load::Ready(searched) => {
                        let query = search_key(&searched.response);
                        searched
                            .response
                            .sections
                            .iter()
                            .map(|section| (query.clone(), section.category.clone()))
                            .collect::<Vec<_>>()
                    }
                    _ => Vec::new(),
                })
                key=|key| key.clone()
                children=move |(query, category)| view! {
                    <Section
                        results=results
                        category=category
                        query=query
                        open=open
                        loaded=loaded
                        selected=selected
                        choose=choose
                        load=load_section
                        toggled=toggled
                        focus_after_load=focus_after_load
                    />
                }
            />
        </ul>
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn Section(
    results: RwSignal<Load<Searched>>,
    category: String,
    query: String,
    open: RwSignal<OpenSections>,
    loaded: RwSignal<Loaded>,
    selected: RwSignal<Option<String>>,
    choose: impl Fn(String) + Copy + Send + Sync + 'static,
    load: impl Fn(String) + Copy + Send + Sync + 'static,
    toggled: impl Fn() + Copy + Send + Sync + 'static,
    focus_after_load: StoredValue<Option<String>>,
) -> impl IntoView {
    let is_searching = searching(&query);
    // This section's contents in the latest answer.
    let section = {
        let category = category.clone();
        Memo::new(move |_| {
            results.with(|results| match results {
                Load::Ready(searched) => searched
                    .response
                    .sections
                    .iter()
                    .find(|section| section.category == category)
                    .cloned(),
                _ => None,
            })
        })
    };
    let is_open = {
        let (category, query) = (category.clone(), query.clone());
        // A memo, so opening one section redraws only that one.
        Memo::new(move |_| open.with(|open| open.is_open(&category, &query)))
    };
    let mine = {
        let category = category.clone();
        Memo::new(move |_| loaded.with(|loaded| loaded.sections.get(&category).cloned()))
    };
    let reloading = {
        let category = category.clone();
        Memo::new(move |_| loaded.with(|loaded| loaded.reloading.contains(&category)))
    };
    // Without a search, an open section loads its parts the first time.
    {
        let category = category.clone();
        Effect::new(move |_| {
            if is_open.get() && !is_searching && mine.with(Option::is_none) {
                load(category.clone());
            }
        });
    }
    let shown = Memo::new(move |_| {
        section.with(|section| {
            mine.with(|mine| match section {
                Some(section) => section_rows(
                    is_searching,
                    mine.as_ref(),
                    reloading.get(),
                    &section.hits,
                    section.total,
                ),
                None => (Vec::new(), SectionEnd::Nothing),
            })
        })
    });
    // The rows drawn, by IPN.
    let drawn = Memo::new(move |_| shown.with(|(rows, _)| row_ipns(rows)));
    let id = section_id(&category);
    let name = move || {
        section.with(|section| section.as_ref().map(|s| s.name.clone()).unwrap_or_default())
    };
    let total = move || section.with(|section| section.as_ref().map_or(0, |s| s.total));
    let show_all = {
        let category = category.clone();
        move || {
            loaded.update(|loaded| {
                loaded.shown_all.insert(category.clone());
            });
            focus_after_load.set_value(Some(category.clone()));
            load(category.clone());
        }
    };
    view! {
        <li class="result-section">
            <button
                type="button"
                class="section-toggle"
                aria-expanded=move || if is_open.get() { "true" } else { "false" }
                aria-controls=id.clone()
                on:click={
                    let category = category.clone();
                    move |_| {
                        toggled();
                        open.update(|open| open.toggle(&category, &query));
                    }
                }
            >
                <span class="section-name">{name}</span>
                " "
                <span class="section-count">
                    {move || grouped(total())}
                    <span class="visually-hidden">
                        {move || if total() == 1 { " part" } else { " parts" }}
                    </span>
                </span>
            </button>
            <ul
                id=id
                class="section-parts"
                aria-label=name
                aria-busy=move || reloading.get().then_some("true")
                hidden=move || !is_open.get()
            >
                {move || is_open.get().then(|| {
                    let show_all = show_all.clone();
                    view! {
                        // One row per part, keyed by IPN. A part that
                        // changes updates its row in place, so a focused
                        // row keeps focus.
                        <For
                            each=move || drawn.get()
                            key=|ipn| ipn.clone()
                            children=move |ipn| {
                                let part = row_part(shown, ipn.clone());
                                view! {
                                    <PartRow
                                        ipn=ipn
                                        part=part
                                        selected=selected
                                        choose=choose
                                    />
                                }
                            }
                        />
                        {move || {
                            let show_all = show_all.clone();
                            match shown.with(|(_, end)| end.clone()) {
                                SectionEnd::Nothing => ().into_any(),
                                SectionEnd::ShowAll(total) => view! {
                                    <li class="section-more">
                                        <button
                                            type="button"
                                            class="section-show-all"
                                            on:click=move |_| show_all()
                                        >
                                            {format!("Show all {}", grouped(total))}
                                        </button>
                                    </li>
                                }
                                .into_any(),
                                SectionEnd::Loading => view! {
                                    <li class="section-more" aria-busy="true">"Loading…"</li>
                                }
                                .into_any(),
                                SectionEnd::Failed(error) => view! {
                                    <li class="section-more problem">{format!("Could not load these parts: {error}")}</li>
                                }
                                .into_any(),
                                SectionEnd::Capped { shown, total } => view! {
                                    <li class="section-more">
                                        {format!(
                                            "Showing {} of {}. Search to narrow the list.",
                                            grouped(shown),
                                            grouped(total),
                                        )}
                                    </li>
                                }
                                .into_any(),
                            }
                        }}
                    }
                })}
            </ul>
        </li>
    }
}

/// What the row of `ipn` shows: its part in the section's latest rows.
/// While the row is being removed, it keeps the last part it had.
fn row_part(shown: Memo<(Vec<PartSummary>, SectionEnd)>, ipn: String) -> Memo<Option<PartSummary>> {
    Memo::new(move |before: Option<&Option<PartSummary>>| {
        shown
            .with(|(rows, _)| rows.iter().find(|row| row.ipn == ipn).cloned())
            .or_else(|| before.cloned().flatten())
    })
}

#[component]
fn PartRow(
    ipn: String,
    part: Memo<Option<PartSummary>>,
    selected: RwSignal<Option<String>>,
    choose: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let is_selected = {
        let ipn = ipn.clone();
        move || selected.with(|current| current.as_deref() == Some(&ipn))
    };
    let labels =
        Memo::new(move |_| part.with(|part| part.as_ref().map(row_labels).unwrap_or_default()));
    let description = move || part.with(|part| part.as_ref().map(|part| part.description.clone()));
    view! {
        <li>
            <button
                type="button"
                class="result"
                data-ipn=ipn.clone()
                aria-current=move || is_selected().then_some("true")
                on:click=move |_| choose(ipn.clone())
            >
                <span class="result-title">{move || labels.with(|(title, _)| title.clone())}</span>
                <span class="result-ipn">{move || labels.with(|(_, aside)| aside.clone())}</span>
                <span class="result-description">{description}</span>
            </button>
        </li>
    }
}

/// What [`scroll_selected_into_view`] did.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Scrolled {
    /// No selected part is drawn.
    NotShown,
    /// Scrolled, but its section's other parts are still loading, which
    /// will move it.
    WhileLoading,
    Done,
}

/// Scrolls the selected part into view in the list, if it is drawn. The
/// list's scroll padding keeps it clear of its sticky section header.
fn scroll_selected_into_view() -> Scrolled {
    let Ok(Some(row)) = document().query_selector(".results .result[aria-current=\"true\"]") else {
        return Scrolled::NotShown;
    };
    // A section loading its parts, or showing its earlier parts while it
    // loads again, is marked busy.
    let loading = row
        .closest(".section-parts")
        .ok()
        .flatten()
        .is_some_and(|list| {
            list.get_attribute("aria-busy").as_deref() == Some("true")
                || list
                    .query_selector("[aria-busy=\"true\"]")
                    .ok()
                    .flatten()
                    .is_some()
        });
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"block".into(), &"nearest".into());
    if let Ok(method) = js_sys::Reflect::get(&row, &"scrollIntoView".into()) {
        use wasm_bindgen::JsCast;
        if let Some(method) = method.dyn_ref::<js_sys::Function>() {
            let _ = method.call1(&row, &options);
        }
    }
    if loading {
        Scrolled::WhileLoading
    } else {
        Scrolled::Done
    }
}

/// Moves keyboard focus to the element `selector` finds once the view has
/// been redrawn.
fn focus_when_drawn(selector: String) {
    request_animation_frame(move || {
        use wasm_bindgen::JsCast;
        if let Some(element) = document()
            .query_selector(&selector)
            .ok()
            .flatten()
            .and_then(|element| element.dyn_into::<leptos::web_sys::HtmlElement>().ok())
        {
            let _ = element.focus();
        }
    });
}

fn scroll_list_to_top() {
    if let Ok(Some(list)) = document().query_selector(".results") {
        let _ = js_sys::Reflect::set(&list, &"scrollTop".into(), &0.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_starts_closed_and_keeps_what_the_person_opened() {
        let mut open = OpenSections::default();
        assert!(!open.is_open("res", ""));
        open.toggle("res", "");
        open.toggle("cap", "");
        open.toggle("cap", "");
        assert!(open.is_open("res", ""));
        assert!(!open.is_open("cap", ""));
        // Blank space is no search.
        assert!(open.is_open("res", "  "));
    }

    #[test]
    fn a_search_opens_its_sections_and_clearing_it_goes_back() {
        let mut open = OpenSections::default();
        open.toggle("res", "");
        assert!(open.is_open("cap", "0603"));
        assert!(open.is_open("res", "0603"));
        // Closed by hand during this search only.
        open.toggle("cap", "0603");
        assert!(!open.is_open("cap", "0603"));
        assert!(open.is_open("cap", "0603 1%"));
        // Back to what the person had open.
        assert!(open.is_open("res", ""));
        assert!(!open.is_open("cap", ""));
        open.toggle("res", "0603");
        assert!(
            open.is_open("res", ""),
            "closing during a search changes only that search"
        );
    }

    #[test]
    fn a_trailing_space_is_the_same_search() {
        let mut open = OpenSections::default();
        open.toggle("cap", "0603");
        assert!(!open.is_open("cap", "0603 "));
        assert!(!open.is_open("cap", " 0603"));
    }

    #[test]
    fn selecting_a_part_opens_its_section_now_and_after_the_search() {
        let mut open = OpenSections::default();
        open.reveal("res", "");
        assert!(open.is_open("res", ""));
        let mut open = OpenSections::default();
        open.toggle("res", "10k");
        assert!(!open.is_open("res", "10k"));
        open.reveal("res", "10k");
        assert!(open.is_open("res", "10k"));
        assert!(open.is_open("res", ""));
    }

    #[test]
    fn a_selected_part_is_revealed_once_its_section_is_known() {
        let mut reveals = Reveals::default();
        reveals.select(Some("RES-1".into()), "");
        // An answer that does not hold it yet (a new part not in the team
        // catalog's results yet) keeps waiting.
        assert_eq!(reveals.results(Some("RES-1"), None, ""), None);
        assert_eq!(
            reveals.results(Some("RES-1"), Some("res"), ""),
            Some("res".into())
        );
        // Once only: later answers, such as a team change, do not reopen it.
        assert_eq!(reveals.results(Some("RES-1"), Some("res"), ""), None);
        assert_eq!(reveals.results(Some("RES-1"), Some("res"), "10k"), None);
    }

    #[test]
    fn clearing_the_search_reveals_a_part_it_hid() {
        let mut reveals = Reveals::default();
        reveals.select(Some("CAP-1".into()), "100n ");
        assert_eq!(reveals.results(Some("CAP-1"), None, "100n"), None);
        assert_eq!(
            reveals.results(Some("CAP-1"), Some("cap"), ""),
            Some("cap".into())
        );
    }

    #[test]
    fn typing_another_search_forgets_a_part_it_does_not_hold() {
        let mut reveals = Reveals::default();
        reveals.select(Some("CAP-1".into()), "100n");
        assert_eq!(reveals.results(Some("CAP-1"), None, "100n 0603"), None);
        assert_eq!(reveals.results(Some("CAP-1"), Some("cap"), ""), None);
    }

    #[test]
    fn a_part_moved_to_another_category_is_revealed_again() {
        let mut reveals = Reveals::default();
        reveals.select(Some("RES-1".into()), "");
        assert_eq!(
            reveals.results(Some("RES-1"), Some("res"), ""),
            Some("res".into())
        );
        assert_eq!(
            reveals.results(Some("RES-1"), Some("cap"), ""),
            Some("cap".into())
        );
        assert_eq!(reveals.results(Some("RES-1"), Some("cap"), ""), None);
    }

    #[test]
    fn the_list_scrolls_to_the_top_for_a_new_search_and_to_the_part_when_cleared() {
        assert_eq!(scroll_after(None, ""), Scroll::Stay);
        assert_eq!(scroll_after(Some("10k"), "10k "), Scroll::Stay);
        assert_eq!(scroll_after(Some("10k"), "10k 1%"), Scroll::Top);
        assert_eq!(scroll_after(Some(""), "1"), Scroll::Top);
        assert_eq!(scroll_after(Some("10k"), " "), Scroll::Selected);
    }

    fn hit(ipn: &str) -> PartSummary {
        PartSummary {
            ipn: ipn.into(),
            csv_file: "g-res.csv".into(),
            mpn: String::new(),
            manufacturer: String::new(),
            description: String::new(),
            value: String::new(),
        }
    }

    fn ipns(rows: &[PartSummary]) -> Vec<&str> {
        rows.iter().map(|row| row.ipn.as_str()).collect()
    }

    #[test]
    fn a_searched_section_shows_its_first_matches_then_show_all() {
        let hits = [hit("RES-1"), hit("RES-2")];
        let (rows, end) = section_rows(true, None, false, &hits, 30);
        assert_eq!(
            (ipns(&rows), end),
            (vec!["RES-1", "RES-2"], SectionEnd::ShowAll(30))
        );
        let (_, end) = section_rows(true, None, false, &hits, 2);
        assert_eq!(end, SectionEnd::Nothing);
        // After "Show all": the loaded parts, capped.
        let all: Vec<PartSummary> = (0..3).map(|n| hit(&format!("RES-{n}"))).collect();
        let (rows, end) = section_rows(true, Some(&Load::Ready(all)), false, &hits, 300);
        assert_eq!(ipns(&rows), ["RES-0", "RES-1", "RES-2"]);
        assert_eq!(
            end,
            SectionEnd::Capped {
                shown: 3,
                total: 300
            }
        );
    }

    #[test]
    fn an_opened_section_loads_its_parts_and_always_shows_the_selected_one() {
        // Without a search the answer holds only the selected part.
        let selected = [hit("RES-9")];
        let (rows, end) = section_rows(false, None, false, &selected, 10);
        assert_eq!((ipns(&rows), end), (vec!["RES-9"], SectionEnd::Loading));
        let loaded = Load::Ready(vec![hit("RES-1"), hit("RES-2")]);
        let (rows, end) = section_rows(false, Some(&loaded), false, &selected, 3);
        assert_eq!(
            (ipns(&rows), end),
            (vec!["RES-1", "RES-2", "RES-9"], SectionEnd::Nothing)
        );
        let failed = Load::Failed("no".into());
        let (_, end) = section_rows(false, Some(&failed), false, &[], 3);
        assert_eq!(end, SectionEnd::Failed("no".into()));
    }

    #[test]
    fn a_team_catalog_change_keeps_the_open_sections_parts_while_they_load_again() {
        let ready = || Load::Ready(vec![hit("CAP-1"), hit("CAP-2")]);
        let before = Loaded {
            key: (String::new(), Some(1)),
            sections: BTreeMap::from([
                ("cap".to_owned(), ready()),
                ("res".to_owned(), ready()),
                ("ind".to_owned(), Load::Loading),
            ]),
            ..Loaded::default()
        };
        // Only the open, loaded sections are kept, and they load again,
        // marked as reloading.
        let (next, reload) = before.next(
            (String::new(), Some(2)),
            |category| category != "res",
            None,
            None,
        );
        assert_eq!(next.key, (String::new(), Some(2)));
        assert_eq!(next.sections, BTreeMap::from([("cap".to_owned(), ready())]));
        assert_eq!(next.reloading, BTreeSet::from(["cap".to_owned()]));
        assert_eq!(reload, BTreeSet::from(["cap".to_owned()]));
        // Show all is kept, and loaded again, in the same search only. A
        // closed section whose Show all was chosen loads again but keeps
        // no parts.
        let searched = Loaded {
            key: ("10".to_owned(), Some(1)),
            sections: BTreeMap::from([("res".to_owned(), ready()), ("cap".to_owned(), ready())]),
            shown_all: BTreeSet::from(["res".to_owned(), "cap".to_owned()]),
            ..Loaded::default()
        };
        let (next, reload) = searched.next(
            ("10".to_owned(), Some(2)),
            |category| category == "res",
            None,
            None,
        );
        assert_eq!(next.shown_all, searched.shown_all);
        assert_eq!(next.sections, BTreeMap::from([("res".to_owned(), ready())]));
        assert_eq!(next.reloading, BTreeSet::from(["res".to_owned()]));
        assert_eq!(reload, BTreeSet::from(["cap".to_owned(), "res".to_owned()]));
        // A new search keeps nothing.
        let (next, reload) = searched.next(("100".to_owned(), Some(2)), |_| true, None, None);
        assert_eq!(
            next,
            Loaded {
                key: ("100".to_owned(), Some(2)),
                ..Loaded::default()
            }
        );
        assert!(reload.is_empty());
    }

    #[test]
    fn a_selected_part_that_moved_section_is_kept_only_where_it_is_now() {
        let before = Loaded {
            key: (String::new(), Some(1)),
            sections: BTreeMap::from([
                (
                    "cap".to_owned(),
                    Load::Ready(vec![hit("CAP-1"), hit("CAP-2")]),
                ),
                (
                    "res".to_owned(),
                    Load::Ready(vec![hit("CAP-2"), hit("RES-1")]),
                ),
            ]),
            ..Loaded::default()
        };
        let (next, _) = before.next(
            (String::new(), Some(2)),
            |_| true,
            Some("CAP-2"),
            Some("res"),
        );
        let kept = |category: &str| match &next.sections[category] {
            Load::Ready(rows) => ipns(rows).join(" "),
            other => panic!("{other:?}"),
        };
        assert_eq!(kept("cap"), "CAP-1");
        assert_eq!(kept("res"), "CAP-2 RES-1");
    }

    #[test]
    fn loading_a_section_keeps_its_parts_until_the_new_ones_arrive() {
        let mut loaded = Loaded {
            sections: BTreeMap::from([("cap".to_owned(), Load::Ready(vec![hit("CAP-1")]))]),
            ..Loaded::default()
        };
        loaded.start_load("cap");
        loaded.start_load("res");
        assert_eq!(loaded.sections["cap"], Load::Ready(vec![hit("CAP-1")]));
        assert_eq!(loaded.sections["res"], Load::Loading);
        assert_eq!(loaded.reloading, BTreeSet::from(["cap".to_owned()]));
        loaded.finish_load("cap".to_owned(), Load::Ready(Vec::new()));
        assert_eq!(loaded.sections["cap"], Load::Ready(Vec::new()));
        assert!(loaded.reloading.is_empty());
    }

    #[test]
    fn a_reloading_section_shows_its_parts_with_nothing_after_them() {
        // The new count says 41, but the parts are from before.
        let earlier = Load::Ready(vec![hit("RES-1"), hit("RES-2")]);
        let (rows, end) = section_rows(false, Some(&earlier), true, &[], 41);
        assert_eq!(
            (ipns(&rows), end),
            (vec!["RES-1", "RES-2"], SectionEnd::Nothing)
        );
    }

    #[test]
    fn a_list_limited_to_a_reference_counts_as_a_search() {
        let response = |query: &str, uses: Option<&str>| SearchResponse {
            query: query.into(),
            uses: uses.map(str::to_owned),
            total: 0,
            sections: Vec::new(),
            selected: None,
            elapsed_micros: 0,
        };
        assert_eq!(search_key(&response(" r ", None)), "r");
        let limited = search_key(&response("", Some("g-res:R_US")));
        assert!(searching(&limited));
        assert_ne!(limited, search_key(&response("", Some("g-res:R_Small"))));
        assert_ne!(limited, search_key(&response("47", Some("g-res:R_US"))));
    }

    #[test]
    fn the_count_line_counts_every_match() {
        let response = |total| SearchResponse {
            query: String::new(),
            uses: None,
            total,
            sections: Vec::new(),
            selected: None,
            elapsed_micros: 0,
        };
        assert_eq!(count_text(&response(0)), "No parts match.");
        assert_eq!(count_text(&response(1)), "1 matching part");
        assert_eq!(count_text(&response(2500)), "2,500 matching parts");
    }

    #[test]
    fn section_ids_are_safe() {
        assert_eq!(section_id("res"), "parts-in-res");
        assert_eq!(section_id("a b/c"), "parts-in-a-b-c");
    }

    #[test]
    fn a_row_is_titled_by_mpn_or_else_by_ipn_with_its_csv_file() {
        let with_mpn = PartSummary {
            mpn: "RC0603FR-072KL".to_owned(),
            ..hit("RES-0000-2001")
        };
        assert_eq!(
            row_labels(&with_mpn),
            ("RC0603FR-072KL".to_owned(), "RES-0000-2001".to_owned())
        );
        let without_mpn = PartSummary {
            mpn: String::new(),
            csv_file: "g-res.csv".to_owned(),
            ..hit("RES-0000-2001")
        };
        assert_eq!(
            row_labels(&without_mpn),
            ("RES-0000-2001".to_owned(), "g-res.csv".to_owned())
        );
    }

    #[test]
    fn rows_are_identified_by_ipn_alone_so_a_changed_part_keeps_its_row() {
        let before = vec![hit("A"), hit("B")];
        let after = vec![
            hit("A"),
            PartSummary {
                description: "Changed by a teammate".to_owned(),
                ..hit("B")
            },
        ];
        assert_eq!(row_ipns(&before), row_ipns(&after));
    }
}

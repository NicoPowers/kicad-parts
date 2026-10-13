//! The part list on the left: the search results, one section per category
//! that opens and closes (an accordion).
//!
//! With an empty search every section starts closed and keeps what the
//! person opened and closed; a section's parts are loaded when it opens.
//! While searching, the sections with matches are open, each showing its
//! first matches with "Show all", and the others are not shown; clearing
//! the search goes back to what the person had open. Selecting a part
//! opens its section and scrolls the part into view.

use crate::{Load, backend, grouped};
use desktop_contract::{PartSummary, SearchResponse, SearchSection};
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
    /// The team catalog's position then; loaded sections are kept only
    /// while it and the search stay the same.
    pub(crate) version: Option<u64>,
}

/// Whether `query` is a search, rather than the whole catalog.
fn searching(query: &str) -> bool {
    !query.trim().is_empty()
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
    /// result set holds it; a new search forgets it.
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
                if asked != query {
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
fn section_rows(
    searching: bool,
    loaded: Option<&Load<Vec<PartSummary>>>,
    hits: &[PartSummary],
    total: usize,
) -> (Vec<PartSummary>, SectionEnd) {
    let (mut rows, end) = match loaded {
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

/// Identifies what a section shows, so a new result set redraws only the
/// sections that changed.
fn section_key(query: &str, section: &SearchSection) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    (
        query.trim(),
        &section.category,
        &section.name,
        section.total,
    )
        .hash(&mut hasher);
    for hit in &section.hits {
        (
            &hit.ipn,
            &hit.mpn,
            &hit.csv_file,
            &hit.description,
            &hit.manufacturer,
        )
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// The parts loaded for open sections, for one search and team catalog
/// position.
#[derive(Clone, Debug, Default, PartialEq)]
struct Loaded {
    key: (String, Option<u64>),
    sections: BTreeMap<String, Load<Vec<PartSummary>>>,
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
                if scroll_selected_into_view() {
                    want_scroll.set_value(false);
                }
            });
        }
    };

    Effect::new(move |previous: Option<Option<String>>| {
        let now = selected.get();
        if previous.as_ref() != Some(&now) {
            let shown = shown_query.get_value().unwrap_or_default();
            reveals.update_value(|reveals| reveals.select(now.clone(), &shown));
        }
        now
    });
    Effect::new(move |_| {
        let Some((query, asked_for, category, version)) = results.with(|results| match results {
            Load::Ready(searched) => Some((
                searched.response.query.trim().to_owned(),
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
            loaded.set(Loaded {
                key,
                sections: BTreeMap::new(),
            });
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

    // Loads up to SECTION_ROWS parts of `category` for the search shown.
    let load_section = move |category: String| {
        let Load::Ready(searched) = results.get_untracked() else {
            return;
        };
        let key = loaded.with_untracked(|loaded| loaded.key.clone());
        loaded.update(|loaded| {
            loaded.sections.insert(category.clone(), Load::Loading);
        });
        spawn_local(async move {
            let answer = backend::search(
                searched.response.query.clone(),
                SECTION_ROWS,
                selected.get_untracked(),
                Some(category.clone()),
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
                loaded.update(|loaded| {
                    loaded.sections.insert(category, rows);
                });
                try_scroll();
            }
        });
    };
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
            // Keyed by what each section shows, so a new result set redraws
            // only the sections that changed; a section's parts are drawn
            // only while it is open.
            <For
                each=move || results.with(|results| match results {
                    Load::Ready(searched) => {
                        let query = searched.response.query.clone();
                        searched
                            .response
                            .sections
                            .iter()
                            .map(|section| (section_key(&query, section), query.clone(), section.clone()))
                            .collect::<Vec<_>>()
                    }
                    _ => Vec::new(),
                })
                key=|(key, _, _)| *key
                children=move |(_, query, section)| view! {
                    <Section
                        section=section
                        query=query
                        open=open
                        loaded=loaded
                        selected=selected
                        choose=choose
                        load=load_section
                        toggled=toggled
                    />
                }
            />
        </ul>
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn Section(
    section: SearchSection,
    query: String,
    open: RwSignal<OpenSections>,
    loaded: RwSignal<Loaded>,
    selected: RwSignal<Option<String>>,
    choose: impl Fn(String) + Copy + Send + Sync + 'static,
    load: impl Fn(String) + Copy + Send + Sync + 'static,
    toggled: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let category = section.category.clone();
    let is_searching = searching(&query);
    let is_open = {
        let (category, query) = (category.clone(), query.clone());
        // A memo, so opening one section redraws only that one.
        Memo::new(move |_| open.with(|open| open.is_open(&category, &query)))
    };
    let mine = {
        let category = category.clone();
        Memo::new(move |_| loaded.with(|loaded| loaded.sections.get(&category).cloned()))
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
    let id = section_id(&category);
    let noun = if section.total == 1 {
        " part"
    } else {
        " parts"
    };
    let total = section.total;
    let hits = StoredValue::new(section.hits);
    let name = section.name.clone();
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
                <span class="section-name">{section.name.clone()}</span>
                " "
                <span class="section-count">
                    {grouped(total)}
                    <span class="visually-hidden">{noun}</span>
                </span>
            </button>
            <ul id=id class="section-parts" aria-label=name hidden=move || !is_open.get()>
                {move || is_open.get().then(|| {
                    let (rows, end) = mine.with(|mine| {
                        hits.with_value(|hits| section_rows(is_searching, mine.as_ref(), hits, total))
                    });
                    let rows = rows
                        .into_iter()
                        .map(|hit| view! { <PartRow hit=hit selected=selected choose=choose /> })
                        .collect::<Vec<_>>();
                    let category = category.clone();
                    let end = match end {
                        SectionEnd::Nothing => ().into_any(),
                        SectionEnd::ShowAll(total) => view! {
                            <li class="section-more">
                                <button
                                    type="button"
                                    class="section-show-all"
                                    on:click=move |_| load(category.clone())
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
                    };
                    view! { {rows}{end} }
                })}
            </ul>
        </li>
    }
}

#[component]
fn PartRow(
    hit: PartSummary,
    selected: RwSignal<Option<String>>,
    choose: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let ipn = hit.ipn.clone();
    let is_selected = {
        let ipn = ipn.clone();
        move || selected.with(|current| current.as_deref() == Some(&ipn))
    };
    // Parts without an MPN are titled by IPN; show the CSV file beside it
    // instead of repeating the IPN.
    let (title, aside) = if hit.mpn.is_empty() {
        (hit.ipn.clone(), hit.csv_file.clone())
    } else {
        (hit.mpn.clone(), hit.ipn.clone())
    };
    view! {
        <li>
            <button
                type="button"
                class="result"
                aria-current=move || is_selected().then_some("true")
                on:click=move |_| choose(ipn.clone())
            >
                <span class="result-title">{title}</span>
                <span class="result-ipn">{aside}</span>
                <span class="result-description">{hit.description}</span>
            </button>
        </li>
    }
}

/// Scrolls the selected part into view in the list, if it is drawn. The
/// list's scroll padding keeps it clear of its sticky section header.
/// Says whether that is final: not while its section's other parts are
/// still loading, which moves it.
fn scroll_selected_into_view() -> bool {
    let Ok(Some(row)) = document().query_selector(".results .result[aria-current=\"true\"]") else {
        return false;
    };
    let loading = row
        .closest(".section-parts")
        .ok()
        .flatten()
        .and_then(|list| list.query_selector("[aria-busy=\"true\"]").ok().flatten())
        .is_some();
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"block".into(), &"nearest".into());
    if let Ok(method) = js_sys::Reflect::get(&row, &"scrollIntoView".into()) {
        use wasm_bindgen::JsCast;
        if let Some(method) = method.dyn_ref::<js_sys::Function>() {
            let _ = method.call1(&row, &options);
        }
    }
    !loading
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
    fn a_new_search_forgets_a_part_it_does_not_hold() {
        let mut reveals = Reveals::default();
        reveals.select(Some("CAP-1".into()), "100n ");
        assert_eq!(reveals.results(Some("CAP-1"), None, "100n"), None);
        assert_eq!(reveals.results(Some("CAP-1"), Some("cap"), ""), None);
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
        let (rows, end) = section_rows(true, None, &hits, 30);
        assert_eq!(
            (ipns(&rows), end),
            (vec!["RES-1", "RES-2"], SectionEnd::ShowAll(30))
        );
        let (_, end) = section_rows(true, None, &hits, 2);
        assert_eq!(end, SectionEnd::Nothing);
        // After "Show all": the loaded parts, capped.
        let all: Vec<PartSummary> = (0..3).map(|n| hit(&format!("RES-{n}"))).collect();
        let (rows, end) = section_rows(true, Some(&Load::Ready(all)), &hits, 300);
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
        let (rows, end) = section_rows(false, None, &selected, 10);
        assert_eq!((ipns(&rows), end), (vec!["RES-9"], SectionEnd::Loading));
        let loaded = Load::Ready(vec![hit("RES-1"), hit("RES-2")]);
        let (rows, end) = section_rows(false, Some(&loaded), &selected, 3);
        assert_eq!(
            (ipns(&rows), end),
            (vec!["RES-1", "RES-2", "RES-9"], SectionEnd::Nothing)
        );
        let failed = Load::Failed("no".into());
        let (_, end) = section_rows(false, Some(&failed), &[], 3);
        assert_eq!(end, SectionEnd::Failed("no".into()));
    }

    #[test]
    fn the_count_line_counts_every_match() {
        let response = |total| SearchResponse {
            query: String::new(),
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
}

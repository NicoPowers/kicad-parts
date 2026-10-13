//! The part list on the left: the search results, one section per category
//! that opens and closes (an accordion).
//!
//! With an empty search every section starts closed and keeps what the
//! person opened and closed. While searching, the sections with matches
//! are open and the others are not shown; clearing the search goes back to
//! what the person had open. Selecting a part opens its section and
//! scrolls the part into view.

use crate::{Load, grouped};
use desktop_contract::{SearchResponse, SearchSection};
use leptos::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// Which sections are open.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OpenSections {
    /// The sections the person has open while not searching, by category
    /// code. Kept for the session.
    opened: BTreeSet<String>,
    /// While searching, the sections the person opened or closed by hand,
    /// and the search they did it in. A new search forgets them.
    during_search: Option<(String, BTreeMap<String, bool>)>,
}

/// Whether `query` is a search, rather than the whole catalog.
fn searching(query: &str) -> bool {
    !query.trim().is_empty()
}

impl OpenSections {
    /// Whether `category`'s section is open in the results for `query`.
    pub(crate) fn is_open(&self, category: &str, query: &str) -> bool {
        if !searching(query) {
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
        if !searching(query) {
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

/// The section holding the part `ipn`, if the results show it.
fn section_of<'a>(response: &'a SearchResponse, ipn: &str) -> Option<&'a SearchSection> {
    response
        .sections
        .iter()
        .find(|section| section.hits.iter().any(|hit| hit.ipn == ipn))
}

/// The count line above the list.
fn count_text(response: &SearchResponse) -> String {
    let shown = response.shown();
    if response.total == 0 {
        "No parts match.".to_owned()
    } else if response.total > shown {
        format!(
            "Showing {} of {} matching parts",
            grouped(shown),
            grouped(response.total),
        )
    } else {
        let noun = if response.total == 1 { "part" } else { "parts" };
        format!("{} matching {noun}", grouped(response.total))
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

#[component]
pub(crate) fn ResultList(
    results: RwSignal<Load<SearchResponse>>,
    selected: RwSignal<Option<String>>,
    adding: RwSignal<bool>,
    checking: RwSignal<bool>,
) -> impl IntoView {
    let open = RwSignal::new(OpenSections::default());
    // A part selected (here, by Add part or from a notice) whose section
    // has not been opened yet: it may only appear in a later result set,
    // such as a new part once the team catalog has it.
    let to_reveal = StoredValue::new(None::<String>);
    // The search the shown results are for.
    let shown_query = StoredValue::new(None::<String>);
    Effect::new(move |previous: Option<Option<String>>| {
        let now = selected.get();
        if previous.as_ref() != Some(&now) {
            to_reveal.set_value(now.clone());
        }
        // Read the results first and change the open sections afterwards,
        // never while the results are borrowed.
        let shown = results.with(|results| match results {
            Load::Ready(response) => {
                let found = to_reveal.with_value(|ipn| {
                    ipn.as_ref()
                        .and_then(|ipn| section_of(response, ipn))
                        .map(|section| section.category.clone())
                });
                Some((response.query.clone(), found))
            }
            _ => None,
        });
        let Some((query, found)) = shown else {
            return now;
        };
        let new_search = shown_query.with_value(|shown| shown.as_ref() != Some(&query));
        shown_query.set_value(Some(query.clone()));
        if let Some(category) = found {
            open.update(|open| open.reveal(&category, &query));
            to_reveal.set_value(None);
            scroll_selected_into_view();
        } else if new_search {
            // After a new search, or clearing it, the selected part stays
            // in view if its section is open.
            scroll_selected_into_view();
        }
        now
    });
    let choose = move |ipn: String| {
        adding.set(false);
        checking.set(false);
        selected.set(Some(ipn));
    };
    view! {
        <p id="result-count" class="result-count" aria-live="polite">
            {move || match results.get() {
                Load::Loading => "Searching…".to_owned(),
                Load::Failed(error) => format!("Search failed: {error}"),
                Load::Ready(response) => count_text(&response),
            }}
        </p>
        <ul
            class="results"
            aria-label="Search results"
            data-query=move || match results.get() {
                Load::Ready(response) => Some(response.query),
                _ => None,
            }
        >
            // Built once per result set; a section's parts are drawn only
            // while it is open.
            {move || {
                let Load::Ready(response) = results.get() else {
                    return Vec::new();
                };
                let query = response.query;
                response
                    .sections
                    .into_iter()
                    .map(|section| view! {
                        <Section section=section query=query.clone() open=open selected=selected choose=choose />
                    })
                    .collect::<Vec<_>>()
            }}
        </ul>
    }
}

#[component]
fn Section(
    section: SearchSection,
    query: String,
    open: RwSignal<OpenSections>,
    selected: RwSignal<Option<String>>,
    choose: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let category = section.category.clone();
    let is_open = {
        let (category, query) = (category.clone(), query.clone());
        // A memo, so opening one section redraws only that one.
        Memo::new(move |_| open.with(|open| open.is_open(&category, &query)))
    };
    let id = section_id(&category);
    let more = (section.total > section.hits.len()).then(|| {
        format!(
            "Showing {} of {}. Search to narrow the list.",
            grouped(section.hits.len()),
            grouped(section.total),
        )
    });
    let noun = if section.total == 1 {
        " part"
    } else {
        " parts"
    };
    let hits = StoredValue::new(section.hits);
    view! {
        <li class="result-section">
            <button
                type="button"
                class="section-toggle"
                aria-expanded=move || if is_open.get() { "true" } else { "false" }
                aria-controls=id.clone()
                on:click=move |_| open.update(|open| open.toggle(&category, &query))
            >
                <span class="section-name">{section.name.clone()}</span>
                " "
                <span class="section-count">
                    {grouped(section.total)}
                    <span class="visually-hidden">{noun}</span>
                </span>
            </button>
            <ul id=id class="section-parts" aria-label=section.name hidden=move || !is_open.get()>
                {move || is_open.get().then(|| {
                    let rows = hits.with_value(|hits| {
                        hits.iter()
                            .map(|hit| {
                                let ipn = hit.ipn.clone();
                                let is_selected = {
                                    let ipn = ipn.clone();
                                    move || selected.with(|current| current.as_deref() == Some(&ipn))
                                };
                                // Parts without an MPN are titled by IPN; show the
                                // CSV file beside it instead of repeating the IPN.
                                let (title, aside) = if hit.mpn.is_empty() {
                                    (hit.ipn.clone(), hit.csv_file.clone())
                                } else {
                                    (hit.mpn.clone(), hit.ipn.clone())
                                };
                                let description = hit.description.clone();
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
                                            <span class="result-description">{description}</span>
                                        </button>
                                    </li>
                                }
                            })
                            .collect::<Vec<_>>()
                    });
                    view! {
                        {rows}
                        {more.clone().map(|more| view! { <li class="section-more">{more}</li> })}
                    }
                })}
            </ul>
        </li>
    }
}

/// Scrolls the selected part into view in the list once it is drawn.
fn scroll_selected_into_view() {
    request_animation_frame(|| {
        let Ok(Some(row)) = document().query_selector(".results .result[aria-current=\"true\"]")
        else {
            return;
        };
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &"block".into(), &"nearest".into());
        if let Ok(method) = js_sys::Reflect::get(&row, &"scrollIntoView".into()) {
            use wasm_bindgen::JsCast;
            if let Some(method) = method.dyn_ref::<js_sys::Function>() {
                let _ = method.call1(&row, &options);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_contract::PartSummary;

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

    fn response(total: usize, sections: Vec<(&str, usize, Vec<&str>)>) -> SearchResponse {
        SearchResponse {
            query: String::new(),
            total,
            sections: sections
                .into_iter()
                .map(|(category, total, ipns)| SearchSection {
                    category: category.into(),
                    name: category.into(),
                    total,
                    hits: ipns.into_iter().map(hit).collect(),
                })
                .collect(),
            elapsed_micros: 0,
        }
    }

    #[test]
    fn finds_the_section_of_a_part() {
        let found = response(
            3,
            vec![
                ("cap", 1, vec!["CAP-1"]),
                ("res", 2, vec!["RES-1", "RES-2"]),
            ],
        );
        assert_eq!(
            section_of(&found, "RES-2").map(|s| s.category.as_str()),
            Some("res")
        );
        assert!(section_of(&found, "RES-3").is_none());
    }

    #[test]
    fn the_count_line_says_how_many_are_shown() {
        assert_eq!(count_text(&response(0, vec![])), "No parts match.");
        assert_eq!(
            count_text(&response(1, vec![("res", 1, vec!["RES-1"])])),
            "1 matching part"
        );
        assert_eq!(
            count_text(&response(2500, vec![("res", 2500, vec!["RES-1", "RES-2"])])),
            "Showing 2 of 2,500 matching parts"
        );
    }

    #[test]
    fn section_ids_are_safe() {
        assert_eq!(section_id("res"), "parts-in-res");
        assert_eq!(section_id("a b/c"), "parts-in-a-b-c");
    }
}

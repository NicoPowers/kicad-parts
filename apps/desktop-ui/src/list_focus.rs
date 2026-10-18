//! Keeps keyboard focus in the part list when the list changes under it,
//! such as after a teammate saves.
//!
//! Sections and rows keep their elements across a new answer, so focus
//! usually just stays. But the browser can drop focus to the page:
//!
//! - when it removes the focused row, section or "Show all" button;
//! - when it moves the focused element. Leptos's keyed `For` moves every
//!   item it jumps over, so a section that sorts higher after a save can
//!   move the section holding focus, and a row that sorts elsewhere moves.
//!
//! After every change the list says what it now draws. If focus was
//! dropped, it goes back to the same item, or to the item that took its
//! place: the next row, the section header, the next section header, or
//! the search box. It never falls to the top of the page. A short note
//! tells screen reader users why focus moved.

use crate::SEARCH_BOX;
use crate::focus::{Scrolling, focus_now};
use crate::part_list::{RESULTS, section_id};
use leptos::prelude::*;
use leptos::web_sys::{Element, FocusEvent};
use std::collections::BTreeMap;
use wasm_bindgen::JsCast;

/// What has keyboard focus in the part list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focused {
    /// The row of part `ipn`, in the section of `category`.
    Row { category: String, ipn: String },
    /// The "Show all" button at the end of the section of `category`.
    ShowAll { category: String },
    /// The header button of the section of `category`.
    Section { category: String },
}

impl Focused {
    fn category(&self) -> &str {
        match self {
            Focused::Row { category, .. }
            | Focused::ShowAll { category }
            | Focused::Section { category } => category,
        }
    }
}

/// What one section draws, as far as focus is concerned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DrawnSection {
    /// The section's plain name, such as "Resistors".
    pub(crate) name: String,
    /// The IPNs of the rows drawn, in order; none while it is closed.
    pub(crate) rows: Vec<String>,
    /// Whether it ends with a "Show all" button.
    pub(crate) show_all: bool,
}

/// Where focus goes after the list changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Restore {
    To(Focused),
    /// No section is left, so focus goes to the search box above the list.
    SearchBox,
}

/// Where focus goes when the list changed while `focused` had it.
/// `sections` are the category codes shown before and now, in order.
/// `drawn` is what the focused item's section drew before and now.
///
/// Focus stays on `focused` while it is still drawn, even if it moved. A
/// row that left gives focus to the row that took its place (see
/// [`where_focus_goes`]), else to "Show all", else to its section header.
/// A "Show all" that left gives focus to the last row. A section that left
/// gives focus to the section that took its place, else to the search box.
pub(crate) fn restore_target(
    sections: (&[String], &[String]),
    drawn: (&DrawnSection, &DrawnSection),
    focused: &Focused,
) -> Restore {
    let (sections_before, sections_now) = sections;
    let (drawn_before, drawn_now) = drawn;
    let category = focused.category().to_owned();
    if !sections_now.iter().any(|now| *now == category) {
        return match where_focus_goes(sections_before, sections_now, &category) {
            Some(category) => Restore::To(Focused::Section {
                category: category.to_owned(),
            }),
            None => Restore::SearchBox,
        };
    }
    let header = || Focused::Section {
        category: category.clone(),
    };
    let target = match focused {
        Focused::Section { .. } => header(),
        Focused::Row { ipn, .. } => {
            match where_focus_goes(&drawn_before.rows, &drawn_now.rows, ipn) {
                Some(ipn) => Focused::Row {
                    category: category.clone(),
                    ipn: ipn.to_owned(),
                },
                None if drawn_now.show_all => Focused::ShowAll {
                    category: category.clone(),
                },
                None => header(),
            }
        }
        Focused::ShowAll { .. } if drawn_now.show_all => focused.clone(),
        Focused::ShowAll { .. } => match drawn_now.rows.last() {
            Some(ipn) => Focused::Row {
                category: category.clone(),
                ipn: ipn.clone(),
            },
            None => header(),
        },
    };
    Restore::To(target)
}

/// Where focus goes when a list changed from `before` to `now` while
/// `focused` had it. Focus stays on `focused` if it is still listed.
/// Otherwise it goes to the first item after it in `before` that is still
/// listed: the one that took its place. At the end of the list it goes to
/// the last item before it. When nothing from `before` is left, it goes to
/// the first item. `None` when `now` is empty.
pub(crate) fn where_focus_goes<'a>(
    before: &[String],
    now: &'a [String],
    focused: &str,
) -> Option<&'a str> {
    let listed = |item: &str| now.iter().find(|listed| listed.as_str() == item);
    if let Some(item) = listed(focused) {
        return Some(item);
    }
    let neighbour = before
        .iter()
        .position(|item| item == focused)
        .and_then(|at| {
            let after = before[at + 1..].iter().find_map(|item| listed(item));
            after.or_else(|| before[..at].iter().rev().find_map(|item| listed(item)))
        });
    neighbour.or_else(|| now.first()).map(String::as_str)
}

/// The note for screen readers when focus moved from `from` to `to`
/// because `from` left the list. `None` when focus stays on the same item,
/// or when "Show all" went because more parts are shown.
pub(crate) fn moved_note(from: &Focused, to: &Restore, section_name: &str) -> Option<String> {
    if *to == Restore::To(from.clone()) {
        return None;
    }
    match from {
        Focused::Row { ipn, .. } => Some(format!("{ipn} left the list.")),
        Focused::Section { .. } => Some(format!("{section_name} left the list.")),
        Focused::ShowAll { .. } => None,
    }
}

/// Remembers what had focus in the part list and what the list draws, and
/// puts focus back after a change, if the browser dropped it.
#[derive(Clone, Copy)]
pub(crate) struct ListFocus {
    focused: StoredValue<Option<Focused>>,
    /// The category codes of the sections shown, in order.
    sections: StoredValue<Vec<String>>,
    /// What each section drew last, by category code. A section that is
    /// gone keeps its entry, so its name is known when it leaves.
    drawn: StoredValue<BTreeMap<String, DrawnSection>>,
    /// The note for screen readers, read out politely.
    pub(crate) note: RwSignal<String>,
}

impl ListFocus {
    /// Starts with nothing focused. When the person presses the mouse
    /// outside the list, it forgets what had focus. So clicking in the
    /// details panel and then a teammate's save never pulls focus back.
    pub(crate) fn new() -> Self {
        let focus = ListFocus {
            focused: StoredValue::new(None),
            sections: StoredValue::new(Vec::new()),
            drawn: StoredValue::new(BTreeMap::new()),
            note: RwSignal::new(String::new()),
        };
        let listener = window_event_listener(leptos::ev::pointerdown, move |event| {
            let in_list = event
                .target()
                .and_then(|target| target.dyn_into::<Element>().ok())
                .and_then(|element| element.closest(RESULTS).ok().flatten())
                .is_some();
            if !in_list {
                focus.focused.set_value(None);
            }
        });
        on_cleanup(move || listener.remove());
        focus
    }

    /// A row, section header or "Show all" took focus.
    pub(crate) fn took(&self, focused: Focused) {
        self.focused.set_value(Some(focused));
        self.note.set(String::new());
    }

    /// Focus left an element in the list. When the person moves focus
    /// outside the list, it forgets what had focus. Focus that goes nowhere
    /// is kept, because that is how the browser drops it from an element
    /// it removes or moves.
    pub(crate) fn left(&self, event: &FocusEvent) {
        let Some(to) = event
            .related_target()
            .and_then(|target| target.dyn_into::<Element>().ok())
        else {
            return;
        };
        if to.closest(RESULTS).ok().flatten().is_none() {
            self.focused.set_value(None);
        }
    }

    /// The sections shown are now `now` (category codes, in order).
    pub(crate) fn sections_shown(&self, now: Vec<String>) {
        let before = self.sections.get_value();
        self.sections.set_value(now.clone());
        self.check(&before, &now, None);
    }

    /// The section of `category` now draws `now`.
    pub(crate) fn section_drawn(&self, category: &str, now: DrawnSection) {
        let before = self
            .drawn
            .try_update_value(|drawn| drawn.insert(category.to_owned(), now))
            .flatten()
            .unwrap_or_default();
        let sections = self.sections.get_value();
        self.check(&sections, &sections, Some((category, before)));
    }

    /// Works out where focus goes after a change, and puts it there once
    /// the list is drawn, if the browser dropped it. `changed` is the
    /// section whose rows changed, with what it drew before.
    fn check(
        &self,
        sections_before: &[String],
        sections_now: &[String],
        changed: Option<(&str, DrawnSection)>,
    ) {
        let Some(focused) = self.focused.get_value() else {
            return;
        };
        let category = focused.category();
        let drawn_now = self
            .drawn
            .with_value(|drawn| drawn.get(category).cloned())
            .unwrap_or_default();
        let drawn_before = match changed {
            Some((changed, before)) if changed == category => before,
            _ => drawn_now.clone(),
        };
        let target = restore_target(
            (sections_before, sections_now),
            (&drawn_before, &drawn_now),
            &focused,
        );
        let note = moved_note(&focused, &target, &drawn_now.name);
        self.focused.set_value(match &target {
            Restore::To(target) => Some(target.clone()),
            Restore::SearchBox => None,
        });
        let note_signal = self.note;
        request_animation_frame(move || {
            if !focus_is_lost() {
                return;
            }
            let Some(element) = element_for(&target) else {
                return;
            };
            focus_now(&element, Scrolling::Keep);
            // After focusing, because taking focus clears the note.
            if let Some(note) = note {
                note_signal.set(note);
            }
        });
    }
}

/// Whether nothing has focus: the browser drops focus to the page body
/// when it removes or moves the focused element.
fn focus_is_lost() -> bool {
    let document = document();
    match document.active_element() {
        None => true,
        Some(active) => document
            .body()
            .is_some_and(|body| active == *body.unchecked_ref::<Element>()),
    }
}

/// The element drawn for `target`, if it is drawn.
fn element_for(target: &Restore) -> Option<Element> {
    let selector = match target {
        Restore::SearchBox => return document().get_element_by_id(SEARCH_BOX),
        Restore::To(Focused::Section { category }) => format!(
            "{RESULTS} .section-toggle[aria-controls={}]",
            css_string(&section_id(category))
        ),
        Restore::To(Focused::ShowAll { category }) => {
            format!("#{} button.section-show-all", section_id(category))
        }
        Restore::To(Focused::Row { category, ipn }) => format!(
            "#{} button.result[data-ipn={}]",
            section_id(category),
            css_string(ipn)
        ),
    };
    document().query_selector(&selector).ok().flatten()
}

/// `text` as a quoted CSS string, so any IPN is safe in a selector.
fn css_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            // Control characters are written as hex escapes, ended by a space.
            c if c.is_control() => quoted.push_str(&format!("\\{:x} ", u32::from(c))),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn drawn(rows: &[&str], show_all: bool) -> DrawnSection {
        DrawnSection {
            name: "Resistors".to_owned(),
            rows: list(rows),
            show_all,
        }
    }

    fn row(category: &str, ipn: &str) -> Focused {
        Focused::Row {
            category: category.to_owned(),
            ipn: ipn.to_owned(),
        }
    }

    fn header(category: &str) -> Focused {
        Focused::Section {
            category: category.to_owned(),
        }
    }

    fn show_all(category: &str) -> Focused {
        Focused::ShowAll {
            category: category.to_owned(),
        }
    }

    #[test]
    fn a_row_whose_section_moved_up_keeps_focus() {
        // A save gave a resistor a better match, so Resistors moved above
        // Capacitors. The browser moved the focused row with its section.
        let rows = drawn(&["RES-1", "RES-2"], false);
        let target = restore_target(
            (&list(&["cap", "res"]), &list(&["res", "cap"])),
            (&rows, &rows),
            &row("res", "RES-1"),
        );
        assert_eq!(target, Restore::To(row("res", "RES-1")));
        // A header that moved keeps focus too.
        let target = restore_target(
            (&list(&["cap", "res"]), &list(&["res", "cap"])),
            (&rows, &rows),
            &header("res"),
        );
        assert_eq!(target, Restore::To(header("res")));
    }

    #[test]
    fn a_row_that_left_gives_focus_to_the_next_row() {
        let sections = list(&["res"]);
        let target = restore_target(
            (&sections, &sections),
            (
                &drawn(&["RES-1", "RES-2", "RES-3"], false),
                &drawn(&["RES-1", "RES-3"], false),
            ),
            &row("res", "RES-2"),
        );
        assert_eq!(target, Restore::To(row("res", "RES-3")));
    }

    #[test]
    fn the_last_row_leaving_gives_focus_to_show_all_or_else_the_header() {
        let sections = list(&["res"]);
        let target = restore_target(
            (&sections, &sections),
            (&drawn(&["RES-1"], true), &drawn(&[], true)),
            &row("res", "RES-1"),
        );
        assert_eq!(target, Restore::To(show_all("res")));
        let target = restore_target(
            (&sections, &sections),
            (&drawn(&["RES-1"], false), &drawn(&[], false)),
            &row("res", "RES-1"),
        );
        assert_eq!(target, Restore::To(header("res")));
    }

    #[test]
    fn show_all_that_left_gives_focus_to_the_last_row() {
        // A save removed a match beyond the first 25: the rows drawn stay
        // the same, but Show all is no longer needed.
        let sections = list(&["res"]);
        let target = restore_target(
            (&sections, &sections),
            (
                &drawn(&["RES-1", "RES-2"], true),
                &drawn(&["RES-1", "RES-2"], false),
            ),
            &show_all("res"),
        );
        assert_eq!(target, Restore::To(row("res", "RES-2")));
        let still = drawn(&["RES-1"], true);
        let target = restore_target((&sections, &sections), (&still, &still), &show_all("res"));
        assert_eq!(target, Restore::To(show_all("res")));
    }

    #[test]
    fn a_section_that_left_gives_focus_to_the_next_section_or_the_search_box() {
        let rows = drawn(&["CAP-1"], false);
        let target = restore_target(
            (&list(&["cap", "res"]), &list(&["res"])),
            (&rows, &rows),
            &row("cap", "CAP-1"),
        );
        assert_eq!(target, Restore::To(header("res")));
        let target = restore_target((&list(&["cap"]), &[]), (&rows, &rows), &row("cap", "CAP-1"));
        assert_eq!(target, Restore::SearchBox);
    }

    #[test]
    fn the_note_says_what_left_and_nothing_when_focus_stays() {
        assert_eq!(
            moved_note(
                &row("res", "RES-1"),
                &Restore::To(row("res", "RES-2")),
                "Resistors"
            ),
            Some("RES-1 left the list.".to_owned())
        );
        assert_eq!(
            moved_note(&header("res"), &Restore::To(header("cap")), "Resistors"),
            Some("Resistors left the list.".to_owned())
        );
        assert_eq!(
            moved_note(
                &row("res", "RES-1"),
                &Restore::To(row("res", "RES-1")),
                "Resistors"
            ),
            None
        );
        assert_eq!(
            moved_note(
                &show_all("res"),
                &Restore::To(row("res", "RES-1")),
                "Resistors"
            ),
            None
        );
    }

    #[test]
    fn any_ipn_is_quoted_safely_for_a_selector() {
        assert_eq!(css_string("RES-0000-1000"), "\"RES-0000-1000\"");
        assert_eq!(css_string(r#"A"B\C"#), r#""A\"B\\C""#);
        assert_eq!(css_string("A\nB"), r#""A\a B""#);
    }

    #[test]
    fn focus_stays_on_an_item_still_listed_even_if_it_moved() {
        let before = list(&["A", "B", "C"]);
        assert_eq!(
            where_focus_goes(&before, &list(&["A", "B", "C", "D"]), "B"),
            Some("B")
        );
        assert_eq!(
            where_focus_goes(&before, &list(&["B", "A", "C"]), "B"),
            Some("B")
        );
    }

    #[test]
    fn a_removed_item_gives_focus_to_the_one_that_took_its_place() {
        let before = list(&["A", "B", "C", "D"]);
        assert_eq!(
            where_focus_goes(&before, &list(&["A", "C", "D"]), "B"),
            Some("C")
        );
        // The next one left, even when the one right after it went too.
        assert_eq!(
            where_focus_goes(&before, &list(&["A", "D"]), "B"),
            Some("D")
        );
        // A new item where it was is not "after it" in the old list.
        assert_eq!(
            where_focus_goes(&before, &list(&["A", "N", "C", "D"]), "B"),
            Some("C")
        );
    }

    #[test]
    fn at_the_end_focus_goes_to_the_item_before() {
        let before = list(&["A", "B", "C"]);
        assert_eq!(
            where_focus_goes(&before, &list(&["A", "B"]), "C"),
            Some("B")
        );
        assert_eq!(where_focus_goes(&before, &list(&["A"]), "C"), Some("A"));
    }

    #[test]
    fn nothing_left_means_nowhere_to_go() {
        assert_eq!(where_focus_goes(&list(&["A"]), &[], "A"), None);
    }

    #[test]
    fn an_item_not_seen_before_gives_focus_to_the_first_item() {
        assert_eq!(
            where_focus_goes(&list(&["A"]), &list(&["B", "C"]), "X"),
            Some("B")
        );
    }

    #[test]
    fn when_only_new_items_are_left_focus_goes_to_the_first() {
        assert_eq!(
            where_focus_goes(&list(&["A", "B"]), &list(&["N", "M"]), "A"),
            Some("N")
        );
    }
}

//! Keeps keyboard focus in the part list when the list changes under it,
//! such as after a teammate saves.
//!
//! Sections and rows keep their elements across a new answer, so focus
//! usually just stays. The browser drops focus in two cases: when it
//! removes the focused row, because its part no longer matches, and when
//! it moves the row, because its part now sorts elsewhere. Then focus goes
//! back to the moved row, or to the row that took the removed row's place.
//! It never falls to the top of the page.

use leptos::prelude::*;
use leptos::web_sys::{Element, FocusEvent, HtmlElement};
use wasm_bindgen::JsCast;

/// What has keyboard focus in the part list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focused {
    /// The row of part `ipn`, in the section of `category`.
    Row { category: String, ipn: String },
    /// The header button of the section of `category`.
    Section { category: String },
}

impl Focused {
    fn category(&self) -> &str {
        match self {
            Focused::Row { category, .. } | Focused::Section { category } => category,
        }
    }
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

/// Remembers what had focus in the part list, and puts focus back after
/// the list changes, if the browser dropped it.
#[derive(Clone, Copy)]
pub(crate) struct ListFocus {
    focused: StoredValue<Option<Focused>>,
}

impl ListFocus {
    /// Starts with nothing focused. A press of the mouse outside the list
    /// forgets what had focus, so clicking in the details panel and then
    /// a teammate's save never pulls focus back into the list.
    pub(crate) fn new() -> Self {
        let focus = ListFocus {
            focused: StoredValue::new(None),
        };
        let listener = window_event_listener(leptos::ev::pointerdown, move |event| {
            let in_list = event
                .target()
                .and_then(|target| target.dyn_into::<Element>().ok())
                .and_then(|element| element.closest(".results").ok().flatten())
                .is_some();
            if !in_list {
                focus.focused.set_value(None);
            }
        });
        on_cleanup(move || listener.remove());
        focus
    }

    /// A row or section header took focus.
    pub(crate) fn took(&self, focused: Focused) {
        self.focused.set_value(Some(focused));
    }

    /// Focus left a row or section header. Focus that moves to an element
    /// outside the list is forgotten. Focus that goes nowhere is kept,
    /// because that is how the browser drops it from a row it removes or
    /// moves.
    pub(crate) fn left(&self, event: &FocusEvent) {
        let Some(to) = event
            .related_target()
            .and_then(|target| target.dyn_into::<Element>().ok())
        else {
            return;
        };
        if to.closest(".results").ok().flatten().is_none() {
            self.focused.set_value(None);
        }
    }

    /// The rows drawn in the section of `category` changed from `before`
    /// to `now` (part IPNs, in order).
    pub(crate) fn rows_changed(&self, category: &str, before: &[String], now: &[String]) {
        let Some(Focused::Row { category: had, ipn }) = self.focused.get_value() else {
            return;
        };
        if had != category {
            return;
        }
        let target = match where_focus_goes(before, now, &ipn) {
            Some(ipn) => Focused::Row {
                category: had,
                ipn: ipn.to_owned(),
            },
            // The section is empty now: its header is next to where the
            // rows were.
            None => Focused::Section { category: had },
        };
        self.restore_later(target);
    }

    /// The sections shown changed from `before` to `now` (category codes,
    /// in order). A section that is gone takes its rows with it, so focus
    /// goes to the section that took its place, or to the search box when
    /// no section is left.
    pub(crate) fn sections_changed(&self, before: &[String], now: &[String]) {
        let Some(focused) = self.focused.get_value() else {
            return;
        };
        if now.iter().any(|category| category == focused.category()) {
            return;
        }
        match where_focus_goes(before, now, focused.category()) {
            Some(category) => self.restore_later(Focused::Section {
                category: category.to_owned(),
            }),
            None => {
                self.focused.set_value(None);
                request_animation_frame(|| {
                    if focus_is_lost() {
                        focus_quietly(element_by_selector("#part-search"));
                    }
                });
            }
        }
    }

    /// Once the list is drawn, focuses `target`, but only if focus was
    /// dropped; focus the person put somewhere else stays there.
    fn restore_later(&self, target: Focused) {
        self.focused.set_value(Some(target.clone()));
        request_animation_frame(move || {
            if focus_is_lost() {
                focus_quietly(element_for(&target));
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

/// The element drawn for `focused`, if it is drawn.
fn element_for(focused: &Focused) -> Option<Element> {
    match focused {
        Focused::Section { category } => element_by_selector(&format!(
            ".results .section-toggle[aria-controls=\"{}\"]",
            crate::part_list::section_id(category)
        )),
        Focused::Row { category, ipn } => {
            let list = document().get_element_by_id(&crate::part_list::section_id(category))?;
            // Each <li> holds one row button. The IPN is compared as data,
            // not put in a selector, so no IPN can break the selector.
            let mut item = list.first_element_child();
            while let Some(li) = item {
                if let Some(row) = li.first_element_child() {
                    if row.get_attribute("data-ipn").as_deref() == Some(ipn.as_str()) {
                        return Some(row);
                    }
                }
                item = li.next_element_sibling();
            }
            None
        }
    }
}

fn element_by_selector(selector: &str) -> Option<Element> {
    document().query_selector(selector).ok().flatten()
}

/// Focuses `element` without scrolling, so the list stays where the person
/// left it.
fn focus_quietly(element: Option<Element>) {
    let Some(element) = element.and_then(|element| element.dyn_into::<HtmlElement>().ok()) else {
        return;
    };
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"preventScroll".into(), &true.into());
    if let Ok(focus) = js_sys::Reflect::get(&element, &"focus".into()) {
        if let Some(focus) = focus.dyn_ref::<js_sys::Function>() {
            let _ = focus.call1(&element, &options);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
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

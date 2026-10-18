//! Moves keyboard focus once the view is drawn.
//!
//! Leptos draws a change after the code that asks for it has run. So focus
//! moves in the next animation frame, when the element exists.

use leptos::prelude::*;
use leptos::web_sys::{Element, HtmlElement};
use wasm_bindgen::JsCast;

/// Whether moving focus to an element may scroll it into view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scrolling {
    /// The browser scrolls the element into view, as it usually does.
    IntoView,
    /// Nothing scrolls, so a list stays where the person left it.
    Keep,
}

/// Moves focus to the element with `id` once the view is drawn.
pub(crate) fn focus_later(id: &str) {
    let id = id.to_owned();
    focus_found_later(
        move || document().get_element_by_id(&id),
        Scrolling::IntoView,
    );
}

/// Moves focus to the element `find` returns once the view is drawn. When
/// `find` returns `None`, focus stays where it is.
pub(crate) fn focus_found_later(
    find: impl FnOnce() -> Option<Element> + 'static,
    scrolling: Scrolling,
) {
    request_animation_frame(move || {
        if let Some(element) = find() {
            focus_now(&element, scrolling);
        }
    });
}

/// Moves focus to `element` now.
pub(crate) fn focus_now(element: &Element, scrolling: Scrolling) {
    let Some(element) = element.dyn_ref::<HtmlElement>() else {
        return;
    };
    match scrolling {
        Scrolling::IntoView => {
            let _ = element.focus();
        }
        Scrolling::Keep => {
            // focus({ preventScroll: true }), called by name because the
            // web-sys feature for its options is not turned on.
            let options = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&options, &"preventScroll".into(), &true.into());
            if let Ok(focus) = js_sys::Reflect::get(element, &"focus".into()) {
                if let Some(focus) = focus.dyn_ref::<js_sys::Function>() {
                    let _ = focus.call1(element, &options);
                }
            }
        }
    }
}

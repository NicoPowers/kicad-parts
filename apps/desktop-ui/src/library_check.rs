//! The library check (kicad-parts-da65) for people who can edit: a short
//! line under the search box when it finds something, which opens the
//! Library check view in the part panel, grouped as To fix, Not in the
//! Catalog yet, Worth a look and Clean-up.

use crate::cad_reference;
use crate::team::focus_later;
use desktop_contract::{LibraryCheckGroup, LibraryCheckItem, LibraryCheckView};
use leptos::prelude::*;

/// The line's words, such as `Library check: 1 to fix, 2 new items`, or
/// `None` when the check found nothing. To fix and new items come first;
/// the other groups are counted only when neither has any.
pub(crate) fn line_text(view: &LibraryCheckView) -> Option<String> {
    let counted = |count: usize, one: &str, many: &str| {
        (count > 0).then(|| format!("{count} {}", if count == 1 { one } else { many }))
    };
    let mut parts: Vec<String> = [
        counted(view.to_fix, "to fix", "to fix"),
        counted(view.not_in_catalog, "new item", "new items"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        parts.extend(
            [
                counted(view.worth_a_look, "worth a look", "worth a look"),
                counted(view.clean_up, "to clean up", "to clean up"),
            ]
            .into_iter()
            .flatten(),
        );
    }
    (!parts.is_empty()).then(|| format!("Library check: {}", parts.join(", ")))
}

/// The line under the search box; nothing when the check found nothing or
/// is not shown (signed out, or someone who can only view).
#[component]
pub(crate) fn LibraryCheckLine(
    check: RwSignal<Option<LibraryCheckView>>,
    open: Callback<()>,
) -> impl IntoView {
    move || {
        let view = check.get()?;
        let text = line_text(&view)?;
        let urgent = view.to_fix > 0;
        Some(view! {
            <p class="library-check-line">
                <button
                    type="button"
                    id="library-check-open"
                    class="link-button"
                    class:urgent=urgent
                    on:click=move |_| open.run(())
                >
                    {text}
                </button>
            </p>
        })
    }
}

/// The Library check view, in the part panel.
#[component]
pub(crate) fn LibraryCheckPanel(
    check: RwSignal<Option<LibraryCheckView>>,
    close: Callback<()>,
) -> impl IntoView {
    // Keyboard and screen reader users land on the view's heading.
    focus_later("library-check-title");
    view! {
        <div class="part-view library-check">
            <header class="part-header library-check-header">
                <div>
                    <h2 id="library-check-title" tabindex="-1">"Library check"</h2>
                    <p class="part-description">
                        "The symbols, footprints and 3D models of our CSV catalog folder, compared with the Catalog."
                    </p>
                </div>
                <button type="button" id="library-check-close" on:click=move |_| close.run(())>
                    "Close"
                </button>
            </header>
            <div class="library-check-body" role="region" aria-label="What the library check found" tabindex="0">
                {move || match check.get() {
                    Some(view) if !view.is_empty() => view
                        .groups
                        .into_iter()
                        .enumerate()
                        .map(|(index, group)| view! { <GroupView group=group index=index /> })
                        .collect_view()
                        .into_any(),
                    Some(_) => view! { <p class="hint">"The library check found nothing."</p> }.into_any(),
                    None => view! { <p class="hint">"Sign in as someone who can edit to see the library check."</p> }.into_any(),
                }}
            </div>
        </div>
    }
}

/// One group. Clean-up starts folded: those old problems stop nothing.
#[component]
fn GroupView(group: LibraryCheckGroup, index: usize) -> impl IntoView {
    let heading_id = format!("library-check-group-{index}");
    let to_fix = group.title == "To fix";
    let title = format!("{} ({})", group.title, group.items.len());
    let items = view! {
        <p class="hint">{group.hint.clone()}</p>
        <ul class="library-check-items">
            {group.items.into_iter().map(|item| view! { <ItemView item=item /> }).collect_view()}
        </ul>
    };
    if group.title == "Clean-up" {
        view! {
            <details class="library-check-group">
                <summary><h3 id=heading_id>{title}</h3></summary>
                {items}
            </details>
        }
        .into_any()
    } else {
        let labelled_by = heading_id.clone();
        view! {
            <section class="library-check-group" class:to-fix=to_fix aria-labelledby=labelled_by>
                <h3 id=heading_id>{title}</h3>
                {items}
            </section>
        }
        .into_any()
    }
}

#[component]
fn ItemView(item: LibraryCheckItem) -> impl IntoView {
    // A 3D model is named by its folder and file; a symbol or footprint by
    // its library reference.
    let name = if item.library.ends_with(".3dshapes") {
        format!("{}/{}", item.library, item.name).into_any()
    } else {
        cad_reference(&format!("{}:{}", item.library, item.name))
    };
    view! {
        <li class="library-check-item">
            <p class="library-check-name"><code>{name}</code></p>
            <p class="library-check-file">{item.file}</p>
            <p>{item.message}</p>
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(to_fix: usize, new: usize, look: usize, clean_up: usize) -> LibraryCheckView {
        LibraryCheckView {
            to_fix,
            not_in_catalog: new,
            worth_a_look: look,
            clean_up,
            groups: Vec::new(),
        }
    }

    #[test]
    fn the_line_says_what_to_fix_and_what_is_new_first() {
        assert_eq!(
            line_text(&view(1, 2, 3, 4)).as_deref(),
            Some("Library check: 1 to fix, 2 new items")
        );
        assert_eq!(
            line_text(&view(0, 1, 3, 0)).as_deref(),
            Some("Library check: 1 new item")
        );
        assert_eq!(
            line_text(&view(0, 0, 3, 14)).as_deref(),
            Some("Library check: 3 worth a look, 14 to clean up")
        );
        assert_eq!(line_text(&view(0, 0, 0, 0)), None);
    }
}

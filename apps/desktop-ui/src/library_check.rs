//! The library check (kicad-parts-da65) for people who can edit: a short
//! line under the search box when it finds something, which opens the
//! Library check view in the part panel, grouped as To fix, Not in the
//! Catalog yet, Worth a look and Clean-up. A symbol or footprint not in
//! the Catalog yet can start Add part (kicad-parts-37a7).
//!
//! The check compares our folder with one team's Catalog: the team chosen
//! in the part list, else the one the person chose here last, else the
//! first they can edit (kicad-parts-8a71). With All teams and two teams or
//! more, the view has its own Team list box, and the line names the team.

use crate::cad_reference;
use crate::focus::focus_later;
use crate::team_marks::TeamSelect;
use desktop_contract::{LibraryCheckGroup, LibraryCheckItem, LibraryCheckView, TeamLibraryChoice};
use leptos::prelude::*;

/// The line's words, such as `Library check: 1 to fix, 2 new items`, or
/// `None` when the check found nothing. To fix and new items come first;
/// the other groups are counted only when neither has any. With `team`,
/// the line names the team checked: `MA library check: 1 to fix`.
pub(crate) fn line_text(view: &LibraryCheckView, team: Option<&str>) -> Option<String> {
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
    let name = match team {
        Some(team) => format!("{team} library check"),
        None => "Library check".to_owned(),
    };
    (!parts.is_empty()).then(|| format!("{name}: {}", parts.join(", ")))
}

/// The line under the search box; nothing when the check found nothing or
/// is not shown (signed out, or someone who can only view).
#[component]
pub(crate) fn LibraryCheckLine(
    check: RwSignal<Option<LibraryCheckView>>,
    /// Why the check could not run, if it could not.
    problem: RwSignal<Option<String>>,
    /// The code of the team checked, while the person can edit in two
    /// teams or more.
    team: Signal<Option<String>>,
    open: Callback<()>,
) -> impl IntoView {
    move || {
        if let Some(message) = problem.get() {
            return Some(
                view! {
                    <p class="library-check-line library-check-problem" role="status">
                        {format!("Library check could not run: {message}")}
                    </p>
                }
                .into_any(),
            );
        }
        let view = check.get()?;
        let text = line_text(&view, team.get().as_deref())?;
        let urgent = view.to_fix > 0;
        Some(
            view! {
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
            }
            .into_any(),
        )
    }
}

/// The Library check view, in the part panel.
#[component]
pub(crate) fn LibraryCheckPanel(
    check: RwSignal<Option<LibraryCheckView>>,
    /// The teams to choose from, with All teams in the part list; empty
    /// when it shows one team.
    teams: Memo<Vec<(String, String)>>,
    /// The team checked, by server address.
    team: Signal<String>,
    choose_team: Callback<String>,
    close: Callback<()>,
    /// Opens Add part with a symbol or footprint of the list chosen.
    add_from: Callback<TeamLibraryChoice>,
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
                    <TeamSelect id="library-check-team" options=teams chosen=team changed=choose_team />
                </div>
                <button type="button" id="library-check-close" on:click=move |_| close.run(())>
                    "Close"
                </button>
            </header>
            <div class="library-check-body" role="region" aria-label="What the library check found" tabindex="0">
                {move || match check.get() {
                    Some(view) if !view.is_empty() => {
                        let unchecked = (!view.kicad_footprints_checked).then(|| view! {
                            <p class="hint">
                                "KiCad's own libraries were not found, so symbols naming one of KiCad's footprints were not checked."
                            </p>
                        });
                        let groups = view
                            .groups
                            .into_iter()
                            .enumerate()
                            .map(|(index, group)| view! { <GroupView group=group index=index add_from=add_from /> })
                            .collect_view();
                        view! { {unchecked}{groups} }.into_any()
                    }
                    Some(_) => view! { <p class="hint">"The library check found nothing."</p> }.into_any(),
                    None => view! { <p class="hint">"Sign in as someone who can edit to see the library check."</p> }.into_any(),
                }}
            </div>
        </div>
    }
}

/// One group. Clean-up starts folded: those old problems stop nothing.
#[component]
fn GroupView(
    group: LibraryCheckGroup,
    index: usize,
    add_from: Callback<TeamLibraryChoice>,
) -> impl IntoView {
    let heading_id = format!("library-check-group-{index}");
    let to_fix = group.title == "To fix";
    // Only what is not in the Catalog yet can start a new part.
    let new_items = group.title == "Not in the Catalog yet";
    let title = format!("{} ({})", group.title, group.items.len());
    let items = view! {
        <p class="hint">{group.hint.clone()}</p>
        <ul class="library-check-items">
            {group.items.into_iter().map(|item| {
                let add_from = new_items.then_some(add_from);
                view! { <ItemView item=item add_from=add_from /> }
            }).collect_view()}
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

/// The choice Add part starts from for `item`: its symbol or footprint,
/// or `None` for anything else, such as a 3D model.
pub(crate) fn choice_of(item: &LibraryCheckItem) -> Option<TeamLibraryChoice> {
    if item.library.is_empty() {
        return None;
    }
    let reference = Some(format!("{}:{}", item.library, item.name));
    if item.file.starts_with("symbols/") {
        Some(TeamLibraryChoice {
            symbol: reference,
            footprint: None,
        })
    } else if item.file.starts_with("footprints/") {
        Some(TeamLibraryChoice {
            symbol: None,
            footprint: reference,
        })
    } else {
        None
    }
}

#[component]
fn ItemView(
    item: LibraryCheckItem,
    add_from: Option<Callback<TeamLibraryChoice>>,
) -> impl IntoView {
    // A symbol or footprint is named by its library reference; a 3D model
    // or a file that could not be read by its path.
    let name = if item.library.is_empty() {
        item.name.clone().into_any()
    } else {
        cad_reference(&format!("{}:{}", item.library, item.name))
    };
    let add = add_from.zip(choice_of(&item)).map(|(add_from, choice)| {
        // Which item, for screen readers; the visible text stays short.
        let reference = format!("{}:{}", item.library, item.name);
        view! {
            <button
                type="button"
                class="library-check-add"
                data-reference=reference.clone()
                on:click=move |_| add_from.run(choice.clone())
            >
                "Add part from it"
                <span class="visually-hidden">{format!(" ({reference})")}</span>
            </button>
        }
    });
    view! {
        <li class="library-check-item">
            <div class="library-check-item-head">
                <p class="library-check-name"><code>{name}</code></p>
                {add}
            </div>
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
            kicad_footprints_checked: true,
            groups: Vec::new(),
        }
    }

    #[test]
    fn only_symbols_and_footprints_start_a_part() {
        let item = |library: &str, name: &str, file: &str| LibraryCheckItem {
            library: library.into(),
            name: name.into(),
            file: file.into(),
            message: String::new(),
        };
        assert_eq!(
            choice_of(&item("g-ics", "IC_TP4333", "symbols/g-ics.kicad_sym")),
            Some(TeamLibraryChoice {
                symbol: Some("g-ics:IC_TP4333".into()),
                footprint: None,
            })
        );
        assert_eq!(
            choice_of(&item(
                "g-Crystal",
                "X",
                "footprints/g-Crystal.pretty/X.kicad_mod"
            )),
            Some(TeamLibraryChoice {
                symbol: None,
                footprint: Some("g-Crystal:X".into()),
            })
        );
        assert_eq!(
            choice_of(&item(
                "",
                "3d-models/g-Crystal.3dshapes/X.step",
                "3d-models/g-Crystal.3dshapes/X.step"
            )),
            None
        );
    }

    #[test]
    fn the_line_says_what_to_fix_and_what_is_new_first() {
        assert_eq!(
            line_text(&view(1, 2, 3, 4), None).as_deref(),
            Some("Library check: 1 to fix, 2 new items")
        );
        assert_eq!(
            line_text(&view(0, 1, 3, 0), None).as_deref(),
            Some("Library check: 1 new item")
        );
        assert_eq!(
            line_text(&view(0, 0, 3, 14), None).as_deref(),
            Some("Library check: 3 worth a look, 14 to clean up")
        );
        // With two teams to check, the line names the team checked.
        assert_eq!(
            line_text(&view(1, 0, 0, 0), Some("MA")).as_deref(),
            Some("MA library check: 1 to fix")
        );
        assert_eq!(line_text(&view(0, 0, 0, 0), None), None);
    }
}

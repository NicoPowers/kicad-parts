//! What Edit part and Add part share for choosing a symbol or footprint
//! (kicad-parts-bc6f): no CAD name is typed. A field shows the chosen
//! reference, read only, with Choose... which opens the Libraries browser
//! for that role on the current item. A file that goes into our libraries
//! (a download, or an installed library's item) gets one of our libraries
//! from a list, never typed.

use crate::backend;
use crate::libraries::BrowseFor;
use desktop_contract::{
    LibraryItemRequest, OurLibraries, PickUse, PreviewKind, PreviewOutcome, TeamPick,
    TeamPickRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Opens the Libraries browser; provided by the app.
#[derive(Clone, Copy)]
pub(crate) struct LibrariesOpener(pub RwSignal<Option<BrowseFor>>);

/// Our libraries' nicknames, read once; provided by the app.
#[derive(Clone, Copy)]
pub(crate) struct OurLibrariesList(pub RwSignal<Option<OurLibraries>>);

/// Our libraries for `kind`, once read.
pub(crate) fn our_libraries(kind: PreviewKind) -> Signal<Vec<String>> {
    let list = use_context::<OurLibrariesList>().map(|list| list.0);
    Signal::derive(move || {
        list.and_then(|list| {
            list.with(|found| {
                found.as_ref().map(|found| match kind {
                    PreviewKind::Symbol => found.symbols.clone(),
                    PreviewKind::Footprint => found.footprints.clone(),
                })
            })
        })
        .unwrap_or_default()
    })
}

/// `symbol` or `footprint`.
pub(crate) fn noun(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::Symbol => "symbol",
        PreviewKind::Footprint => "footprint",
    }
}

/// A drawing being made or made, as [`crate::team::FilePreview`] shows it.
pub(crate) type Drawing = RwSignal<Option<Result<PreviewOutcome, String>>>;

/// Draws the library item `pick` into `drawing`, as the Catalog of the
/// team at `team` has it when `catalog`; only the newest drawing asked for
/// through `draws` is shown.
pub(crate) fn draw_item(
    team: String,
    pick: &TeamPickRequest,
    catalog: bool,
    drawing: Drawing,
    busy: RwSignal<bool>,
    draws: StoredValue<u64>,
) {
    draws.update_value(|number| *number += 1);
    let asked = draws.get_value();
    busy.set(true);
    let request = LibraryItemRequest {
        kind: pick.kind,
        library: pick.library.clone(),
        name: pick.name.clone(),
        selection: None,
    };
    spawn_local(async move {
        let result = if catalog {
            backend::team_item_preview(&team, request).await
        } else {
            backend::library_preview(request).await
        };
        if draws.try_get_value() == Some(asked) {
            busy.set(false);
            drawing.set(Some(result));
        }
    });
}

/// The Choose... button of a field: opens the Libraries browser for `kind`
/// on `current`, with KiCad's two filters on for `for_symbol`, and gives the
/// chosen item to `chose`. Focus comes back to the button.
#[component]
pub(crate) fn ChooseButton(
    #[prop(into)] id: Signal<String>,
    kind: PreviewKind,
    /// What the button chooses, for screen readers, such as `the symbol`.
    what: String,
    #[prop(into)] current: Signal<Option<String>>,
    #[prop(into)] for_symbol: Signal<Option<String>>,
    chose: Callback<TeamPickRequest>,
    /// The visible label; `Choose…` by default.
    #[prop(optional)]
    label: Option<&'static str>,
) -> impl IntoView {
    let opener = use_context::<LibrariesOpener>();
    view! {
        <button
            type="button"
            id=id
            class="cad-choose"
            on:click=move |_| {
                if let Some(opener) = opener {
                    opener.0.set(Some(BrowseFor {
                        role: Some(kind),
                        current: current.get_untracked().filter(|reference| reference.contains(':')),
                        for_symbol: if kind == PreviewKind::Footprint {
                            for_symbol.get_untracked().filter(|reference| reference.contains(':'))
                        } else {
                            None
                        },
                        choose: Some(chose),
                        return_focus: Some(id.get_untracked()),
                    }));
                }
            }
        >
            {label.unwrap_or("Choose…")}
            <span class="visually-hidden">{format!(" {what}")}</span>
        </button>
    }
}

/// The library a file goes into: one of our libraries for its role.
#[component]
pub(crate) fn LibrarySelect(
    id: String,
    kind: PreviewKind,
    #[prop(into)] chosen: Signal<String>,
    changed: Callback<String>,
) -> impl IntoView {
    let libraries = our_libraries(kind);
    let label_for = id.clone();
    view! {
        <div class="file-symbol cad-library">
            <label for=label_for>{match kind {
                PreviewKind::Symbol => "Symbol library",
                PreviewKind::Footprint => "Footprint library",
            }}</label>
            <select id=id on:change:target=move |event| changed.run(event.target().value())>
                {move || {
                    let now = chosen.get();
                    let list = libraries.get();
                    let known = list.contains(&now);
                    let mut options = vec![view! {
                        <option value="" selected=!known>"Choose a library"</option>
                    }
                    .into_any()];
                    options.extend(list.into_iter().map(|library| {
                        let selected = library == now;
                        view! { <option value=library.clone() selected=selected>{library.clone()}</option> }.into_any()
                    }));
                    options.collect_view()
                }}
            </select>
        </div>
    }
}

/// What choosing `pick` means, in a short line, such as "From KiCad's own
/// libraries."
pub(crate) fn use_words(pick: &TeamPick) -> String {
    match pick.use_as {
        PickUse::Catalog => "In the Catalog: the part uses it as the Catalog has it.".to_owned(),
        PickUse::Ours => {
            "From our libraries: added to the Catalog when you save, as committed.".to_owned()
        }
        PickUse::Kicad => {
            "KiCad's own: the part refers to it in KiCad's library, which every engineer has."
                .to_owned()
        }
        PickUse::Copy => format!(
            "From the installed library {}, which teammates may not have: copied into the library you choose.",
            pick.library.nickname
        ),
    }
}

/// The problems of `pick` that stop the save, each a plain line; with
/// `no_models`, its unreadable 3D models do not.
pub(crate) fn pick_problems(pick: &TeamPick, no_models: bool) -> Vec<String> {
    let mut problems = pick.problems.clone();
    if !no_models {
        problems.extend(pick.model_problems.iter().cloned());
    }
    problems
}

/// The 3D model choice of a footprint copied from an installed library:
/// copy its models, or No 3D model.
#[component]
pub(crate) fn CopyModels(
    id: String,
    pick: TeamPick,
    #[prop(into)] no_models: Signal<bool>,
    changed: Callback<bool>,
) -> impl IntoView {
    let outside = pick.models.len() + pick.model_problems.len();
    (outside > 0).then(|| {
        let copy = if pick.models.len() == 1 && pick.model_problems.is_empty() {
            format!("Copy its 3D model ({})", pick.models[0].file_name)
        } else {
            "Copy its 3D models".to_owned()
        };
        let label_for = id.clone();
        let list_id = format!("{id}-list");
        let described = list_id.clone();
        // Every file that would be copied, named by its link as written, so
        // the person sees exactly what goes to the team server.
        let files: Vec<(String, String)> = pick
            .models
            .iter()
            .map(|model| {
                let link = model
                    .installed
                    .as_ref()
                    .and_then(|installed| installed.link.clone())
                    .unwrap_or_default();
                (model.file_name.clone(), link)
            })
            .collect();
        view! {
            <div class="file-symbol cad-models">
                <label for=label_for>"3D model"</label>
                <select
                    id=id
                    aria-describedby=described
                    on:change:target=move |event| changed.run(event.target().value() == "none")
                >
                    <option value="copy" selected=move || !no_models.get()>{copy}</option>
                    <option value="none" selected=move || no_models.get()>"No 3D model"</option>
                </select>
            </div>
            <ul class="cad-model-list" id=list_id>
                {move || if no_models.get() {
                    view! { <li>"No 3D model is copied."</li> }.into_any()
                } else if files.is_empty() {
                    view! { <li>"No 3D model can be copied."</li> }.into_any()
                } else {
                    files
                        .iter()
                        .map(|(name, link)| view! {
                            <li>{format!("Copies {name}, from {link}")}</li>
                        })
                        .collect_view()
                        .into_any()
                }}
            </ul>
        }
    })
}

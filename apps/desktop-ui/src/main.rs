//! KiCad Parts desktop UI: search the catalog, select a part, and see the
//! exact KiCad symbol (every unit) and footprint exported by kicad-cli.

mod backend;

use desktop_contract::{
    AppStatus, KicadStatus, PartDetail, PreviewKind, PreviewOutcome, PreviewProblem,
    PreviewProblemReason, PreviewSet, SearchResponse, unit_label,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;

/// Results shown per search. The catalog search itself covers every row.
const RESULT_LIMIT: usize = 200;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Clone, Debug)]
enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let status = RwSignal::new(Load::<AppStatus>::Loading);
    // Bumped whenever KiCad discovery changes, so previews reload.
    let kicad_epoch = RwSignal::new(0u32);
    let query = RwSignal::new(String::new());
    let results = RwSignal::new(Load::<SearchResponse>::Loading);
    let selected = RwSignal::new(None::<String>);
    let search_seq = StoredValue::new(0u64);
    // The kicad-cli path being typed; kept here so it survives the panel
    // re-rendering when a path is rejected.
    let override_input = RwSignal::new(String::new());

    spawn_local(async move {
        let loaded = into_load(backend::app_status().await);
        if let Load::Ready(app) = &loaded {
            override_input.set(app.kicad_override.clone().unwrap_or_default());
        }
        status.set(loaded);
    });

    Effect::new(move |_| {
        let text = query.get();
        let seq = search_seq.get_value() + 1;
        search_seq.set_value(seq);
        set_timeout(
            move || {
                if search_seq.get_value() != seq {
                    return;
                }
                spawn_local(async move {
                    let response = backend::search(text, RESULT_LIMIT).await;
                    // Ignore answers to queries the user has already replaced.
                    if search_seq.get_value() == seq {
                        results.set(into_load(response));
                    }
                });
            },
            SEARCH_DEBOUNCE,
        );
    });

    view! {
        <div class="app">
            <header class="topbar">
                <h1 class="brand">"KiCad Parts"</h1>
                <CatalogBadge status=status />
                <KicadBadge status=status kicad_epoch=kicad_epoch override_input=override_input />
            </header>
            <KicadProblem status=status kicad_epoch=kicad_epoch override_input=override_input />
            <SettingsProblem status=status />
            <main class="workspace">
                <section class="search-pane" aria-label="Part search">
                    <label class="search-label" for="part-search">
                        "Search parts"
                    </label>
                    <input
                        id="part-search"
                        class="search-input"
                        type="search"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="IPN, MPN, manufacturer, value or footprint"
                        aria-describedby="result-count"
                        prop:value=move || query.get()
                        on:input:target=move |event| query.set(event.target().value())
                    />
                    <ResultList results=results selected=selected />
                </section>
                <PartPanel selected=selected kicad_epoch=kicad_epoch />
            </main>
        </div>
    }
}

/// `20000` as `20,000`.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut text = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// `1 part`, `2 parts`, `20,000 parts`.
fn counted(n: usize, singular: &str, plural: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { singular } else { plural })
}

fn into_load<T>(result: Result<T, String>) -> Load<T> {
    match result {
        Ok(value) => Load::Ready(value),
        Err(error) => Load::Failed(error),
    }
}

#[component]
fn CatalogBadge(status: RwSignal<Load<AppStatus>>) -> impl IntoView {
    move || match status.get() {
        Load::Ready(status) => match status.catalog.error {
            None => view! {
                <p class="catalog-summary">
                    {format!(
                        "{} in {}",
                        counted(status.catalog.parts, "part", "parts"),
                        counted(status.catalog.categories, "category", "categories"),
                    )}
                </p>
            }
            .into_any(),
            Some(error) => view! {
                <p class="catalog-summary catalog-error" role="alert">
                    {format!("Catalog not loaded: {error}")}
                </p>
            }
            .into_any(),
        },
        _ => ().into_any(),
    }
}

/// Header status: the KiCad version in use, with a form to change it.
#[component]
fn KicadBadge(
    status: RwSignal<Load<AppStatus>>,
    kicad_epoch: RwSignal<u32>,
    override_input: RwSignal<String>,
) -> impl IntoView {
    move || match status.get() {
        Load::Loading => view! { <p class="kicad-badge">"Looking for KiCad…"</p> }.into_any(),
        Load::Failed(error) => view! {
            <p class="kicad-badge kicad-badge-problem" role="alert">{error}</p>
        }
        .into_any(),
        Load::Ready(app) => match &app.kicad {
            KicadStatus::Ready {
                version,
                cli_path,
                from_override,
            } => {
                let source = if *from_override {
                    "your chosen path"
                } else {
                    "auto-detected"
                };
                view! {
                    <details class="kicad-settings">
                        <summary class="kicad-badge kicad-badge-ready">
                            {format!("KiCad {version}")}
                        </summary>
                        <div class="kicad-settings-body">
                            <p>
                                "Previews are exported by "
                                <code>{cli_path.clone()}</code>
                                {format!(" ({source}).")}
                            </p>
                            <OverrideForm
                                status=status
                                kicad_epoch=kicad_epoch
                                input=override_input
                                saved=app.kicad_override.is_some()
                                rejected=app.override_rejected.clone()
                            />
                        </div>
                    </details>
                }
                .into_any()
            }
            _ => view! {
                <p class="kicad-badge kicad-badge-problem">"KiCad needs attention"</p>
            }
            .into_any(),
        },
    }
}

/// A prominent, actionable panel whenever KiCad is not usable.
#[component]
fn KicadProblem(
    status: RwSignal<Load<AppStatus>>,
    kicad_epoch: RwSignal<u32>,
    override_input: RwSignal<String>,
) -> impl IntoView {
    move || {
        let Load::Ready(app) = status.get() else {
            return ().into_any();
        };
        let (title, explanation, details): (String, String, Vec<String>) = match &app.kicad {
            KicadStatus::Ready { .. } => return ().into_any(),
            KicadStatus::Missing { searched } => (
                "KiCad 10 was not found".into(),
                "Symbol and footprint previews are drawn by KiCad's own kicad-cli. Install \
                 KiCad 10 from kicad.org/download and choose Search again, or enter where \
                 kicad-cli is installed."
                    .into(),
                searched
                    .iter()
                    .map(|path| format!("Searched {path}"))
                    .chain(std::iter::once("Searched the folders on PATH".to_owned()))
                    .collect(),
            ),
            KicadStatus::Unsupported {
                version,
                cli_path,
                supported_major,
            } => (
                format!("KiCad {version} is not supported"),
                format!(
                    "This app needs KiCad {supported_major}. Install KiCad {supported_major} \
                     from kicad.org/download, or enter the path of a KiCad \
                     {supported_major} kicad-cli."
                ),
                vec![format!("Found {cli_path}")],
            ),
            // The saved path itself is already in the input below.
            KicadStatus::OverrideInvalid { problem, .. } => (
                "The kicad-cli location you entered does not work".into(),
                "Correct the path below, or clear it to search the standard install \
                 locations again."
                    .into(),
                vec![problem.clone()],
            ),
            KicadStatus::Broken { cli_path, message } => (
                "kicad-cli did not respond".into(),
                "Reinstall or repair KiCad 10, or enter the path of a working kicad-cli.".into(),
                vec![format!("{cli_path}: {message}")],
            ),
        };
        view! {
            <section class="kicad-problem" role="alert" aria-labelledby="kicad-problem-title">
                <h2 id="kicad-problem-title">{title}</h2>
                <p>{explanation}</p>
                <ul class="kicad-problem-details">
                    {details.into_iter().map(|line| view! { <li>{line}</li> }).collect_view()}
                </ul>
                <OverrideForm
                    status=status
                    kicad_epoch=kicad_epoch
                    input=override_input
                    saved=app.kicad_override.is_some()
                    rejected=app.override_rejected.clone()
                />
            </section>
        }
        .into_any()
    }
}

#[component]
fn OverrideForm(
    status: RwSignal<Load<AppStatus>>,
    kicad_epoch: RwSignal<u32>,
    /// The path being typed.
    input: RwSignal<String>,
    /// Whether a path is saved (offers "Clear saved path").
    saved: bool,
    /// Why the last entered path was not saved.
    rejected: Option<String>,
) -> impl IntoView {
    let path = input;
    let busy = RwSignal::new(false);
    let apply = move |result: Result<AppStatus, String>| {
        busy.set(false);
        status.set(into_load(result));
        kicad_epoch.update(|epoch| *epoch += 1);
    };
    let save = move |value: Option<String>| {
        busy.set(true);
        if value.is_none() {
            path.set(String::new());
        }
        spawn_local(async move { apply(backend::set_kicad_override(value).await) });
    };
    let has_override = saved;
    view! {
        <form
            class="override-form"
            aria-label="kicad-cli location"
            on:submit=move |event| {
                event.prevent_default();
                save(Some(path.get_untracked()));
            }
        >
            <label for="kicad-cli-path">"kicad-cli path or KiCad install folder"</label>
            <div class="override-row">
                <input
                    id="kicad-cli-path"
                    type="text"
                    spellcheck="false"
                    placeholder="C:\\Program Files\\KiCad\\10.0\\bin\\kicad-cli.exe"
                    prop:value=move || path.get()
                    on:input:target=move |event| path.set(event.target().value())
                />
                <button type="submit" class="primary" disabled=move || busy.get()>
                    "Use this path"
                </button>
                <button
                    type="button"
                    disabled=move || busy.get()
                    on:click=move |_| {
                        busy.set(true);
                        spawn_local(async move { apply(backend::rediscover_kicad().await) });
                    }
                >
                    "Search again"
                </button>
                {has_override
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                disabled=move || busy.get()
                                on:click=move |_| save(None)
                            >
                                "Clear saved path"
                            </button>
                        }
                    })}
            </div>
            {rejected
                .map(|message| view! {
                    <p class="problem override-rejected" role="alert">
                        {format!("Not saved: {message}")}
                    </p>
                })}
        </form>
    }
}

/// Shown when the saved settings file exists but cannot be used.
#[component]
fn SettingsProblem(status: RwSignal<Load<AppStatus>>) -> impl IntoView {
    move || {
        let Load::Ready(app) = status.get() else {
            return ().into_any();
        };
        let Some(error) = app.settings_error else {
            return ().into_any();
        };
        view! {
            <section class="kicad-problem" role="alert" aria-labelledby="settings-problem-title">
                <h2 id="settings-problem-title">"Saved settings could not be read"</h2>
                <p>
                    "Default settings are in use. Saving a kicad-cli path replaces the file."
                </p>
                <ul class="kicad-problem-details">
                    <li>{error}</li>
                </ul>
            </section>
        }
        .into_any()
    }
}

#[component]
fn ResultList(
    results: RwSignal<Load<SearchResponse>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <p id="result-count" class="result-count" aria-live="polite">
            {move || match results.get() {
                Load::Loading => "Searching…".to_owned(),
                Load::Failed(error) => format!("Search failed: {error}"),
                Load::Ready(response) if response.total == 0 => "No parts match.".to_owned(),
                Load::Ready(response) if response.total > response.hits.len() => {
                    format!(
                        "Showing {} of {} matching parts",
                        grouped(response.hits.len()),
                        grouped(response.total),
                    )
                }
                Load::Ready(response) => {
                    let noun = if response.total == 1 { "part" } else { "parts" };
                    format!("{} matching {noun}", grouped(response.total))
                }
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
            {move || {
                let Load::Ready(response) = results.get() else {
                    return Vec::new();
                };
                response
                    .hits
                    .into_iter()
                    .map(|hit| {
                        let ipn = hit.ipn.clone();
                        let is_selected = {
                            let ipn = ipn.clone();
                            move || selected.with(|current| current.as_deref() == Some(&ipn))
                        };
                        // Parts without an MPN are titled by IPN; show the
                        // category beside it instead of repeating the IPN.
                        let (title, aside) = if hit.mpn.is_empty() {
                            (hit.ipn.clone(), hit.category.clone())
                        } else {
                            (hit.mpn.clone(), hit.ipn.clone())
                        };
                        view! {
                            <li>
                                <button
                                    type="button"
                                    class="result"
                                    aria-current=move || is_selected().then_some("true")
                                    on:click=move |_| selected.set(Some(ipn.clone()))
                                >
                                    <span class="result-title">{title}</span>
                                    <span class="result-ipn">{aside}</span>
                                    <span class="result-description">{hit.description}</span>
                                </button>
                            </li>
                        }
                    })
                    .collect::<Vec<_>>()
            }}
        </ul>
    }
}

#[component]
fn PartPanel(selected: RwSignal<Option<String>>, kicad_epoch: RwSignal<u32>) -> impl IntoView {
    let detail = RwSignal::new(None::<Load<Option<PartDetail>>>);
    let symbol = RwSignal::new(Load::<PreviewOutcome>::Loading);
    let footprint = RwSignal::new(Load::<PreviewOutcome>::Loading);
    let seq = StoredValue::new(0u64);

    Effect::new(move |_| {
        kicad_epoch.track();
        let Some(ipn) = selected.get() else {
            detail.set(None);
            return;
        };
        let current = seq.get_value() + 1;
        seq.set_value(current);
        let fresh = move || seq.get_value() == current;
        detail.set(Some(Load::Loading));
        symbol.set(Load::Loading);
        footprint.set(Load::Loading);
        {
            let ipn = ipn.clone();
            spawn_local(async move {
                let result = backend::part_detail(&ipn).await;
                if fresh() {
                    detail.set(Some(into_load(result)));
                }
            });
        }
        for (kind, target) in [
            (PreviewKind::Symbol, symbol),
            (PreviewKind::Footprint, footprint),
        ] {
            let ipn = ipn.clone();
            spawn_local(async move {
                let result = backend::preview(ipn, kind).await;
                if fresh() {
                    target.set(into_load(result));
                }
            });
        }
    });

    view! {
        <section class="part-panel" aria-label="Selected part">
            {move || match detail.get() {
                None => view! {
                    <div class="empty-state">
                        <h2>"No part selected"</h2>
                        <p>"Select a part to see its KiCad symbol, every unit, and its footprint."</p>
                    </div>
                }
                .into_any(),
                Some(Load::Loading) => view! {
                    <p class="loading" aria-busy="true">"Loading part…"</p>
                }
                .into_any(),
                Some(Load::Failed(error)) => view! {
                    <p class="problem" role="alert">{format!("Could not load the part: {error}")}</p>
                }
                .into_any(),
                Some(Load::Ready(None)) => view! {
                    <p class="problem" role="alert">"This part is no longer in the catalog."</p>
                }
                .into_any(),
                Some(Load::Ready(Some(part))) => view! {
                    <PartView part=part symbol=symbol footprint=footprint />
                }
                .into_any(),
            }}
        </section>
    }
}

#[component]
fn PartView(
    part: PartDetail,
    symbol: RwSignal<Load<PreviewOutcome>>,
    footprint: RwSignal<Load<PreviewOutcome>>,
) -> impl IntoView {
    let summary = part.summary.clone();
    let heading = if summary.mpn.is_empty() {
        summary.ipn.clone()
    } else {
        summary.mpn.clone()
    };
    let facts: Vec<(&str, String)> = [
        ("IPN", summary.ipn.clone()),
        ("Manufacturer", summary.manufacturer.clone()),
        ("Value", summary.value.clone()),
        ("Category", summary.category.clone()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .collect();
    let field_count = part.fields.len();
    view! {
        <header class="part-header">
            <h2 id="part-title">{heading}</h2>
            <p class="part-description">{summary.description.clone()}</p>
            <dl class="facts">
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
        </header>
        <PreviewSection kind=PreviewKind::Symbol state=symbol />
        <PreviewSection kind=PreviewKind::Footprint state=footprint />
        <details class="all-fields">
            <summary>{format!("All catalog fields ({field_count})")}</summary>
            <table>
                <caption class="visually-hidden">"Catalog fields for this part"</caption>
                <thead>
                    <tr><th scope="col">"Field"</th><th scope="col">"Value"</th></tr>
                </thead>
                <tbody>
                    {part
                        .fields
                        .into_iter()
                        .map(|field| view! {
                            <tr><th scope="row">{field.name}</th><td>{field.value}</td></tr>
                        })
                        .collect_view()}
                </tbody>
            </table>
        </details>
    }
}

fn kind_title(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::Symbol => "Symbol",
        PreviewKind::Footprint => "Footprint",
    }
}

#[component]
fn PreviewSection(kind: PreviewKind, state: RwSignal<Load<PreviewOutcome>>) -> impl IntoView {
    let title = kind_title(kind);
    let heading_id = format!("{}-heading", title.to_lowercase());
    let labelled_by = heading_id.clone();
    view! {
        <section class="preview" aria-labelledby=labelled_by>
            <h3 id=heading_id>{title}</h3>
            {move || match state.get() {
                Load::Loading => view! {
                    <p class="loading" aria-busy="true">
                        {format!("Exporting the {} with kicad-cli…", title.to_lowercase())}
                    </p>
                }
                .into_any(),
                Load::Failed(error) => view! {
                    <div class="problem" role="alert"><p>{error}</p></div>
                }
                .into_any(),
                Load::Ready(PreviewOutcome::Ready(set)) => view! { <PreviewImages set=set /> }.into_any(),
                Load::Ready(PreviewOutcome::Problem(problem)) => {
                    view! { <PreviewProblemView problem=problem /> }.into_any()
                }
            }}
        </section>
    }
}

#[component]
fn PreviewImages(set: PreviewSet) -> impl IntoView {
    let title = kind_title(set.kind);
    let total = set.images.len();
    let units = {
        let mut units: Vec<u32> = set.images.iter().filter_map(|image| image.unit).collect();
        units.dedup();
        units.len()
    };
    let source = if set.from_kicad_bundle {
        "KiCad library"
    } else {
        "project library"
    };
    let provenance = format!(
        "{} · kicad-cli {} · {}",
        source,
        set.kicad_version,
        if set.from_cache {
            "from cache"
        } else {
            "exported now"
        },
    );
    let reference = set.reference.clone();
    let alternates = set.alternates.clone();
    let canvas = match set.kind {
        PreviewKind::Symbol => "canvas canvas-symbol",
        PreviewKind::Footprint => "canvas canvas-footprint",
    };
    // One enlarge dialog per preview section, showing the clicked image.
    let dialog = NodeRef::<leptos::html::Dialog>::new();
    let zoomed = RwSignal::new(None::<(String, String)>);
    let open = move |src: String, alt: String| {
        zoomed.set(Some((src, alt)));
        if let Some(dialog) = dialog.get() {
            show_modal(&dialog);
        }
    };
    view! {
        <p class="preview-meta">
            <code class="reference">{set.reference.clone()}</code>
            <span class="provenance" title=set.library_path.clone()>{provenance}</span>
        </p>
        {(!alternates.is_empty())
            .then(|| view! {
                <p class="alternates">{format!("Also listed: {}", alternates.join(", "))}</p>
            })}
        <div class={if total > 1 { "preview-grid multi" } else { "preview-grid" }}>
            {set
                .images
                .into_iter()
                .map(|image| {
                    let body = if image.demorgan { ", De Morgan" } else { "" };
                    let (caption, alt) = match image.unit {
                        Some(unit) if total > 1 => {
                            let letter = unit_label(unit);
                            (
                                format!("Unit {letter}{body} ({unit} of {units})"),
                                format!("{title} {reference}, unit {letter}{body} ({unit} of {units})"),
                            )
                        }
                        _ => (title.to_owned(), format!("{title} {reference}")),
                    };
                    let src = image.data_uri;
                    let (zoom_src, zoom_alt) = (src.clone(), alt.clone());
                    view! {
                        <figure class="preview-figure">
                            <button
                                type="button"
                                class=canvas
                                title="Enlarge"
                                on:click=move |_| open(zoom_src.clone(), zoom_alt.clone())
                            >
                                <img src=src alt=alt decoding="async" />
                            </button>
                            {(total > 1).then(|| view! { <figcaption>{caption}</figcaption> })}
                        </figure>
                    }
                })
                .collect_view()}
        </div>
        <dialog node_ref=dialog class="zoom-dialog" aria-labelledby=format!("{}-zoom-title", title.to_lowercase())>
            <form method="dialog" class="zoom-header">
                <h4 id=format!("{}-zoom-title", title.to_lowercase())>
                    {move || zoomed.get().map(|(_, alt)| alt).unwrap_or_default()}
                </h4>
                <button type="submit" class="primary">"Close"</button>
            </form>
            <div class=canvas>
                {move || zoomed.get().map(|(src, alt)| view! { <img src=src alt=alt /> })}
            </div>
        </dialog>
    }
}

/// `HTMLDialogElement.showModal()`, which traps focus and closes on Escape.
fn show_modal(dialog: &wasm_bindgen::JsValue) {
    use wasm_bindgen::JsCast;
    if let Ok(method) = js_sys::Reflect::get(dialog, &"showModal".into()) {
        if let Some(method) = method.dyn_ref::<js_sys::Function>() {
            let _ = method.call0(dialog);
        }
    }
}

#[component]
fn PreviewProblemView(problem: PreviewProblem) -> impl IntoView {
    let noun = kind_title(problem.kind).to_lowercase();
    let (headline, hint) = match problem.reason {
        PreviewProblemReason::Unassigned => (format!("No {noun} assigned"), String::new()),
        PreviewProblemReason::Unresolved => (
            format!("The {noun} library was not found"),
            format!(
                "Check the {noun} reference in the catalog, or add the library to the \
                 project's {noun}s folder."
            ),
        ),
        PreviewProblemReason::KicadUnavailable => (
            "Previews need KiCad 10".to_owned(),
            "Fix the KiCad problem shown above; previews load automatically.".to_owned(),
        ),
        PreviewProblemReason::ExportFailed => (
            format!("kicad-cli could not export this {noun}"),
            format!(
                "Check that the library really contains this {noun}, or correct the \
                 catalog reference."
            ),
        ),
        PreviewProblemReason::Unexportable => (
            format!("kicad-cli cannot export this {noun}"),
            format!("Rename the {noun} in its library so the name does not begin with '-'."),
        ),
        PreviewProblemReason::UnknownPart | PreviewProblemReason::Internal => {
            (format!("The {noun} preview failed"), String::new())
        }
    };
    // A missing assignment is not an error, and a KiCad problem is already
    // announced by the panel at the top; neither needs another alert.
    let quiet = matches!(
        problem.reason,
        PreviewProblemReason::Unassigned | PreviewProblemReason::KicadUnavailable
    );
    // The headline and reference already say what failed to export.
    let message = (problem.reason != PreviewProblemReason::ExportFailed).then_some(problem.message);
    view! {
        <div class={if quiet { "problem quiet" } else { "problem" }} role={(!quiet).then_some("alert")}>
            <p class="problem-title">{headline}</p>
            {problem.reference.map(|reference| view! { <p><code class="reference">{reference}</code></p> })}
            {message.map(|message| view! { <p>{message}</p> })}
            {problem
                .cli_message
                .map(|message| view! {
                    <p class="cli-label">"kicad-cli said:"</p>
                    <pre class="cli-message">{message}</pre>
                })}
            {(!hint.is_empty()).then(|| view! { <p class="hint">{hint}</p> })}
            {problem
                .command
                .map(|command| view! {
                    <details class="command">
                        <summary>"Command"</summary>
                        <pre>{command}</pre>
                    </details>
                })}
        </div>
    }
}

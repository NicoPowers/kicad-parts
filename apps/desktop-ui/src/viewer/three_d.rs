//! A footprint's 3D view in its close-up (kicad-parts-c9c1).
//!
//! The 3D view is made only when the person turns it on, because kicad-cli
//! takes seconds. The app makes a board of the footprint, exports it as
//! GLB, and the page draws it with three.js through `js/view3d.js`. A
//! panel says which 3D model file each link uses, or why it is not shown.

use super::*;
use desktop_contract::{Footprint3dRequest, Footprint3dView, ModelSource};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/view3d.js")]
extern "C" {
    /// Whether this computer can draw with WebGL.
    #[wasm_bindgen(js_name = webglAvailable)]
    pub(super) fn webgl_available() -> bool;

    /// Reads a GLB and draws it in an element; resolves to the view.
    #[wasm_bindgen(js_name = createView, catch)]
    async fn create_view(
        container: JsValue,
        bytes: JsValue,
        background: &str,
    ) -> Result<JsValue, JsValue>;

    /// A drawn 3D view.
    pub(super) type View3d;

    #[wasm_bindgen(method)]
    pub(super) fn reset(this: &View3d);
    #[wasm_bindgen(method)]
    pub(super) fn top(this: &View3d);
    #[wasm_bindgen(method)]
    pub(super) fn bottom(this: &View3d);
    #[wasm_bindgen(method)]
    fn dispose(this: &View3d);
}

/// What the person sees when WebGL is off.
pub(super) const NO_WEBGL: &str = "This computer can't show 3D views (WebGL is off).";

/// Where a 3D view is.
#[derive(Clone, Debug, PartialEq)]
enum Making {
    /// kicad-cli is making the GLB.
    Export,
    /// The GLB came; the page is drawing it.
    Drawing,
    Ready,
    Problem(String),
}

/// The 3D view's area. It asks for the 3D view when it opens and frees
/// the drawing when it closes.
#[component]
pub(super) fn ThreeDArea(
    picture: Picture,
    help_id: String,
    /// The drawn view, for the Reset view, Top and Bottom buttons.
    handle: StoredValue<Option<View3d>, LocalStorage>,
    /// What the panel shows: the 3D model links, once known.
    found: RwSignal<Option<Result<Footprint3dView, String>>>,
) -> impl IntoView {
    let area = NodeRef::<leptos::html::Div>::new();
    let making = RwSignal::new(Making::Export);
    // Set when the area closes, so a view drawn after that is freed at once.
    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    found.set(None);
    let request = Footprint3dRequest {
        key: picture.drawing_key.clone().unwrap_or_default(),
        stored: picture.stored.clone(),
        files: picture.model_files.clone(),
        download_model: picture.download_model.clone(),
        // The newest request: waiting exports for older ones are dropped.
        selection: Some(js_sys::Date::now() as u64),
    };
    {
        let closed = closed.clone();
        spawn_local(async move {
            let view = match backend::footprint_3d(request).await {
                Ok(view) => view,
                Err(why) => {
                    found.try_set(Some(Err(why.clone())));
                    making.try_set(Making::Problem(format!(
                        "The 3D view could not be made: {why}"
                    )));
                    return;
                }
            };
            let glb = view.glb.clone();
            let problem = view.problem.clone();
            found.try_set(Some(Ok(view)));
            let Some(glb) = glb else {
                making.try_set(Making::Problem(
                    problem.unwrap_or_else(|| "The 3D view could not be made.".into()),
                ));
                return;
            };
            making.try_set(Making::Drawing);
            let drawn = async {
                let bytes = backend::footprint_glb(&glb.key).await?;
                let element = area
                    .try_get_untracked()
                    .flatten()
                    .ok_or_else(|| "the close-up closed".to_owned())?;
                create_view(element.into(), bytes, BOARD)
                    .await
                    .map_err(|error| {
                        js_sys::Reflect::get(&error, &"message".into())
                            .ok()
                            .and_then(|message| message.as_string())
                            .or_else(|| error.as_string())
                            .unwrap_or_else(|| "three.js could not read it".into())
                    })
            };
            match drawn.await {
                Ok(view) => {
                    let view: View3d = view.unchecked_into();
                    if closed.load(std::sync::atomic::Ordering::SeqCst) {
                        view.dispose();
                        return;
                    }
                    handle.set_value(Some(view));
                    making.try_set(Making::Ready);
                }
                Err(why) => {
                    making.try_set(Making::Problem(format!(
                        "The 3D view could not be drawn: {why}."
                    )));
                }
            }
        });
    }
    on_cleanup(move || {
        closed.store(true, std::sync::atomic::Ordering::SeqCst);
        handle.try_update_value(|view| {
            if let Some(view) = view.take() {
                view.dispose();
            }
        });
    });
    let state = move || match making.get() {
        Making::Export => "making",
        Making::Drawing => "drawing",
        Making::Ready => "ready",
        Making::Problem(_) => "problem",
    };
    let status = move || match making.get() {
        Making::Export => Some(
            view! {
                <p class="view3d-status" aria-busy="true">"Making the 3D view…"</p>
            }
            .into_any(),
        ),
        Making::Drawing => Some(
            view! {
                <p class="view3d-status" aria-busy="true">"Drawing the 3D view…"</p>
            }
            .into_any(),
        ),
        Making::Ready => None,
        Making::Problem(problem) => Some(
            view! {
                <p class="view3d-status view3d-problem" role="alert">{problem}</p>
            }
            .into_any(),
        ),
    };
    view! {
        <div
            node_ref=area
            class="viewer-area viewer-3d"
            data-state=state
            tabindex="0"
            role="application"
            aria-roledescription="3D view"
            aria-label=format!("{} in 3D", picture.label)
            aria-describedby=help_id
            aria-busy=move || if matches!(making.get(), Making::Export | Making::Drawing) { "true" } else { "false" }
        >
            {status}
        </div>
    }
}

/// The 3D model panel: each link, where its file was found and its full
/// path, or why it is not shown.
#[component]
pub(super) fn ModelPanel(
    found: RwSignal<Option<Result<Footprint3dView, String>>>,
    id: String,
) -> impl IntoView {
    let heading = format!("{id}-models");
    let labelled_by = heading.clone();
    let body = move || match found.get() {
        None => {
            view! { <p class="viewer-note" aria-busy="true">"Looking for the 3D model files…"</p> }
                .into_any()
        }
        Some(Err(_)) => {
            view! { <p class="viewer-note">"The 3D model files could not be looked for."</p> }
                .into_any()
        }
        Some(Ok(view)) => {
            let cli = view.cli_message.clone();
            view! {
                <ul class="model-links">
                    {view.models.into_iter().map(|model| {
                        let found = model.source.map(source_words);
                        let looked = model.problem.is_some();
                        view! {
                            <li class="model-link" class:model-missing=looked>
                                <code class="model-link-text">{model.link}</code>
                                {found.map(|words| view! { <p class="model-found">{words}</p> })}
                                {model.problem.map(|problem| view! { <p class="model-problem">{problem}</p> })}
                                {model.path.map(|path| view! {
                                    <p class="model-path">
                                        {if looked { "Looked for:" } else { "File:" }}
                                        <span class="model-path-text">{path}</span>
                                    </p>
                                })}
                            </li>
                        }
                    }).collect_view()}
                </ul>
                {view.notes.into_iter().map(|note| view! { <p class="viewer-note model-note">{note}</p> }).collect_view()}
                {cli.map(|message| view! {
                    <details class="model-cli">
                        <summary>"What kicad-cli said"</summary>
                        <pre>{message}</pre>
                    </details>
                })}
            }
            .into_any()
        }
    };
    view! {
        <section class="viewer-models" aria-labelledby=labelled_by>
            <h5 id=heading>"3D model"</h5>
            {body}
        </section>
    }
}

/// Where a 3D model file was found, in plain words.
fn source_words(source: ModelSource) -> &'static str {
    match source {
        ModelSource::Catalog => "From the Catalog's stored model for this revision.",
        ModelSource::Ours => "From our library.",
        ModelSource::Kicad => "From KiCad's own library.",
        ModelSource::Installed => "From an installed library.",
        ModelSource::Chosen => "From the file you chose.",
        ModelSource::Embedded => "Embedded in the footprint file.",
    }
}

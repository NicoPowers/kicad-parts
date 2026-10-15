//! The drawing viewer (kicad-parts-34df), used in every close-up and in
//! the Libraries browser's drawing pane. The mouse wheel zooms at the
//! pointer, a drag with the left or middle button moves the drawing, and
//! Fit shows all of it again. The keyboard does the same.
//!
//! A footprint is drawn from the app's own drawing of its file, one SVG
//! group per layer, with a layer list. A symbol, or a footprint whose file
//! could not be read, shows kicad-cli's SVG as an SVG `<image>`: it stays
//! sharp when zoomed, and an image never runs script.
//!
//! The view is the SVG's `viewBox`, in drawing millimetres, shown with
//! `xMidYMid meet`. So a view needs no size to be drawn; only zooming and
//! moving read the element's size.

use crate::{backend, bounds, show_modal};
use desktop_contract::PreviewImage;
use desktop_contract::drawing::{
    DrawnText, Edge, FootprintDrawing, PadKind, Point, Shape, TextAlign, circle_center,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod markup;
mod view;

use markup::*;
use view::*;

/// The closest zoom, in screen pixels per millimetre: enough to inspect a
/// 0.1 mm gap across 500 pixels.
const MAX_SCALE: f64 = 5_000.0;
/// How far out the view zooms, as a share of the size that fits.
const MIN_SHARE_OF_FIT: f64 = 0.5;
/// The space around a fitted drawing, as a share of its larger side.
const FIT_MARGIN: f64 = 0.06;
/// How much one press of `+` or `-`, or one zoom button click, zooms.
const ZOOM_STEP: f64 = 1.5;
/// How far one arrow key moves the drawing, as a share of the view.
const PAN_STEP: f64 = 0.1;
/// KiCad's board background.
pub(crate) const BOARD: &str = "#001023";

/// What a viewer shows.
#[derive(Clone, Debug)]
pub(crate) struct Picture {
    /// The accessible name, such as `Footprint g-res:R_0603`.
    pub label: String,
    /// kicad-cli's SVG, as a data URI.
    pub data_uri: String,
    /// kicad-cli's `viewBox`, in millimetres.
    pub view_box: Option<[f64; 4]>,
    /// For a footprint, the key to ask the app's own drawing for, with
    /// layers.
    pub drawing_key: Option<String>,
    /// Drawn on KiCad's dark board, not on white paper.
    pub footprint: bool,
}

impl Picture {
    pub(crate) fn new(image: &PreviewImage, label: String, footprint: bool) -> Self {
        Self {
            label,
            data_uri: image.data_uri.clone(),
            view_box: image.view_box,
            drawing_key: image.drawing_key.clone(),
            footprint,
        }
    }

    /// kicad-cli's drawing's box: `[min_x, min_y, max_x, max_y]`.
    fn image_box(&self) -> [f64; 4] {
        let [x, y, w, h] = self.view_box.unwrap_or([0.0, 0.0, 100.0, 100.0]);
        [x, y, x + w, y + h]
    }
}

// ---- Events --------------------------------------------------------------

/// Gives the next id for a viewer or close-up, so several on one page never
/// share ids.
pub(crate) fn next_id(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The size of an element on screen and its top left corner. Measure the
/// SVG itself, inside the drawing area's border, so a pointer maps to the
/// exact drawing point.
fn measure(element: &wasm_bindgen::JsValue) -> Option<((f64, f64), (f64, f64))> {
    let [left, right, top, bottom] = bounds(element)?;
    let size = (right - left, bottom - top);
    (size.0 > 0.0 && size.1 > 0.0).then_some((size, (left, top)))
}

/// Calls `method` on `target` with `args`, ignoring a missing method.
fn call(target: &wasm_bindgen::JsValue, method: &str, args: &[wasm_bindgen::JsValue]) {
    use wasm_bindgen::JsCast;
    let Ok(function) = js_sys::Reflect::get(target, &method.into()) else {
        return;
    };
    let Some(function) = function.dyn_ref::<js_sys::Function>() else {
        return;
    };
    let list = js_sys::Array::new();
    for arg in args {
        list.push(arg);
    }
    let _ = function.apply(target, &list);
}

// ---- The viewer ----------------------------------------------------------

/// A zoomable, movable drawing. With `layers`, a footprint also shows its
/// layer list. `enlarge` adds an Enlarge button that opens a close-up.
///
/// A footprint's own drawing is asked for only now, when it is shown, not
/// with every thumbnail; until it comes, the viewer says it is busy.
#[component]
pub(crate) fn Viewer(
    picture: Picture,
    /// Show a footprint's layer list beside it (the close-up).
    #[prop(optional)]
    layers: bool,
    #[prop(optional)] enlarge: Option<Callback<()>>,
) -> impl IntoView {
    // None while the drawing is on its way.
    let loaded = RwSignal::new(None::<Option<Arc<FootprintDrawing>>>);
    match picture.drawing_key.clone() {
        Some(key) => spawn_local(async move {
            let drawing = backend::footprint_drawing(&key).await.ok().flatten();
            loaded.try_set(Some(drawing.map(Arc::new)));
        }),
        None => loaded.set(Some(None)),
    }
    move || match loaded.get() {
        None => view! {
            <div class="viewer" data-ready="false">
                <p class="loading" aria-busy="true">"Drawing the layers…"</p>
            </div>
        }
        .into_any(),
        Some(drawing) => view! {
            <DrawingView picture=picture.clone() drawing=drawing layers=layers enlarge=enlarge />
        }
        .into_any(),
    }
}

/// The viewer once its drawing is known: the app's own footprint drawing,
/// or kicad-cli's when there is none.
#[component]
fn DrawingView(
    picture: Picture,
    drawing: Option<Arc<FootprintDrawing>>,
    layers: bool,
    enlarge: Option<Callback<()>>,
) -> impl IntoView {
    let id = next_id("viewer");
    let (content, everything) = match &drawing {
        Some(drawing) => (drawing.fit, drawing.bounds),
        None => (picture.image_box(), picture.image_box()),
    };
    let fit = fit_view(content);
    let outer = fit_view(everything);
    let view = RwSignal::new(fit);
    let area = NodeRef::<leptos::html::Div>::new();
    let pointer = RwSignal::new(None::<Point>);
    // The pointer id and last position of a drag that moves the drawing.
    let drag = StoredValue::new(None::<(i32, f64, f64)>);
    let dragging = RwSignal::new(false);
    let all_layers: Vec<String> = drawing
        .as_ref()
        .map(|drawing| drawing.layers.clone())
        .unwrap_or_default();
    let active = RwSignal::new(first_active(&all_layers));
    let hidden = RwSignal::new(BTreeSet::<String>::new());
    let dim = RwSignal::new(false);
    // An SVG <image> loads in the background; the journeys wait for it.
    let ready = RwSignal::new(drawing.is_some());

    let size = move || {
        area.get_untracked()
            .and_then(|area| area.first_element_child())
            .and_then(|svg| measure(&svg))
    };
    let zoom_by = move |factor: f64, at_pixel: Option<(f64, f64)>| {
        let Some((size, (left, top))) = size() else {
            return;
        };
        view.update(|current| {
            let pixel = at_pixel.map_or((size.0 / 2.0, size.1 / 2.0), |(x, y)| (x - left, y - top));
            let at = to_drawing(*current, size, pixel);
            *current = zoomed(*current, size, factor, at, limits(fit, outer, size));
        });
    };
    let pan_by = move |dx: f64, dy: f64| {
        if let Some((size, _)) = size() {
            view.update(|current| *current = panned(*current, size, dx, dy));
        }
    };

    // The wheel zooms at the pointer. Leptos handles most events with one
    // listener for the whole page, and a browser may not let such a wheel
    // listener stop the page scrolling, so this listener is the element's
    // own and says it is not passive.
    let wheel = StoredValue::new_local(
        None::<wasm_bindgen::closure::Closure<dyn FnMut(leptos::ev::WheelEvent)>>,
    );
    Effect::new(move |_| {
        use wasm_bindgen::JsCast;
        let Some(element) = area.get() else { return };
        if wheel.with_value(Option::is_some) {
            return;
        }
        let listener = wasm_bindgen::closure::Closure::<dyn FnMut(leptos::ev::WheelEvent)>::new(
            move |event: leptos::ev::WheelEvent| {
                event.prevent_default();
                // Lines and pages are about 40 and 800 pixels.
                let unit = match event.delta_mode() {
                    1 => 40.0,
                    2 => 800.0,
                    _ => 1.0,
                };
                let factor = (-event.delta_y() * unit * 0.002).clamp(-1.0, 1.0).exp();
                zoom_by(
                    factor,
                    Some((f64::from(event.client_x()), f64::from(event.client_y()))),
                );
            },
        );
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &"passive".into(), &false.into());
        let function: &js_sys::Function = listener.as_ref().unchecked_ref();
        call(
            &element,
            "addEventListener",
            &["wheel".into(), function.clone().into(), options.into()],
        );
        wheel.set_value(Some(listener));
    });
    on_cleanup(move || {
        use wasm_bindgen::JsCast;
        let Some(element) = area.try_get_untracked().flatten() else {
            return;
        };
        wheel.try_with_value(|listener| {
            if let Some(listener) = listener {
                let function: &js_sys::Function = listener.as_ref().unchecked_ref();
                call(
                    &element,
                    "removeEventListener",
                    &["wheel".into(), function.clone().into()],
                );
            }
        });
    });

    let on_pointer_down = move |event: leptos::ev::PointerEvent| {
        // The left button and the middle button move the drawing.
        if event.button() != 0 && event.button() != 1 {
            return;
        }
        if event.button() == 1 {
            // No autoscroll with the middle button.
            event.prevent_default();
        }
        drag.set_value(Some((
            event.pointer_id(),
            f64::from(event.client_x()),
            f64::from(event.client_y()),
        )));
        dragging.set(true);
        if let Some(element) = area.get_untracked() {
            call(&element, "setPointerCapture", &[event.pointer_id().into()]);
            // Keys work at once after a click.
            call(&element, "focus", &[]);
        }
    };
    let stop_drag = move || {
        drag.set_value(None);
        dragging.set(false);
    };
    let on_pointer_move = move |event: leptos::ev::PointerEvent| {
        let (x, y) = (f64::from(event.client_x()), f64::from(event.client_y()));
        // A release the viewer never heard of, such as one outside the
        // window, ends the drag.
        if event.buttons() == 0 {
            stop_drag();
        }
        if let Some((pointer_id, last_x, last_y)) = drag.get_value() {
            if pointer_id == event.pointer_id() {
                drag.set_value(Some((pointer_id, x, y)));
                pan_by(x - last_x, y - last_y);
            }
        }
        if let Some((size, (left, top))) = size() {
            pointer.set(Some(to_drawing(
                view.get_untracked(),
                size,
                (x - left, y - top),
            )));
        }
    };
    let end_drag = move |event: leptos::ev::PointerEvent| {
        if drag
            .get_value()
            .is_some_and(|(pointer_id, ..)| pointer_id == event.pointer_id())
        {
            stop_drag();
        }
    };
    let on_key = move |event: leptos::ev::KeyboardEvent| {
        // Ctrl, Alt and the Windows key with these keys belong to others,
        // such as the WebView's own zoom.
        if event.ctrl_key() || event.alt_key() || event.meta_key() {
            return;
        }
        let step = size().map_or(40.0, |(size, _)| size.0.min(size.1) * PAN_STEP);
        match event.key().as_str() {
            "+" | "=" => zoom_by(ZOOM_STEP, None),
            "-" | "_" => zoom_by(1.0 / ZOOM_STEP, None),
            "0" => view.set(fit),
            "ArrowLeft" => pan_by(step, 0.0),
            "ArrowRight" => pan_by(-step, 0.0),
            "ArrowUp" => pan_by(0.0, step),
            "ArrowDown" => pan_by(0.0, -step),
            _ => return,
        }
        event.prevent_default();
    };

    let help_id = format!("{id}-help");
    let has_drawing = drawing.is_some();
    let footprint = picture.footprint;
    let svg = match drawing.clone() {
        Some(drawing) => footprint_svg(drawing, view, active, hidden, dim).into_any(),
        None => {
            let [x, y, w, h] = picture.view_box.unwrap_or([0.0, 0.0, 100.0, 100.0]);
            view! {
                <svg
                    class="viewer-svg"
                    viewBox=move || view_box_text(view.get())
                    preserveAspectRatio="xMidYMid meet"
                    aria-hidden="true"
                >
                    <image
                        href=picture.data_uri.clone()
                        x=number(x)
                        y=number(y)
                        width=number(w)
                        height=number(h)
                        preserveAspectRatio="none"
                        on:load=move |_| ready.set(true)
                        on:error=move |_| ready.set(true)
                    />
                </svg>
            }
            .into_any()
        }
    };
    let position = move || {
        pointer
            .get()
            .map(|point| format!("X {} mm, Y {} mm", format_mm(point.x), format_mm(point.y)))
    };
    let instructions = if footprint {
        "Scroll or press plus and minus to zoom. Drag, or use the arrow keys, to move the drawing. Press 0 to fit it. The pointer's position is shown in millimetres."
    } else {
        "Scroll or press plus and minus to zoom. Drag, or use the arrow keys, to move the drawing. Press 0 to fit it."
    };
    let layer_list = (layers && drawing.is_some()).then(|| {
        let drawing = drawing.clone().expect("a drawing");
        view! { <LayerList drawing=drawing active=active hidden=hidden dim=dim id=id.clone() /> }
    });
    view! {
        <div
            class="viewer"
            class:viewer-footprint=footprint
            data-active-layer=move || has_drawing.then(|| active.get())
            data-ready=move || if ready.get() { "true" } else { "false" }
        >
            <div class="viewer-main">
                <div class="viewer-toolbar">
                    <button type="button" class="viewer-fit" on:click=move |_| view.set(fit)>"Fit"</button>
                    <button type="button" aria-label="Zoom in" title="Zoom in" on:click=move |_| zoom_by(ZOOM_STEP, None)>"+"</button>
                    <button type="button" aria-label="Zoom out" title="Zoom out" on:click=move |_| zoom_by(1.0 / ZOOM_STEP, None)>"-"</button>
                    {enlarge.map(|enlarge| view! {
                        <button type="button" class="viewer-enlarge" on:click=move |_| enlarge.run(())>"Enlarge"</button>
                    })}
                    {footprint.then(|| view! {
                        <span class="viewer-position" aria-hidden="true">{position}</span>
                    })}
                </div>
                <div
                    node_ref=area
                    class="viewer-area"
                    class:panning=move || dragging.get()
                    tabindex="0"
                    role="application"
                    aria-roledescription="drawing"
                    aria-label=picture.label.clone()
                    aria-describedby=help_id.clone()
                    on:pointerdown=on_pointer_down
                    on:pointermove=on_pointer_move
                    on:pointerup=end_drag
                    on:pointercancel=end_drag
                    on:lostpointercapture=end_drag
                    on:pointerleave=move |_| pointer.set(None)
                    on:mousedown=move |event: leptos::ev::MouseEvent| {
                        if event.button() == 1 {
                            event.prevent_default();
                        }
                    }
                    on:keydown=on_key
                >
                    {svg}
                </div>
                <p id=help_id class="visually-hidden">{instructions}</p>
            </div>
            {layer_list}
        </div>
    }
}

/// The footprint's SVG: one group per layer, in drawing order, with the
/// holes on top.
fn footprint_svg(
    drawing: Arc<FootprintDrawing>,
    view: RwSignal<ViewBox>,
    active: RwSignal<String>,
    hidden: RwSignal<BTreeSet<String>>,
    dim: RwSignal<bool>,
) -> impl IntoView {
    let layers = drawing.layers.clone();
    let holes = holes_markup(&drawing);
    let order = move || draw_order(&layers, &active.get());
    view! {
        <svg
            class="viewer-svg"
            class:dimmed=move || dim.get()
            viewBox=move || view_box_text(view.get())
            preserveAspectRatio="xMidYMid meet"
            aria-hidden="true"
            stroke-linecap="round"
            stroke-linejoin="round"
            font-family="Segoe UI, sans-serif"
        >
            <For
                each=order
                key=|layer: &String| layer.clone()
                children=move |layer: String| {
                    let (color, alpha, _) = layer_look(&layer);
                    let markup = layer_markup(&drawing, &layer);
                    let name = layer.clone();
                    let shown = layer.clone();
                    let current = layer.clone();
                    view! {
                        <g
                            class="layer"
                            class:active=move || active.get() == current
                            data-layer=name
                            display=move || if hidden.with(|hidden| hidden.contains(&shown)) { "none" } else { "inline" }
                        >
                            <g color=color opacity=number(alpha) inner_html=markup></g>
                        </g>
                    }
                }
            />
            <g class="holes" inner_html=holes></g>
        </svg>
    }
}

/// The footprint's layers: each can be shown or hidden, and one is the
/// active layer, drawn on top.
#[component]
fn LayerList(
    drawing: Arc<FootprintDrawing>,
    active: RwSignal<String>,
    hidden: RwSignal<BTreeSet<String>>,
    dim: RwSignal<bool>,
    id: String,
) -> impl IntoView {
    let group = format!("{id}-active");
    let notes = drawing.left_out.clone();
    view! {
        <div class="viewer-layers">
            <fieldset aria-describedby=format!("{id}-layers-hint")>
                <legend>"Layers"</legend>
                <p class="viewer-layers-hint" id=format!("{id}-layers-hint")>
                    "Choose the active layer, drawn on top. Tick a layer to show it."
                </p>
                <ul>
                    {drawing
                        .layers
                        .iter()
                        .enumerate()
                        .map(|(index, layer)| {
                            let (color, _, what) = layer_look(layer);
                            let radio = format!("{id}-layer-{index}");
                            let (is_active, choose, shown, toggle) =
                                (layer.clone(), layer.clone(), layer.clone(), layer.clone());
                            let row_active = layer.clone();
                            view! {
                                <li class:active=move || active.get() == row_active>
                                    <input
                                        type="radio"
                                        id=radio.clone()
                                        name=group.clone()
                                        prop:checked=move || active.get() == is_active
                                        on:change=move |_| active.set(choose.clone())
                                    />
                                    <span class="layer-swatch" style:background=color></span>
                                    <label for=radio>
                                        <span class="layer-name">{layer.clone()}</span>
                                        <span class="layer-what">{what}</span>
                                    </label>
                                    <input
                                        type="checkbox"
                                        class="layer-shown"
                                        aria-label=format!("Show {layer}")
                                        prop:checked=move || hidden.with(|hidden| !hidden.contains(&shown))
                                        on:change=move |_| hidden.update(|hidden| {
                                            if !hidden.remove(&toggle) {
                                                hidden.insert(toggle.clone());
                                            }
                                        })
                                    />
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </fieldset>
            <label class="viewer-dim">
                <input type="checkbox" prop:checked=move || dim.get() on:change=move |_| dim.update(|dim| *dim = !*dim) />
                "Dim the other layers"
            </label>
            <p class="viewer-note">"A board's own settings can change paste and mask openings."</p>
            {notes.into_iter().map(|note| view! { <p class="viewer-note">{note}</p> }).collect_view()}
        </div>
    }
}

// ---- The close-up ----------------------------------------------------------

/// A close-up dialog and what it shows. Copy it into event handlers.
#[derive(Clone, Copy)]
pub(crate) struct CloseUp {
    dialog: NodeRef<leptos::html::Dialog>,
    shown: RwSignal<Option<Picture>>,
}

impl CloseUp {
    pub(crate) fn new() -> Self {
        Self {
            dialog: NodeRef::new(),
            shown: RwSignal::new(None),
        }
    }

    /// Shows `picture` in the close-up, fitted.
    pub(crate) fn open(self, picture: Picture) {
        self.shown.set(Some(picture));
        if let Some(dialog) = self.dialog.get_untracked() {
            show_modal(&dialog);
        }
    }
}

/// The close-up dialog. It closes on Escape, Close, or a click on the
/// dim backdrop around it, where both the press and the release must be,
/// so a drag that ends outside the drawing never closes it.
#[component]
pub(crate) fn CloseUpDialog(close_up: CloseUp) -> impl IntoView {
    let title_id = next_id("close-up-title");
    let dialog = close_up.dialog;
    let labelled_by = title_id.clone();
    let pressed_outside = StoredValue::new(false);
    let outside = move |event: &leptos::ev::MouseEvent| {
        dialog.get_untracked().is_some_and(|dialog| {
            let Some([left, right, top, bottom]) = bounds(&dialog) else {
                return false;
            };
            let (x, y) = (f64::from(event.client_x()), f64::from(event.client_y()));
            x < left || x > right || y < top || y > bottom
        })
    };
    view! {
        <dialog
            node_ref=dialog
            class="zoom-dialog"
            aria-labelledby=labelled_by
            on:pointerdown=move |event| pressed_outside.set_value(outside(&event))
            on:click=move |event| {
                if pressed_outside.get_value() && outside(&event) {
                    if let Some(dialog) = dialog.get_untracked() {
                        dialog.close();
                    }
                }
                pressed_outside.set_value(false);
            }
        >
            // Not a form: a close-up can open inside Edit part's form.
            <div class="zoom-header">
                <h4 id=title_id>
                    {move || close_up.shown.get().map(|picture| picture.label).unwrap_or_default()}
                </h4>
                <button
                    type="button"
                    class="primary"
                    on:click=move |_| {
                        if let Some(dialog) = dialog.get_untracked() {
                            dialog.close();
                        }
                    }
                >
                    "Close"
                </button>
            </div>
            <div class="zoom-body">
                {move || close_up.shown.get().map(|picture| view! { <Viewer picture=picture layers=true /> })}
            </div>
        </dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let view = [0.0, 0.0, 10.0, 5.0];
        let size = (400.0, 400.0);
        // 40 pixels per unit; the view is centred, 100 pixels down.
        assert!(close(scale(view, size), 40.0));
        let pixel = (100.0, 150.0);
        let at = to_drawing(view, size, pixel);
        assert!(close(at.x, 2.5) && close(at.y, 1.25), "{at:?}");
        let zoomed_in = zoomed(view, size, 2.0, at, (1.0, 1000.0));
        assert!(close(scale(zoomed_in, size), 80.0));
        let again = to_drawing(zoomed_in, size, pixel);
        assert!(close(again.x, at.x) && close(again.y, at.y), "{again:?}");
        // The limits hold.
        let far = zoomed(view, size, 1e9, at, (1.0, 1000.0));
        assert!(close(scale(far, size), 1000.0));
        let out = zoomed(view, size, 1e-9, at, (1.0, 1000.0));
        assert!(close(scale(out, size), 1.0));
        // Nonsense changes nothing.
        assert_eq!(zoomed(view, size, f64::NAN, at, (1.0, 1000.0)), view);
        assert_eq!(panned(view, (0.0, 0.0), 5.0, 5.0), view);
    }

    #[test]
    fn moving_follows_the_pointer() {
        let view = [0.0, 0.0, 10.0, 10.0];
        let size = (500.0, 500.0);
        let moved = panned(view, size, 50.0, -100.0);
        assert!(close(moved[0], -1.0) && close(moved[1], 2.0));
        let fit = fit_view([-1.0, -0.5, 1.0, 0.5]);
        assert!(close(fit[2], 2.24) && close(fit[0], -1.12));
        let (low, high) = limits(fit, fit, size);
        assert!(close(low, scale(fit, size) * 0.5) && close(high, MAX_SCALE));
        // A long text far beyond the shapes can still be zoomed out to.
        let wide = fit_view([-20.0, -1.0, 20.0, 1.0]);
        let (low, _) = limits(fit, wide, size);
        assert!(close(low, scale(wide, size)));
    }

    #[test]
    fn arcs_are_exact_svg_arcs() {
        let mut path = String::new();
        // Over the top of the circle, clockwise on screen.
        arc_commands(
            &mut path,
            Point::new(-1.0, 0.0),
            Point::new(0.0, -1.0),
            Point::new(1.0, 0.0),
        );
        assert_eq!(path, " A 1 1 0 0 1 1 0");
        // Three quarters round, the long way, counterclockwise.
        let mut path = String::new();
        let h = std::f64::consts::FRAC_1_SQRT_2;
        arc_commands(
            &mut path,
            Point::new(1.0, 0.0),
            Point::new(-h, -h),
            Point::new(0.0, 1.0),
        );
        assert_eq!(path, " A 1 1 0 1 0 0 1");
        // A whole circle, and three points on a line.
        let mut path = String::new();
        arc_commands(
            &mut path,
            Point::new(1.0, 0.0),
            Point::new(-1.0, 0.0),
            Point::new(1.0, 0.0),
        );
        assert_eq!(path, " A 1 1 0 0 1 -1 0 A 1 1 0 0 1 1 0");
        let mut path = String::new();
        arc_commands(
            &mut path,
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(2.0, 2.0),
        );
        assert_eq!(path, " L 1 1 L 2 2");
    }

    #[test]
    fn layers_draw_back_to_front_with_the_active_one_on_top() {
        let layers: Vec<String> = [
            "F.Cu",
            "B.Cu",
            "F.Paste",
            "F.SilkS",
            "F.Mask",
            "B.Mask",
            "Dwgs.User",
            "F.CrtYd",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(
            draw_order(&layers, "F.Cu"),
            [
                "B.Cu",
                "B.Mask",
                "Dwgs.User",
                "F.Mask",
                "F.Paste",
                "F.SilkS",
                "F.CrtYd",
                "F.Cu"
            ]
        );
        assert_eq!(
            draw_order(&layers, "B.Cu").last().map(String::as_str),
            Some("B.Cu")
        );
        assert_eq!(first_active(&layers), "F.Cu");
        assert_eq!(
            first_active(&["F.SilkS".to_owned(), "B.Cu".to_owned()]),
            "B.Cu"
        );
        assert_eq!(first_active(&["F.SilkS".to_owned()]), "F.SilkS");
        assert_eq!(first_active(&[]), "");
        assert_eq!(layer_look("F.Paste").0, "#B4A09A");
        assert_eq!(layer_look("In2.Cu").2, "Inner copper 2");
        assert_eq!(layer_look("User.3").2, "User layer 3");
    }

    #[test]
    fn markup_escapes_text_and_trims_numbers() {
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(-0.0000001), "0");
        assert_eq!(number(2.0), "2");
        assert_eq!(format_mm(-0.0001), "0.000");
        assert_eq!(format_mm(-1.25), "\u{2212}1.250");
        let mut out = String::new();
        text_markup(
            &mut out,
            &DrawnText {
                layer: "F.SilkS".into(),
                text: "<script>&\"".into(),
                at: Point::new(1.0, 2.0),
                angle: 90.0,
                height: 0.7,
                thickness: 0.1,
                horizontal: TextAlign::Start,
                vertical: TextAlign::End,
                mirrored: true,
                bold: false,
                italic: true,
            },
        );
        assert!(out.contains(">&lt;script&gt;&amp;&quot;</text>"), "{out}");
        assert!(
            out.contains("translate(1 2) rotate(-90) scale(-1 1)"),
            "{out}"
        );
        assert!(out.contains("font-size=\"1\""), "{out}");
        assert!(out.contains("text-anchor=\"start\""), "{out}");
        assert!(out.contains("font-style=\"italic\""), "{out}");
    }
}

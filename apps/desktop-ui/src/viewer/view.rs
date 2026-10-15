//! The view: the drawing area a viewer shows, as its SVG `viewBox`, and
//! how zooming and moving change it.

use super::*;

/// The drawing area shown: `[x, y, width, height]` in drawing units.
pub(super) type ViewBox = [f64; 4];

/// The view that shows all of `content`, with a margin.
pub(super) fn fit_view([x0, y0, x1, y1]: [f64; 4]) -> ViewBox {
    let (w, h) = ((x1 - x0).max(1e-3), (y1 - y0).max(1e-3));
    let margin = w.max(h) * FIT_MARGIN;
    [x0 - margin, y0 - margin, w + 2.0 * margin, h + 2.0 * margin]
}

/// Screen pixels per drawing unit when `view` is shown in an element of
/// `size` pixels.
pub(super) fn scale(view: ViewBox, size: (f64, f64)) -> f64 {
    (size.0 / view[2]).min(size.1 / view[3])
}

/// The drawing point at `pixel`, measured from the element's top left.
pub(super) fn to_drawing(view: ViewBox, size: (f64, f64), pixel: (f64, f64)) -> Point {
    let s = scale(view, size);
    let left = (size.0 - view[2] * s) / 2.0;
    let top = (size.1 - view[3] * s) / 2.0;
    Point::new(
        view[0] + (pixel.0 - left) / s,
        view[1] + (pixel.1 - top) / s,
    )
}

/// The pixel where drawing point `at` is shown, from the element's top
/// left: the reverse of [`to_drawing`].
pub(super) fn to_pixel(view: ViewBox, size: (f64, f64), at: Point) -> (f64, f64) {
    let s = scale(view, size);
    let left = (size.0 - view[2] * s) / 2.0;
    let top = (size.1 - view[3] * s) / 2.0;
    (left + (at.x - view[0]) * s, top + (at.y - view[1]) * s)
}

/// `view` zoomed by `factor` around the drawing point `at`, which stays
/// where it is on screen. The zoom stays within `limits` (pixels per unit).
pub(super) fn zoomed(
    view: ViewBox,
    size: (f64, f64),
    factor: f64,
    at: Point,
    limits: (f64, f64),
) -> ViewBox {
    let s = scale(view, size);
    if !(s.is_finite() && s > 0.0 && factor.is_finite() && factor > 0.0) {
        return view;
    }
    let k = (s * factor).clamp(limits.0, limits.1) / s;
    [
        at.x - (at.x - view[0]) / k,
        at.y - (at.y - view[1]) / k,
        view[2] / k,
        view[3] / k,
    ]
}

/// `view` with the drawing moved by `dx`, `dy` screen pixels.
pub(super) fn panned(view: ViewBox, size: (f64, f64), dx: f64, dy: f64) -> ViewBox {
    let s = scale(view, size);
    if !(s.is_finite() && s > 0.0) {
        return view;
    }
    [view[0] - dx / s, view[1] - dy / s, view[2], view[3]]
}

/// How far the view may zoom in an element of `size`: out to half the fit,
/// or further if `outer` (everything, long texts too) needs it; in to
/// [`MAX_SCALE`] (or the fit, for a tiny drawing).
pub(super) fn limits(fit: ViewBox, outer: ViewBox, size: (f64, f64)) -> (f64, f64) {
    let fitted = scale(fit, size);
    let lowest = (fitted * MIN_SHARE_OF_FIT).min(scale(outer, size));
    (lowest, MAX_SCALE.max(fitted))
}

pub(super) fn view_box_text(view: ViewBox) -> String {
    view.map(number).join(" ")
}

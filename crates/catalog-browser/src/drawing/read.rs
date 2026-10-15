//! Reading numbers, points and settings from a footprint's lists.

use super::*;

/// A finite number no larger than [`MAX_MM`].
pub(super) fn number(word: Option<&str>) -> Option<f64> {
    word?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && value.abs() <= MAX_MM)
}

pub(super) fn child<'a>(node: &'a Sexp, head: &'a str) -> Option<&'a Sexp> {
    node.children(head).next()
}

/// The two numbers of `(head x y)`.
pub(super) fn pair(node: &Sexp, head: &str) -> Option<(f64, f64)> {
    let item = child(node, head)?;
    Some((number(item.word_at(1))?, number(item.word_at(2))?))
}

pub(super) fn point(node: &Sexp, head: &str) -> Option<Point> {
    pair(node, head).map(|(x, y)| Point::new(x, y))
}

/// `(at x y [angle])`: the position and the angle (0 when not a number,
/// as in `(at 0 0 unlocked)`).
pub(super) fn at(node: &Sexp) -> Option<(Point, f64)> {
    let item = child(node, "at")?;
    let position = Point::new(number(item.word_at(1))?, number(item.word_at(2))?);
    let angle = item
        .word_at(3)
        .and_then(|word| word.parse::<f64>().ok())
        .filter(|angle| angle.is_finite())
        .map_or(0.0, |angle| angle % 360.0);
    Some((position, angle))
}

/// The stroke width: `(stroke (width w))`, or the older `(width w)`.
pub(super) fn stroke_width(node: &Sexp) -> f64 {
    child(node, "stroke")
        .and_then(|stroke| child(stroke, "width"))
        .or_else(|| child(node, "width"))
        .and_then(|width| number(width.word_at(1)))
        .map_or(0.0, f64::abs)
}

/// `(fill yes|solid)` fills; `(fill no|none)` does not; with no `fill`,
/// `default`.
pub(super) fn filled(node: &Sexp, default: bool) -> bool {
    match child(node, "fill").and_then(|fill| fill.word_at(1)) {
        Some("yes" | "solid") => true,
        Some(word) => hatch(word),
        None => default,
    }
}

/// Whether a shape has one of KiCad 10's hatched fills, which the close-up
/// draws solid.
pub(super) fn hatched(node: &Sexp) -> bool {
    child(node, "fill")
        .and_then(|fill| fill.word_at(1))
        .is_some_and(hatch)
}

fn hatch(word: &str) -> bool {
    word.ends_with("hatch")
}

pub(super) fn layer_of(node: &Sexp) -> Option<String> {
    child(node, "layer")
        .and_then(|layer| layer.word_at(1))
        .map(str::to_owned)
}

/// Whether a text item is hidden: a bare `hide`, `(hide)` or `(hide yes)`
/// on the item or in its `effects`.
///
/// A text's own words (its kind or name, and its text) come first, so a
/// text that reads "hide" is not hidden.
pub(super) fn hidden(node: &Sexp) -> bool {
    let says_hide = |node: &Sexp, skip: usize| {
        node.items().iter().skip(skip).any(|item| match item {
            Sexp::Word(word) => word == "hide",
            Sexp::List(_) => item.head() == Some("hide") && item.word_at(1) != Some("no"),
        })
    };
    says_hide(node, 3) || child(node, "effects").is_some_and(|effects| says_hide(effects, 1))
}

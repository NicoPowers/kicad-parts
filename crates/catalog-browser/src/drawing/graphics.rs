//! Footprint graphics and custom pad primitives.

use super::*;

/// A footprint graphic (`fp_line` and the like) or a custom pad's
/// primitive (`gr_line` and the like), by its kind without the prefix.
/// Primitives are filled unless they say otherwise.
pub(super) fn graphic(head: &str, node: &Sexp, primitive: bool) -> Option<Shape> {
    let kind = head
        .strip_prefix("fp_")
        .or_else(|| head.strip_prefix("gr_"))?;
    let width = stroke_width(node);
    let shape = match kind {
        "line" => Shape::Line {
            start: point(node, "start")?,
            end: point(node, "end")?,
            width,
        },
        "rect" => {
            let (a, b) = (point(node, "start")?, point(node, "end")?);
            polygon(
                &[a, Point::new(b.x, a.y), b, Point::new(a.x, b.y)],
                width,
                filled(node, primitive),
            )
        }
        "circle" => {
            let center = point(node, "center")?;
            Shape::Circle {
                center,
                radius: distance(center, point(node, "end")?),
                width,
                filled: filled(node, primitive),
            }
        }
        "arc" => {
            let start = point(node, "start")?;
            let end = point(node, "end")?;
            match point(node, "mid") {
                Some(mid) => Shape::Arc {
                    start,
                    mid,
                    end,
                    width,
                },
                None => old_arc(node, start, end, width)?,
            }
        }
        "poly" => {
            let edges = poly_edges(child(node, "pts")?)?;
            Shape::Polygon {
                edges,
                width,
                filled: filled(node, primitive),
            }
        }
        "curve" | "bezier" => {
            let points: Vec<Point> = child(node, "pts")?
                .children("xy")
                .filter_map(|xy| Some(Point::new(number(xy.word_at(1))?, number(xy.word_at(2))?)))
                .collect();
            let [start, control1, control2, end] = points.as_slice() else {
                return None;
            };
            Shape::Curve {
                start: *start,
                control1: *control1,
                control2: *control2,
                end: *end,
                width,
            }
        }
        _ => return None,
    };
    Some(shape)
}

/// A KiCad 5 arc: `start` is its centre, `end` where it begins, and
/// `(angle a)` how far it turns, clockwise on screen.
pub(super) fn old_arc(node: &Sexp, center: Point, from: Point, width: f64) -> Option<Shape> {
    let angle = number(child(node, "angle")?.word_at(1))?;
    let turn = |degrees: f64| {
        let offset = Point::new(from.x - center.x, from.y - center.y);
        add(center, rotate(offset, -degrees))
    };
    Some(Shape::Arc {
        start: from,
        mid: turn(angle / 2.0),
        end: turn(angle),
        width,
    })
}

/// The edges of a polygon's `(pts (xy ...) (arc (start) (mid) (end)) ...)`.
pub(super) fn poly_edges(pts: &Sexp) -> Option<Vec<Edge>> {
    let mut edges = Vec::new();
    let mut first: Option<Point> = None;
    let mut last: Option<Point> = None;
    let line_to = |edges: &mut Vec<Edge>, last: &mut Option<Point>, to: Point| {
        if let Some(from) = *last {
            if from != to {
                edges.push(Edge::Line {
                    start: from,
                    end: to,
                });
            }
        }
        *last = Some(to);
    };
    for item in pts.items().iter().skip(1) {
        match item.head() {
            Some("xy") => {
                let p = Point::new(number(item.word_at(1))?, number(item.word_at(2))?);
                first.get_or_insert(p);
                line_to(&mut edges, &mut last, p);
            }
            Some("arc") => {
                let (start, mid, end) = (
                    point(item, "start")?,
                    point(item, "mid")?,
                    point(item, "end")?,
                );
                first.get_or_insert(start);
                line_to(&mut edges, &mut last, start);
                edges.push(Edge::Arc { start, mid, end });
                last = Some(end);
            }
            _ => {}
        }
    }
    if let (Some(first), Some(last)) = (first, last) {
        if first != last {
            edges.push(Edge::Line {
                start: last,
                end: first,
            });
        }
    }
    (!edges.is_empty()).then_some(edges)
}

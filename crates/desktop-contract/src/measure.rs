//! The geometry behind the close-up's Measure tool (kicad-parts-e95e): the
//! points a measurement snaps to on one layer, the nearest of them, and the
//! gap between two pads on that layer.
//!
//! Like [`crate::drawing`], everything is in footprint millimetres, with Y
//! pointing down.

use crate::drawing::{DrawnPad, Edge, FootprintDrawing, Point, Shape, circle_center};

/// How far a curved edge may stray from its straight pieces when a gap is
/// measured: 0.1 micrometre, well under the 1 micrometre the readout shows.
const ARC_TOLERANCE: f64 = 1e-4;
/// The most straight pieces one arc becomes.
const MAX_ARC_PIECES: usize = 4096;
/// The straight pieces one Bezier curve becomes.
const CURVE_PIECES: usize = 64;
/// Two snap points closer than this are one point.
const SAME_POINT: f64 = 1e-6;

/// A point a measurement can snap to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapPoint {
    pub at: Point,
    pub kind: SnapKind,
    /// The index of the pad in [`FootprintDrawing::pads`] that the point
    /// belongs to; `None` for a graphic's point.
    pub pad: Option<usize>,
}

/// What a snap point is on its pad or graphic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapKind {
    /// A pad's position.
    Centre,
    /// Where two straight edges meet.
    Corner,
    /// Halfway along a pad's straight edge.
    EdgeMiddle,
    ArcEnd,
    ArcMiddle,
    ArcCentre,
    CircleCentre,
    /// The top, bottom, left or right of a circle.
    Top,
    Bottom,
    Left,
    Right,
    HoleCentre,
    LineEnd,
    LineMiddle,
    CurveEnd,
}

impl SnapKind {
    fn words(self) -> &'static str {
        match self {
            Self::Centre => "centre",
            Self::Corner => "corner",
            Self::EdgeMiddle => "edge middle",
            Self::ArcEnd => "arc end",
            Self::ArcMiddle => "arc middle",
            Self::ArcCentre => "arc centre",
            Self::CircleCentre => "circle centre",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
            Self::HoleCentre => "hole centre",
            Self::LineEnd => "line end",
            Self::LineMiddle => "line middle",
            Self::CurveEnd => "curve end",
        }
    }

    fn is_side(self) -> bool {
        matches!(self, Self::Top | Self::Bottom | Self::Left | Self::Right)
    }
}

impl SnapPoint {
    /// The point's name for the readout, such as `Pad 3 corner`,
    /// `Unnumbered pad centre`, `Line end` or `Circle top`.
    pub fn name(&self, drawing: &FootprintDrawing) -> String {
        let words = self.kind.words();
        match self.pad.and_then(|index| drawing.pads.get(index)) {
            Some(pad) if pad.number.is_empty() => format!("Unnumbered pad {words}"),
            Some(pad) => format!("Pad {} {words}", pad.number),
            None if self.kind.is_side() => format!("Circle {words}"),
            None => {
                let mut name = words.to_owned();
                name[..1].make_ascii_uppercase();
                name
            }
        }
    }
}

/// A pad's name inside a sentence: `pad 3`, or `an unnumbered pad`.
pub fn pad_name(pad: &DrawnPad) -> String {
    if pad.number.is_empty() {
        "an unnumbered pad".to_owned()
    } else {
        format!("pad {}", pad.number)
    }
}

/// Every point on `layer` a measurement can snap to, in reading order:
/// top to bottom, then left to right. Where points meet, a pad's point
/// comes before a graphic's, and its centre before its outline.
///
/// - A pad on the layer gives its centre and its outline's corners, edge
///   middles, arc ends, middles and centres, or a circle's centre and
///   sides. A wider stroke moves the outline points out by half the width.
/// - On a copper layer, every drilled pad gives its hole's centre.
/// - A graphic gives a line's ends and middle, an arc's ends, middle and
///   centre, a circle's centre and sides, and a polygon's or rectangle's
///   corners, on the line the file draws.
pub fn snap_points(drawing: &FootprintDrawing, layer: &str) -> Vec<SnapPoint> {
    let mut points = Vec::new();
    let copper = layer.ends_with(".Cu");
    for (index, pad) in drawing.pads.iter().enumerate() {
        let mut add = |at: Point, kind: SnapKind| {
            points.push(SnapPoint {
                at,
                kind,
                pad: Some(index),
            });
        };
        let mut shapes = pad
            .shapes
            .iter()
            .filter(|shape| shape.layer == layer)
            .peekable();
        if shapes.peek().is_some() {
            add(pad.center, SnapKind::Centre);
        }
        for shape in shapes {
            shape_points(&shape.shape, true, &mut add);
        }
        if copper {
            if let Some(hole) = &pad.hole {
                add(hole_centre(hole), SnapKind::HoleCentre);
            }
        }
    }
    for graphic in drawing.graphics.iter().filter(|shape| shape.layer == layer) {
        shape_points(&graphic.shape, false, &mut |at, kind| {
            points.push(SnapPoint {
                at,
                kind,
                pad: None,
            });
        });
    }
    // Keep the first of the points that meet on one pad, or on graphics.
    let key = |p: Point| ((p.x / SAME_POINT).round(), (p.y / SAME_POINT).round());
    let mut seen = std::collections::HashSet::new();
    points.retain(|point| {
        let (x, y) = key(point.at);
        seen.insert((point.pad, x.to_bits(), y.to_bits()))
    });
    // A stable sort keeps pads before graphics where points meet.
    points.sort_by(|a, b| {
        let (ax, ay) = key(a.at);
        let (bx, by) = key(b.at);
        ay.total_cmp(&by).then(ax.total_cmp(&bx))
    });
    points
}

/// The index of the snap point nearest `at`, if one is within `radius`.
/// Of points equally near, the first wins.
pub fn nearest_snap(points: &[SnapPoint], at: Point, radius: f64) -> Option<usize> {
    let mut best = None;
    let mut best_distance = radius * radius;
    for (index, point) in points.iter().enumerate() {
        let d = (point.at.x - at.x).powi(2) + (point.at.y - at.y).powi(2);
        if d <= best_distance && best.is_none_or(|_| d < best_distance) {
            best = Some(index);
            best_distance = d;
        }
    }
    best
}

/// The shortest distance between two pads' shapes on one layer, and where
/// it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap {
    /// In millimetres; zero or less when the pads touch or overlap.
    pub distance: f64,
    /// The nearest points on each pad's edge, `from` on the first pad.
    /// When the pads touch or overlap these mean nothing.
    pub from: Point,
    pub to: Point,
}

/// The gap between pads `a` and `b` (indexes into
/// [`FootprintDrawing::pads`]) on `layer`, counting each shape's stroke
/// width; `None` when either pad has nothing on the layer.
pub fn pad_gap(drawing: &FootprintDrawing, a: usize, b: usize, layer: &str) -> Option<Gap> {
    let regions = |pad: &DrawnPad| -> Vec<Region> {
        pad.shapes
            .iter()
            .filter(|shape| shape.layer == layer)
            .filter_map(|shape| Region::of(&shape.shape))
            .collect()
    };
    let first = regions(drawing.pads.get(a)?);
    let second = regions(drawing.pads.get(b)?);
    let mut best: Option<Gap> = None;
    for one in &first {
        for other in &second {
            let gap = one.gap(other);
            if best.is_none_or(|best| gap.distance < best.distance) {
                best = Some(gap);
            }
        }
    }
    best
}

// ---- Snap points ---------------------------------------------------------

/// The snap points of one shape. On a pad (`pad`), a filled shape's
/// stroke moves its outline points out by half its width.
fn shape_points(shape: &Shape, pad: bool, add: &mut impl FnMut(Point, SnapKind)) {
    match shape {
        Shape::Line { start, end, .. } => {
            add(*start, SnapKind::LineEnd);
            add(*end, SnapKind::LineEnd);
            add(middle(*start, *end), SnapKind::LineMiddle);
        }
        Shape::Arc {
            start, mid, end, ..
        } => {
            add(*start, SnapKind::ArcEnd);
            add(*end, SnapKind::ArcEnd);
            add(*mid, SnapKind::ArcMiddle);
            if let Some(centre) = circle_center(*start, *mid, *end) {
                add(centre, SnapKind::ArcCentre);
            }
        }
        Shape::Circle {
            center,
            radius,
            width,
            ..
        } => {
            let r = if pad { radius + width / 2.0 } else { *radius };
            add(
                *center,
                if pad {
                    SnapKind::Centre
                } else {
                    SnapKind::CircleCentre
                },
            );
            add(Point::new(center.x, center.y - r), SnapKind::Top);
            add(Point::new(center.x, center.y + r), SnapKind::Bottom);
            add(Point::new(center.x - r, center.y), SnapKind::Left);
            add(Point::new(center.x + r, center.y), SnapKind::Right);
        }
        Shape::Polygon {
            edges,
            width,
            filled,
        } => {
            let grow = if pad && *filled { width / 2.0 } else { 0.0 };
            polygon_points(edges, grow, pad, add);
        }
        Shape::Curve { start, end, .. } => {
            add(*start, SnapKind::CurveEnd);
            add(*end, SnapKind::CurveEnd);
        }
    }
}

/// A polygon's corners, and its arcs' ends, middles and centres; on a pad,
/// also its straight edges' middles. Outline points move out by `grow`.
fn polygon_points(edges: &[Edge], grow: f64, pad: bool, add: &mut impl FnMut(Point, SnapKind)) {
    let outward = Outward::of(edges);
    let count = edges.len();
    for (index, edge) in edges.iter().enumerate() {
        let before = &edges[(index + count - 1) % count];
        // The corner where this edge starts.
        let corner = match (before, edge) {
            (Edge::Line { .. }, Edge::Line { .. }) => SnapKind::Corner,
            _ => SnapKind::ArcEnd,
        };
        let start = edge_start(edge);
        let normal = unit(add_points(
            outward.normal(tangent_at_end(before)),
            outward.normal(tangent_at_start(edge)),
        ));
        add(moved(start, normal, grow), corner);
        match *edge {
            Edge::Line { start, end } if pad => {
                let normal = outward.normal(sub(end, start));
                add(
                    moved(middle(start, end), normal, grow),
                    SnapKind::EdgeMiddle,
                );
            }
            Edge::Line { .. } => {}
            Edge::Arc { start, mid, end } => {
                let normal = outward.normal(arc_tangent(mid, start, mid, end));
                add(moved(mid, normal, grow), SnapKind::ArcMiddle);
                if let Some(centre) = circle_center(start, mid, end) {
                    add(centre, SnapKind::ArcCentre);
                }
            }
        }
    }
}

/// Which side of a polygon's edges is outside.
struct Outward(f64);

impl Outward {
    /// From the polygon's turning direction, by its signed area.
    fn of(edges: &[Edge]) -> Self {
        let mut area = 0.0;
        let mut step = |a: Point, b: Point| area += a.x * b.y - b.x * a.y;
        for edge in edges {
            match *edge {
                Edge::Line { start, end } => step(start, end),
                Edge::Arc { start, mid, end } => {
                    step(start, mid);
                    step(mid, end);
                }
            }
        }
        Self(if area < 0.0 { -1.0 } else { 1.0 })
    }

    /// The unit normal pointing out of the polygon, for an edge running in
    /// direction `tangent`.
    fn normal(&self, tangent: Point) -> Point {
        let t = unit(tangent);
        Point::new(self.0 * t.y, -self.0 * t.x)
    }
}

fn edge_start(edge: &Edge) -> Point {
    match *edge {
        Edge::Line { start, .. } | Edge::Arc { start, .. } => start,
    }
}

fn tangent_at_start(edge: &Edge) -> Point {
    match *edge {
        Edge::Line { start, end } => sub(end, start),
        Edge::Arc { start, mid, end } => arc_tangent(start, start, mid, end),
    }
}

fn tangent_at_end(edge: &Edge) -> Point {
    match *edge {
        Edge::Line { start, end } => sub(end, start),
        Edge::Arc { start, mid, end } => arc_tangent(end, start, mid, end),
    }
}

/// The direction the arc from `start` through `mid` to `end` runs at `at`,
/// one of those three points. At the middle it runs along the chord.
fn arc_tangent(at: Point, start: Point, mid: Point, end: Point) -> Point {
    let Some(centre) = circle_center(start, mid, end) else {
        return sub(end, start);
    };
    let radial = sub(at, centre);
    let across = Point::new(-radial.y, radial.x);
    let along = if at == start {
        sub(mid, start)
    } else if at == end {
        sub(end, mid)
    } else {
        sub(end, start)
    };
    if dot(across, along) < 0.0 {
        Point::new(-across.x, -across.y)
    } else {
        across
    }
}

/// A hole's centre: a circle's centre, or the middle of a slot's box.
fn hole_centre(hole: &Shape) -> Point {
    match hole {
        Shape::Circle { center, .. } => *center,
        other => {
            let points = outline(other);
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &points {
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
            if points.is_empty() {
                Point::default()
            } else {
                Point::new((x0 + x1) / 2.0, (y0 + y1) / 2.0)
            }
        }
    }
}

// ---- Gaps ------------------------------------------------------------------

/// A shape as straight pieces grown by a distance: a filled circle is its
/// centre grown by its radius, a stroke is its line grown by half its
/// width, and a filled polygon also covers its inside.
struct Region {
    /// The line, as points joined in order.
    path: Vec<Point>,
    closed: bool,
    filled: bool,
    grow: f64,
}

impl Region {
    fn of(shape: &Shape) -> Option<Self> {
        let region = match shape {
            Shape::Circle {
                center,
                radius,
                width,
                filled: true,
            } => Self {
                path: vec![*center],
                closed: false,
                filled: false,
                grow: radius + width / 2.0,
            },
            Shape::Circle { width, .. } => Self {
                path: outline(shape),
                closed: true,
                filled: false,
                grow: width / 2.0,
            },
            Shape::Polygon { width, filled, .. } => Self {
                path: outline(shape),
                closed: true,
                filled: *filled,
                grow: width / 2.0,
            },
            Shape::Line { width, .. } | Shape::Arc { width, .. } | Shape::Curve { width, .. } => {
                Self {
                    path: outline(shape),
                    closed: false,
                    filled: false,
                    grow: width / 2.0,
                }
            }
        };
        (!region.path.is_empty()).then_some(region)
    }

    /// The straight pieces, as pairs of points; a single point is a piece
    /// of no length.
    fn pieces(&self) -> impl Iterator<Item = (Point, Point)> + '_ {
        let n = self.path.len();
        let count = match n {
            1 => 1,
            _ if self.closed => n,
            _ => n - 1,
        };
        (0..count).map(move |i| (self.path[i], self.path[(i + 1) % n]))
    }

    fn gap(&self, other: &Region) -> Gap {
        let mut best = (f64::MAX, Point::default(), Point::default());
        for (a0, a1) in self.pieces() {
            for (b0, b1) in other.pieces() {
                let (d, p, q) = segment_distance(a0, a1, b0, b1);
                if d < best.0 {
                    best = (d, p, q);
                }
            }
        }
        let (d, p, q) = best;
        let distance = d - self.grow - other.grow;
        // From each line out to its grown edge, towards the other.
        let toward = unit(sub(q, p));
        let from = moved(p, toward, self.grow);
        let to = moved(q, toward, -other.grow);
        let inside = (self.filled && contains(&self.path, other.path[0]))
            || (other.filled && contains(&other.path, self.path[0]));
        Gap {
            distance: if inside { distance.min(0.0) } else { distance },
            from,
            to,
        }
    }
}

/// A shape's line as points: arcs and curves become short straight pieces.
/// A closed outline does not repeat its first point.
fn outline(shape: &Shape) -> Vec<Point> {
    let mut points = Vec::new();
    match shape {
        Shape::Line { start, end, .. } => points.extend([*start, *end]),
        Shape::Arc {
            start, mid, end, ..
        } => arc_points(&mut points, *start, *mid, *end),
        Shape::Circle { center, radius, .. } => {
            let start = Point::new(center.x + radius, center.y);
            let opposite = Point::new(center.x - radius, center.y);
            arc_points(&mut points, start, opposite, start);
            points.pop();
        }
        Shape::Polygon { edges, .. } => {
            for edge in edges {
                match *edge {
                    Edge::Line { start, .. } => points.push(start),
                    Edge::Arc { start, mid, end } => {
                        arc_points(&mut points, start, mid, end);
                        points.pop();
                    }
                }
            }
        }
        Shape::Curve {
            start,
            control1,
            control2,
            end,
            ..
        } => {
            for i in 0..=CURVE_PIECES {
                let t = i as f64 / CURVE_PIECES as f64;
                let u = 1.0 - t;
                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                points.push(Point::new(
                    a * start.x + b * control1.x + c * control2.x + d * end.x,
                    a * start.y + b * control1.y + c * control2.y + d * end.y,
                ));
            }
        }
    }
    points
}

/// Adds the arc from `start` through `mid` to `end` as points, both ends
/// included, close enough that no piece strays more than
/// [`ARC_TOLERANCE`] from the arc. A whole circle when `start` is `end`.
fn arc_points(points: &mut Vec<Point>, start: Point, mid: Point, end: Point) {
    let whole = distance(start, end) < 1e-9;
    let centre = if whole {
        Some(middle(start, mid))
    } else {
        circle_center(start, mid, end)
    };
    let Some(centre) = centre else {
        points.extend([start, mid, end]);
        return;
    };
    let radius = distance(start, centre);
    let angle = |p: Point| (p.y - centre.y).atan2(p.x - centre.x);
    let tau = std::f64::consts::TAU;
    let (a0, am, a1) = (angle(start), angle(mid), angle(end));
    let sweep = if whole {
        tau
    } else {
        let to_end = (a1 - a0).rem_euclid(tau);
        if (am - a0).rem_euclid(tau) < to_end {
            to_end
        } else {
            to_end - tau
        }
    };
    let step = if radius > ARC_TOLERANCE {
        2.0 * (1.0 - ARC_TOLERANCE / radius).acos()
    } else {
        tau
    };
    // An even count keeps the arc's middle exact, such as an oval's tip.
    let pieces = ((sweep.abs() / step).ceil() as usize).clamp(2, MAX_ARC_PIECES);
    let pieces = pieces + pieces % 2;
    points.push(start);
    for i in 1..pieces {
        let a = a0 + sweep * i as f64 / pieces as f64;
        points.push(Point::new(
            centre.x + radius * a.cos(),
            centre.y + radius * a.sin(),
        ));
    }
    points.push(end);
}

/// Whether `p` is inside the closed outline `path`, by the even-odd rule.
fn contains(path: &[Point], p: Point) -> bool {
    let mut inside = false;
    let n = path.len();
    for i in 0..n {
        let (a, b) = (path[i], path[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// The shortest distance between two straight pieces, and the nearest
/// point on each.
fn segment_distance(a0: Point, a1: Point, b0: Point, b1: Point) -> (f64, Point, Point) {
    if let Some(p) = crossing(a0, a1, b0, b1) {
        return (0.0, p, p);
    }
    let mut best = (f64::MAX, a0, b0);
    for (p, a, b, flip) in [
        (a0, b0, b1, false),
        (a1, b0, b1, false),
        (b0, a0, a1, true),
        (b1, a0, a1, true),
    ] {
        let q = nearest_on(a, b, p);
        let d = distance(p, q);
        if d < best.0 {
            best = if flip { (d, q, p) } else { (d, p, q) };
        }
    }
    best
}

/// Where two straight pieces cross, if they do.
fn crossing(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<Point> {
    let r = sub(a1, a0);
    let s = sub(b1, b0);
    let across = cross(r, s);
    if across.abs() < 1e-18 {
        return None;
    }
    let t = cross(sub(b0, a0), s) / across;
    let u = cross(sub(b0, a0), r) / across;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u))
        .then(|| Point::new(a0.x + t * r.x, a0.y + t * r.y))
}

/// The point on the piece from `a` to `b` nearest `p`.
fn nearest_on(a: Point, b: Point, p: Point) -> Point {
    let ab = sub(b, a);
    let length = dot(ab, ab);
    if length == 0.0 {
        return a;
    }
    let t = (dot(sub(p, a), ab) / length).clamp(0.0, 1.0);
    Point::new(a.x + t * ab.x, a.y + t * ab.y)
}

// ---- Small vector helpers ------------------------------------------------

fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

fn add_points(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}

fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}

fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}

fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn middle(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

/// `p` unit length, or zero.
fn unit(p: Point) -> Point {
    let length = p.x.hypot(p.y);
    if length > 0.0 {
        Point::new(p.x / length, p.y / length)
    } else {
        Point::default()
    }
}

fn moved(p: Point, direction: Point, by: f64) -> Point {
    Point::new(p.x + direction.x * by, p.y + direction.y * by)
}

#[cfg(test)]
mod tests;

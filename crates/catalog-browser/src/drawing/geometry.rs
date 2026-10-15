//! Shapes in footprint millimetres: turning and placing them, rectangles
//! with cut corners, ovals, and the box around shapes.

use super::*;

/// `p` rotated by `angle` degrees counterclockwise on screen (Y down).
pub(super) fn rotate(p: Point, angle: f64) -> Point {
    if angle == 0.0 {
        return p;
    }
    let (sin, cos) = angle.to_radians().sin_cos();
    Point::new(p.x * cos + p.y * sin, -p.x * sin + p.y * cos)
}

/// `shape`, moved by `offset`, then rotated by `angle` and moved to `at`:
/// a pad's local shape placed in the footprint.
pub(super) fn place(shape: &Shape, offset: Point, angle: f64, at: Point) -> Shape {
    let map = |p: Point| rotate(p + offset, angle) + at;
    map_shape(shape, &map)
}

pub(super) fn map_shape(shape: &Shape, map: &dyn Fn(Point) -> Point) -> Shape {
    match shape.clone() {
        Shape::Line { start, end, width } => Shape::Line {
            start: map(start),
            end: map(end),
            width,
        },
        Shape::Arc {
            start,
            mid,
            end,
            width,
        } => Shape::Arc {
            start: map(start),
            mid: map(mid),
            end: map(end),
            width,
        },
        Shape::Circle {
            center,
            radius,
            width,
            filled,
        } => Shape::Circle {
            center: map(center),
            radius,
            width,
            filled,
        },
        Shape::Polygon {
            edges,
            width,
            filled,
        } => Shape::Polygon {
            edges: edges
                .into_iter()
                .map(|edge| match edge {
                    Edge::Line { start, end } => Edge::Line {
                        start: map(start),
                        end: map(end),
                    },
                    Edge::Arc { start, mid, end } => Edge::Arc {
                        start: map(start),
                        mid: map(mid),
                        end: map(end),
                    },
                })
                .collect(),
            width,
            filled,
        },
        Shape::Curve {
            start,
            control1,
            control2,
            end,
            width,
        } => Shape::Curve {
            start: map(start),
            control1: map(control1),
            control2: map(control2),
            end: map(end),
            width,
        },
    }
}

/// A closed polygon through `points`, with straight edges.
pub(super) fn polygon(points: &[Point], width: f64, filled: bool) -> Shape {
    let edges = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(start, end)| Edge::Line {
            start: *start,
            end: *end,
        })
        .collect();
    Shape::Polygon {
        edges,
        width,
        filled,
    }
}

/// How each corner of a rectangle is cut.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Corner {
    Round(f64),
    Chamfer(f64),
}

/// A filled `w` by `h` rectangle centred on the origin, its corners cut as
/// `corners` says (top left, top right, bottom right, bottom left).
pub(super) fn cut_rect(w: f64, h: f64, corners: [Corner; 4]) -> Shape {
    let (hw, hh) = (w / 2.0, h / 2.0);
    // Each corner: its point, and the directions to the previous and
    // next corners, going clockwise on screen from the top left.
    let points = [
        Point::new(-hw, -hh),
        Point::new(hw, -hh),
        Point::new(hw, hh),
        Point::new(-hw, hh),
    ];
    let mut edges = Vec::new();
    let mut previous_end: Option<Point> = None;
    let mut first_start: Option<Point> = None;
    for index in 0..4 {
        let corner = points[index];
        let before = points[(index + 3) % 4];
        let after = points[(index + 1) % 4];
        let toward = |target: Point, length: f64| {
            let full = corner.distance(target);
            let t = if full > 0.0 { length / full } else { 0.0 };
            Point::new(
                corner.x + (target.x - corner.x) * t,
                corner.y + (target.y - corner.y) * t,
            )
        };
        let (start, end, cut) = match corners[index] {
            Corner::Round(r) if r > 0.0 => {
                let (start, end) = (toward(before, r), toward(after, r));
                // Halfway along the quarter circle.
                let center = start + (end - corner);
                let k = std::f64::consts::FRAC_1_SQRT_2;
                let mid = Point::new(
                    center.x + (corner.x - center.x) * k,
                    center.y + (corner.y - center.y) * k,
                );
                (start, end, Some(Edge::Arc { start, mid, end }))
            }
            Corner::Chamfer(c) if c > 0.0 => {
                let (start, end) = (toward(before, c), toward(after, c));
                (start, end, Some(Edge::Line { start, end }))
            }
            _ => (corner, corner, None),
        };
        if let Some(previous) = previous_end {
            if previous != start {
                edges.push(Edge::Line {
                    start: previous,
                    end: start,
                });
            }
        }
        first_start.get_or_insert(start);
        edges.extend(cut);
        previous_end = Some(end);
    }
    if let (Some(last), Some(first)) = (previous_end, first_start) {
        if last != first {
            edges.push(Edge::Line {
                start: last,
                end: first,
            });
        }
    }
    Shape::Polygon {
        edges,
        width: 0.0,
        filled: true,
    }
}

/// A filled stadium (KiCad's oval) `w` by `h`, centred on the origin.
pub(super) fn oval(w: f64, h: f64) -> Shape {
    if (w - h).abs() < 1e-9 {
        return Shape::Circle {
            center: Point::default(),
            radius: w / 2.0,
            width: 0.0,
            filled: true,
        };
    }
    // Two straight sides and two half circles, drawn wide; a tall oval is
    // a wide one turned.
    let (long, r) = (w.max(h), w.min(h) / 2.0);
    let a = long / 2.0 - r;
    let edges = vec![
        Edge::Line {
            start: Point::new(-a, -r),
            end: Point::new(a, -r),
        },
        Edge::Arc {
            start: Point::new(a, -r),
            mid: Point::new(a + r, 0.0),
            end: Point::new(a, r),
        },
        Edge::Line {
            start: Point::new(a, r),
            end: Point::new(-a, r),
        },
        Edge::Arc {
            start: Point::new(-a, r),
            mid: Point::new(-a - r, 0.0),
            end: Point::new(-a, -r),
        },
    ];
    let wide = Shape::Polygon {
        edges,
        width: 0.0,
        filled: true,
    };
    if w >= h {
        wide
    } else {
        place(&wide, Point::default(), 90.0, Point::default())
    }
}

#[derive(Default)]
pub(super) struct Bounds {
    pub(super) box_: Option<[f64; 4]>,
}

impl Bounds {
    pub(super) fn point(&mut self, p: Point, grow: f64) {
        let [x0, y0, x1, y1] = self.box_.get_or_insert([p.x, p.y, p.x, p.y]);
        *x0 = x0.min(p.x - grow);
        *y0 = y0.min(p.y - grow);
        *x1 = x1.max(p.x + grow);
        *y1 = y1.max(p.y + grow);
    }

    pub(super) fn arc(&mut self, start: Point, mid: Point, end: Point, grow: f64) {
        // Enough points along the arc for a box that fits it closely.
        for p in arc_points(start, mid, end, 32) {
            self.point(p, grow);
        }
    }

    pub(super) fn shape(&mut self, shape: &Shape) {
        match shape {
            Shape::Line { start, end, width } => {
                self.point(*start, width / 2.0);
                self.point(*end, width / 2.0);
            }
            Shape::Arc {
                start,
                mid,
                end,
                width,
            } => self.arc(*start, *mid, *end, width / 2.0),
            Shape::Circle {
                center,
                radius,
                width,
                ..
            } => self.point(*center, radius + width / 2.0),
            Shape::Polygon { edges, width, .. } => {
                for edge in edges {
                    match edge {
                        Edge::Line { start, end } => {
                            self.point(*start, width / 2.0);
                            self.point(*end, width / 2.0);
                        }
                        Edge::Arc { start, mid, end } => {
                            self.arc(*start, *mid, *end, width / 2.0);
                        }
                    }
                }
            }
            Shape::Curve {
                start,
                control1,
                control2,
                end,
                width,
            } => {
                // A Bezier curve stays inside its control points.
                for p in [start, control1, control2, end] {
                    self.point(*p, width / 2.0);
                }
            }
        }
    }

    /// Roughly the box a text covers: each letter about 0.8 of its
    /// height wide, placed as it is aligned, then turned.
    pub(super) fn text(&mut self, text: &DrawnText) {
        let width = text.text.chars().count() as f64 * text.height * 0.8;
        let height = text.height * 1.2;
        let span = |align: TextAlign, size: f64| match align {
            TextAlign::Start => (0.0, size),
            TextAlign::Center => (-size / 2.0, size / 2.0),
            TextAlign::End => (-size, 0.0),
        };
        let (mut x0, mut x1) = span(text.horizontal, width);
        if text.mirrored {
            (x0, x1) = (-x1, -x0);
        }
        let (y0, y1) = span(text.vertical, height);
        for corner in [
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ] {
            self.point(text.at + rotate(corner, text.angle), 0.0);
        }
    }

    pub(super) fn finish(self) -> [f64; 4] {
        self.box_.unwrap_or([-1.0, -1.0, 1.0, 1.0])
    }
}

/// A convex, filled polygon's outline moved out by `by` (in, when
/// negative), as a margin moves a pad's outline. Its edges must go
/// clockwise on screen, as [`cut_rect`] and [`polygon`] make them.
///
/// Moving out keeps every edge's length and joins each corner with an arc
/// of radius `by`; an arc edge grows its radius. Moving in works for
/// straight edges only: each corner is where the moved edges meet. An
/// outline that would turn inside out comes back with no edges.
pub(super) fn offset_convex(shape: &Shape, by: f64) -> Shape {
    let Shape::Polygon { edges, .. } = shape else {
        return shape.clone();
    };
    let edges: Vec<Edge> = edges
        .iter()
        .copied()
        .filter(|edge| {
            let (start, end) = ends(edge);
            start.distance(end) > 1e-12
        })
        .collect();
    let edges = merge_straight(edges);
    let moved = if by >= 0.0 {
        offset_out(&edges, by)
    } else {
        offset_in(&edges, -by)
    };
    Shape::Polygon {
        edges: moved,
        width: 0.0,
        filled: true,
    }
}

/// `edges` with each run of straight edges that point the same way, such
/// as a side with a point in its middle, made one edge. Moving in needs
/// it: two such edges never meet.
fn merge_straight(edges: Vec<Edge>) -> Vec<Edge> {
    let same_way = |a: &Edge, b: &Edge| match (*a, *b) {
        (Edge::Line { start: a0, end: a1 }, Edge::Line { start: b0, end: b1 }) => {
            let (u, v) = ((a1 - a0).unit(), (b1 - b0).unit());
            (u.x * v.y - u.y * v.x).abs() < 1e-9 && u.x * v.x + u.y * v.y > 0.0
        }
        _ => false,
    };
    let mut merged: Vec<Edge> = Vec::new();
    for edge in edges {
        match merged.last_mut() {
            Some(last) if same_way(last, &edge) => {
                if let (Edge::Line { end, .. }, Edge::Line { end: next, .. }) = (last, edge) {
                    *end = next;
                }
            }
            _ => merged.push(edge),
        }
    }
    // The last run may go on into the first.
    while merged.len() > 1 && same_way(&merged[merged.len() - 1], &merged[0]) {
        let last = merged.pop().expect("more than one edge");
        if let (Edge::Line { start, .. }, Edge::Line { start: first, .. }) = (last, &mut merged[0])
        {
            *first = start;
        }
    }
    merged
}

fn ends(edge: &Edge) -> (Point, Point) {
    match edge {
        Edge::Line { start, end } | Edge::Arc { start, end, .. } => (*start, *end),
    }
}

/// The outward normals at an edge's start and end.
fn normals(edge: &Edge) -> (Point, Point) {
    match edge {
        Edge::Line { start, end } => {
            let n = Point::new(end.y - start.y, start.x - end.x).unit();
            (n, n)
        }
        Edge::Arc { start, mid, end } => match circle_center(*start, *mid, *end) {
            Some(c) => ((*start - c).unit(), (*end - c).unit()),
            None => normals(&Edge::Line {
                start: *start,
                end: *end,
            }),
        },
    }
}

fn offset_out(edges: &[Edge], by: f64) -> Vec<Edge> {
    let mut out = Vec::new();
    for (index, edge) in edges.iter().enumerate() {
        let (start_normal, end_normal) = normals(edge);
        out.push(match *edge {
            Edge::Line { start, end } => Edge::Line {
                start: start + start_normal * by,
                end: end + end_normal * by,
            },
            Edge::Arc { start, mid, end } => {
                let mid_normal = match circle_center(start, mid, end) {
                    Some(c) => (mid - c).unit(),
                    None => start_normal,
                };
                Edge::Arc {
                    start: start + start_normal * by,
                    mid: mid + mid_normal * by,
                    end: end + end_normal * by,
                }
            }
        });
        let next = &edges[(index + 1) % edges.len()];
        let (next_normal, _) = normals(next);
        if end_normal.distance(next_normal) > 1e-9 {
            let corner = ends(edge).1;
            let middle = (end_normal + next_normal).unit();
            out.push(Edge::Arc {
                start: corner + end_normal * by,
                mid: corner + middle * by,
                end: corner + next_normal * by,
            });
        }
    }
    out
}

fn offset_in(edges: &[Edge], by: f64) -> Vec<Edge> {
    // Each edge's line, moved in: a point on it and its direction.
    let mut lines: Vec<(Point, Point)> = edges
        .iter()
        .map(|edge| {
            let (start, end) = ends(edge);
            let (normal, _) = normals(edge);
            (
                start + normal * -by,
                Point::new(end.x - start.x, end.y - start.y),
            )
        })
        .collect();
    let meet = |(p1, d1): (Point, Point), (p2, d2): (Point, Point)| {
        let cross = d1.x * d2.y - d1.y * d2.x;
        if cross.abs() < 1e-12 {
            return None;
        }
        let t = ((p2.x - p1.x) * d2.y - (p2.y - p1.y) * d2.x) / cross;
        Some(Point::new(p1.x + d1.x * t, p1.y + d1.y * t))
    };
    // A short edge can turn inside out; as in KiCad, it is dropped and its
    // neighbours meet instead, until every edge points the way it did.
    while lines.len() >= 3 {
        let count = lines.len();
        let mut corners = Vec::new();
        for index in 0..count {
            let Some(corner) = meet(lines[(index + count - 1) % count], lines[index]) else {
                return Vec::new();
            };
            corners.push(corner);
        }
        let turned: Vec<usize> = (0..count)
            .filter(|&index| {
                let (start, end) = (corners[index], corners[(index + 1) % count]);
                let direction = lines[index].1;
                (end.x - start.x) * direction.x + (end.y - start.y) * direction.y <= 0.0
            })
            .collect();
        if turned.is_empty() {
            return match polygon(&corners, 0.0, true) {
                Shape::Polygon { edges, .. } => edges,
                _ => Vec::new(),
            };
        }
        if turned.len() == count {
            return Vec::new();
        }
        let mut index = 0;
        lines.retain(|_| {
            index += 1;
            !turned.contains(&(index - 1))
        });
    }
    Vec::new()
}

/// A polygon's edges going clockwise on screen, as [`offset_convex`] needs
/// them: reversed when they go the other way.
pub(super) fn clockwise(edges: &[Edge]) -> Vec<Edge> {
    // Twice the area, positive for clockwise on screen (Y down).
    let twice: f64 = edges
        .iter()
        .map(|edge| {
            let (a, b) = ends(edge);
            a.x * b.y - b.x * a.y
        })
        .sum();
    if twice >= 0.0 {
        return edges.to_vec();
    }
    edges
        .iter()
        .rev()
        .map(|edge| match *edge {
            Edge::Line { start, end } => Edge::Line {
                start: end,
                end: start,
            },
            Edge::Arc { start, mid, end } => Edge::Arc {
                start: end,
                mid,
                end: start,
            },
        })
        .collect()
}

/// Whether clockwise, straight `edges` make a convex polygon: every corner
/// turns the same way.
pub(super) fn convex(edges: &[Edge]) -> bool {
    if edges.len() < 3 || edges.iter().any(|edge| matches!(edge, Edge::Arc { .. })) {
        return false;
    }
    edges
        .iter()
        .zip(edges.iter().cycle().skip(1))
        .all(|(a, b)| {
            let ((a0, a1), (b0, b1)) = (ends(a), ends(b));
            let (u, v) = (
                Point::new(a1.x - a0.x, a1.y - a0.y),
                Point::new(b1.x - b0.x, b1.y - b0.y),
            );
            u.x * v.y - u.y * v.x >= -1e-12
        })
}

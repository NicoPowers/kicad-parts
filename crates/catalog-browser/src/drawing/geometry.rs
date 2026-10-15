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

pub(super) fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}

pub(super) fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// `shape`, moved by `offset`, then rotated by `angle` and moved to `at`:
/// a pad's local shape placed in the footprint.
pub(super) fn place(shape: &Shape, offset: Point, angle: f64, at: Point) -> Shape {
    let map = |p: Point| add(rotate(add(p, offset), angle), at);
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
            let full = distance(corner, target);
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
                let center = add(start, Point::new(end.x - corner.x, end.y - corner.y));
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
            self.point(add(text.at, rotate(corner, text.angle)), 0.0);
        }
    }

    pub(super) fn finish(self) -> [f64; 4] {
        self.box_.unwrap_or([-1.0, -1.0, 1.0, 1.0])
    }
}

/// `count + 1` points along the arc from `start` through `mid` to `end`;
/// just the three points when they lie on a line.
pub(super) fn arc_points(start: Point, mid: Point, end: Point, count: usize) -> Vec<Point> {
    let Some(center) = circle_center(start, mid, end) else {
        return vec![start, mid, end];
    };
    let radius = distance(center, start);
    let angle_of = |p: Point| (p.y - center.y).atan2(p.x - center.x);
    let a0 = angle_of(start);
    let tau = std::f64::consts::TAU;
    let positive = |a: f64| a.rem_euclid(tau);
    // Sweep from start to end, the way that passes through mid.
    let to_mid = positive(angle_of(mid) - a0);
    let to_end = positive(angle_of(end) - a0);
    let sweep = if to_mid <= to_end {
        if to_end == 0.0 { tau } else { to_end }
    } else {
        to_end - tau
    };
    (0..=count)
        .map(|i| {
            let a = a0 + sweep * i as f64 / count as f64;
            Point::new(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect()
}

use super::*;
use crate::drawing::{LayerShape, PadKind};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn lines(points: &[Point], width: f64) -> Shape {
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
        filled: true,
    }
}

fn rect(cx: f64, cy: f64, w: f64, h: f64) -> Shape {
    let (hw, hh) = (w / 2.0, h / 2.0);
    lines(
        &[
            p(cx - hw, cy - hh),
            p(cx + hw, cy - hh),
            p(cx + hw, cy + hh),
            p(cx - hw, cy + hh),
        ],
        0.0,
    )
}

/// A rectangle with rounded corners of radius `r`, clockwise on screen.
fn rounded(cx: f64, cy: f64, w: f64, h: f64, r: f64) -> Shape {
    let (x0, y0, x1, y1) = (cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
    let k = r * (1.0 - std::f64::consts::FRAC_1_SQRT_2);
    let edges = vec![
        Edge::Line {
            start: p(x0 + r, y0),
            end: p(x1 - r, y0),
        },
        Edge::Arc {
            start: p(x1 - r, y0),
            mid: p(x1 - k, y0 + k),
            end: p(x1, y0 + r),
        },
        Edge::Line {
            start: p(x1, y0 + r),
            end: p(x1, y1 - r),
        },
        Edge::Arc {
            start: p(x1, y1 - r),
            mid: p(x1 - k, y1 - k),
            end: p(x1 - r, y1),
        },
        Edge::Line {
            start: p(x1 - r, y1),
            end: p(x0 + r, y1),
        },
        Edge::Arc {
            start: p(x0 + r, y1),
            mid: p(x0 + k, y1 - k),
            end: p(x0, y1 - r),
        },
        Edge::Line {
            start: p(x0, y1 - r),
            end: p(x0, y0 + r),
        },
        Edge::Arc {
            start: p(x0, y0 + r),
            mid: p(x0 + k, y0 + k),
            end: p(x0 + r, y0),
        },
    ];
    Shape::Polygon {
        edges,
        width: 0.0,
        filled: true,
    }
}

/// A wide oval: two straight sides and two half circles.
fn oval(cx: f64, cy: f64, w: f64, h: f64) -> Shape {
    let (r, a) = (h / 2.0, w / 2.0 - h / 2.0);
    let edges = vec![
        Edge::Line {
            start: p(cx - a, cy - r),
            end: p(cx + a, cy - r),
        },
        Edge::Arc {
            start: p(cx + a, cy - r),
            mid: p(cx + a + r, cy),
            end: p(cx + a, cy + r),
        },
        Edge::Line {
            start: p(cx + a, cy + r),
            end: p(cx - a, cy + r),
        },
        Edge::Arc {
            start: p(cx - a, cy + r),
            mid: p(cx - a - r, cy),
            end: p(cx - a, cy - r),
        },
    ];
    Shape::Polygon {
        edges,
        width: 0.0,
        filled: true,
    }
}

fn circle(cx: f64, cy: f64, r: f64) -> Shape {
    Shape::Circle {
        center: p(cx, cy),
        radius: r,
        width: 0.0,
        filled: true,
    }
}

fn pad(number: &str, center: Point, layers: &[(&str, Shape)]) -> DrawnPad {
    DrawnPad {
        number: number.to_owned(),
        kind: PadKind::Smd,
        center,
        shapes: layers
            .iter()
            .map(|(layer, shape)| LayerShape {
                layer: (*layer).to_owned(),
                shape: shape.clone(),
            })
            .collect(),
        hole: None,
    }
}

fn drawing(pads: Vec<DrawnPad>, graphics: Vec<(&str, Shape)>) -> FootprintDrawing {
    FootprintDrawing {
        pads,
        graphics: graphics
            .into_iter()
            .map(|(layer, shape)| LayerShape {
                layer: layer.to_owned(),
                shape,
            })
            .collect(),
        ..FootprintDrawing::default()
    }
}

fn names(drawing: &FootprintDrawing, layer: &str) -> Vec<(String, f64, f64)> {
    snap_points(drawing, layer)
        .iter()
        .map(|point| (point.name(drawing), point.at.x, point.at.y))
        .collect()
}

fn has(points: &[(String, f64, f64)], name: &str, x: f64, y: f64) -> bool {
    points
        .iter()
        .any(|(n, px, py)| n == name && close(*px, x) && close(*py, y))
}

fn gap(drawing: &FootprintDrawing, layer: &str) -> f64 {
    pad_gap(drawing, 0, 1, layer)
        .expect("both pads on the layer")
        .distance
}

#[test]
fn a_rectangular_pad_snaps_to_its_centre_corners_and_edge_middles() {
    let d = drawing(
        vec![pad("3", p(1.0, 0.0), &[("F.Cu", rect(1.0, 0.0, 2.0, 1.0))])],
        vec![],
    );
    let points = names(&d, "F.Cu");
    assert_eq!(points.len(), 9, "{points:?}");
    assert!(has(&points, "Pad 3 centre", 1.0, 0.0));
    assert!(has(&points, "Pad 3 corner", 0.0, -0.5));
    assert!(has(&points, "Pad 3 corner", 2.0, 0.5));
    assert!(has(&points, "Pad 3 edge middle", 1.0, -0.5));
    assert!(has(&points, "Pad 3 edge middle", 2.0, 0.0));
    // Reading order: top row left to right first.
    assert_eq!(points[0].0, "Pad 3 corner");
    assert!(close(points[0].1, 0.0) && close(points[0].2, -0.5));
    assert_eq!(points[1].0, "Pad 3 edge middle");
    // Nothing on another layer.
    assert!(snap_points(&d, "F.Paste").is_empty());
}

#[test]
fn rounded_circular_and_oval_pads_snap_to_their_arcs() {
    let d = drawing(
        vec![
            pad(
                "1",
                p(0.0, 0.0),
                &[("F.Cu", rounded(0.0, 0.0, 2.0, 2.0, 0.5))],
            ),
            pad("", p(5.0, 0.0), &[("F.Cu", circle(5.0, 0.0, 0.5))]),
            pad("2", p(10.0, 0.0), &[("F.Cu", oval(10.0, 0.0, 3.0, 1.0))]),
        ],
        vec![],
    );
    let points = names(&d, "F.Cu");
    let k = 0.5 * (1.0 - std::f64::consts::FRAC_1_SQRT_2);
    assert!(has(&points, "Pad 1 arc end", 0.5, -1.0));
    assert!(has(&points, "Pad 1 arc middle", 1.0 - k, -1.0 + k));
    assert!(has(&points, "Pad 1 arc centre", 0.5, -0.5));
    assert!(has(&points, "Pad 1 edge middle", 1.0, 0.0));
    // The circle's centre is the pad's centre, named once.
    assert!(has(&points, "Unnumbered pad centre", 5.0, 0.0));
    assert!(
        !points
            .iter()
            .any(|(n, ..)| n == "Unnumbered pad circle centre")
    );
    assert!(has(&points, "Unnumbered pad top", 5.0, -0.5));
    assert!(has(&points, "Unnumbered pad right", 5.5, 0.0));
    assert!(has(&points, "Pad 2 arc middle", 11.5, 0.0));
    assert!(has(&points, "Pad 2 arc centre", 11.0, 0.0));
    assert!(has(&points, "Pad 2 arc end", 9.0, -0.5));
}

#[test]
fn a_drilled_pad_snaps_to_its_hole_on_copper_only() {
    let mut drilled = pad(
        "1",
        p(0.0, 0.0),
        &[
            ("F.Cu", circle(0.0, 0.0, 1.0)),
            ("F.Mask", circle(0.0, 0.0, 1.1)),
        ],
    );
    drilled.kind = PadKind::ThruHole;
    drilled.hole = Some(Shape::Circle {
        center: p(0.2, 0.0),
        radius: 0.4,
        width: 0.0,
        filled: true,
    });
    let d = drawing(vec![drilled], vec![]);
    assert!(has(&names(&d, "F.Cu"), "Pad 1 hole centre", 0.2, 0.0));
    // Back copper has no pad shape here, but the hole goes through it.
    assert_eq!(
        names(&d, "B.Cu"),
        [("Pad 1 hole centre".to_owned(), 0.2, 0.0)]
    );
    assert!(!names(&d, "F.Mask").iter().any(|(n, ..)| n.contains("hole")));
}

#[test]
fn graphics_snap_to_ends_middles_centres_sides_and_corners() {
    let d = drawing(
        vec![],
        vec![
            (
                "F.SilkS",
                Shape::Line {
                    start: p(0.0, 0.0),
                    end: p(2.0, 0.0),
                    width: 0.12,
                },
            ),
            (
                "F.SilkS",
                Shape::Arc {
                    start: p(-1.0, 5.0),
                    mid: p(0.0, 4.0),
                    end: p(1.0, 5.0),
                    width: 0.12,
                },
            ),
            (
                "F.Fab",
                Shape::Circle {
                    center: p(0.0, 0.0),
                    radius: 1.0,
                    width: 0.1,
                    filled: false,
                },
            ),
            (
                "F.CrtYd",
                lines(
                    &[p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)],
                    0.05,
                ),
            ),
            (
                "F.SilkS",
                Shape::Curve {
                    start: p(3.0, 3.0),
                    control1: p(4.0, 2.0),
                    control2: p(5.0, 4.0),
                    end: p(6.0, 3.0),
                    width: 0.1,
                },
            ),
        ],
    );
    let silk = names(&d, "F.SilkS");
    assert!(has(&silk, "Line end", 0.0, 0.0));
    assert!(has(&silk, "Line end", 2.0, 0.0));
    assert!(has(&silk, "Line middle", 1.0, 0.0));
    assert!(has(&silk, "Arc end", -1.0, 5.0));
    assert!(has(&silk, "Arc middle", 0.0, 4.0));
    assert!(has(&silk, "Arc centre", 0.0, 5.0));
    assert!(has(&silk, "Curve end", 6.0, 3.0));
    let fab = names(&d, "F.Fab");
    assert!(has(&fab, "Circle centre", 0.0, 0.0));
    // A graphic's circle is snapped on the line the file draws.
    assert!(has(&fab, "Circle top", 0.0, -1.0));
    assert!(has(&fab, "Circle left", -1.0, 0.0));
    let courtyard = names(&d, "F.CrtYd");
    assert_eq!(courtyard.len(), 4, "corners only: {courtyard:?}");
    assert!(has(&courtyard, "Corner", 1.0, -1.0));
}

#[test]
fn lines_that_meet_give_one_point() {
    let line = |a: Point, b: Point| Shape::Line {
        start: a,
        end: b,
        width: 0.1,
    };
    let d = drawing(
        vec![],
        vec![
            ("F.SilkS", line(p(0.0, 0.0), p(1.0, 0.0))),
            ("F.SilkS", line(p(1.0, 0.0), p(1.0, 1.0))),
        ],
    );
    let points = names(&d, "F.SilkS");
    assert_eq!(points.len(), 5, "{points:?}");
}

#[test]
fn a_widened_stroke_moves_a_pads_outline_points_out() {
    // A custom pad's square primitive, grown 0.1 by a wider stroke.
    let square = lines(
        &[p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)],
        0.2,
    );
    let ring = Shape::Circle {
        center: p(5.0, 0.0),
        radius: 1.0,
        width: 0.2,
        filled: true,
    };
    let d = drawing(
        vec![
            pad("1", p(0.0, 0.0), &[("F.Mask", square)]),
            pad("2", p(5.0, 0.0), &[("F.Mask", ring)]),
        ],
        vec![],
    );
    let points = names(&d, "F.Mask");
    assert!(has(&points, "Pad 1 edge middle", 1.1, 0.0), "{points:?}");
    assert!(has(&points, "Pad 1 edge middle", 0.0, -1.1));
    let out = 1.0 + 0.1 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(has(&points, "Pad 1 corner", out, out));
    assert!(has(&points, "Pad 2 left", 3.9, 0.0));
    // The gap counts the strokes: 5 - 1.1 - 1.1.
    assert!(close(gap(&d, "F.Mask"), 2.8), "{}", gap(&d, "F.Mask"));
}

#[test]
fn the_nearest_snap_point_within_the_radius_wins() {
    let points = [
        SnapPoint {
            at: p(0.0, 0.0),
            kind: SnapKind::Centre,
            pad: Some(0),
        },
        SnapPoint {
            at: p(1.0, 0.0),
            kind: SnapKind::Corner,
            pad: Some(0),
        },
        SnapPoint {
            at: p(1.0, 0.0),
            kind: SnapKind::LineEnd,
            pad: None,
        },
    ];
    assert_eq!(nearest_snap(&points, p(0.2, 0.0), 0.5), Some(0));
    assert_eq!(nearest_snap(&points, p(0.8, 0.1), 0.5), Some(1));
    // Equally near: the first, which is the pad's.
    assert_eq!(nearest_snap(&points, p(1.0, 0.0), 0.5), Some(1));
    assert_eq!(nearest_snap(&points, p(0.5, 2.0), 0.5), None);
    assert_eq!(
        nearest_snap(&points, p(0.0, 0.5), 0.5),
        Some(0),
        "on the radius"
    );
    assert_eq!(nearest_snap(&[], p(0.0, 0.0), 1.0), None);
}

#[test]
fn gaps_between_rectangles_rounded_rectangles_circles_and_ovals() {
    let pair = |a: Shape, b: Shape| {
        drawing(
            vec![
                pad("1", p(0.0, 0.0), &[("F.Cu", a)]),
                pad("2", p(0.0, 0.0), &[("F.Cu", b)]),
            ],
            vec![],
        )
    };
    // Side by side: 0.825 * 2 - 0.8.
    let d = pair(rect(-0.825, 0.0, 0.8, 0.95), rect(0.825, 0.0, 0.8, 0.95));
    let g = pad_gap(&d, 0, 1, "F.Cu").unwrap();
    assert!(close(g.distance, 0.85), "{g:?}");
    assert!(close(g.from.x, -0.425) && close(g.to.x, 0.425), "{g:?}");
    // Facing edges: the gap line runs across their middle.
    assert!(close(g.from.y, 0.0) && close(g.to.y, 0.0), "{g:?}");
    // Corner to corner, diagonally: the rounding counts.
    let d = pair(rect(0.0, 0.0, 1.0, 1.0), rect(2.0, 2.0, 1.0, 1.0));
    assert!(close(gap(&d, "F.Cu"), 2f64.sqrt()));
    let d = pair(
        rounded(0.0, 0.0, 1.0, 1.0, 0.25),
        rounded(2.0, 2.0, 1.0, 1.0, 0.25),
    );
    let centres = (1.5f64 * 1.5 * 2.0).sqrt();
    let expected = centres - 2.0 * 0.25;
    assert!(
        (gap(&d, "F.Cu") - expected).abs() < 1e-4,
        "{}",
        gap(&d, "F.Cu")
    );
    // Circles, exactly.
    let d = pair(circle(0.0, 0.0, 0.5), circle(3.0, 4.0, 1.0));
    assert!(close(gap(&d, "F.Cu"), 3.5));
    // Ovals end to end, along their round ends.
    let d = pair(oval(0.0, 0.0, 3.0, 1.0), oval(4.0, 0.0, 3.0, 1.0));
    assert!(close(gap(&d, "F.Cu"), 1.0));
    // An oval and a circle, across the oval's curved end at an angle.
    let d = pair(oval(0.0, 0.0, 3.0, 1.0), circle(3.0, 2.0, 0.5));
    let expected = (2.0f64 * 2.0 + 2.0 * 2.0).sqrt() - 1.0;
    assert!(
        (gap(&d, "F.Cu") - expected).abs() < 1e-4,
        "{}",
        gap(&d, "F.Cu")
    );
}

#[test]
fn a_rotated_pad_measures_along_its_turned_edges() {
    // A 1 by 1 square turned 45 degrees: its corner points right.
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let diamond = lines(&[p(0.0, -h), p(h, 0.0), p(0.0, h), p(-h, 0.0)], 0.0);
    let d = drawing(
        vec![
            pad("1", p(0.0, 0.0), &[("F.Cu", diamond)]),
            pad("2", p(2.0, 0.0), &[("F.Cu", rect(2.0, 0.0, 1.0, 1.0))]),
        ],
        vec![],
    );
    assert!(close(gap(&d, "F.Cu"), 1.5 - h));
    assert!(has(&names(&d, "F.Cu"), "Pad 1 corner", h, 0.0));
}

#[test]
fn touching_and_overlapping_pads_have_no_gap() {
    let pair = |a: Shape, b: Shape| {
        drawing(
            vec![
                pad("1", p(0.0, 0.0), &[("F.Cu", a)]),
                pad("1", p(0.0, 0.0), &[("F.Cu", b)]),
            ],
            vec![],
        )
    };
    assert!(
        gap(
            &pair(rect(0.0, 0.0, 1.0, 1.0), rect(1.0, 0.0, 1.0, 1.0)),
            "F.Cu"
        ) <= 1e-12
    );
    assert!(
        gap(
            &pair(rect(0.0, 0.0, 1.0, 1.0), rect(0.5, 0.5, 1.0, 1.0)),
            "F.Cu"
        ) <= 0.0
    );
    // One inside the other, far from its edges.
    assert!(
        gap(
            &pair(rect(0.0, 0.0, 4.0, 4.0), circle(0.0, 0.0, 0.2)),
            "F.Cu"
        ) <= 0.0
    );
    assert!(
        gap(
            &pair(circle(0.0, 0.0, 0.2), rect(0.0, 0.0, 4.0, 4.0)),
            "F.Cu"
        ) <= 0.0
    );
    // A pad with nothing on the layer has no gap.
    let d = pair(rect(0.0, 0.0, 1.0, 1.0), rect(3.0, 0.0, 1.0, 1.0));
    assert_eq!(pad_gap(&d, 0, 1, "F.Paste"), None);
    assert_eq!(pad_gap(&d, 0, 7, "F.Cu"), None);
}

#[test]
fn paste_and_copper_gaps_differ_for_the_same_pads() {
    // Copper 0.8 wide; paste shrunk by 0.05 on each side.
    let d = drawing(
        vec![
            pad(
                "1",
                p(-0.825, 0.0),
                &[
                    ("F.Cu", rect(-0.825, 0.0, 0.8, 0.95)),
                    ("F.Paste", rect(-0.825, 0.0, 0.7, 0.85)),
                ],
            ),
            pad(
                "2",
                p(0.825, 0.0),
                &[
                    ("F.Cu", rect(0.825, 0.0, 0.8, 0.95)),
                    ("F.Paste", rect(0.825, 0.0, 0.7, 0.85)),
                ],
            ),
        ],
        vec![],
    );
    assert!(close(gap(&d, "F.Cu"), 0.85));
    assert!(close(gap(&d, "F.Paste"), 0.95));
}

#[test]
fn pads_are_named_by_number_or_as_unnumbered() {
    let d = drawing(
        vec![pad("A1", p(0.0, 0.0), &[]), pad("", p(0.0, 0.0), &[])],
        vec![],
    );
    assert_eq!(pad_name(&d.pads[0]), "pad A1");
    assert_eq!(pad_name(&d.pads[1]), "an unnumbered pad");
}

/// A 1,924-ball grid has about 10,000 snap points. Finding them is done
/// once per layer, and the nearest one on every pointer move.
#[test]
fn a_big_ball_grid_snaps_quickly() {
    let pads: Vec<DrawnPad> = (0..1924)
        .map(|i| {
            let c = p(f64::from(i % 44), f64::from(i / 44));
            pad(&i.to_string(), c, &[("F.Cu", circle(c.x, c.y, 0.25))])
        })
        .collect();
    let d = drawing(pads, vec![]);
    let started = std::time::Instant::now();
    let points = snap_points(&d, "F.Cu");
    assert_eq!(points.len(), 1924 * 5);
    let found = started.elapsed();
    let started = std::time::Instant::now();
    for step in 0..100 {
        let at = p(f64::from(step) * 0.4, 20.1);
        std::hint::black_box(nearest_snap(&points, at, 0.2));
    }
    let per_move = started.elapsed() / 100;
    // Debug builds are slow; a release build is ten times faster.
    assert!(
        per_move.as_millis() < 5,
        "{per_move:?} per move, {found:?} to find"
    );
}

#[test]
fn a_gap_under_half_a_micrometre_is_touching() {
    let at = |distance: f64| Gap {
        distance,
        from: p(0.0, 0.0),
        to: p(distance, 0.0),
    };
    assert!(at(0.0004).touching());
    assert!(at(-0.1).touching());
    assert!(!at(0.0005).touching());
}

#[test]
fn a_custom_pads_circle_away_from_its_position_is_a_circle_centre() {
    let d = drawing(
        vec![pad(
            "4",
            p(0.0, 0.0),
            &[
                ("F.Cu", rect(0.0, 0.0, 1.0, 1.0)),
                ("F.Cu", circle(1.0, 0.0, 0.3)),
            ],
        )],
        vec![],
    );
    let points = names(&d, "F.Cu");
    assert!(has(&points, "Pad 4 centre", 0.0, 0.0));
    assert!(has(&points, "Pad 4 circle centre", 1.0, 0.0), "{points:?}");
    assert!(!has(&points, "Pad 4 centre", 1.0, 0.0));
}

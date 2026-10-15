use super::*;
use kicad_integration::library_file::footprint_pads;
use std::path::Path;

fn draw(text: &str) -> FootprintDrawing {
    footprint_drawing(text.as_bytes()).expect("a footprint")
}

/// The box around `shapes`, as `[min_x, min_y, max_x, max_y]`.
fn box_of<'a>(shapes: impl IntoIterator<Item = &'a Shape>) -> [f64; 4] {
    let mut bounds = Bounds::default();
    for shape in shapes {
        bounds.shape(shape);
    }
    bounds.finish()
}

fn on<'a>(pad: &'a DrawnPad, layer: &'a str) -> impl Iterator<Item = &'a Shape> + 'a {
    pad.shapes
        .iter()
        .filter(move |shape| shape.layer == layer)
        .map(|shape| &shape.shape)
}

fn size_on(pad: &DrawnPad, layer: &str) -> Option<(f64, f64)> {
    on(pad, layer).next()?;
    let [x0, y0, x1, y1] = box_of(on(pad, layer));
    Some((x1 - x0, y1 - y0))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn assert_size(actual: Option<(f64, f64)>, expected: (f64, f64)) {
    let actual = actual.expect("a shape on the layer");
    assert!(
        close(actual.0, expected.0) && close(actual.1, expected.1),
        "{actual:?} is not {expected:?}"
    );
}

#[test]
fn draws_pads_with_their_number_centre_and_layers() {
    let drawing = draw(
        "(footprint \"R\" (layer \"F.Cu\")
          (pad \"1\" smd roundrect (at -0.825 0) (size 0.8 0.95) (layers \"F.Cu\" \"F.Mask\" \"F.Paste\") (roundrect_rratio 0.25))
          (pad \"2\" smd roundrect (at 0.825 0) (size 0.8 0.95) (layers \"F.Cu\" \"F.Mask\" \"F.Paste\") (roundrect_rratio 0.25)))",
    );
    assert_eq!(drawing.layers, ["F.Cu", "F.Paste", "F.Mask"]);
    assert_eq!(drawing.pads.len(), 2);
    let pad = &drawing.pads[1];
    assert_eq!(pad.number, "2");
    assert_eq!(pad.kind, PadKind::Smd);
    assert_eq!(pad.center, Point::new(0.825, 0.0));
    for layer in ["F.Cu", "F.Mask", "F.Paste"] {
        assert_size(size_on(pad, layer), (0.8, 0.95));
    }
    // A rounded rectangle: four sides and four quarter circles.
    let Some(Shape::Polygon { edges, filled, .. }) = on(pad, "F.Cu").next() else {
        panic!("a polygon");
    };
    assert!(filled);
    assert_eq!(edges.len(), 8);
    assert_eq!(
        edges
            .iter()
            .filter(|edge| matches!(edge, Edge::Arc { .. }))
            .count(),
        4
    );
    // Each edge starts where the one before it ends.
    for (edge, next) in edges.iter().zip(edges.iter().cycle().skip(1)) {
        let end = match edge {
            Edge::Line { end, .. } | Edge::Arc { end, .. } => *end,
        };
        let start = match next {
            Edge::Line { start, .. } | Edge::Arc { start, .. } => *start,
        };
        assert!(distance(end, start) < 1e-9, "{edge:?} then {next:?}");
    }
    let [x0, y0, x1, y1] = drawing.bounds;
    assert!(close(x0, -1.225) && close(x1, 1.225) && close(y0, -0.475) && close(y1, 0.475));
}

#[test]
fn paste_and_mask_follow_the_pad_then_the_footprint() {
    let drawing = draw(
        "(footprint \"F\" (solder_mask_margin 0.05) (solder_paste_margin -0.02) (solder_paste_margin_ratio -0.1)
          (pad \"1\" smd rect (at 0 0) (size 2 1) (layers \"F.Cu\" \"F.Mask\" \"F.Paste\"))
          (pad \"2\" smd rect (at 5 0) (size 2 1) (layers \"F.Cu\" \"F.Mask\" \"F.Paste\")
            (solder_mask_margin -0.1) (solder_paste_margin 0.1))
          (pad \"3\" smd rect (at 10 0) (size 2 1) (layers \"F.Cu\" \"F.Paste\") (solder_paste_margin_ratio -0.6))
          (pad \"4\" smd rect (at 15 0) (size 2 1) (layers \"F.Cu\" \"F.Paste\") (solder_paste_ratio 0.1)))",
    );
    let pads = &drawing.pads;
    // Pad 1 uses the footprint's values: paste -0.02 - 0.1 * size per
    // axis, mask 0.05 all round.
    assert_size(size_on(&pads[0], "F.Cu"), (2.0, 1.0));
    assert_size(
        size_on(&pads[0], "F.Paste"),
        (2.0 - 0.04 - 0.4, 1.0 - 0.04 - 0.2),
    );
    assert_size(size_on(&pads[0], "F.Mask"), (2.1, 1.1));
    // An equal positive margin rounds the corners by the margin.
    let Some(Shape::Polygon { edges, .. }) = on(&pads[0], "F.Mask").next() else {
        panic!("a polygon");
    };
    assert_eq!(edges.len(), 8);
    // Pad 2's own margin, with the footprint's ratio: 0.1 - 0.1 * size.
    assert_size(
        size_on(&pads[1], "F.Paste"),
        (2.0 + 0.2 - 0.4, 1.0 + 0.2 - 0.2),
    );
    assert_size(size_on(&pads[1], "F.Mask"), (1.8, 0.8));
    // A ratio that would shrink past nothing leaves no opening.
    assert!(size_on(&pads[2], "F.Paste").is_none());
    assert!(size_on(&pads[2], "F.Cu").is_some());
    // KiCad 5's solder_paste_ratio, with the footprint's margin.
    assert_size(
        size_on(&pads[3], "F.Paste"),
        (2.0 - 0.04 + 0.4, 1.0 - 0.04 + 0.2),
    );
}

#[test]
fn paste_only_pads_and_wildcard_layers() {
    let drawing = draw(
        "(footprint \"QFN\"
          (pad \"\" smd roundrect (at 0 -0.6) (size 0.5 1) (layers \"F.Paste\") (roundrect_rratio 0.25))
          (pad \"9\" smd rect (at 0 0) (size 1.7 1.7) (layers \"F.Cu\" \"F.Mask\") (solder_paste_margin_ratio -0.2))
          (pad \"1\" thru_hole circle (at 0 3) (size 1.7 1.7) (drill 1) (layers \"*.Cu\" \"*.Mask\"))
          (pad \"2\" thru_hole oval (at 3 3) (size 1.7 2) (drill 1) (layers \"F&B.Cu\" \"B.Mask\")))",
    );
    assert_eq!(
        drawing.layers,
        ["F.Cu", "B.Cu", "F.Paste", "F.Mask", "B.Mask"]
    );
    let aperture = &drawing.pads[0];
    assert_eq!(aperture.number, "");
    assert_eq!(aperture.shapes.len(), 1);
    assert_size(size_on(aperture, "F.Paste"), (0.5, 1.0));
    // No paste layer, so no paste opening, whatever its ratio.
    assert!(size_on(&drawing.pads[1], "F.Paste").is_none());
    let layers: Vec<&str> = drawing.pads[2]
        .shapes
        .iter()
        .map(|shape| shape.layer.as_str())
        .collect();
    assert_eq!(layers, ["F.Cu", "B.Cu", "F.Mask", "B.Mask"]);
    let layers: Vec<&str> = drawing.pads[3]
        .shapes
        .iter()
        .map(|shape| shape.layer.as_str())
        .collect();
    assert_eq!(layers, ["F.Cu", "B.Cu", "B.Mask"]);
}

#[test]
fn rotates_pads_counterclockwise_with_their_drills() {
    let drawing = draw(
        "(footprint \"J\"
          (pad \"1\" smd rect (at 1 2 90) (size 2 1) (layers \"F.Cu\"))
          (pad \"2\" thru_hole oval (at 0 0 90) (size 3 2) (drill oval 2 1 (offset 0.5 0)) (layers \"*.Cu\")))",
    );
    let pad = &drawing.pads[0];
    assert_eq!(pad.center, Point::new(1.0, 2.0));
    assert_size(size_on(pad, "F.Cu"), (1.0, 2.0));
    let [x0, y0, _, _] = box_of(on(pad, "F.Cu"));
    assert!(close(x0, 0.5) && close(y0, 1.0));
    // The drill stays at the pad's position; the shape moves by the
    // offset, turned with the pad: (0.5, 0) turned 90 degrees is (0, -0.5).
    let pad = &drawing.pads[1];
    let [x0, y0, x1, y1] = box_of(on(pad, "F.Cu"));
    assert!(close(x0, -1.0) && close(x1, 1.0), "{x0} {x1}");
    assert!(close(y0, -2.0) && close(y1, 1.0), "{y0} {y1}");
    let hole = pad.hole.as_ref().expect("a hole");
    let [x0, y0, x1, y1] = box_of([hole]);
    assert!(close(x1 - x0, 1.0) && close(y1 - y0, 2.0));
    assert!(close(x0 + x1, 0.0) && close(y0 + y1, 0.0));
}

#[test]
fn draws_trapezoid_chamfered_and_custom_pads() {
    let drawing = draw(
        "(footprint \"D\"
          (pad \"1\" smd trapezoid (at 0 0) (size 3.6 1.7) (rect_delta 0.6 0) (layers \"F.Cu\"))
          (pad \"2\" smd roundrect (at 10 0) (size 5.8 2.1) (layers \"F.Cu\") (roundrect_rratio 0)
            (chamfer_ratio 0.2) (chamfer top_left top_right))
          (pad \"3\" smd custom (at 20 0 180) (size 1 1) (layers \"F.Cu\" \"F.Mask\") (solder_mask_margin 0.1)
            (options (clearance outline) (anchor rect))
            (primitives (gr_poly (pts (xy 0 0) (xy 2 0) (xy 2 1)) (width 0.1) (fill yes)) (gr_line (start 0 0) (end 0 -3) (width 0.2)))))",
    );
    // The left side is taller than the right, as KiCad draws it.
    let Some(Shape::Polygon { edges, .. }) = on(&drawing.pads[0], "F.Cu").next() else {
        panic!("a polygon");
    };
    let starts: Vec<Point> = edges
        .iter()
        .map(|edge| match edge {
            Edge::Line { start, .. } | Edge::Arc { start, .. } => *start,
        })
        .collect();
    assert_eq!(
        starts,
        [
            Point::new(-1.8, -1.15),
            Point::new(1.8, -0.55),
            Point::new(1.8, 0.55),
            Point::new(-1.8, 1.15)
        ]
    );
    // Two chamfered corners: six edges, all straight.
    let Some(Shape::Polygon { edges, .. }) = on(&drawing.pads[1], "F.Cu").next() else {
        panic!("a polygon");
    };
    assert_eq!(edges.len(), 6);
    assert!(edges.iter().all(|edge| matches!(edge, Edge::Line { .. })));
    assert_size(size_on(&drawing.pads[1], "F.Cu"), (5.8, 2.1));
    // The anchor and both primitives, turned 180 degrees round the pad.
    let custom = &drawing.pads[2];
    assert_eq!(on(custom, "F.Cu").count(), 3);
    let [x0, y0, x1, y1] = box_of(on(custom, "F.Cu"));
    assert!(close(x0, 18.0 - 0.05) && close(x1, 20.5), "{x0} {x1}");
    assert!(close(y0, -1.05) && close(y1, 3.1), "{y0} {y1}");
    // The mask opening grows every shape by the margin.
    let [mx0, _, mx1, _] = box_of(on(custom, "F.Mask"));
    assert!(close(mx0, x0 - 0.1) && close(mx1, x1 + 0.1));
}

#[test]
fn draws_graphics_on_their_layers() {
    let drawing = draw(
        "(footprint \"G\"
          (fp_line (start 0 0) (end 1 0) (stroke (width 0.12) (type solid)) (layer \"F.SilkS\"))
          (fp_rect (start -1 -1) (end 1 1) (stroke (width 0.05)) (fill no) (layer \"F.CrtYd\"))
          (fp_circle (center 0 0) (end 0.5 0) (width 0.1) (fill solid) (layer \"F.Fab\"))
          (fp_arc (start -1 0) (mid 0 -1) (end 1 0) (stroke (width 0.1)) (layer \"F.Fab\"))
          (fp_poly (pts (xy 0 0) (arc (start 1 0) (mid 2 1) (end 1 2)) (xy 0 2)) (stroke (width 0)) (fill yes) (layer \"Dwgs.User\"))
          (fp_curve (pts (xy 0 0) (xy 1 1) (xy 2 1) (xy 3 0)) (stroke (width 0.1)) (layer \"User.2\"))
          (fp_line (start 0 0) (end 1 0) (layer \"Edge.Cuts\") (width 0.1))
          (zone (layer \"F.Cu\") (polygon (pts (xy 0 0) (xy 1 0) (xy 1 1)))))",
    );
    assert_eq!(
        drawing.layers,
        [
            "F.SilkS",
            "Dwgs.User",
            "Edge.Cuts",
            "F.CrtYd",
            "F.Fab",
            "User.2"
        ]
    );
    assert_eq!(drawing.graphics.len(), 7);
    assert!(matches!(
        drawing.graphics[1].shape,
        Shape::Polygon { filled: false, .. }
    ));
    assert!(matches!(
        drawing.graphics[2].shape,
        Shape::Circle { radius, filled: true, .. } if close(radius, 0.5)
    ));
    let Shape::Polygon { edges, .. } = &drawing.graphics[4].shape else {
        panic!("a polygon");
    };
    // A line to the arc, the arc, a line on, and the closing line.
    assert_eq!(edges.len(), 4);
    assert!(matches!(edges[1], Edge::Arc { .. }));
    // The arc's top reaches y = -1, less half its width.
    let [_, y0, _, _] = drawing.bounds;
    assert!(close(y0, -1.05), "{y0}");
    assert_eq!(drawing.left_out, ["Rule areas and zones are not drawn."]);
}

#[test]
fn draws_visible_text_with_the_reference_filled_in() {
    let drawing = draw(
        "(footprint \"T\" (layer \"F.Cu\")
          (property \"Reference\" \"REF**\" (at 0 -1.43 0) (layer \"F.SilkS\") (effects (font (size 1 1) (thickness 0.15))))
          (property \"Value\" \"T\" (at 0 1.43 90) (layer \"F.Fab\") (effects (font (size 1.2 1) (thickness 0.2) (bold yes)) (justify left mirror)))
          (property \"Datasheet\" \"x\" (at 0 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
          (property \"Description\" \"y\" (at 0 0 0) (layer \"F.Fab\") (effects (font (size 1 1)) (hide yes)))
          (fp_text user \"${REFERENCE}\" (at 0 0 unlocked) (layer F.Fab) (effects (font (size 0.4 0.4) (thickness 0.06))))
          (fp_text value \"Old\" (at 0 0) (layer F.Fab) hide (effects (font (size 1 1)))))",
    );
    let texts: Vec<&str> = drawing
        .texts
        .iter()
        .map(|text| text.text.as_str())
        .collect();
    assert_eq!(texts, ["REF**", "T", "REF**"]);
    let value = &drawing.texts[1];
    assert_eq!(value.angle, 90.0);
    assert_eq!(value.height, 1.2);
    assert_eq!(value.horizontal, TextAlign::Start);
    assert_eq!(value.vertical, TextAlign::Center);
    assert!(value.mirrored && value.bold && !value.italic);
    assert_eq!(drawing.texts[2].at, Point::new(0.0, 0.0));
}

#[test]
fn hostile_files_are_refused_or_drawn_in_part() {
    assert!(footprint_drawing(b"(kicad_symbol_lib)").is_none());
    assert!(footprint_drawing(b"(footprint").is_none());
    assert!(footprint_drawing(b"").is_none());
    let deep = format!(
        "(footprint \"X\" {}{})",
        "(a ".repeat(10_000),
        ")".repeat(10_000)
    );
    assert!(footprint_drawing(deep.as_bytes()).is_none());
    // Numbers that are not finite or are absurd leave their item out.
    let drawing = draw(
        "(footprint \"X\"
          (pad \"1\" smd rect (at NaN 0) (size 1 1) (layers \"F.Cu\"))
          (pad \"2\" smd rect (at 0 0) (size 1e308 1) (layers \"F.Cu\"))
          (pad \"3\" smd rect (at 0 0 inf) (size 1 1) (layers \"F.Cu\") (roundrect_rratio -5))
          (pad \"4\" smd circle (at 0 0) (size -2 -2) (layers \"F.Cu\"))
          (pad \"5\" smd blob (at 0 0) (size 1 1) (layers \"F.Cu\"))
          (fp_line (start 0 0) (end 1e999 0) (layer \"F.SilkS\"))
          (fp_circle (center 0 0) (end 0 0) (layer \"F.SilkS\"))
          (fp_arc (start 0 0) (mid 1 1) (end 2 2) (layer \"F.SilkS\"))
          (fp_poly (pts) (layer \"F.SilkS\"))
          (fp_curve (pts (xy 0 0)) (layer \"F.SilkS\")))",
    );
    let numbers: Vec<&str> = drawing.pads.iter().map(|pad| pad.number.as_str()).collect();
    assert_eq!(numbers, ["3", "4"]);
    assert_size(size_on(&drawing.pads[1], "F.Cu"), (2.0, 2.0));
    // The arc through three points on a line is still drawn and bounded.
    assert_eq!(drawing.graphics.len(), 2);
    assert!(drawing.bounds.iter().all(|value| value.is_finite()));
    // Too many shapes: some are drawn, and the drawing says so.
    let mut many = String::from("(footprint \"X\"");
    for index in 0..(MAX_SHAPES + 10) {
        many.push_str(&format!(
            "(fp_line (start 0 0) (end {} 1) (layer \"F.SilkS\"))",
            index % 100
        ));
    }
    many.push(')');
    let drawing = draw(&many);
    assert_eq!(drawing.graphics.len(), MAX_SHAPES);
    assert_eq!(drawing.left_out, [TOO_MANY]);
    // An empty footprint still has a box to fit.
    assert_eq!(draw("(footprint \"E\")").bounds, [-1.0, -1.0, 1.0, 1.0]);
}

#[test]
fn arcs_go_the_way_through_their_middle() {
    // Over the top, and the long way round under it.
    let short = arc_points(
        Point::new(-1.0, 0.0),
        Point::new(0.0, -1.0),
        Point::new(1.0, 0.0),
        4,
    );
    assert!(short.iter().all(|p| p.y <= 1e-9), "{short:?}");
    let long = arc_points(
        Point::new(-1.0, 0.0),
        Point::new(0.0, 1.0),
        Point::new(0.0, -1.0),
        8,
    );
    assert!(long.iter().any(|p| p.x > 0.9), "{long:?}");
    assert!(
        circle_center(
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(2.0, 2.0)
        )
        .is_none()
    );
}

/// Every footprint in our libraries: each pad is where the file says, and
/// a rectangle's copper is its size, turned with the pad.
#[test]
fn our_footprints_draw_their_pads_where_the_file_says() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../footprints");
    let mut checked = 0;
    for library in std::fs::read_dir(&root).unwrap() {
        let library = library.unwrap().path();
        for file in std::fs::read_dir(&library).unwrap() {
            let file = file.unwrap().path();
            if file.extension().and_then(|e| e.to_str()) != Some("kicad_mod") {
                continue;
            }
            let bytes = std::fs::read(&file).unwrap();
            let drawing = footprint_drawing(&bytes)
                .unwrap_or_else(|| panic!("{} is not drawn", file.display()));
            let pads = footprint_pads(&bytes);
            assert_eq!(drawing.pads.len(), pads.len(), "{}", file.display());
            for (drawn, pad) in drawing.pads.iter().zip(&pads) {
                assert_eq!(drawn.number, pad.number);
                assert_eq!(drawn.center, Point::new(pad.at.0, pad.at.1));
                let turned = (pad.rotation.rem_euclid(180.0) - 90.0).abs() < 1e-9;
                let expected = if turned {
                    (pad.size.1, pad.size.0)
                } else {
                    (pad.size.0, pad.size.1)
                };
                let square = pad.rotation.rem_euclid(90.0) == 0.0;
                let copper = drawn
                    .shapes
                    .iter()
                    .any(|shape| shape.layer.ends_with(".Cu"));
                if matches!(pad.shape.as_str(), "rect" | "roundrect") && square && copper {
                    let layer = &drawn
                        .shapes
                        .iter()
                        .find(|shape| shape.layer.ends_with(".Cu"))
                        .unwrap()
                        .layer;
                    let size = size_on(drawn, layer).unwrap();
                    assert!(
                        (size.0 - expected.0).abs() < 1e-6 && (size.1 - expected.1).abs() < 1e-6,
                        "{} pad {}: {size:?} is not {expected:?}",
                        file.display(),
                        pad.number
                    );
                }
            }
            checked += 1;
        }
    }
    assert!(checked > 50, "only {checked} footprints");
}

#[test]
fn a_text_counts_in_the_box_as_aligned_and_turned() {
    let drawing = draw(
        "(footprint \"T\" (fp_text user \"ABCDE\" (at 0 0 90) (layer \"F.Fab\") (effects (font (size 1 1)))))",
    );
    // Five letters about 4 mm wide, turned upright, and 1.2 mm high.
    let [x0, y0, x1, y1] = drawing.bounds;
    assert!(close(x0, -0.6) && close(x1, 0.6), "{x0} {x1}");
    assert!(close(y0, -2.0) && close(y1, 2.0), "{y0} {y1}");
    let drawing = draw(
        "(footprint \"T\" (fp_text user \"ABCDE\" (at 1 0) (layer \"F.Fab\") (effects (font (size 1 1)) (justify left top))))",
    );
    assert_eq!(drawing.bounds, [1.0, 0.0, 5.0, 1.2]);
}

#[test]
fn pads_without_copper_get_no_margins() {
    let drawing = draw(
        "(footprint \"F\" (solder_paste_margin -0.05) (solder_mask_margin 0.07)
          (pad \"\" smd rect (at 0 0) (size 0.6 0.9) (layers \"F.Paste\"))
          (pad \"1\" smd rect (at 3 0) (size 2.4 3.1) (layers \"F.Mask\") (solder_mask_margin 0.1))
          (pad \"2\" smd rect (at 6 0) (size 1 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\")))",
    );
    assert_size(size_on(&drawing.pads[0], "F.Paste"), (0.6, 0.9));
    assert_size(size_on(&drawing.pads[1], "F.Mask"), (2.4, 3.1));
    // A pad on copper still gets them.
    assert_size(size_on(&drawing.pads[2], "F.Paste"), (0.9, 0.9));
    assert_size(size_on(&drawing.pads[2], "F.Mask"), (1.14, 1.14));
}

#[test]
fn polygons_without_a_fill_and_shapes_without_a_width_are_filled() {
    let drawing = draw(
        "(module \"Old\" (layer F.Cu)
          (fp_poly (pts (xy 0 0) (xy 1 0) (xy 1 1)) (layer F.SilkS) (width 0.1))
          (fp_rect (start 0 0) (end 1 1) (layer F.Fab) (width 0) (fill none))
          (fp_circle (center 0 0) (end 1 0) (layer F.Fab) (width 0))
          (fp_circle (center 0 0) (end 1 0) (layer F.Fab) (width 0.1))
          (fp_poly (pts (xy 0 0) (xy 1 0) (xy 1 1)) (layer F.Fab) (stroke (width 0.1)) (fill no))
          (fp_rect (start 0 0) (end 1 1) (layer F.Fab) (stroke (width 0.1)) (fill hatch)))",
    );
    let filled: Vec<bool> = drawing
        .graphics
        .iter()
        .map(|graphic| match graphic.shape {
            Shape::Polygon { filled, .. } | Shape::Circle { filled, .. } => filled,
            _ => panic!("{graphic:?}"),
        })
        .collect();
    assert_eq!(filled, [true, true, true, false, false, true]);
    assert_eq!(drawing.left_out, ["Hatched fills are drawn solid."]);
}

/// The radius of each arc edge of a polygon.
fn arc_radii(shape: &Shape) -> Vec<f64> {
    let Shape::Polygon { edges, .. } = shape else {
        panic!("a polygon: {shape:?}");
    };
    edges
        .iter()
        .filter_map(|edge| match *edge {
            Edge::Arc { start, mid, end } => {
                circle_center(start, mid, end).map(|c| distance(c, start))
            }
            Edge::Line { .. } => None,
        })
        .collect()
}

#[test]
fn a_margin_changes_a_rounded_corner_by_the_margin() {
    // LFPAK56's pad 1, with the footprint's paste margin of -0.05.
    let drawing = draw(
        "(footprint \"F\" (solder_paste_margin -0.05) (solder_mask_margin 0.05)
          (pad \"1\" smd roundrect (at 0 0) (size 0.7 1.15) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\") (roundrect_rratio 0.25))
          (pad \"2\" smd roundrect (at 3 0) (size 0.7 1.15) (layers \"F.Cu\" \"F.Paste\") (roundrect_rratio 0.25)
            (solder_paste_margin 0) (solder_paste_margin_ratio -0.1)))",
    );
    let pad = &drawing.pads[0];
    let radius = |layer| arc_radii(on(pad, layer).next().unwrap());
    assert!(radius("F.Cu").iter().all(|r| close(*r, 0.175)));
    assert!(
        radius("F.Paste").iter().all(|r| close(*r, 0.125)),
        "{:?}",
        radius("F.Paste")
    );
    assert!(radius("F.Mask").iter().all(|r| close(*r, 0.225)));
    assert_size(size_on(pad, "F.Paste"), (0.6, 1.05));
    // Different margins (-0.07 and -0.115): the radius changes by the larger.
    let pad = &drawing.pads[1];
    let radii = arc_radii(on(pad, "F.Paste").next().unwrap());
    assert!(radii.iter().all(|r| close(*r, 0.105)), "{radii:?}");
    assert_size(size_on(pad, "F.Paste"), (0.56, 0.92));
}

#[test]
fn trapezoid_and_chamfered_openings_are_true_outlines() {
    let drawing = draw(
        "(footprint \"F\" (solder_mask_margin 0.1) (solder_paste_margin -0.05)
          (pad \"1\" smd trapezoid (at 0 0) (size 3.6 1.7) (rect_delta 0.6 0) (layers \"F.Cu\" \"F.Mask\" \"F.Paste\"))
          (pad \"2\" smd roundrect (at 10 0) (size 5.8 2.1) (layers \"F.Cu\" \"F.Mask\") (roundrect_rratio 0)
            (chamfer_ratio 0.2) (chamfer top_left top_right)))",
    );
    // Four sides moved out, joined by four arcs of the margin's radius.
    let trapezoid = &drawing.pads[0];
    let mask = on(trapezoid, "F.Mask").next().unwrap();
    let Shape::Polygon { width, .. } = mask else {
        panic!()
    };
    assert_eq!(*width, 0.0, "a true outline, not a wide stroke");
    let radii = arc_radii(mask);
    assert_eq!(radii.len(), 4);
    assert!(radii.iter().all(|r| close(*r, 0.1)), "{radii:?}");
    let [x0, _, x1, _] = box_of([mask]);
    assert!(close(x0, -1.9) && close(x1, 1.9), "{x0} {x1}");
    // Moved in by 0.05: straight sides only, each 0.05 inside.
    let paste = on(trapezoid, "F.Paste").next().unwrap();
    assert!(arc_radii(paste).is_empty());
    let [x0, _, x1, _] = box_of([paste]);
    assert!(close(x0, -1.75) && close(x1, 1.75), "{x0} {x1}");
    // Two chamfers and two sharp corners: six sides and six arcs.
    let chamfered = on(&drawing.pads[1], "F.Mask").next().unwrap();
    let radii = arc_radii(chamfered);
    assert_eq!(radii.len(), 6);
    assert!(radii.iter().all(|r| close(*r, 0.1)));
    assert_size(size_on(&drawing.pads[1], "F.Mask"), (6.0, 2.3));
    // Shrinking past nothing leaves no opening.
    let gone = offset_convex(
        &polygon(
            &[
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(1.0, 1.0),
            ],
            0.0,
            true,
        ),
        -1.0,
    );
    assert!(matches!(gone, Shape::Polygon { ref edges, .. } if edges.is_empty()));
}

/// The size of the drawing as the UI receives it.
fn json_size(drawing: &FootprintDrawing) -> usize {
    serde_json::to_vec(drawing).unwrap().len()
}

#[test]
fn hostile_files_send_the_ui_only_a_few_megabytes() {
    let footprint = |items: &str, count: usize| {
        let mut text = String::from("(footprint \"X\"");
        for index in 0..count {
            text.push_str(&items.replace("N", &(index % 1000).to_string()));
        }
        text.push(')');
        text
    };
    for (name, items, count) in [
        (
            "pads with no layers",
            "(pad \"1\" smd rect (at N 0) (size 1 1))",
            200_000,
        ),
        (
            "texts",
            "(fp_text user \"ABCDEFGHIJKLMNOPQRSTUVWXYZ\" (at N 0) (layer \"F.SilkS\"))",
            200_000,
        ),
        (
            "pads on six layers",
            "(pad \"1\" smd rect (at N 0) (size 1 1) (layers \"F.Cu\" \"B.Cu\" \"F.Paste\" \"B.Paste\" \"F.Mask\" \"B.Mask\"))",
            100_000,
        ),
    ] {
        let drawing = draw(&footprint(items, count));
        let size = json_size(&drawing);
        assert!(size < 25_000_000, "{name}: {size} bytes");
        assert_eq!(drawing.left_out, [TOO_MANY], "{name}");
    }
    // A polygon with too many points is left out, and the drawing says so.
    let mut points = String::new();
    for index in 0..(MAX_EDGES + 10) {
        points.push_str(&format!("(xy {} {})", index % 100, index % 7));
    }
    let drawing = draw(&format!(
        "(footprint \"X\" (fp_poly (pts {points}) (layer \"F.SilkS\")) (fp_line (start 0 0) (end 1 1) (layer \"F.SilkS\")))"
    ));
    assert_eq!(drawing.graphics.len(), 1);
    assert_eq!(drawing.left_out, [TOO_MANY]);
}

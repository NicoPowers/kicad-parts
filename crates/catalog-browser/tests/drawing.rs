//! The app's own footprint drawing (kicad-parts-34df), checked against the
//! footprint file and, roughly, against kicad-cli's SVG of it, for real
//! footprints of ours and KiCad's. Runs the installed KiCad 10.

use base64::Engine;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use desktop_contract::drawing::{Edge, FootprintDrawing, Point, Shape};
use desktop_contract::{PreviewKind, PreviewOutcome};
use kicad_integration::SearchLocations;
use kicad_integration::library_file::footprint_pads;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn open_browser(dir: &Path) -> CatalogBrowser {
    CatalogBrowser::open(BrowserConfig {
        catalog_root: repo_root(),
        cache_dir: dir.join("cache"),
        settings_file: dir.join("config").join("settings.json"),
        kicad_locations: SearchLocations::Standard,
        kicad_config_dir: None,
        kicad_documents_dir: None,
    })
}

/// The drawing, the kicad-cli SVG and the footprint file of `reference`.
fn drawn(browser: &CatalogBrowser, reference: &str) -> (FootprintDrawing, String, Vec<u8>) {
    let set = match browser.preview_field("ANY", PreviewKind::Footprint, Some(reference)) {
        PreviewOutcome::Ready(set) => set,
        PreviewOutcome::Problem(problem) => panic!("{reference}: {problem:?}"),
    };
    let image = set.images.into_iter().next().expect("one image");
    let base64 = image
        .data_uri
        .trim_start_matches("data:image/svg+xml;base64,");
    let svg = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .unwrap();
    // The library folder, which holds NAME.kicad_mod.
    let name = reference.split_once(':').unwrap().1;
    let file = std::fs::read(Path::new(&set.library_path).join(format!("{name}.kicad_mod")))
        .expect("the footprint file");
    let key = image
        .drawing_key
        .unwrap_or_else(|| panic!("{reference} has no drawing key"));
    let drawing = browser
        .footprint_drawing(&key)
        .unwrap_or_else(|| panic!("{reference} has no drawing"));
    let view_box = image.view_box.expect("a view box");
    assert!(view_box[2] > 0.0 && view_box[3] > 0.0);
    (drawing, String::from_utf8(svg).unwrap(), file)
}

/// A box being grown to fit points: `[min_x, min_y, max_x, max_y]`.
fn grow(found: &mut Option<[f64; 4]>, x: f64, y: f64, reach: f64) {
    let b = found.get_or_insert([x, y, x, y]);
    b[0] = b[0].min(x - reach);
    b[1] = b[1].min(y - reach);
    b[2] = b[2].max(x + reach);
    b[3] = b[3].max(y + reach);
}

/// What kicad-cli drew in one colour, such as `#C83434` for front copper.
struct SvgLayer {
    /// The box around its filled paths and circles, and its strokes
    /// widened by half their width.
    bounds: [f64; 4],
    /// The area of its filled paths and circles.
    filled_area: f64,
    /// Whether it also drew strokes, whose area is not counted.
    strokes: bool,
}

fn svg_layer(svg: &str, color: &str) -> Option<SvgLayer> {
    let mut found = None;
    let mut filled_area = 0.0;
    let mut strokes = false;
    // The style of each open <g>, innermost last.
    let mut styles: Vec<String> = Vec::new();
    let mut rest = svg;
    while let Some(at) = rest.find('<') {
        rest = &rest[at..];
        let end = rest.find('>').unwrap_or(rest.len() - 1);
        let tag = &rest[..=end];
        rest = &rest[end + 1..];
        let attribute = |name: &str| {
            let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
            Some(tag[start..][..tag[start..].find('"')?].to_owned())
        };
        if tag.starts_with("</g") {
            styles.pop();
            continue;
        }
        if tag.starts_with("<g") {
            styles.push(attribute("style").unwrap_or_default());
            continue;
        }
        let own = attribute("style").unwrap_or_default();
        let style = format!("{};{own}", styles.join(";"));
        let last = |key: &str| {
            style
                .split(';')
                .filter_map(|part| part.trim().strip_prefix(key))
                .next_back()
                .map(|value| value.trim().to_owned())
        };
        let fill = last("fill:").unwrap_or_default();
        let stroke = last("stroke:").unwrap_or_default();
        let width: f64 = last("stroke-width:")
            .and_then(|w| w.parse().ok())
            .unwrap_or(0.0);
        let filled = fill.eq_ignore_ascii_case(color);
        let reach = if filled {
            0.0
        } else if stroke.eq_ignore_ascii_case(color) && width > 0.0 {
            width / 2.0
        } else {
            continue;
        };
        if tag.starts_with("<circle") {
            let number = |name: &str| attribute(name).and_then(|v| v.parse::<f64>().ok());
            if let (Some(cx), Some(cy), Some(r)) = (number("cx"), number("cy"), number("r")) {
                grow(&mut found, cx, cy, r + reach);
                if filled {
                    filled_area += std::f64::consts::PI * r * r;
                }
            }
        } else if tag.starts_with("<path") {
            let d = attribute("d").unwrap_or_default();
            let mut points = Vec::new();
            for pair in d.split_whitespace() {
                let pair = pair.trim_start_matches(['M', 'L']);
                if let Some((x, y)) = pair.split_once(',') {
                    if let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) {
                        grow(&mut found, x, y, reach);
                        points.push((x, y));
                    }
                }
            }
            if filled {
                filled_area += shoelace(&points);
            }
        }
        strokes |= !filled;
    }
    Some(SvgLayer {
        bounds: found?,
        filled_area,
        strokes,
    })
}

/// The area inside a closed outline.
fn shoelace(points: &[(f64, f64)]) -> f64 {
    let twice: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum();
    twice.abs() / 2.0
}

/// Points along the arc from `a` through `m` to `b`, without `a`.
fn arc_points(a: Point, m: Point, b: Point) -> Vec<Point> {
    let d = 2.0 * (a.x * (m.y - b.y) + m.x * (b.y - a.y) + b.x * (a.y - m.y));
    if d.abs() < 1e-12 {
        return vec![m, b];
    }
    let sq = |p: Point| p.x * p.x + p.y * p.y;
    let cx = (sq(a) * (m.y - b.y) + sq(m) * (b.y - a.y) + sq(b) * (a.y - m.y)) / d;
    let cy = (sq(a) * (b.x - m.x) + sq(m) * (a.x - b.x) + sq(b) * (m.x - a.x)) / d;
    let r = (a.x - cx).hypot(a.y - cy);
    let angle = |p: Point| (p.y - cy).atan2(p.x - cx);
    let tau = std::f64::consts::TAU;
    let to_mid = (angle(m) - angle(a)).rem_euclid(tau);
    let to_end = (angle(b) - angle(a)).rem_euclid(tau);
    let sweep = if to_mid <= to_end {
        to_end
    } else {
        to_end - tau
    };
    (1..=256)
        .map(|i| {
            let t = angle(a) + sweep * f64::from(i) / 256.0;
            Point::new(cx + r * t.cos(), cy + r * t.sin())
        })
        .collect()
}

/// The drawing's pad shapes and graphics on `layer`.
fn on_layer<'a>(drawing: &'a FootprintDrawing, layer: &'a str) -> impl Iterator<Item = &'a Shape> {
    drawing
        .pads
        .iter()
        .flat_map(|pad| &pad.shapes)
        .chain(&drawing.graphics)
        .filter(move |shape| shape.layer == layer)
        .map(|shape| &shape.shape)
}

/// The box around the drawing's shapes on `layer`.
fn drawing_box(drawing: &FootprintDrawing, layer: &str) -> Option<[f64; 4]> {
    let mut found = None;
    for shape in on_layer(drawing, layer) {
        match shape {
            Shape::Circle {
                center,
                radius,
                width,
                ..
            } => grow(&mut found, center.x, center.y, radius + width / 2.0),
            Shape::Polygon { edges, width, .. } => {
                for edge in edges {
                    let points = match edge {
                        Edge::Line { start, end } => vec![*start, *end],
                        Edge::Arc { start, mid, end } => vec![*start, *mid, *end],
                    };
                    for p in points {
                        grow(&mut found, p.x, p.y, width / 2.0);
                    }
                }
            }
            Shape::Line { start, end, width } => {
                grow(&mut found, start.x, start.y, width / 2.0);
                grow(&mut found, end.x, end.y, width / 2.0);
            }
            other => panic!("an unexpected pad shape {other:?}"),
        }
    }
    found
}

/// The area of the drawing's filled shapes on `layer` that have no
/// stroke, and whether there are any other shapes there, or overlapping
/// ones.
fn drawing_area(drawing: &FootprintDrawing, layer: &str) -> (f64, bool) {
    let mut area = 0.0;
    // A custom pad's shapes overlap, and kicad-cli draws their union, so
    // their areas do not add up.
    let mut others = drawing.pads.iter().any(|pad| {
        pad.shapes
            .iter()
            .filter(|shape| shape.layer == layer)
            .count()
            > 1
    });
    for shape in on_layer(drawing, layer) {
        match shape {
            Shape::Circle {
                radius,
                width,
                filled: true,
                ..
            } if *width == 0.0 => area += std::f64::consts::PI * radius * radius,
            Shape::Polygon {
                edges,
                width,
                filled: true,
            } if *width == 0.0 => {
                let mut points = Vec::new();
                for edge in edges {
                    match edge {
                        Edge::Line { end, .. } => points.push((end.x, end.y)),
                        Edge::Arc { start, mid, end } => points.extend(
                            arc_points(*start, *mid, *end)
                                .into_iter()
                                .map(|p| (p.x, p.y)),
                        ),
                    }
                }
                area += shoelace(&points);
            }
            _ => others = true,
        }
    }
    (area, others)
}

/// KiCad's built-in colours for the layers compared.
const LAYERS: [(&str, &str); 3] = [
    ("F.Cu", "#C83434"),
    ("F.Paste", "#B4A09A"),
    ("F.Mask", "#D864FF"),
];

/// Checks that each pad is where the file says, and that the pads on each
/// layer cover a box the size of kicad-cli's. kicad-cli crops its SVG to
/// what it drew, so only sizes compare, not positions. Where both drew
/// only plain filled shapes on a layer, their areas add up to the same, so
/// every pad there counts, including paste openings inside others.
fn check(browser: &CatalogBrowser, reference: &str) -> FootprintDrawing {
    let (drawing, svg, file) = drawn(browser, reference);
    let pads = footprint_pads(&file);
    assert_eq!(drawing.pads.len(), pads.len(), "{reference}");
    for (drawn, pad) in drawing.pads.iter().zip(&pads) {
        assert_eq!(drawn.number, pad.number, "{reference}");
        assert_eq!((drawn.center.x, drawn.center.y), pad.at, "{reference}");
    }
    let mut compared_areas = 0;
    for (layer, color) in LAYERS {
        match (drawing_box(&drawing, layer), svg_layer(&svg, color)) {
            (Some(ours), Some(theirs)) => {
                let size = |b: [f64; 4]| (b[2] - b[0], b[3] - b[1]);
                let (our_size, their_size) = (size(ours), size(theirs.bounds));
                // kicad-cli draws arcs as short lines, inside the curve.
                assert!(
                    (our_size.0 - their_size.0).abs() < 0.01
                        && (our_size.1 - their_size.1).abs() < 0.01,
                    "{reference} {layer}: ours {our_size:?}, kicad-cli's {their_size:?}"
                );
                let (area, others) = drawing_area(&drawing, layer);
                if !others && !theirs.strokes {
                    let difference = (area - theirs.filled_area).abs() / area.max(1e-9);
                    assert!(
                        difference < 0.01,
                        "{reference} {layer}: area {area}, kicad-cli's {}",
                        theirs.filled_area
                    );
                    compared_areas += 1;
                }
            }
            (None, None) => {}
            (ours, theirs) => panic!(
                "{reference} {layer}: ours {ours:?}, kicad-cli's {:?}",
                theirs.map(|theirs| theirs.bounds)
            ),
        }
    }
    eprintln!("{reference}: areas compared on {compared_areas} layers");
    drawing
}

#[test]
fn real_footprints_match_their_files_and_kicad_cli() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path());
    for reference in [
        // KiCad's own: a 0603 resistor, an SOIC, a QFN with an exposed pad
        // and paste openings, a connector with oval drills, custom pads,
        // a trapezoid, chamfered corners, and a BGA with a paste ratio.
        "Resistor_SMD:R_0603_1608Metric",
        "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
        "Package_DFN_QFN:QFN-16-1EP_3x3mm_P0.5mm_EP1.7x1.7mm",
        "Package_DFN_QFN:DFN-10-1EP_2x3mm_P0.5mm_EP0.64x2.4mm",
        "Connector_AMASS:AMASS_XT60PW-M_1x02_P7.20mm_Horizontal",
        "Battery:BatteryHolder_Keystone_1057_1x2032",
        "Diode_SMD:D_SMA-SMB_Universal_Handsoldering",
        "Inductor_SMD:L_Bourns_SDR0604",
        "Package_BGA:ST_TFBGA-225_13x13mm_Layout15x15_P0.8mm",
        // Paste openings and a mask opening drawn as pads of their own,
        // next to a footprint margin that must not change them.
        "Package_TO_SOT_SMD:LFPAK56",
        // Custom pads whose paste shrinks, primitives too.
        "Package_DFN_QFN:Texas_RGY_R-PVQFN-N16_EP2.05x2.55mm",
        "Package_DFN_QFN:Texas_RGY_R-PVQFN-N16_EP2.05x2.55mm_ThermalVias",
        "Package_SO:Texas_HTSOP-8-1EP_3.9x4.9mm_P1.27mm_EP2.95x4.9mm_Mask2.4x3.1mm_ThermalVias",
        // Ours.
        "g-Resistor_SMD:R_0603_1608Metric",
        "g-Package_DFN_QFN:RSL0048B-IPC_A",
        "g-Package_SO:TI_SO-PowerPAD-8",
    ] {
        check(&browser, reference);
    }
}

#[test]
fn the_paste_ratio_and_oval_drills_are_kicads() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path());
    // The footprint says solder_paste_ratio 0.0125 and mask margin 0.035:
    // a 0.4 mm ball pad gets 0.41 mm of paste and 0.47 mm of mask.
    let (drawing, _, _) = drawn(
        &browser,
        "Package_BGA:ST_TFBGA-225_13x13mm_Layout15x15_P0.8mm",
    );
    let radius_on = |layer: &str| {
        drawing.pads[0]
            .shapes
            .iter()
            .find(|shape| shape.layer == layer)
            .map(|shape| match shape.shape {
                Shape::Circle { radius, .. } => radius,
                ref other => panic!("{other:?}"),
            })
            .unwrap()
    };
    assert!((radius_on("F.Cu") - 0.2).abs() < 1e-9);
    assert!((radius_on("F.Paste") - 0.205).abs() < 1e-9);
    assert!((radius_on("F.Mask") - 0.235).abs() < 1e-9);
    // Oval drills are slots: two sides and two half circles.
    let (drawing, _, _) = drawn(
        &browser,
        "Connector_AMASS:AMASS_XT60PW-M_1x02_P7.20mm_Horizontal",
    );
    let slot = drawing
        .pads
        .iter()
        .find_map(|pad| {
            pad.hole
                .as_ref()
                .filter(|hole| matches!(hole, Shape::Polygon { .. }))
        })
        .expect("a slot");
    let Shape::Polygon { edges, .. } = slot else {
        unreachable!()
    };
    assert_eq!(edges.len(), 4);
}

#[test]
fn a_bga_with_two_thousand_pads_draws_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path());
    let reference = "Package_BGA:Xilinx_FFG1926_FFG1927_FFG1928_FFG1930";
    drawn(&browser, reference);
    // The second time comes from the cache; the drawing is made again.
    let started = Instant::now();
    let drawing = check(&browser, reference);
    let elapsed = started.elapsed();
    assert!(drawing.pads.len() > 1900);
    let size = serde_json::to_vec(&drawing).unwrap().len();
    assert!(size < 2_000_000, "{size} bytes");
    assert!(elapsed.as_secs_f64() < 5.0, "{elapsed:?}");
}

/// The drawing and kicad-cli's SVG of a footprint file's text, drawn as a
/// chosen file is.
fn drawn_file(browser: &CatalogBrowser, name: &str, text: &str) -> (FootprintDrawing, String) {
    let base64 = base64::engine::general_purpose::STANDARD.encode(text);
    let request: desktop_contract::FilePreviewRequest = serde_json::from_value(serde_json::json!({
        "file": { "kind": "footprint", "file_name": format!("{name}.kicad_mod"), "base64": base64 },
        "reference": format!("g-test:{name}"),
    }))
    .unwrap();
    let set = match browser.preview_file(&request) {
        PreviewOutcome::Ready(set) => set,
        PreviewOutcome::Problem(problem) => panic!("{name}: {problem:?}"),
    };
    let image = set.images.into_iter().next().unwrap();
    let svg = base64::engine::general_purpose::STANDARD
        .decode(
            image
                .data_uri
                .trim_start_matches("data:image/svg+xml;base64,"),
        )
        .unwrap();
    let drawing = browser
        .footprint_drawing(&image.drawing_key.unwrap())
        .unwrap();
    (drawing, String::from_utf8(svg).unwrap())
}

/// Custom pads with shrinking margins, like those the round 3 review
/// measured: the paste opening's box matches kicad-cli's.
#[test]
fn custom_pads_shrink_as_kicad_cli_draws_them() {
    let dir = tempfile::tempdir().unwrap();
    let browser = open_browser(dir.path());
    let pad = |margin: &str, body: &str| {
        format!(
            "(footprint \"P\" (version 20241229) (layer \"F.Cu\") (attr smd)
               (pad \"1\" smd custom (at 0 0) (size {body} (layers \"F.Cu\" \"F.Paste\") {margin}
                 (options (clearance outline) (anchor rect))
                 (primitives PRIMITIVES))
               (fp_line (start -3 -3) (end 3 3) (stroke (width 0.05) (type solid)) (layer \"F.CrtYd\")))"
        )
    };
    for (name, text) in [
        (
            "line",
            pad("(solder_paste_margin -0.05)", "0.4 0.4)").replace(
                "PRIMITIVES",
                "(gr_line (start -0.75 0) (end 0.75 0) (width 0.3))",
            ),
        ),
        (
            "anchor_gone",
            pad("(solder_paste_margin -0.1)", "0.2 0.2)").replace(
                "PRIMITIVES",
                "(gr_poly (pts (xy -0.75 -0.5) (xy 0.75 -0.5) (xy 0.75 0.5) (xy -0.75 0.5)) (width 0) (fill yes))",
            ),
        ),
        (
            "ratio",
            pad("(solder_paste_margin_ratio -0.1)", "1 0.6)").replace(
                "PRIMITIVES",
                "(gr_poly (pts (xy -1 -0.3) (xy 1 -0.3) (xy 1 0.3) (xy -1 0.3)) (width 0) (fill yes))",
            ),
        ),
    ] {
        let (drawing, svg) = drawn_file(&browser, name, &text);
        let ours = drawing_box(&drawing, "F.Paste").unwrap();
        let theirs = svg_layer(&svg, "#B4A09A").unwrap().bounds;
        let size = |b: [f64; 4]| (b[2] - b[0], b[3] - b[1]);
        let (ours, theirs) = (size(ours), size(theirs));
        eprintln!("{name}: ours {ours:?}, kicad-cli {theirs:?}");
        // kicad-cli draws round ends as short lines, inside the curve.
        for (a, b) in [(ours.0, theirs.0), (ours.1, theirs.1)] {
            assert!((a - b).abs() < 0.01, "{name}: ours {ours:?}, kicad-cli {theirs:?}");
        }
    }
}

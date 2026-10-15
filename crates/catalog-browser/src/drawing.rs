//! Draws a footprint's layers from its `.kicad_mod`, for the close-up
//! (kicad-parts-34df). kicad-cli's SVG has no layer groups and is cropped,
//! so the close-up cannot turn layers on and off or show exact positions
//! from it.
//!
//! Paste and mask openings follow KiCad's rules: the pad's value first,
//! then the footprint's, else 0. A board's own settings can change them,
//! and the close-up says so.
//!
//! A downloaded footprint may be hostile: [`Sexp::parse`] refuses deep
//! nesting, numbers that are not finite or are absurdly large are skipped,
//! and the number of shapes and points is limited. Nothing here panics.

use desktop_contract::drawing::{
    DrawnPad, DrawnText, Edge, FootprintDrawing, LayerShape, PadKind, Point, Shape, TextAlign,
};
use kicad_integration::library_file::Sexp;

/// The largest coordinate or size read, in millimetres. A footprint is
/// never 10 m across, so anything larger is a broken or hostile file.
const MAX_MM: f64 = 10_000.0;
/// The most shapes drawn, counting each pad's shape on each layer.
const MAX_SHAPES: usize = 200_000;
/// The most polygon edges drawn, over all polygons.
const MAX_EDGES: usize = 1_000_000;
/// The most characters of one text drawn.
const MAX_TEXT_CHARS: usize = 200;

/// The footprint in `bytes`, drawn layer by layer; `None` when the bytes
/// are not a footprint.
pub fn footprint_drawing(bytes: &[u8]) -> Option<FootprintDrawing> {
    let tree = Sexp::parse(bytes).ok()?;
    if !matches!(tree.head(), Some("footprint" | "module")) {
        return None;
    }
    let mut drawer = Drawer::default();
    let footprint_margins = Margins::read(&tree);
    let (reference, value) = reference_and_value(&tree);
    for item in tree.items() {
        if drawer.full() {
            drawer.note("The footprint has too many shapes; only some are drawn.");
            break;
        }
        match item.head() {
            Some("pad") => drawer.pad(item, &footprint_margins),
            Some(
                head @ ("fp_line" | "fp_rect" | "fp_circle" | "fp_arc" | "fp_poly" | "fp_curve"),
            ) => {
                if let (Some(layer), Some(shape)) = (layer_of(item), graphic(head, item, false)) {
                    drawer.graphic(layer, shape);
                }
            }
            Some("fp_text" | "property") => {
                if let Some(text) = text(item, &reference, &value) {
                    drawer.texts.push(text);
                }
            }
            // Rule areas and copper zones are board features a footprint
            // can carry; the close-up draws the footprint itself.
            Some("zone") => drawer.note("Rule areas and zones are not drawn."),
            Some("fp_text_box" | "textbox") => drawer.note("Text boxes are not drawn."),
            Some("dimension") => drawer.note("Dimensions are not drawn."),
            Some("image") => drawer.note("Pictures are not drawn."),
            _ => {}
        }
    }
    Some(drawer.finish())
}

#[derive(Default)]
struct Drawer {
    pads: Vec<DrawnPad>,
    graphics: Vec<LayerShape>,
    texts: Vec<DrawnText>,
    left_out: Vec<String>,
    shapes: usize,
    edges: usize,
}

impl Drawer {
    fn full(&self) -> bool {
        self.shapes >= MAX_SHAPES || self.edges >= MAX_EDGES
    }

    fn note(&mut self, message: &str) {
        if !self.left_out.iter().any(|known| known == message) {
            self.left_out.push(message.to_owned());
        }
    }

    /// Counts `shape` against the limits; false when it does not fit.
    fn admit(&mut self, shape: &Shape) -> bool {
        let edges = match shape {
            Shape::Polygon { edges, .. } => edges.len(),
            _ => 0,
        };
        if self.shapes + 1 > MAX_SHAPES || self.edges + edges > MAX_EDGES {
            return false;
        }
        self.shapes += 1;
        self.edges += edges;
        true
    }

    fn graphic(&mut self, layer: String, shape: Shape) {
        if self.admit(&shape) {
            self.graphics.push(LayerShape { layer, shape });
        }
    }

    fn pad(&mut self, pad: &Sexp, footprint: &Margins) {
        let Some(geometry) = PadGeometry::read(pad) else {
            return;
        };
        let margins = Margins::read(pad).or(footprint);
        let kind = match pad.word_at(2) {
            Some("thru_hole") => PadKind::ThruHole,
            Some("np_thru_hole") => PadKind::NpThruHole,
            Some("connect") => PadKind::Connect,
            _ => PadKind::Smd,
        };
        let drill = Drill::read(pad);
        let mut shapes = Vec::new();
        for layer in pad_layers(pad) {
            let outline = if layer.ends_with(".Paste") {
                let (dx, dy) = margins.paste(geometry.size);
                geometry.outline(dx, dy)
            } else if layer.ends_with(".Mask") {
                let margin = margins.mask.unwrap_or(0.0);
                geometry.outline(margin, margin)
            } else {
                // KiCad draws no copper for a hole with no pad around it.
                if kind == PadKind::NpThruHole
                    && drill
                        .as_ref()
                        .is_some_and(|drill| drill.covers(geometry.size))
                {
                    continue;
                }
                geometry.outline(0.0, 0.0)
            };
            for shape in outline {
                let shape = place(&shape, drill_offset(&drill), geometry.angle, geometry.at);
                if !self.admit(&shape) {
                    self.note("The footprint has too many shapes; only some are drawn.");
                    break;
                }
                shapes.push(LayerShape {
                    layer: layer.clone(),
                    shape,
                });
            }
        }
        let hole = drill.map(|drill| {
            place(
                &drill.shape(),
                Point::default(),
                geometry.angle,
                geometry.at,
            )
        });
        self.pads.push(DrawnPad {
            number: pad.word_at(1).unwrap_or_default().to_owned(),
            kind,
            center: geometry.at,
            shapes,
            hole,
        });
    }

    fn finish(self) -> FootprintDrawing {
        let mut layers: Vec<String> = Vec::new();
        let mut bounds = Bounds::default();
        let mut add_layer = |layer: &str| {
            if !layers.iter().any(|known| known == layer) {
                layers.push(layer.to_owned());
            }
        };
        for pad in &self.pads {
            for shape in &pad.shapes {
                add_layer(&shape.layer);
                bounds.shape(&shape.shape);
            }
            if let Some(hole) = &pad.hole {
                bounds.shape(hole);
            }
        }
        for graphic in &self.graphics {
            add_layer(&graphic.layer);
            bounds.shape(&graphic.shape);
        }
        for text in &self.texts {
            add_layer(&text.layer);
            bounds.text(text);
        }
        layers.sort_by_key(|layer| layer_rank(layer));
        FootprintDrawing {
            layers,
            bounds: bounds.finish(),
            pads: self.pads,
            graphics: self.graphics,
            texts: self.texts,
            left_out: self.left_out,
        }
    }
}

// ---- Reading numbers and points --------------------------------------------

/// A finite number no larger than [`MAX_MM`].
fn number(word: Option<&str>) -> Option<f64> {
    word?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && value.abs() <= MAX_MM)
}

fn child<'a>(node: &'a Sexp, head: &'a str) -> Option<&'a Sexp> {
    node.children(head).next()
}

/// The two numbers of `(head x y)`.
fn pair(node: &Sexp, head: &str) -> Option<(f64, f64)> {
    let item = child(node, head)?;
    Some((number(item.word_at(1))?, number(item.word_at(2))?))
}

fn point(node: &Sexp, head: &str) -> Option<Point> {
    pair(node, head).map(|(x, y)| Point::new(x, y))
}

/// `(at x y [angle])`: the position and the angle (0 when not a number,
/// as in `(at 0 0 unlocked)`).
fn at(node: &Sexp) -> Option<(Point, f64)> {
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
fn stroke_width(node: &Sexp) -> f64 {
    child(node, "stroke")
        .and_then(|stroke| child(stroke, "width"))
        .or_else(|| child(node, "width"))
        .and_then(|width| number(width.word_at(1)))
        .map_or(0.0, f64::abs)
}

/// `(fill yes|solid)` fills; `(fill no|none)` does not; with no `fill`,
/// `default`.
fn filled(node: &Sexp, default: bool) -> bool {
    match child(node, "fill").and_then(|fill| fill.word_at(1)) {
        Some("yes" | "solid") => true,
        Some(_) => false,
        None => default,
    }
}

fn layer_of(node: &Sexp) -> Option<String> {
    child(node, "layer")
        .and_then(|layer| layer.word_at(1))
        .map(str::to_owned)
}

/// Whether a text item is hidden: a bare `hide`, `(hide)` or `(hide yes)`
/// on the item or in its `effects`.
fn hidden(node: &Sexp) -> bool {
    let says_hide = |node: &Sexp| {
        node.items().iter().skip(1).any(|item| match item {
            Sexp::Word(word) => word == "hide",
            Sexp::List(_) => item.head() == Some("hide") && item.word_at(1) != Some("no"),
        })
    };
    says_hide(node) || child(node, "effects").is_some_and(says_hide)
}

// ---- Geometry ----------------------------------------------------------------

/// `p` rotated by `angle` degrees counterclockwise on screen (Y down).
fn rotate(p: Point, angle: f64) -> Point {
    if angle == 0.0 {
        return p;
    }
    let (sin, cos) = angle.to_radians().sin_cos();
    Point::new(p.x * cos + p.y * sin, -p.x * sin + p.y * cos)
}

fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}

fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// `shape`, moved by `offset`, then rotated by `angle` and moved to `at`:
/// a pad's local shape placed in the footprint.
fn place(shape: &Shape, offset: Point, angle: f64, at: Point) -> Shape {
    let map = |p: Point| add(rotate(add(p, offset), angle), at);
    map_shape(shape, &map)
}

fn map_shape(shape: &Shape, map: &dyn Fn(Point) -> Point) -> Shape {
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
fn polygon(points: &[Point], width: f64, filled: bool) -> Shape {
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
enum Corner {
    Round(f64),
    Chamfer(f64),
}

/// A filled `w` by `h` rectangle centred on the origin, its corners cut as
/// `corners` says (top left, top right, bottom right, bottom left).
fn cut_rect(w: f64, h: f64, corners: [Corner; 4]) -> Shape {
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
fn oval(w: f64, h: f64) -> Shape {
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

// ---- Pads ----------------------------------------------------------------

/// A pad's own shape, before margins, in its own unrotated frame.
struct PadGeometry {
    at: Point,
    angle: f64,
    size: (f64, f64),
    form: PadForm,
}

enum PadForm {
    Circle,
    Rect,
    Oval,
    Trapezoid {
        delta: (f64, f64),
    },
    /// A rounded and maybe chamfered rectangle: corner radius and chamfer
    /// as ratios of the shorter side, and which corners are chamfered (top
    /// left, top right, bottom right, bottom left).
    RoundRect {
        radius_ratio: f64,
        chamfer_ratio: f64,
        chamfered: [bool; 4],
    },
    /// The anchor (a circle or a rectangle of the pad's size) and the
    /// primitives drawn with it.
    Custom {
        circle_anchor: bool,
        primitives: Vec<Shape>,
    },
}

impl PadGeometry {
    fn read(pad: &Sexp) -> Option<Self> {
        let (at, angle) = at(pad)?;
        let (w, h) = pair(pad, "size")?;
        let size = (w.abs(), h.abs());
        let ratio = |head: &str| {
            child(pad, head)
                .and_then(|item| number(item.word_at(1)))
                .map_or(0.0, |ratio| ratio.clamp(0.0, 0.5))
        };
        let form = match pad.word_at(3)? {
            "circle" => PadForm::Circle,
            "rect" => PadForm::Rect,
            "oval" => PadForm::Oval,
            "trapezoid" => PadForm::Trapezoid {
                delta: pair(pad, "rect_delta").unwrap_or((0.0, 0.0)),
            },
            "roundrect" => {
                let corners = child(pad, "chamfer");
                let chamfered =
                    ["top_left", "top_right", "bottom_right", "bottom_left"].map(|name| {
                        corners.is_some_and(|corners| {
                            corners.items().iter().any(|item| item.word() == Some(name))
                        })
                    });
                PadForm::RoundRect {
                    radius_ratio: ratio("roundrect_rratio"),
                    chamfer_ratio: ratio("chamfer_ratio"),
                    chamfered,
                }
            }
            "custom" => {
                let circle_anchor = child(pad, "options")
                    .and_then(|options| child(options, "anchor"))
                    .and_then(|anchor| anchor.word_at(1))
                    == Some("circle");
                let primitives = child(pad, "primitives")
                    .map(|primitives| {
                        primitives
                            .items()
                            .iter()
                            .filter_map(|item| graphic(item.head()?, item, true))
                            .collect()
                    })
                    .unwrap_or_default();
                PadForm::Custom {
                    circle_anchor,
                    primitives,
                }
            }
            _ => return None,
        };
        Some(Self {
            at,
            angle,
            size,
            form,
        })
    }

    /// The pad's outline grown by `dx` and `dy` on each side along its own
    /// axes (shrunk when negative), in its own frame. Empty when it shrinks
    /// to nothing.
    ///
    /// As in KiCad, an equal positive margin rounds the corners by the
    /// margin, and anything else resizes the pad, keeping its corner ratios.
    fn outline(&self, dx: f64, dy: f64) -> Vec<Shape> {
        let (w, h) = self.size;
        let inflate = dx == dy && dx > 0.0;
        let (w2, h2) = if inflate {
            (w, h)
        } else {
            (w + 2.0 * dx, h + 2.0 * dy)
        };
        if w2 <= 0.0 || h2 <= 0.0 {
            return Vec::new();
        }
        let grow = if inflate { dx } else { 0.0 };
        let shapes = match &self.form {
            PadForm::Circle => vec![Shape::Circle {
                center: Point::default(),
                radius: w2 / 2.0 + grow,
                width: 0.0,
                filled: true,
            }],
            PadForm::Oval => vec![oval(w2 + 2.0 * grow, h2 + 2.0 * grow)],
            PadForm::Rect => vec![cut_rect(
                w2 + 2.0 * grow,
                h2 + 2.0 * grow,
                [Corner::Round(grow); 4],
            )],
            PadForm::RoundRect {
                radius_ratio,
                chamfer_ratio,
                chamfered,
            } if !chamfered.contains(&true) || *chamfer_ratio == 0.0 => {
                let radius = radius_ratio * w2.min(h2) + grow;
                vec![cut_rect(
                    w2 + 2.0 * grow,
                    h2 + 2.0 * grow,
                    [Corner::Round(radius); 4],
                )]
            }
            PadForm::RoundRect {
                radius_ratio,
                chamfer_ratio,
                chamfered,
            } => {
                let short = w2.min(h2);
                let corners = chamfered.map(|chamfer| {
                    if chamfer {
                        Corner::Chamfer(chamfer_ratio * short)
                    } else {
                        Corner::Round(radius_ratio * short)
                    }
                });
                vec![stroked(cut_rect(w2, h2, corners), grow)]
            }
            PadForm::Trapezoid { delta } => {
                let (hw, hh) = (w2 / 2.0, h2 / 2.0);
                // KiCad's corners: delta.x changes the left and right
                // sides' heights, delta.y the top and bottom widths.
                let (ddx, ddy) = (
                    (delta.0 / 2.0).clamp(-hh, hh),
                    (delta.1 / 2.0).clamp(-hw, hw),
                );
                let points = [
                    Point::new(-hw + ddy, -hh - ddx),
                    Point::new(hw - ddy, -hh + ddx),
                    Point::new(hw + ddy, hh - ddx),
                    Point::new(-hw - ddy, hh + ddx),
                ];
                vec![stroked(polygon(&points, 0.0, true), grow)]
            }
            PadForm::Custom {
                circle_anchor,
                primitives,
            } => {
                let anchor = if *circle_anchor {
                    Shape::Circle {
                        center: Point::default(),
                        radius: w2 / 2.0,
                        width: 0.0,
                        filled: true,
                    }
                } else {
                    polygon(
                        &[
                            Point::new(-w2 / 2.0, -h2 / 2.0),
                            Point::new(w2 / 2.0, -h2 / 2.0),
                            Point::new(w2 / 2.0, h2 / 2.0),
                            Point::new(-w2 / 2.0, h2 / 2.0),
                        ],
                        0.0,
                        true,
                    )
                };
                // A negative margin shrinks only the anchor; the primitives
                // keep their own size.
                std::iter::once(anchor)
                    .chain(primitives.iter().cloned())
                    .map(|shape| stroked(shape, grow))
                    .collect()
            }
        };
        shapes
    }
}

/// `shape` grown by `grow` on every side, with rounded corners, by
/// widening its stroke.
fn stroked(shape: Shape, grow: f64) -> Shape {
    if grow <= 0.0 {
        return shape;
    }
    let widen = |width: f64| width + 2.0 * grow;
    match shape {
        Shape::Line { start, end, width } => Shape::Line {
            start,
            end,
            width: widen(width),
        },
        Shape::Arc {
            start,
            mid,
            end,
            width,
        } => Shape::Arc {
            start,
            mid,
            end,
            width: widen(width),
        },
        Shape::Circle {
            center,
            radius,
            width,
            filled,
        } => Shape::Circle {
            center,
            radius,
            width: widen(width),
            filled,
        },
        Shape::Polygon {
            edges,
            width,
            filled,
        } => Shape::Polygon {
            edges,
            width: widen(width),
            filled,
        },
        Shape::Curve {
            start,
            control1,
            control2,
            end,
            width,
        } => Shape::Curve {
            start,
            control1,
            control2,
            end,
            width: widen(width),
        },
    }
}

/// A pad's or footprint's own paste and mask settings.
#[derive(Clone, Copy, Default)]
struct Margins {
    mask: Option<f64>,
    paste: Option<f64>,
    paste_ratio: Option<f64>,
}

impl Margins {
    fn read(node: &Sexp) -> Self {
        let value = |head: &str| child(node, head).and_then(|item| number(item.word_at(1)));
        Self {
            mask: value("solder_mask_margin"),
            paste: value("solder_paste_margin"),
            // KiCad 5 called it solder_paste_ratio.
            paste_ratio: value("solder_paste_margin_ratio")
                .or_else(|| value("solder_paste_ratio"))
                .map(|ratio| ratio.clamp(-1.0, 1.0)),
        }
    }

    /// Each value of `self`, or else `other`'s.
    fn or(self, other: &Self) -> Self {
        Self {
            mask: self.mask.or(other.mask),
            paste: self.paste.or(other.paste),
            paste_ratio: self.paste_ratio.or(other.paste_ratio),
        }
    }

    /// The paste margin along each axis of a pad of `size`: the margin
    /// plus the ratio times the size, never shrinking past nothing.
    fn paste(&self, (w, h): (f64, f64)) -> (f64, f64) {
        let margin = self.paste.unwrap_or(0.0);
        let ratio = self.paste_ratio.unwrap_or(0.0);
        (
            (margin + ratio * w).max(-w / 2.0),
            (margin + ratio * h).max(-h / 2.0),
        )
    }
}

/// A pad's layers, with `*.Cu` and `F&B.Cu` style names spelled out.
fn pad_layers(pad: &Sexp) -> Vec<String> {
    let mut layers: Vec<String> = Vec::new();
    let Some(list) = child(pad, "layers") else {
        return layers;
    };
    for name in list.items().iter().skip(1).filter_map(Sexp::word) {
        let both = name
            .strip_prefix("*.")
            .or_else(|| name.strip_prefix("F&B."));
        let names = match both {
            Some(rest) => vec![format!("F.{rest}"), format!("B.{rest}")],
            None => vec![name.to_owned()],
        };
        for name in names {
            if !layers.contains(&name) {
                layers.push(name);
            }
        }
    }
    layers
}

/// A pad's drill: its size, and how far the pad's shape is offset from it.
struct Drill {
    size: (f64, f64),
    oval: bool,
    offset: Point,
}

impl Drill {
    fn read(pad: &Sexp) -> Option<Self> {
        let drill = child(pad, "drill")?;
        let oval = drill.word_at(1) == Some("oval");
        let numbers: Vec<f64> = drill
            .items()
            .iter()
            .skip(1)
            .filter_map(|item| number(item.word()))
            .map(f64::abs)
            .collect();
        let offset = point(drill, "offset").unwrap_or_default();
        let size = match numbers.as_slice() {
            [] => (0.0, 0.0),
            [d] => (*d, *d),
            [w, h, ..] if oval => (*w, *h),
            [d, ..] => (*d, *d),
        };
        Some(Self { size, oval, offset })
    }

    /// Whether the hole is at least as large as a pad of `size`.
    fn covers(&self, (w, h): (f64, f64)) -> bool {
        w <= self.size.0 + 1e-6 && h <= self.size.1 + 1e-6
    }

    fn shape(&self) -> Shape {
        let (w, h) = self.size;
        if self.oval {
            oval(w, h)
        } else {
            Shape::Circle {
                center: Point::default(),
                radius: w / 2.0,
                width: 0.0,
                filled: true,
            }
        }
    }
}

/// The offset a drill gives its pad's shapes. KiCad keeps the pad's
/// position at the hole and moves the shape.
fn drill_offset(drill: &Option<Drill>) -> Point {
    drill.as_ref().map(|drill| drill.offset).unwrap_or_default()
}

// ---- Graphics ------------------------------------------------------------

/// A footprint graphic (`fp_line` and the like) or a custom pad's
/// primitive (`gr_line` and the like), by its kind without the prefix.
/// Primitives are filled unless they say otherwise.
fn graphic(head: &str, node: &Sexp, primitive: bool) -> Option<Shape> {
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
fn old_arc(node: &Sexp, center: Point, from: Point, width: f64) -> Option<Shape> {
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
fn poly_edges(pts: &Sexp) -> Option<Vec<Edge>> {
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

// ---- Text ----------------------------------------------------------------

/// The footprint's reference and value, for `${REFERENCE}` and
/// `${VALUE}`.
fn reference_and_value(tree: &Sexp) -> (String, String) {
    let mut reference = "REF**".to_owned();
    let mut value = String::new();
    for item in tree.items() {
        let (name, text) = match item.head() {
            Some("property") => (item.word_at(1), item.word_at(2)),
            Some("fp_text") => (item.word_at(1), item.word_at(2)),
            _ => continue,
        };
        match (name, text) {
            (Some("Reference" | "reference"), Some(text)) => reference = text.to_owned(),
            (Some("Value" | "value"), Some(text)) => value = text.to_owned(),
            _ => {}
        }
    }
    (reference, value)
}

/// A visible `(fp_text KIND "TEXT" ...)` or `(property "NAME" "VALUE" ...)`.
fn text(node: &Sexp, reference: &str, value: &str) -> Option<DrawnText> {
    if hidden(node) {
        return None;
    }
    let layer = layer_of(node)?;
    let raw = node.word_at(2)?;
    let shown = raw
        .replace("${REFERENCE}", reference)
        .replace("${VALUE}", value);
    let shown: String = shown.chars().take(MAX_TEXT_CHARS).collect();
    if shown.trim().is_empty() {
        return None;
    }
    let (at, angle) = at(node)?;
    let effects = child(node, "effects");
    let font = effects.and_then(|effects| child(effects, "font"));
    let (height, thickness) = font
        .map(|font| {
            let height = pair(font, "size").map_or(1.0, |(h, _)| h.abs());
            let thickness = child(font, "thickness")
                .and_then(|item| number(item.word_at(1)))
                .map_or(height * 0.15, f64::abs);
            (height, thickness)
        })
        .unwrap_or((1.0, 0.15));
    let says = |node: Option<&Sexp>, word: &str| {
        node.is_some_and(|node| {
            node.items().iter().any(|item| item.word() == Some(word))
                || child(node, word).is_some_and(|item| item.word_at(1) != Some("no"))
        })
    };
    let justify = effects.and_then(|effects| child(effects, "justify"));
    let horizontal = if says(justify, "left") {
        TextAlign::Start
    } else if says(justify, "right") {
        TextAlign::End
    } else {
        TextAlign::Center
    };
    let vertical = if says(justify, "top") {
        TextAlign::Start
    } else if says(justify, "bottom") {
        TextAlign::End
    } else {
        TextAlign::Center
    };
    Some(DrawnText {
        layer,
        text: shown,
        at,
        angle,
        height,
        thickness,
        horizontal,
        vertical,
        mirrored: says(justify, "mirror"),
        bold: says(font, "bold"),
        italic: says(font, "italic"),
    })
}

// ---- Layers and bounds -----------------------------------------------------

/// KiCad's order of the technical layers, as its layer list shows them
/// after the copper layers.
const LAYER_ORDER: [&str; 18] = [
    "F.Adhes",
    "B.Adhes",
    "F.Paste",
    "B.Paste",
    "F.SilkS",
    "B.SilkS",
    "F.Mask",
    "B.Mask",
    "Dwgs.User",
    "Cmts.User",
    "Eco1.User",
    "Eco2.User",
    "Edge.Cuts",
    "Margin",
    "F.CrtYd",
    "B.CrtYd",
    "F.Fab",
    "B.Fab",
];

/// Where `layer` sorts: front, inner and back copper first, then
/// [`LAYER_ORDER`], then `User.N` in number order, then anything else by
/// name.
fn layer_rank(layer: &str) -> (u32, u32, String) {
    let numbered = |prefix: &str, suffix: &str| {
        layer
            .strip_prefix(prefix)?
            .strip_suffix(suffix)?
            .parse::<u32>()
            .ok()
    };
    if layer == "F.Cu" {
        return (0, 0, String::new());
    }
    if let Some(n) = numbered("In", ".Cu") {
        return (1, n, String::new());
    }
    if layer == "B.Cu" {
        return (2, 0, String::new());
    }
    if let Some(index) = LAYER_ORDER.iter().position(|known| *known == layer) {
        return (3, index as u32, String::new());
    }
    if let Some(n) = numbered("User.", "") {
        return (4, n, String::new());
    }
    (5, 0, layer.to_owned())
}

#[derive(Default)]
struct Bounds {
    box_: Option<[f64; 4]>,
}

impl Bounds {
    fn point(&mut self, p: Point, grow: f64) {
        let [x0, y0, x1, y1] = self.box_.get_or_insert([p.x, p.y, p.x, p.y]);
        *x0 = x0.min(p.x - grow);
        *y0 = y0.min(p.y - grow);
        *x1 = x1.max(p.x + grow);
        *y1 = y1.max(p.y + grow);
    }

    fn arc(&mut self, start: Point, mid: Point, end: Point, grow: f64) {
        // Enough points along the arc for a box that fits it closely.
        for p in arc_points(start, mid, end, 32) {
            self.point(p, grow);
        }
    }

    fn shape(&mut self, shape: &Shape) {
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

    /// Roughly the box a text covers, ignoring its angle.
    fn text(&mut self, text: &DrawnText) {
        let half_width = text.text.chars().count() as f64 * text.height * 0.4;
        let reach = half_width.max(text.height);
        self.point(text.at, reach);
    }

    fn finish(self) -> [f64; 4] {
        self.box_.unwrap_or([-1.0, -1.0, 1.0, 1.0])
    }
}

/// `count + 1` points along the arc from `start` through `mid` to `end`;
/// just the three points when they lie on a line.
fn arc_points(start: Point, mid: Point, end: Point, count: usize) -> Vec<Point> {
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

/// The centre of the circle through three points; `None` when they lie on
/// a line.
fn circle_center(a: Point, b: Point, c: Point) -> Option<Point> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let sq = |p: Point| p.x * p.x + p.y * p.y;
    let x = (sq(a) * (b.y - c.y) + sq(b) * (c.y - a.y) + sq(c) * (a.y - b.y)) / d;
    let y = (sq(a) * (c.x - b.x) + sq(b) * (a.x - c.x) + sq(c) * (b.x - a.x)) / d;
    (x.is_finite() && y.is_finite()).then_some(Point::new(x, y))
}

#[cfg(test)]
mod tests;

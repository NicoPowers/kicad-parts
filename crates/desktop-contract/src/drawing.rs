//! A footprint drawn by the app itself from its `.kicad_mod`, for the
//! close-up's layers (kicad-parts-34df).
//!
//! Every coordinate is in footprint millimetres, with the footprint's origin
//! at (0, 0). Y points down, as in KiCad and SVG. Angles are in degrees,
//! counterclockwise on screen, as KiCad writes them.
//!
//! Shapes keep their exact geometry (arc points, circle centres, polygon
//! corners), so a tool can find the points it snaps to and the gap between
//! two pads on one layer without the drawing changing.

use serde::{Deserialize, Serialize};

/// One footprint, layer by layer.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct FootprintDrawing {
    /// Every layer something is drawn on, in KiCad's layer order, such as
    /// `F.Cu`, `F.Paste`, `F.SilkS`.
    pub layers: Vec<String>,
    /// The box around everything drawn, which a close-up fits:
    /// `[min_x, min_y, max_x, max_y]`. A text's box is a rough guess.
    pub bounds: [f64; 4],
    pub pads: Vec<DrawnPad>,
    /// Lines, arcs, circles, rectangles, polygons and curves that are not
    /// pads, each on its layer.
    pub graphics: Vec<LayerShape>,
    pub texts: Vec<DrawnText>,
    /// What the drawing leaves out, in plain words, such as rule areas.
    pub left_out: Vec<String>,
}

/// One pad.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DrawnPad {
    /// Blank for an unnumbered pad, such as a paste opening or a
    /// mechanical hole.
    pub number: String,
    pub kind: PadKind,
    /// The pad's position.
    pub center: Point,
    /// The pad's outline on each layer it is on. Copper layers get the
    /// pad's own shape; paste and mask layers get the opening, after the
    /// pad's and footprint's margins. A custom pad has several shapes per
    /// layer: its anchor and its primitives.
    pub shapes: Vec<LayerShape>,
    /// The drilled hole, if the pad has one: a circle, or a slot drawn as
    /// a polygon.
    pub hole: Option<Shape>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PadKind {
    Smd,
    ThruHole,
    NpThruHole,
    Connect,
}

/// A shape on one layer.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LayerShape {
    pub layer: String,
    pub shape: Shape,
}

/// A point in footprint millimetres.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// One shape. `width` is the stroke width: a stroke is centred on the
/// outline, so a filled shape with a width reaches `width / 2` beyond its
/// outline, with rounded corners, as in KiCad.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    Line {
        start: Point,
        end: Point,
        width: f64,
    },
    /// The arc from `start` through `mid` to `end`.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
        width: f64,
    },
    Circle {
        center: Point,
        radius: f64,
        width: f64,
        filled: bool,
    },
    /// A closed outline of lines and arcs, each edge starting where the
    /// one before it ends.
    Polygon {
        edges: Vec<Edge>,
        width: f64,
        filled: bool,
    },
    /// A cubic Bezier curve.
    Curve {
        start: Point,
        control1: Point,
        control2: Point,
        end: Point,
        width: f64,
    },
}

/// One edge of a [`Shape::Polygon`].
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Edge {
    Line {
        start: Point,
        end: Point,
    },
    /// The arc from `start` through `mid` to `end`; `mid` is halfway
    /// along it.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
    },
}

/// A text item that KiCad shows: a visible property, such as the
/// reference, or a footprint text.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DrawnText {
    pub layer: String,
    /// The text as KiCad shows it in the footprint editor, with
    /// `${REFERENCE}` and `${VALUE}` filled in.
    pub text: String,
    pub at: Point,
    /// Counterclockwise on screen, in degrees.
    pub angle: f64,
    /// The height of a capital letter.
    pub height: f64,
    pub thickness: f64,
    pub horizontal: TextAlign,
    pub vertical: TextAlign,
    /// Drawn mirrored, as text on the back of a board is.
    pub mirrored: bool,
    pub bold: bool,
    pub italic: bool,
}

/// Where text sits around its position: `Start` is left or top, `End` is
/// right or bottom.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Start,
    #[default]
    Center,
    End,
}

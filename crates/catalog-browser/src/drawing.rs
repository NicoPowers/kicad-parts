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
    circle_center,
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
        let layers = pad_layers(pad);
        // KiCad gives margins only to pads on copper: a paste or mask
        // opening drawn as a pad of its own is exactly its size.
        let margins = if layers.iter().any(|layer| layer.ends_with(".Cu")) {
            Margins::read(pad).or(footprint)
        } else {
            Margins::default()
        };
        let kind = match pad.word_at(2) {
            Some("thru_hole") => PadKind::ThruHole,
            Some("np_thru_hole") => PadKind::NpThruHole,
            Some("connect") => PadKind::Connect,
            _ => PadKind::Smd,
        };
        let drill = Drill::read(pad);
        let mut shapes = Vec::new();
        for layer in layers {
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
        // Texts count too, as in KiCad's zoom to fit, so a close-up that
        // fits the drawing shows all of it.
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

mod geometry;
mod graphics;
mod pads;
mod read;
mod text;

use geometry::*;
use graphics::*;
use pads::*;
use read::*;
use text::*;

#[cfg(test)]
mod tests;

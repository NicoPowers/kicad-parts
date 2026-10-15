//! SVG markup for a footprint drawing, and KiCad's layer colours and
//! drawing order.

use super::*;

/// KiCad's built-in colour for `layer`, its opacity, and a plain
/// description.
pub(crate) fn layer_look(layer: &str) -> (&'static str, f64, String) {
    let inner = |n: u32| {
        // KiCad's first inner copper colours, repeated.
        const INNER: [&str; 8] = [
            "#7FC87F", "#CE7D2C", "#4FCBCB", "#DB628B", "#A7A5C6", "#28CCD9", "#E8B2A7", "#8DCB81",
        ];
        INNER[(n.saturating_sub(1) % 8) as usize]
    };
    let (color, alpha, what) = match layer {
        "F.Cu" => ("#C83434", 1.0, "Front copper"),
        "B.Cu" => ("#4D7FC4", 1.0, "Back copper"),
        "F.Paste" => ("#B4A09A", 0.9, "Front paste (stencil)"),
        "B.Paste" => ("#00C2C2", 0.9, "Back paste (stencil)"),
        "F.Mask" => ("#D864FF", 0.4, "Front solder mask"),
        "B.Mask" => ("#02FFEE", 0.4, "Back solder mask"),
        "F.SilkS" => ("#F2EDA1", 1.0, "Front silkscreen"),
        "B.SilkS" => ("#E8B2A7", 1.0, "Back silkscreen"),
        "F.Fab" => ("#AFAFAF", 1.0, "Front fabrication"),
        "B.Fab" => ("#585D84", 1.0, "Back fabrication"),
        "F.CrtYd" => ("#FF26E2", 1.0, "Front courtyard"),
        "B.CrtYd" => ("#26E9FF", 1.0, "Back courtyard"),
        "F.Adhes" => ("#840084", 1.0, "Front adhesive"),
        "B.Adhes" => ("#000084", 1.0, "Back adhesive"),
        "Edge.Cuts" => ("#D0D2CD", 1.0, "Board edge"),
        "Margin" => ("#FF26E2", 1.0, "Margin"),
        "Dwgs.User" => ("#C2C2C2", 1.0, "User drawings"),
        "Cmts.User" => ("#5994DC", 1.0, "User comments"),
        "Eco1.User" => ("#B4DBD2", 1.0, "User Eco 1"),
        "Eco2.User" => ("#D8C852", 1.0, "User Eco 2"),
        _ => {
            let number = |prefix: &str, suffix: &str| {
                layer
                    .strip_prefix(prefix)?
                    .strip_suffix(suffix)?
                    .parse::<u32>()
                    .ok()
            };
            if let Some(n) = number("In", ".Cu") {
                return (inner(n), 1.0, format!("Inner copper {n}"));
            }
            if let Some(n) = number("User.", "") {
                return ("#C2C2C2", 1.0, format!("User layer {n}"));
            }
            return ("#C2C2C2", 1.0, "Other layer".to_owned());
        }
    };
    (color, alpha, what.to_owned())
}

/// The order layers are drawn in, bottom first: the back of the board,
/// inner copper, user layers, then the front, with the active layer on top.
pub(super) fn draw_order(layers: &[String], active: &str) -> Vec<String> {
    let rank = |layer: &str| -> (u32, u32) {
        const BACK: [&str; 7] = [
            "B.Cu", "B.Mask", "B.Paste", "B.Adhes", "B.SilkS", "B.Fab", "B.CrtYd",
        ];
        const FRONT: [&str; 8] = [
            "F.Cu",
            "F.Mask",
            "F.Paste",
            "F.Adhes",
            "F.SilkS",
            "F.Fab",
            "F.CrtYd",
            "Edge.Cuts",
        ];
        if let Some(at) = BACK.iter().position(|known| *known == layer) {
            (0, at as u32)
        } else if layer.starts_with("In") && layer.ends_with(".Cu") {
            (1, 0)
        } else if let Some(at) = FRONT.iter().position(|known| *known == layer) {
            (3, at as u32)
        } else {
            (2, 0)
        }
    };
    let mut order: Vec<String> = layers
        .iter()
        .filter(|layer| *layer != active)
        .cloned()
        .collect();
    order.sort_by_key(|layer| rank(layer));
    if layers.iter().any(|layer| layer == active) {
        order.push(active.to_owned());
    }
    order
}

/// The layer that starts active: front copper, else the first copper
/// layer, else the first layer.
pub(super) fn first_active(layers: &[String]) -> String {
    layers
        .iter()
        .find(|layer| *layer == "F.Cu")
        .or_else(|| layers.iter().find(|layer| layer.ends_with(".Cu")))
        .or_else(|| layers.first())
        .cloned()
        .unwrap_or_default()
}

// ---- SVG markup ----------------------------------------------------------

/// A number for SVG: at most six decimals, without trailing zeros.
pub(super) fn number(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    match text {
        "-0" | "" => "0".to_owned(),
        text => text.to_owned(),
    }
}

pub(super) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// SVG path commands for the arc from `start` (already reached) through
/// `mid` to `end`: an exact elliptical arc, so it stays smooth at any zoom.
pub(super) fn arc_commands(out: &mut String, start: Point, mid: Point, end: Point) {
    let p = |q: Point| format!("{} {}", number(q.x), number(q.y));
    let same = start.distance(end) < 1e-9;
    if same {
        // A whole circle: two half circles through the opposite point.
        let r = number((start.x - mid.x).hypot(start.y - mid.y) / 2.0);
        let _ = write!(
            out,
            " A {r} {r} 0 0 1 {} A {r} {r} 0 0 1 {}",
            p(mid),
            p(start)
        );
        return;
    }
    let Some(turn) = arc_turn(start, mid, end) else {
        let _ = write!(out, " L {} L {}", p(mid), p(end));
        return;
    };
    let r = number(turn.radius);
    // Clockwise on screen (Y down) is SVG's sweep 1; more than half a
    // turn is the long way.
    let sweep = u8::from(turn.sweep > 0.0);
    let large = u8::from(turn.sweep.abs() > std::f64::consts::PI);
    let _ = write!(out, " A {r} {r} 0 {large} {sweep} {}", p(end));
}

/// The stroke and fill attributes of a shape. A zero width draws the
/// thinnest line the screen shows, as KiCad does.
pub(super) fn paint(out: &mut String, width: f64, filled: bool) {
    out.push_str(if filled {
        " fill=\"currentColor\""
    } else {
        " fill=\"none\""
    });
    if width > 0.0 {
        let _ = write!(
            out,
            " stroke=\"currentColor\" stroke-width=\"{}\"",
            number(width)
        );
    } else if !filled {
        out.push_str(
            " stroke=\"currentColor\" stroke-width=\"1\" vector-effect=\"non-scaling-stroke\"",
        );
    }
}

pub(super) fn shape_markup(out: &mut String, shape: &Shape) {
    let p = |q: &Point| format!("{} {}", number(q.x), number(q.y));
    match shape {
        Shape::Line { start, end, width } => {
            let _ = write!(out, "<path d=\"M {} L {}\"", p(start), p(end));
            paint(out, *width, false);
        }
        Shape::Arc {
            start,
            mid,
            end,
            width,
        } => {
            let _ = write!(out, "<path d=\"M {}", p(start));
            arc_commands(out, *start, *mid, *end);
            out.push('"');
            paint(out, *width, false);
        }
        Shape::Circle {
            center,
            radius,
            width,
            filled,
        } => {
            let _ = write!(
                out,
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"",
                number(center.x),
                number(center.y),
                number(*radius)
            );
            paint(out, *width, *filled);
        }
        Shape::Polygon {
            edges,
            width,
            filled,
        } => {
            let Some(first) = edges.first() else {
                return;
            };
            let start = match first {
                Edge::Line { start, .. } | Edge::Arc { start, .. } => start,
            };
            let _ = write!(out, "<path d=\"M {}", p(start));
            for edge in edges {
                match edge {
                    Edge::Line { end, .. } => {
                        let _ = write!(out, " L {}", p(end));
                    }
                    Edge::Arc { start, mid, end } => arc_commands(out, *start, *mid, *end),
                }
            }
            out.push_str(" Z\"");
            paint(out, *width, *filled);
        }
        Shape::Curve {
            start,
            control1,
            control2,
            end,
            width,
        } => {
            let _ = write!(
                out,
                "<path d=\"M {} C {} {} {}\"",
                p(start),
                p(control1),
                p(control2),
                p(end)
            );
            paint(out, *width, false);
        }
    }
    out.push_str("/>");
}

pub(super) fn text_markup(out: &mut String, text: &DrawnText) {
    let anchor = match text.horizontal {
        TextAlign::Start => "start",
        TextAlign::Center => "middle",
        TextAlign::End => "end",
    };
    let baseline = match text.vertical {
        TextAlign::Start => "text-before-edge",
        TextAlign::Center => "central",
        TextAlign::End => "text-after-edge",
    };
    let mirror = if text.mirrored { " scale(-1 1)" } else { "" };
    // A capital letter is about 0.7 of the font size.
    let _ = write!(
        out,
        "<text transform=\"translate({} {}) rotate({}){mirror}\" font-size=\"{}\" text-anchor=\"{anchor}\" dominant-baseline=\"{baseline}\" fill=\"currentColor\"{}{}>{}</text>",
        number(text.at.x),
        number(text.at.y),
        number(-text.angle),
        number(text.height / 0.7),
        if text.bold {
            " font-weight=\"bold\""
        } else {
            ""
        },
        if text.italic {
            " font-style=\"italic\""
        } else {
            ""
        },
        escape(&text.text)
    );
}

/// Everything on `layer`: pads' shapes, graphics and texts.
pub(super) fn layer_markup(drawing: &FootprintDrawing, layer: &str) -> String {
    let mut out = String::new();
    for graphic in drawing.graphics.iter().filter(|shape| shape.layer == layer) {
        shape_markup(&mut out, &graphic.shape);
    }
    for pad in &drawing.pads {
        for shape in pad.shapes.iter().filter(|shape| shape.layer == layer) {
            shape_markup(&mut out, &shape.shape);
        }
    }
    for text in drawing.texts.iter().filter(|text| text.layer == layer) {
        text_markup(&mut out, text);
    }
    out
}

/// The holes, above every layer, as KiCad shows them: the board's
/// background colour, with a plated hole's wall around it.
pub(super) fn holes_markup(drawing: &FootprintDrawing) -> String {
    let mut out = String::new();
    for pad in &drawing.pads {
        let Some(hole) = &pad.hole else { continue };
        let wall = if pad.kind == PadKind::NpThruHole {
            "#1AC4D2"
        } else {
            "#ECECEC"
        };
        let mut shape = String::new();
        shape_markup(&mut shape, hole);
        // A hairline wall around the filled hole.
        let wall =
            format!(" stroke=\"{wall}\" stroke-width=\"1\" vector-effect=\"non-scaling-stroke\"/>");
        let _ = write!(
            out,
            "<g color=\"{BOARD}\">{}</g>",
            shape.replacen("/>", &wall, 1)
        );
    }
    out
}

/// A coordinate in millimetres, to the micrometre, with a real minus sign.
pub(super) fn format_mm(value: f64) -> String {
    let text = format!("{value:.3}");
    match text.strip_prefix('-') {
        Some("0.000") => "0.000".to_owned(),
        Some(rest) => format!("\u{2212}{rest}"),
        None => text,
    }
}

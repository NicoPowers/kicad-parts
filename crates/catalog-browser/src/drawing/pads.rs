//! A pad's shape on each layer, with KiCad's paste and mask rules.

use super::*;

/// A pad's own shape, before margins, in its own unrotated frame.
pub(super) struct PadGeometry {
    pub(super) at: Point,
    pub(super) angle: f64,
    pub(super) size: (f64, f64),
    pub(super) form: PadForm,
}

pub(super) enum PadForm {
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
    pub(super) fn read(pad: &Sexp) -> Option<Self> {
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
                            .take(MAX_SHAPES)
                            .filter(|item| {
                                item.children("pts")
                                    .next()
                                    .is_none_or(|pts| pts.items().len() <= MAX_EDGES)
                            })
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
    /// to nothing. These follow KiCad:
    ///
    /// - An equal margin moves the outline out (or in) by the margin: a
    ///   corner gets a rounding of the margin, and a rounded corner's
    ///   radius grows (or shrinks, never below 0) by it.
    /// - Different margins resize the pad: its sides by twice each margin,
    ///   a rounded corner's radius by the larger margin, a chamfer and a
    ///   trapezoid's slant as before.
    /// - A custom pad's anchor follows the same rules; a positive margin
    ///   grows its primitives by widening their stroke, and a negative one
    ///   leaves them as they are.
    pub(super) fn outline(&self, dx: f64, dy: f64) -> Vec<Shape> {
        let (w, h) = self.size;
        let equal = dx == dy;
        let grow = if equal { dx } else { 0.0 };
        let (w2, h2) = (w + 2.0 * dx, h + 2.0 * dy);
        if w2 <= 0.0 || h2 <= 0.0 {
            return Vec::new();
        }
        let rect = |radius: f64| {
            let radius = radius.clamp(0.0, w2.min(h2) / 2.0);
            cut_rect(w2, h2, [Corner::Round(radius); 4])
        };
        let shapes = match &self.form {
            PadForm::Circle => vec![Shape::Circle {
                center: Point::default(),
                radius: w2 / 2.0,
                width: 0.0,
                filled: true,
            }],
            PadForm::Oval => vec![oval(w2, h2)],
            PadForm::Rect => vec![rect(grow.max(0.0))],
            PadForm::RoundRect {
                radius_ratio,
                chamfer_ratio,
                chamfered,
            } if !chamfered.contains(&true) || *chamfer_ratio == 0.0 => {
                let radius = radius_ratio * w.min(h);
                vec![rect(radius + if equal { dx } else { dx.max(dy) })]
            }
            PadForm::RoundRect {
                radius_ratio,
                chamfer_ratio,
                chamfered,
            } => {
                let corners = |w: f64, h: f64, radius: f64| {
                    chamfered.map(|chamfer| {
                        if chamfer {
                            Corner::Chamfer(chamfer_ratio * w.min(h))
                        } else {
                            Corner::Round(radius.clamp(0.0, w.min(h) / 2.0))
                        }
                    })
                };
                let radius = radius_ratio * w.min(h);
                if equal && grow > 0.0 {
                    let pad = cut_rect(w, h, corners(w, h, radius));
                    vec![offset_convex(&pad, grow)]
                } else {
                    let grown = radius + dx.max(dy);
                    vec![cut_rect(w2, h2, corners(w2, h2, grown))]
                }
            }
            PadForm::Trapezoid { delta } => {
                let corners = |w: f64, h: f64| {
                    let (hw, hh) = (w / 2.0, h / 2.0);
                    // KiCad's corners: delta.x changes the left and right
                    // sides' heights, delta.y the top and bottom widths.
                    let (ddx, ddy) = (
                        (delta.0 / 2.0).clamp(-hh, hh),
                        (delta.1 / 2.0).clamp(-hw, hw),
                    );
                    [
                        Point::new(-hw + ddy, -hh - ddx),
                        Point::new(hw - ddy, -hh + ddx),
                        Point::new(hw + ddy, hh - ddx),
                        Point::new(-hw - ddy, hh + ddx),
                    ]
                };
                if equal && grow != 0.0 {
                    let pad = polygon(&corners(w, h), 0.0, true);
                    let moved = offset_convex(&pad, grow);
                    match &moved {
                        Shape::Polygon { edges, .. } if edges.is_empty() => Vec::new(),
                        _ => vec![moved],
                    }
                } else {
                    vec![polygon(&corners(w2, h2), 0.0, true)]
                }
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
                    rect(grow.max(0.0))
                };
                std::iter::once(anchor)
                    .chain(primitives.iter().map(|shape| stroked(shape.clone(), grow)))
                    .collect()
            }
        };
        shapes
    }
}

/// `shape` grown by `grow` on every side, with rounded corners, by
/// widening its stroke.
pub(super) fn stroked(shape: Shape, grow: f64) -> Shape {
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
pub(super) struct Margins {
    pub(super) mask: Option<f64>,
    pub(super) paste: Option<f64>,
    pub(super) paste_ratio: Option<f64>,
}

impl Margins {
    pub(super) fn read(node: &Sexp) -> Self {
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
    pub(super) fn or(self, other: &Self) -> Self {
        Self {
            mask: self.mask.or(other.mask),
            paste: self.paste.or(other.paste),
            paste_ratio: self.paste_ratio.or(other.paste_ratio),
        }
    }

    /// The paste margin along each axis of a pad of `size`: the margin
    /// plus the ratio times the size, never shrinking past nothing.
    pub(super) fn paste(&self, (w, h): (f64, f64)) -> (f64, f64) {
        let margin = self.paste.unwrap_or(0.0);
        let ratio = self.paste_ratio.unwrap_or(0.0);
        (
            (margin + ratio * w).max(-w / 2.0),
            (margin + ratio * h).max(-h / 2.0),
        )
    }
}

/// A pad's layers, with `*.Cu` and `F&B.Cu` style names spelled out.
pub(super) fn pad_layers(pad: &Sexp) -> Vec<String> {
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
pub(super) struct Drill {
    pub(super) size: (f64, f64),
    pub(super) oval: bool,
    pub(super) offset: Point,
}

impl Drill {
    pub(super) fn read(pad: &Sexp) -> Option<Self> {
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
    pub(super) fn covers(&self, (w, h): (f64, f64)) -> bool {
        w <= self.size.0 + 1e-6 && h <= self.size.1 + 1e-6
    }

    pub(super) fn shape(&self) -> Shape {
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
pub(super) fn drill_offset(drill: &Option<Drill>) -> Point {
    drill.as_ref().map(|drill| drill.offset).unwrap_or_default()
}

//! The close-up's Measure tool (kicad-parts-e95e): what it draws over the
//! footprint and what its readout says. The geometry is in
//! [`desktop_contract::measure`]; the pointer and keys are in the viewer.

use super::*;
use desktop_contract::measure::{Gap, SnapPoint, nearest_snap, pad_gap, pad_name};
use std::cell::Cell;

/// How near the pointer a snap point must be, in screen pixels.
pub(super) const SNAP_PIXELS: f64 = 10.0;
/// Millimetres in a mil, a thousandth of an inch.
const MM_PER_MIL: f64 = 0.0254;

thread_local! {
    /// Whether lengths are shown in mils. The app has no saved place for
    /// UI choices, so this lasts for the session, across close-ups.
    static MILS: Cell<bool> = const { Cell::new(false) };
}

/// The unit choice the session made, or millimetres.
pub(super) fn mils_chosen() -> bool {
    MILS.with(Cell::get)
}

pub(super) fn choose_mils(mils: bool) {
    MILS.with(|cell| cell.set(mils));
}

/// One end of a measurement: a snap point, or a free point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mark {
    pub at: Point,
    pub snap: Option<SnapPoint>,
}

impl Mark {
    pub(super) fn free(at: Point) -> Self {
        Self { at, snap: None }
    }

    pub(super) fn snapped(snap: SnapPoint) -> Self {
        Self {
            at: snap.at,
            snap: Some(snap),
        }
    }

    /// The point's name, such as `Pad 3 centre`, or `Free point` with its
    /// position.
    pub(super) fn name(&self, drawing: &FootprintDrawing, mils: bool) -> String {
        match &self.snap {
            Some(snap) => snap.name(drawing),
            None => format!("Free point at {}", format_position(self.at, mils)),
        }
    }
}

/// The mark at drawing point `at`: the nearest snap point within
/// [`SNAP_PIXELS`] at `scale` pixels per millimetre, else a free point.
pub(super) fn mark_at(points: &[SnapPoint], at: Point, scale: f64, free: bool) -> Mark {
    if free || !(scale.is_finite() && scale > 0.0) {
        return Mark::free(at);
    }
    nearest_snap(points, at, SNAP_PIXELS / scale)
        .map_or(Mark::free(at), |index| Mark::snapped(points[index]))
}

/// A length in the chosen unit: millimetres to the micrometre, or mils to
/// a tenth, with a real minus sign.
pub(super) fn format_length(mm: f64, mils: bool) -> String {
    if mils {
        let text = format!("{:.1}", mm / MM_PER_MIL);
        match text.strip_prefix('-') {
            Some("0.0") => "0.0 mils".to_owned(),
            Some(rest) => format!("\u{2212}{rest} mils"),
            None => format!("{text} mils"),
        }
    } else {
        format!("{} mm", format_mm(mm))
    }
}

/// The pointer's position line, such as `X 1.250 mm, Y −0.400 mm`.
pub(super) fn format_position(at: Point, mils: bool) -> String {
    format!(
        "X {}, Y {}",
        format_length(at.x, mils),
        format_length(at.y, mils)
    )
}

/// The gap between the two pads a measurement's ends snapped to, on
/// `layer`, with the pads' names; `None` unless they are two pads.
pub(super) fn pads_gap(
    drawing: &FootprintDrawing,
    from: &Mark,
    to: &Mark,
    layer: &str,
) -> Option<(String, String, Gap)> {
    let a = from.snap?.pad?;
    let b = to.snap?.pad?;
    if a == b {
        return None;
    }
    let gap = pad_gap(drawing, a, b, layer)?;
    Some((pad_name(&drawing.pads[a]), pad_name(&drawing.pads[b]), gap))
}

/// What the readout says, one sentence per line.
pub(super) struct Readout<'a> {
    pub drawing: &'a FootprintDrawing,
    pub layer: &'a str,
    pub layer_hidden: bool,
    pub start: Option<Mark>,
    pub end: Option<Mark>,
    /// The snap point N picked, if any.
    pub highlighted: Option<Mark>,
    pub mils: bool,
}

impl Readout<'_> {
    pub(super) fn lines(&self) -> Vec<String> {
        let mils = self.mils;
        let mut lines = Vec::new();
        match (self.start, self.end) {
            (None, _) => lines.push(
                "Click a point to start. Hold Shift for a free point. Press N to step through the snap points and Enter to pick one."
                    .to_owned(),
            ),
            (Some(start), None) => lines.push(format!(
                "From {}. Click the end point.",
                start.name(self.drawing, mils)
            )),
            (Some(start), Some(end)) => {
                let (dx, dy) = ((end.at.x - start.at.x).abs(), (end.at.y - start.at.y).abs());
                lines.push(format!(
                    "Distance {}, \u{394}X {}, \u{394}Y {}.",
                    format_length(dx.hypot(dy), mils),
                    format_length(dx, mils),
                    format_length(dy, mils)
                ));
                lines.push(format!(
                    "From {} to {}.",
                    start.name(self.drawing, mils),
                    end.name(self.drawing, mils)
                ));
                if let Some((a, b, gap)) = pads_gap(self.drawing, &start, &end, self.layer) {
                    lines.push(if gap.distance > 0.0 {
                        format!(
                            "Gap between {a} and {b} on {}: {}.",
                            self.layer,
                            format_length(gap.distance, mils)
                        )
                    } else {
                        format!(
                            "{} and {b} are touching or overlapping on {}.",
                            capitalized(&a),
                            self.layer
                        )
                    });
                }
            }
        }
        if self.layer_hidden {
            lines.push(format!(
                "{} is hidden. Show it to snap to its points.",
                self.layer
            ));
        } else if let Some(mark) = self.highlighted {
            lines.push(format!("Highlighted: {}.", mark.name(self.drawing, mils)));
        }
        lines
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// What the overlay draws: the measurement so far, the line to the
/// pointer while the end is not set, and the marker on the point the
/// pointer or N picks.
pub(super) struct Overlay {
    pub start: Option<Mark>,
    /// The set end, or the pointer's mark while it follows.
    pub end: Option<Mark>,
    pub candidate: Option<Mark>,
    pub gap: Option<Gap>,
    /// Screen pixels per millimetre, so marks keep their size on screen.
    pub scale: f64,
    pub mils: bool,
}

const LINE: &str = "#FFFFFF";
const LEG: &str = "#9FB3C8";
const GAP: &str = "#3FE0B5";
const SNAP: &str = "#FFD400";

impl Overlay {
    pub(super) fn markup(&self) -> String {
        let mut out = String::new();
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return out;
        }
        let px = |pixels: f64| pixels / self.scale;
        let p = |q: Point| format!("{} {}", number(q.x), number(q.y));
        let thin = "vector-effect=\"non-scaling-stroke\" fill=\"none\"";
        if let (Some(start), Some(end)) = (self.start, self.end) {
            let (a, b) = (start.at, end.at);
            let corner = Point::new(b.x, a.y);
            let _ = write!(
                out,
                "<path class=\"measure-legs\" d=\"M {} L {} L {}\" stroke=\"{LEG}\" stroke-width=\"1\" stroke-dasharray=\"4 3\" {thin}/>",
                p(a),
                p(corner),
                p(b)
            );
            let _ = write!(
                out,
                "<path class=\"measure-line\" d=\"M {} L {}\" stroke=\"{LINE}\" stroke-width=\"1.5\" {thin}/>",
                p(a),
                p(b)
            );
            // The gap, over the line: often it lies along it.
            if let Some(gap) = self.gap.filter(|gap| gap.distance > 0.0) {
                let _ = write!(
                    out,
                    "<path class=\"measure-gap\" d=\"M {} L {}\" stroke=\"{GAP}\" stroke-width=\"3\" {thin}/>",
                    p(gap.from),
                    p(gap.to)
                );
            }
            let length = (b.x - a.x).hypot(b.y - a.y);
            // The label sits beside the line's middle, on the side away
            // from the legs.
            let middle = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            let (nx, ny) = if length > 0.0 {
                ((a.y - b.y) / length, (b.x - a.x) / length)
            } else {
                (0.0, -1.0)
            };
            let away = if nx * (corner.x - middle.x) + ny * (corner.y - middle.y) > 0.0 {
                -1.0
            } else {
                1.0
            };
            let at = Point::new(
                middle.x + away * nx * px(14.0),
                middle.y + away * ny * px(14.0),
            );
            let _ = write!(
                out,
                "<text class=\"measure-label\" x=\"{}\" y=\"{}\" font-size=\"{}\" text-anchor=\"middle\" dominant-baseline=\"central\" fill=\"{LINE}\" stroke=\"{BOARD}\" stroke-width=\"{}\" paint-order=\"stroke\">{}</text>",
                number(at.x),
                number(at.y),
                number(px(13.0)),
                number(px(3.0)),
                escape(&format_length(length, self.mils))
            );
        }
        for mark in [self.start, self.end].into_iter().flatten() {
            let r = px(4.0);
            let _ = write!(
                out,
                "<path class=\"measure-end\" d=\"M {} {} L {} {} M {} {} L {} {}\" stroke=\"{LINE}\" stroke-width=\"1.5\" {thin}/>",
                number(mark.at.x - r),
                number(mark.at.y - r),
                number(mark.at.x + r),
                number(mark.at.y + r),
                number(mark.at.x - r),
                number(mark.at.y + r),
                number(mark.at.x + r),
                number(mark.at.y - r)
            );
        }
        if let Some(mark) = self.candidate.filter(|mark| mark.snap.is_some()) {
            let r = px(6.0);
            let _ = write!(
                out,
                "<rect class=\"measure-snap\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" stroke=\"{SNAP}\" stroke-width=\"2\" {thin}/>",
                number(mark.at.x - r),
                number(mark.at.y - r),
                number(2.0 * r),
                number(2.0 * r)
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_contract::drawing::{DrawnPad, LayerShape};
    use desktop_contract::measure::{SnapKind, snap_points};

    fn square(cx: f64, w: f64) -> Shape {
        let h = w / 2.0;
        let corners = [
            Point::new(cx - h, -h),
            Point::new(cx + h, -h),
            Point::new(cx + h, h),
            Point::new(cx - h, h),
        ];
        Shape::Polygon {
            edges: (0..4)
                .map(|i| Edge::Line {
                    start: corners[i],
                    end: corners[(i + 1) % 4],
                })
                .collect(),
            width: 0.0,
            filled: true,
        }
    }

    fn two_pads() -> FootprintDrawing {
        let pad = |number: &str, cx: f64, paste: f64| DrawnPad {
            number: number.to_owned(),
            kind: PadKind::Smd,
            center: Point::new(cx, 0.0),
            shapes: vec![
                LayerShape {
                    layer: "F.Cu".into(),
                    shape: square(cx, 0.8),
                },
                LayerShape {
                    layer: "F.Paste".into(),
                    shape: square(cx, paste),
                },
            ],
            hole: None,
        };
        FootprintDrawing {
            layers: vec!["F.Cu".into(), "F.Paste".into()],
            pads: vec![pad("1", -0.825, 0.7), pad("", 0.825, 0.7)],
            ..FootprintDrawing::default()
        }
    }

    #[test]
    fn lengths_show_in_millimetres_or_mils() {
        assert_eq!(format_length(1.65, false), "1.650 mm");
        assert_eq!(format_length(1.65, true), "65.0 mils");
        assert_eq!(format_length(-0.0254, true), "\u{2212}1.0 mils");
        assert_eq!(format_length(-0.00001, true), "0.0 mils");
        assert_eq!(
            format_position(Point::new(0.0254, -0.0508), true),
            "X 1.0 mils, Y \u{2212}2.0 mils"
        );
    }

    #[test]
    fn the_pointer_snaps_within_ten_pixels() {
        let drawing = two_pads();
        let points = snap_points(&drawing, "F.Cu");
        // 100 pixels per millimetre: 10 pixels is 0.1 mm.
        let near = mark_at(&points, Point::new(-0.75, 0.0), 100.0, false);
        assert_eq!(near.snap.map(|snap| snap.kind), Some(SnapKind::Centre));
        assert_eq!(near.at, Point::new(-0.825, 0.0));
        let far = mark_at(&points, Point::new(-0.6, 0.1), 100.0, false);
        assert_eq!(far, Mark::free(Point::new(-0.6, 0.1)));
        // Shift places a free point.
        let free = mark_at(&points, Point::new(-0.8, 0.0), 100.0, true);
        assert_eq!(free.snap, None);
    }

    #[test]
    fn the_readout_names_both_points_and_the_gap_between_pads() {
        let drawing = two_pads();
        let points = snap_points(&drawing, "F.Cu");
        let start = mark_at(&points, Point::new(-0.825, 0.0), 100.0, false);
        let end = mark_at(&points, Point::new(0.825, 0.0), 100.0, false);
        let mut readout = Readout {
            drawing: &drawing,
            layer: "F.Cu",
            layer_hidden: false,
            start: Some(start),
            end: Some(end),
            highlighted: None,
            mils: false,
        };
        assert_eq!(
            readout.lines(),
            [
                "Distance 1.650 mm, \u{394}X 1.650 mm, \u{394}Y 0.000 mm.",
                "From Pad 1 centre to Unnumbered pad centre.",
                "Gap between pad 1 and an unnumbered pad on F.Cu: 0.850 mm."
            ]
        );
        readout.layer = "F.Paste";
        assert_eq!(
            readout.lines()[2],
            "Gap between pad 1 and an unnumbered pad on F.Paste: 0.950 mm."
        );
        readout.end = None;
        readout.layer_hidden = true;
        assert_eq!(
            readout.lines(),
            [
                "From Pad 1 centre. Click the end point.",
                "F.Paste is hidden. Show it to snap to its points."
            ]
        );
        // A free point says where it is.
        readout.layer_hidden = false;
        readout.highlighted = Some(Mark::free(Point::new(1.0, 2.0)));
        assert_eq!(
            readout.lines()[1],
            "Highlighted: Free point at X 1.000 mm, Y 2.000 mm."
        );
    }

    #[test]
    fn overlapping_pads_say_so() {
        let mut drawing = two_pads();
        drawing.pads[1].shapes[0].shape = square(0.0, 1.0);
        let points = snap_points(&drawing, "F.Cu");
        let start =
            Mark::snapped(points[nearest_snap(&points, Point::new(-0.825, 0.0), 0.01).unwrap()]);
        let end =
            Mark::snapped(points[nearest_snap(&points, Point::new(0.825, 0.0), 0.01).unwrap()]);
        let readout = Readout {
            drawing: &drawing,
            layer: "F.Cu",
            layer_hidden: false,
            start: Some(start),
            end: Some(end),
            highlighted: None,
            mils: true,
        };
        assert_eq!(
            readout.lines()[2],
            "Pad 1 and an unnumbered pad are touching or overlapping on F.Cu."
        );
    }

    #[test]
    fn the_overlay_keeps_its_size_on_screen() {
        let overlay = Overlay {
            start: Some(Mark::free(Point::new(0.0, 0.0))),
            end: Some(Mark::free(Point::new(3.0, 4.0))),
            candidate: None,
            gap: None,
            scale: 100.0,
            mils: false,
        };
        let markup = overlay.markup();
        assert!(markup.contains(">5.000 mm</text>"), "{markup}");
        // 13 pixels at 100 pixels per millimetre.
        assert!(markup.contains("font-size=\"0.13\""), "{markup}");
        assert!(markup.contains("d=\"M 0 0 L 3 0 L 3 4\""), "{markup}");
        assert_eq!(markup.matches("class=\"measure-end\"").count(), 2);
        assert!(!markup.contains("measure-gap"));
        assert!(!markup.contains("measure-snap"));
    }
}

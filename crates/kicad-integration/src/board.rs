//! A board holding one footprint, for kicad-cli's 3D export
//! (kicad-parts-c9c1).
//!
//! `kicad-cli pcb export glb` draws a board, not a footprint. So the
//! footprint goes on a board of its own, at the origin, with an Edge.Cuts
//! rectangle [`OUTLINE_MARGIN_MM`] outside the box the caller gives (its
//! courtyard, or all its shapes) and a [`BOARD_THICKNESS_MM`] thickness.
//!
//! Each 3D model link is pointed at the exact file the app chose, as an
//! absolute path, so kicad-cli needs no path variables and never finds a
//! file of its own. A model that is not drawn is left out of the board.
//! The rest of the footprint's text is kept as it is, so each model keeps
//! its offset, scale and rotation.

use crate::library_file::{LibraryFileError, Sexp, root_children};
use std::fmt::Write as _;
use std::ops::Range;
use std::path::PathBuf;

/// The board's thickness, KiCad's default.
pub const BOARD_THICKNESS_MM: f64 = 1.6;
/// How far the board's edge is outside the footprint's box.
pub const OUTLINE_MARGIN_MM: f64 = 0.5;

/// One `(model ...)` of a footprint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelBlock {
    /// The link as written (unescaped), such as
    /// `${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step`.
    pub link: String,
    /// KiCad's hide flag: KiCad does not draw this model.
    pub hidden: bool,
    /// The whole `(model ...)` list in the footprint's text.
    span: Range<usize>,
    /// The link's token in the footprint's text, quotes included.
    path: Range<usize>,
}

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// What the board does with one model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelUse {
    /// Point the link at this file.
    File(PathBuf),
    /// Keep the link as written, such as a model embedded in the footprint.
    Keep,
    /// Leave the model out.
    Leave,
}

/// Every `(model ...)` of the footprint in `bytes`, in file order.
pub fn model_blocks(bytes: &[u8]) -> Result<Vec<ModelBlock>, LibraryFileError> {
    // Spans count from the start of `bytes`, byte order mark included.
    let skipped = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
    let (head, children) = root_children(&bytes[skipped..])?;
    if head != "footprint" && head != "module" {
        return Err(LibraryFileError("the file is not a KiCad footprint".into()));
    }
    let mut blocks = Vec::new();
    for mut child in children.into_iter().filter(|child| child.head == "model") {
        child.span = child.span.start + skipped..child.span.end + skipped;
        let list = Sexp::parse(&bytes[child.span.clone()])?;
        let hidden = list.items().iter().skip(2).any(|item| {
            // `hide` on its own in older files, `(hide yes)` since KiCad 9.
            item.word() == Some("hide")
                || (item.head() == Some("hide") && item.word_at(1) != Some("no"))
        });
        blocks.push(ModelBlock {
            link: list.word_at(1).unwrap_or_default().to_owned(),
            hidden,
            path: link_token(bytes, child.span.clone()),
            span: child.span,
        });
    }
    Ok(blocks)
}

/// The token after `(model` in the list at `span`: a quoted string with
/// its quotes, or a word. Empty, at the list's end, when there is none.
fn link_token(bytes: &[u8], span: Range<usize>) -> Range<usize> {
    let end = span.end - 1;
    let mut at = span.start + 1;
    let skip_space = |mut at: usize| {
        while at < end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        at
    };
    at = skip_space(at) + "model".len();
    at = skip_space(at);
    let start = at;
    if bytes.get(at) == Some(&b'"') {
        at += 1;
        while at < end && bytes[at] != b'"' {
            at += if bytes[at] == b'\\' { 2 } else { 1 };
        }
        return start..(at + 1).min(end);
    }
    while at < end && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'(' | b')') {
        at += 1;
    }
    start..at
}

/// A `.kicad_pcb` holding the footprint in `bytes` at the origin, its
/// models used as `uses` says (one for each of [`model_blocks`], in the
/// same order), and an outline around `content` (`[min_x, min_y, max_x,
/// max_y]` in millimetres) grown by [`OUTLINE_MARGIN_MM`].
pub fn one_footprint_board(
    bytes: &[u8],
    uses: &[(ModelBlock, ModelUse)],
    content: [f64; 4],
) -> Result<Vec<u8>, LibraryFileError> {
    // The footprint's own bytes are kept as they are, even ones that are
    // not UTF-8, such as Latin-1 in an old vendor file: every place
    // changed is a byte offset into them.
    let skipped = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
    // A byte order mark or blank lines would sit in the middle of the board.
    let start = skipped
        + bytes[skipped..]
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
    // Each change: the bytes to replace, and what replaces them.
    let mut changes: Vec<(Range<usize>, String)> = Vec::new();
    for (block, model) in uses {
        match model {
            ModelUse::Keep => {}
            ModelUse::Leave => changes.push((block.span.clone(), String::new())),
            ModelUse::File(file) => {
                // A `(model)` with no link gets one, after the head.
                let space = if block.path.is_empty() { " " } else { "" };
                let link = format!("{space}{}", quoted(&file.to_string_lossy()));
                changes.push((block.path.clone(), link));
            }
        }
    }
    // The footprint goes at the origin, unturned, where its outline is
    // drawn, whatever placement its own file gives it.
    let (_, children) = root_children(&bytes[skipped..])?;
    for child in children.iter().filter(|child| child.head == "at") {
        changes.push((
            child.span.start + skipped..child.span.end + skipped,
            String::new(),
        ));
    }
    changes.sort_by_key(|(span, _)| span.start);
    let mut footprint = Vec::with_capacity(bytes.len());
    let mut at = start;
    for (span, replacement) in changes {
        if span.start < at || span.end > bytes.len() {
            return Err(LibraryFileError(
                "the 3D models do not match the footprint".into(),
            ));
        }
        footprint.extend_from_slice(&bytes[at..span.start]);
        footprint.extend_from_slice(replacement.as_bytes());
        at = span.end;
    }
    footprint.extend_from_slice(&bytes[at..]);
    while footprint.last().is_some_and(u8::is_ascii_whitespace) {
        footprint.pop();
    }

    let [x0, y0, x1, y1] = content;
    let margin = OUTLINE_MARGIN_MM;
    let mut board = String::with_capacity(2048);
    board.push_str("(kicad_pcb\n\t(version 20241229)\n\t(generator \"kicad_parts\")\n");
    let _ = writeln!(
        board,
        "\t(general\n\t\t(thickness {})\n\t)",
        number(BOARD_THICKNESS_MM)
    );
    board.push_str("\t(paper \"A4\")\n\t(layers\n");
    for layer in LAYERS {
        let _ = writeln!(board, "\t\t{layer}");
    }
    for user in 1..=9 {
        let _ = writeln!(board, "\t\t({} \"User.{user}\" user)", 37 + 2 * user);
    }
    board.push_str("\t)\n\t(setup\n\t\t(pad_to_mask_clearance 0)\n\t)\n\t(net 0 \"\")\n");
    let mut trailer = String::from("\n");
    let _ = writeln!(
        trailer,
        "\t(gr_rect\n\t\t(start {} {})\n\t\t(end {} {})\n\t\t(stroke\n\t\t\t(width 0.05)\n\t\t\t(type solid)\n\t\t)\n\t\t(fill no)\n\t\t(layer \"Edge.Cuts\")\n\t)",
        number(x0 - margin),
        number(y0 - margin),
        number(x1 + margin),
        number(y1 + margin),
    );
    trailer.push_str(")\n");
    let mut out = board.into_bytes();
    out.extend_from_slice(&footprint);
    out.extend_from_slice(trailer.as_bytes());
    Ok(out)
}

/// KiCad 9 and 10's layer numbers, as their own boards list them.
const LAYERS: [&str; 20] = [
    "(0 \"F.Cu\" signal)",
    "(2 \"B.Cu\" signal)",
    "(9 \"F.Adhes\" user \"F.Adhesive\")",
    "(11 \"B.Adhes\" user \"B.Adhesive\")",
    "(13 \"F.Paste\" user)",
    "(15 \"B.Paste\" user)",
    "(5 \"F.SilkS\" user \"F.Silkscreen\")",
    "(7 \"B.SilkS\" user \"B.Silkscreen\")",
    "(1 \"F.Mask\" user)",
    "(3 \"B.Mask\" user)",
    "(17 \"Dwgs.User\" user \"User.Drawings\")",
    "(19 \"Cmts.User\" user \"User.Comments\")",
    "(21 \"Eco1.User\" user \"User.Eco1\")",
    "(23 \"Eco2.User\" user \"User.Eco2\")",
    "(25 \"Edge.Cuts\" user)",
    "(27 \"Margin\" user)",
    "(31 \"F.CrtYd\" user \"F.Courtyard\")",
    "(29 \"B.CrtYd\" user \"B.Courtyard\")",
    "(35 \"F.Fab\" user)",
    "(33 \"B.Fab\" user)",
];

/// `text` as a KiCad quoted string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A millimetre value with at most 6 decimals and no trailing zeros.
fn number(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOTPRINT: &str = "\u{feff}(footprint \"R_0603\"
\t(version 20241229)
\t(layer \"F.Cu\")
\t(fp_line (start -1.48 -0.73) (end 1.48 -0.73) (stroke (width 0.05) (type solid)) (layer \"F.CrtYd\"))
\t(pad \"1\" smd roundrect (at -0.825 0) (size 0.8 0.95) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))
\t(model \"${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step\"
\t\t(offset (xyz 0.1 -0.2 0.3))
\t\t(scale (xyz 1 1 2))
\t\t(rotate (xyz 0 0 90))
\t)
\t(model \"${MY_LIB_3D}/Hidden \\\"one\\\".step\" (hide yes) (offset (xyz 0 0 0)))
\t(model ${KIPRJMOD}/old.wrl hide)
\t(model \"kicad-embed://inside.step\")
\t(fp_text user \"(model not-a-model)\" (at 0 0) (layer \"F.Fab\"))
)
";

    #[test]
    fn model_blocks_are_read_with_their_links_and_hide_flags() {
        let blocks = model_blocks(FOOTPRINT.as_bytes()).unwrap();
        let read: Vec<(&str, bool)> = blocks
            .iter()
            .map(|block| (block.link.as_str(), block.hidden))
            .collect();
        assert_eq!(
            read,
            [
                (
                    "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
                    false
                ),
                ("${MY_LIB_3D}/Hidden \"one\".step", true),
                ("${KIPRJMOD}/old.wrl", true),
                ("kicad-embed://inside.step", false),
            ]
        );
        assert_eq!(
            &FOOTPRINT[blocks[0].path.clone()],
            "\"${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step\""
        );
        assert_eq!(&FOOTPRINT[blocks[2].path.clone()], "${KIPRJMOD}/old.wrl");
        assert!(model_blocks(b"(kicad_symbol_lib)").is_err());
        assert!(model_blocks(b"(footprint \"X\"").is_err());
        assert_eq!(
            model_blocks(b"(footprint \"X\" (layer \"F.Cu\"))").unwrap(),
            []
        );
    }

    #[test]
    fn the_board_points_models_at_absolute_paths_and_keeps_their_placement() {
        let blocks = model_blocks(FOOTPRINT.as_bytes()).unwrap();
        let file = PathBuf::from(r"C:\Program Files\KiCad\10.0\share\kicad\3dmodels\R.step");
        let uses = vec![
            (blocks[0].clone(), ModelUse::File(file)),
            (blocks[1].clone(), ModelUse::Leave),
            (
                blocks[2].clone(),
                ModelUse::File(PathBuf::from("/models/old.wrl")),
            ),
            (blocks[3].clone(), ModelUse::Keep),
        ];
        let board =
            one_footprint_board(FOOTPRINT.as_bytes(), &uses, [-1.48, -0.73, 1.48, 0.73]).unwrap();
        let board = String::from_utf8(board).unwrap();
        // The board reads as one list, with the footprint inside.
        let tree = Sexp::parse(board.as_bytes()).unwrap();
        assert_eq!(tree.head(), Some("kicad_pcb"));
        assert!(!board.contains('\u{feff}'));
        let footprint = tree.children("footprint").next().expect("the footprint");
        let links: Vec<&str> = footprint
            .children("model")
            .filter_map(|model| model.word_at(1))
            .collect();
        assert_eq!(
            links,
            [
                r"C:\Program Files\KiCad\10.0\share\kicad\3dmodels\R.step",
                "/models/old.wrl",
                "kicad-embed://inside.step"
            ]
        );
        assert!(board.contains(
            "(model \"C:\\\\Program Files\\\\KiCad\\\\10.0\\\\share\\\\kicad\\\\3dmodels\\\\R.step\"\n\t\t(offset (xyz 0.1 -0.2 0.3))\n\t\t(scale (xyz 1 1 2))\n\t\t(rotate (xyz 0 0 90))"
        ));
        assert!(!board.contains("MY_LIB_3D"), "the hidden model is left out");
        // A text that only looks like a model stays.
        assert!(board.contains("\"(model not-a-model)\""));
        // A 1.6 mm board with its edge 0.5 mm outside the box.
        assert_eq!(
            tree.children("general")
                .next()
                .unwrap()
                .children("thickness")
                .next()
                .unwrap()
                .word_at(1),
            Some("1.6")
        );
        let outline = tree.children("gr_rect").next().expect("the outline");
        let corner = |name: &str| {
            let point = outline.children(name).next().unwrap();
            (
                point.word_at(1).unwrap().to_owned(),
                point.word_at(2).unwrap().to_owned(),
            )
        };
        assert_eq!(corner("start"), ("-1.98".to_owned(), "-1.23".to_owned()));
        assert_eq!(corner("end"), ("1.98".to_owned(), "1.23".to_owned()));
        assert_eq!(
            outline.children("layer").next().unwrap().word_at(1),
            Some("Edge.Cuts")
        );
        let layers = tree.children("layers").next().unwrap();
        assert!(
            layers
                .items()
                .iter()
                .any(|layer| layer.word_at(1) == Some("User.9"))
        );
    }

    /// An old vendor file in Latin-1: its bytes are kept, and the model
    /// links after them are still replaced exactly.
    #[test]
    fn a_footprint_that_is_not_utf8_keeps_its_bytes() {
        let mut bytes =
            b"(footprint \"L\" (descr \"R\xE9sistance \xB5\") (layer \"F.Cu\")\n".to_vec();
        bytes.extend_from_slice(
            b"\t(model \"${X}/a.step\" (offset (xyz 0 0 0)))\n\t(model \"${X}/b.step\")\n)\n",
        );
        let blocks = model_blocks(&bytes).unwrap();
        let uses = vec![
            (
                blocks[0].clone(),
                ModelUse::File(PathBuf::from(r"C:\m\a.step")),
            ),
            (blocks[1].clone(), ModelUse::Leave),
        ];
        let board = one_footprint_board(&bytes, &uses, [-1.0, -1.0, 1.0, 1.0]).unwrap();
        let find = |needle: &[u8]| board.windows(needle.len()).any(|window| window == needle);
        assert!(
            find(b"(descr \"R\xE9sistance \xB5\")"),
            "the Latin-1 bytes are kept"
        );
        assert!(find(b"(model \"C:\\\\m\\\\a.step\" (offset (xyz 0 0 0)))"));
        assert!(!find(b"b.step"));
        let tree = Sexp::parse(&board).unwrap();
        let footprint = tree.children("footprint").next().unwrap();
        assert_eq!(footprint.children("model").count(), 1);
    }

    #[test]
    fn the_footprint_goes_at_the_origin_whatever_its_own_placement() {
        let bytes = b"(footprint \"P\" (layer \"F.Cu\") (at 10 5 90) (pad \"1\" smd rect (at 1 0) (size 1 1) (layers \"F.Cu\")))";
        let board = one_footprint_board(bytes, &[], [-0.5, -0.5, 1.5, 0.5]).unwrap();
        let board = String::from_utf8(board).unwrap();
        let tree = Sexp::parse(board.as_bytes()).unwrap();
        let footprint = tree.children("footprint").next().unwrap();
        assert_eq!(footprint.children("at").count(), 0, "{board}");
        // The pad keeps its place in the footprint.
        let pad = footprint.children("pad").next().unwrap();
        assert_eq!(pad.children("at").next().unwrap().word_at(1), Some("1"));
    }

    #[test]
    fn a_board_without_models_still_holds_the_footprint() {
        let bytes = b"(footprint \"P\" (layer \"F.Cu\") (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\")))";
        let board = one_footprint_board(bytes, &[], [-0.5, -0.5, 0.5, 0.5]).unwrap();
        let board = String::from_utf8(board).unwrap();
        let tree = Sexp::parse(board.as_bytes()).unwrap();
        assert_eq!(tree.children("footprint").count(), 1);
        assert_eq!(number(-0.0000001), "0");
        assert_eq!(number(2.5), "2.5");
    }
}

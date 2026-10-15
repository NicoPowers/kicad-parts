//! Footprint texts and visible properties.

use super::*;

/// The footprint's reference and value, for `${REFERENCE}` and
/// `${VALUE}`.
pub(super) fn reference_and_value(tree: &Sexp) -> (String, String) {
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
pub(super) fn text(node: &Sexp, reference: &str, value: &str) -> Option<DrawnText> {
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
    // KiCad keeps footprint text readable: text turned past upright is
    // turned back by half a turn, its alignment swapped so it stays put.
    let mut angle = angle.rem_euclid(360.0);
    let (mut horizontal, mut vertical) = (horizontal, vertical);
    if angle > 90.0 && angle <= 270.0 {
        angle -= 180.0;
        let swap = |align| match align {
            TextAlign::Start => TextAlign::End,
            TextAlign::End => TextAlign::Start,
            TextAlign::Center => TextAlign::Center,
        };
        horizontal = swap(horizontal);
        vertical = swap(vertical);
    }
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

//! A quick, one-pass read of what a library browser needs from KiCad's
//! library files, without building a tree: per symbol its name,
//! description, keywords, reference, default footprint, footprint filters,
//! power flag and distinct pin numbers; per footprint its description,
//! keywords, distinct numbered pads, how it mounts and its 3D model.
//!
//! KiCad's own libraries hold about 20,000 symbols in 230 MB and 15,000
//! footprints in 200 MB, so this reads bytes once, copies only the values it
//! keeps, and refuses a file nested deeper than [`MAX_DEPTH`].

use crate::library_file::{LibraryFileError, MAX_DEPTH};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

/// One top-level symbol of a `.kicad_sym` file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SymbolSummary {
    pub name: String,
    /// The symbol this one `(extends ...)`, if any.
    pub extends: Option<String>,
    pub description: String,
    /// `ki_keywords`.
    pub keywords: String,
    /// The Reference property, such as `R`, `U` or `#PWR`.
    pub reference: String,
    pub value: String,
    /// The default footprint, such as `Resistor_SMD:R_0603_1608Metric`.
    pub footprint: String,
    /// `ki_fp_filters`, split at white space, such as `R_*`.
    pub footprint_filters: Vec<String>,
    /// A power symbol (`(power)`), such as GND.
    pub power: bool,
    /// How many distinct pin numbers it has across its units and body
    /// styles; a derived symbol has its parent's.
    pub pin_count: u32,
    /// From the entry's `(` to its `)`, inclusive.
    pub span: Range<usize>,
}

/// A `.kicad_sym` file's symbols and `(version ...)`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SymbolFileSummary {
    pub version: Option<String>,
    pub symbols: Vec<SymbolSummary>,
}

/// How a footprint is mounted.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mount {
    Smd,
    ThroughHole,
    /// Neither, such as a mounting hole, a logo or a test point outline.
    #[default]
    Other,
}

/// One `.kicad_mod` footprint.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FootprintSummary {
    /// The name written in the file; KiCad names a footprint by its file.
    pub name: String,
    pub description: String,
    /// `tags`.
    pub keywords: String,
    /// How many distinct pad numbers it has, leaving out unnumbered and
    /// non-plated holes, as KiCad's pin count filter counts them.
    pub pad_count: u32,
    pub mount: Mount,
    /// The first `(model ...)` path, exactly as written.
    pub model: Option<String>,
}

/// Reads a `.kicad_sym` file. Derived symbols take their parent's pins and
/// power flag, and any property they leave empty.
pub fn read_symbol_file(bytes: &[u8]) -> Result<SymbolFileSummary, LibraryFileError> {
    let mut lexer = Lexer::new(bytes);
    match (lexer.next()?, lexer.next()?) {
        (Some(Token::Open(_)), Some(Token::Atom(head))) if head == b"kicad_symbol_lib" => {}
        _ => {
            return Err(LibraryFileError(
                "a symbol library starts with `kicad_symbol_lib`".into(),
            ));
        }
    }
    let mut summary = SymbolFileSummary::default();
    let mut pins: Vec<HashSet<Vec<u8>>> = Vec::new();
    loop {
        match lexer.next()? {
            None => return Err(LibraryFileError("unbalanced `(`".into())),
            Some(Token::Close(_)) => break,
            Some(Token::Open(start)) => match lexer.next()? {
                Some(Token::Atom(b"symbol")) => {
                    let (symbol, numbers) = read_symbol(&mut lexer, start)?;
                    summary.symbols.push(symbol);
                    pins.push(numbers);
                }
                Some(Token::Atom(b"version")) => {
                    summary.version = lexer.word()?;
                    lexer.skip_rest(2)?;
                }
                Some(Token::Close(_)) => {}
                _ => lexer.skip_rest(2)?,
            },
            Some(_) => {}
        }
    }
    if lexer.next()?.is_some() {
        return Err(LibraryFileError("content after the root list".into()));
    }
    for (symbol, numbers) in summary.symbols.iter_mut().zip(&pins) {
        symbol.pin_count = numbers.len() as u32;
    }
    inherit_from_parents(&mut summary.symbols);
    Ok(summary)
}

/// Reads one top-level `(symbol ...)`, whose `(symbol` was just read.
fn read_symbol(
    lexer: &mut Lexer<'_>,
    start: usize,
) -> Result<(SymbolSummary, HashSet<Vec<u8>>), LibraryFileError> {
    let mut symbol = SymbolSummary {
        name: lexer
            .word()?
            .ok_or_else(|| LibraryFileError("a symbol has no name".into()))?,
        ..SymbolSummary::default()
    };
    let mut numbers = HashSet::new();
    loop {
        match lexer.next()? {
            None => return Err(LibraryFileError("unbalanced `(`".into())),
            Some(Token::Close(end)) => {
                symbol.span = start..end + 1;
                return Ok((symbol, numbers));
            }
            Some(Token::Open(_)) => match lexer.next()? {
                Some(Token::Atom(b"property")) => {
                    let name = lexer.word()?.unwrap_or_default();
                    let value = lexer.word()?.unwrap_or_default();
                    match name.as_str() {
                        "Description" | "ki_description" => symbol.description = value,
                        "ki_keywords" => symbol.keywords = value,
                        "Reference" => symbol.reference = value,
                        "Value" => symbol.value = value,
                        "Footprint" => symbol.footprint = value,
                        "ki_fp_filters" => {
                            symbol.footprint_filters =
                                value.split_whitespace().map(str::to_owned).collect();
                        }
                        _ => {}
                    }
                    lexer.skip_rest(3)?;
                }
                Some(Token::Atom(b"extends")) => {
                    symbol.extends = lexer.word()?;
                    lexer.skip_rest(3)?;
                }
                Some(Token::Atom(b"power")) => {
                    symbol.power = true;
                    lexer.skip_rest(3)?;
                }
                Some(Token::Atom(b"symbol")) => read_unit_pins(lexer, &mut numbers)?,
                Some(Token::Close(_)) => {}
                _ => lexer.skip_rest(3)?,
            },
            Some(_) => {}
        }
    }
}

/// Reads a unit `(symbol "X_1_1" ...)` to its end, collecting the numbers
/// of its `(pin ... (number "N" ...))`.
fn read_unit_pins(
    lexer: &mut Lexer<'_>,
    numbers: &mut HashSet<Vec<u8>>,
) -> Result<(), LibraryFileError> {
    // Depth below the unit's own list: 1 is a child of the unit.
    let mut depth = 1usize;
    // The depth of the `pin` list being read, if inside one.
    let mut in_pin: Option<usize> = None;
    let mut just_opened = false;
    let mut number_next = false;
    loop {
        let token = lexer
            .next()?
            .ok_or_else(|| LibraryFileError("unbalanced `(`".into()))?;
        match token {
            Token::Open(_) => {
                depth += 1;
                just_opened = true;
                number_next = false;
                continue;
            }
            Token::Close(_) => {
                if in_pin == Some(depth) {
                    in_pin = None;
                }
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
                number_next = false;
            }
            Token::Atom(word) if just_opened => {
                if word == b"pin" && in_pin.is_none() {
                    in_pin = Some(depth);
                } else if word == b"number" && in_pin == Some(depth - 1) {
                    number_next = true;
                    just_opened = false;
                    continue;
                }
            }
            Token::Atom(word) if number_next => {
                if !word.is_empty() {
                    numbers.insert(word.to_vec());
                }
                number_next = false;
            }
            Token::Text(range, escaped) if number_next => {
                let text = lexer.text(range, escaped);
                if !text.trim().is_empty() {
                    numbers.insert(text.into_bytes());
                }
                number_next = false;
            }
            _ => {}
        }
        just_opened = false;
    }
}

/// A derived symbol takes its root parent's pins and power flag, and any
/// property it leaves empty.
fn inherit_from_parents(symbols: &mut [SymbolSummary]) {
    let by_name: BTreeMap<String, usize> = symbols
        .iter()
        .enumerate()
        .map(|(index, symbol)| (symbol.name.clone(), index))
        .collect();
    for index in 0..symbols.len() {
        let mut parent = symbols[index].extends.clone();
        // Follow the chain a few steps; KiCad allows one level today.
        for _ in 0..8 {
            let Some(at) = parent.as_ref().and_then(|name| by_name.get(name)).copied() else {
                break;
            };
            if at == index {
                break;
            }
            let from = symbols[at].clone();
            let symbol = &mut symbols[index];
            symbol.pin_count = from.pin_count;
            symbol.power |= from.power;
            for (own, theirs) in [
                (&mut symbol.description, &from.description),
                (&mut symbol.keywords, &from.keywords),
                (&mut symbol.reference, &from.reference),
                (&mut symbol.footprint, &from.footprint),
            ] {
                if own.is_empty() {
                    own.clone_from(theirs);
                }
            }
            if symbol.footprint_filters.is_empty() {
                symbol.footprint_filters.clone_from(&from.footprint_filters);
            }
            parent = from.extends;
        }
    }
}

/// Reads a `.kicad_mod` footprint file (`footprint`, or KiCad 5's
/// `module`).
pub fn read_footprint_file(bytes: &[u8]) -> Result<FootprintSummary, LibraryFileError> {
    let mut lexer = Lexer::new(bytes);
    match (lexer.next()?, lexer.next()?) {
        (Some(Token::Open(_)), Some(Token::Atom(b"footprint" | b"module"))) => {}
        _ => {
            return Err(LibraryFileError(
                "a footprint starts with `footprint` or `module`".into(),
            ));
        }
    }
    let mut summary = FootprintSummary {
        name: lexer.word()?.unwrap_or_default(),
        ..FootprintSummary::default()
    };
    let mut attr: Option<Mount> = None;
    let (mut smd_pads, mut hole_pads) = (false, false);
    let mut numbers: HashSet<Vec<u8>> = HashSet::new();
    loop {
        match lexer.next()? {
            None => return Err(LibraryFileError("unbalanced `(`".into())),
            Some(Token::Close(_)) => break,
            Some(Token::Open(_)) => match lexer.next()? {
                Some(Token::Atom(b"descr")) => {
                    summary.description = lexer.word()?.unwrap_or_default();
                    lexer.skip_rest(2)?;
                }
                Some(Token::Atom(b"tags")) => {
                    summary.keywords = lexer.word()?.unwrap_or_default();
                    lexer.skip_rest(2)?;
                }
                Some(Token::Atom(b"attr")) => {
                    // (attr smd exclude_from_bom ...): words until the end.
                    while let Some(word) = lexer.word_or_close()? {
                        match word.as_str() {
                            "smd" => attr = Some(Mount::Smd),
                            "through_hole" => attr = Some(Mount::ThroughHole),
                            _ => {}
                        }
                    }
                }
                Some(Token::Atom(b"pad")) => {
                    let number = lexer.word()?.unwrap_or_default();
                    let kind = lexer.word()?.unwrap_or_default();
                    match kind.as_str() {
                        "smd" => smd_pads = true,
                        "thru_hole" => hole_pads = true,
                        _ => {}
                    }
                    if kind != "np_thru_hole" && !number.trim().is_empty() {
                        numbers.insert(number.into_bytes());
                    }
                    lexer.skip_rest(2)?;
                }
                Some(Token::Atom(b"model")) => {
                    let path = lexer.word()?;
                    if summary.model.is_none() {
                        summary.model = path;
                    }
                    lexer.skip_rest(2)?;
                }
                Some(Token::Close(_)) => {}
                _ => lexer.skip_rest(2)?,
            },
            Some(_) => {}
        }
    }
    if lexer.next()?.is_some() {
        return Err(LibraryFileError("content after the root list".into()));
    }
    summary.pad_count = numbers.len() as u32;
    summary.mount = attr.unwrap_or(if hole_pads {
        Mount::ThroughHole
    } else if smd_pads {
        Mount::Smd
    } else {
        Mount::Other
    });
    Ok(summary)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token<'a> {
    /// `(` at this offset.
    Open(usize),
    /// `)` at this offset.
    Close(usize),
    Atom(&'a [u8]),
    /// A quoted string's inside (byte range), and whether it has escapes.
    Text(Range<usize>, bool),
}

/// Reads tokens one at a time, tracking depth.
struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl<'a> Lexer<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            depth: 0,
        }
    }

    fn next(&mut self) -> Result<Option<Token<'a>>, LibraryFileError> {
        let bytes = self.bytes;
        while self.at < bytes.len() && bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
        let Some(&byte) = bytes.get(self.at) else {
            return Ok(None);
        };
        let start = self.at;
        match byte {
            b'(' => {
                self.at += 1;
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(LibraryFileError(format!(
                        "the file is nested too deeply (more than {MAX_DEPTH} levels) to be a KiCad file"
                    )));
                }
                Ok(Some(Token::Open(start)))
            }
            b')' => {
                self.at += 1;
                self.depth = self
                    .depth
                    .checked_sub(1)
                    .ok_or_else(|| LibraryFileError("unbalanced `)`".into()))?;
                Ok(Some(Token::Close(start)))
            }
            b'"' => {
                let mut index = start + 1;
                let mut escaped = false;
                loop {
                    match bytes.get(index) {
                        None => return Err(LibraryFileError("unterminated string".into())),
                        Some(b'\\') => {
                            escaped = true;
                            index += 2;
                        }
                        Some(b'"') => break,
                        Some(_) => index += 1,
                    }
                }
                self.at = index + 1;
                Ok(Some(Token::Text(start + 1..index, escaped)))
            }
            _ => {
                let mut index = start;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'(' | b')' | b'"')
                {
                    index += 1;
                }
                self.at = index;
                Ok(Some(Token::Atom(&bytes[start..index])))
            }
        }
    }

    /// A quoted string's text, unescaped.
    fn text(&self, range: Range<usize>, escaped: bool) -> String {
        let raw = &self.bytes[range];
        if !escaped {
            return String::from_utf8_lossy(raw).into_owned();
        }
        let mut value = Vec::with_capacity(raw.len());
        let mut index = 0;
        while index < raw.len() {
            if raw[index] == b'\\' {
                if let Some(next) = raw.get(index + 1) {
                    value.push(match next {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => *other,
                    });
                }
                index += 2;
            } else {
                value.push(raw[index]);
                index += 1;
            }
        }
        String::from_utf8_lossy(&value).into_owned()
    }

    /// The next token as a word (atom or string); `None` for a list, whose
    /// `(` is given back by leaving the depth to [`Self::skip_rest`].
    fn word(&mut self) -> Result<Option<String>, LibraryFileError> {
        let before = (self.at, self.depth);
        match self.next()? {
            Some(Token::Atom(word)) => Ok(Some(String::from_utf8_lossy(word).into_owned())),
            Some(Token::Text(range, escaped)) => Ok(Some(self.text(range, escaped))),
            _ => {
                (self.at, self.depth) = before;
                Ok(None)
            }
        }
    }

    /// The next word of the current list, or `None` at its `)`, which is
    /// read. Nested lists are skipped.
    fn word_or_close(&mut self) -> Result<Option<String>, LibraryFileError> {
        loop {
            match self.next()? {
                None => return Err(LibraryFileError("unbalanced `(`".into())),
                Some(Token::Close(_)) => return Ok(None),
                Some(Token::Atom(word)) => {
                    return Ok(Some(String::from_utf8_lossy(word).into_owned()));
                }
                Some(Token::Text(range, escaped)) => return Ok(Some(self.text(range, escaped))),
                Some(Token::Open(_)) => {
                    let depth = self.depth;
                    self.skip_rest(depth)?;
                }
            }
        }
    }

    /// Reads to the `)` that closes the list at `depth` (1 is the root).
    fn skip_rest(&mut self, depth: usize) -> Result<(), LibraryFileError> {
        while self.depth >= depth {
            if self.next()?.is_none() {
                return Err(LibraryFileError("unbalanced `(`".into()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = r##"(kicad_symbol_lib
	(version 20241209)
	(generator "kicad_symbol_editor")
	(symbol "R"
		(pin_numbers (hide yes))
		(exclude_from_sim no)
		(property "Reference" "R" (at 2.032 0 90) (effects (font (size 1.27 1.27))))
		(property "Value" "R" (at 0 0 90))
		(property "Footprint" "" (at -1.778 0 90))
		(property "Description" "Resistor, \"quoted\"" (at 0 0 0))
		(property "ki_keywords" "R res resistor" (at 0 0 0))
		(property "ki_fp_filters" "R_*  Resistor:*" (at 0 0 0))
		(symbol "R_0_1" (rectangle (start -1.016 -2.54) (end 1.016 2.54)))
		(symbol "R_1_1"
			(pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
			(pin passive line (at 0 -3.81 90) (length 1.27) (name "~") (number "2"))
		)
		(embedded_fonts no)
	)
	(symbol "R_Small" (extends "R")
		(property "Reference" "R" (at 0 0 0))
		(property "Description" "Resistor, small symbol" (at 0 0 0))
	)
	(symbol "GND" (power)
		(property "Reference" "#PWR" (at 0 0 0))
		(property "Footprint" "" (at 0 0 0))
		(symbol "GND_0_1" (pin power_in line (at 0 0 270) (length 0) hide (name "GND") (number "1")))
	)
	(symbol "Dual"
		(property "Reference" "U")
		(property "Footprint" "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm")
		(symbol "Dual_1_1" (pin input line (number "1")) (pin input line (number "2")))
		(symbol "Dual_2_1" (pin input line (number "2")) (pin input line (number 3)) (pin passive line (number "")))
	)
)
"##;

    #[test]
    fn reads_what_the_browser_needs_from_symbols() {
        let summary = read_symbol_file(LIBRARY.as_bytes()).unwrap();
        assert_eq!(summary.version.as_deref(), Some("20241209"));
        let names: Vec<&str> = summary.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["R", "R_Small", "GND", "Dual"]);
        let r = &summary.symbols[0];
        assert_eq!(r.description, "Resistor, \"quoted\"");
        assert_eq!(r.keywords, "R res resistor");
        assert_eq!(r.reference, "R");
        assert_eq!(r.footprint, "");
        assert_eq!(r.footprint_filters, ["R_*", "Resistor:*"]);
        assert_eq!(r.pin_count, 2);
        assert!(!r.power);
        assert!(LIBRARY[r.span.clone()].starts_with("(symbol \"R\""));
        assert!(LIBRARY[r.span.clone()].ends_with(')'));
        // A derived symbol has its parent's pins and filters, its own text.
        let small = &summary.symbols[1];
        assert_eq!(small.extends.as_deref(), Some("R"));
        assert_eq!(small.pin_count, 2);
        assert_eq!(small.description, "Resistor, small symbol");
        assert_eq!(small.footprint_filters, ["R_*", "Resistor:*"]);
        let gnd = &summary.symbols[2];
        assert!(gnd.power);
        assert_eq!(gnd.reference, "#PWR");
        assert_eq!(gnd.pin_count, 1);
        // Distinct numbers across units; a blank number is not a pin.
        let dual = &summary.symbols[3];
        assert_eq!(dual.pin_count, 3);
        assert_eq!(dual.footprint, "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm");
    }

    #[test]
    fn reads_what_the_browser_needs_from_footprints() {
        let footprint = r#"(footprint "R_0603_1608Metric"
	(version 20240108)
	(descr "Resistor SMD 0603 (1608 Metric)")
	(tags "resistor")
	(property "Reference" "REF**" (at 0 -1.43 0))
	(attr smd)
	(fp_line (start -0.8 0.4) (end 0.8 0.4))
	(pad "1" smd roundrect (at -0.825 0) (size 0.8 0.95) (layers "F.Cu" "F.Mask" "F.Paste"))
	(pad "2" smd roundrect (at 0.825 0) (size 0.8 0.95))
	(pad "2" smd roundrect (at 0.9 0) (size 0.8 0.95))
	(pad "" np_thru_hole circle (at 0 2) (size 1 1) (drill 1))
	(model "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step" (offset (xyz 0 0 0)))
)"#;
        let summary = read_footprint_file(footprint.as_bytes()).unwrap();
        assert_eq!(summary.name, "R_0603_1608Metric");
        assert_eq!(summary.description, "Resistor SMD 0603 (1608 Metric)");
        assert_eq!(summary.keywords, "resistor");
        assert_eq!(summary.pad_count, 2);
        assert_eq!(summary.mount, Mount::Smd);
        assert_eq!(
            summary.model.as_deref(),
            Some("${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step")
        );
        // No attr: the pads decide; KiCad 5's module and unquoted numbers.
        let old = "(module Pin (layer F.Cu) (pad 1 thru_hole circle (at 0 0) (size 2 2) (drill 1)) (pad 2 smd rect (at 0 0) (size 1 1)))";
        let summary = read_footprint_file(old.as_bytes()).unwrap();
        assert_eq!(summary.mount, Mount::ThroughHole);
        assert_eq!(summary.pad_count, 2);
        let hole = "(footprint \"MountingHole\" (attr exclude_from_pos_files) (pad \"\" np_thru_hole circle (at 0 0) (size 3 3) (drill 3)))";
        let summary = read_footprint_file(hole.as_bytes()).unwrap();
        assert_eq!(summary.mount, Mount::Other);
        assert_eq!(summary.pad_count, 0);
        assert_eq!(summary.model, None);
    }

    #[test]
    fn refuses_broken_and_hostile_files() {
        for bad in [
            "(kicad_symbol_lib (symbol \"A\"",
            "(kicad_symbol_lib (symbol \"A\")))",
            "(kicad_symbol_lib (symbol \"A\")) (x)",
            "(footprint \"A\"",
            "(kicad_symbol_lib (symbol \"A\" (property \"x)))",
        ] {
            assert!(
                read_symbol_file(bad.as_bytes()).is_err()
                    || read_footprint_file(bad.as_bytes()).is_err(),
                "{bad}"
            );
        }
        assert!(read_symbol_file(b"(footprint \"A\")").is_err());
        assert!(read_footprint_file(b"(kicad_symbol_lib)").is_err());
        let deep = format!(
            "(kicad_symbol_lib (symbol \"A\" {}{}))",
            "(a ".repeat(100_000),
            ")".repeat(100_000)
        );
        let error = read_symbol_file(deep.as_bytes()).unwrap_err();
        assert!(error.0.contains("nested too deeply"), "{error}");
        let deep = format!(
            "(footprint \"A\" {}{})",
            "(a ".repeat(100_000),
            ")".repeat(100_000)
        );
        assert!(read_footprint_file(deep.as_bytes()).is_err());
    }
}

/// Reads every library of the installed KiCad and says how long it took:
/// `cargo test -p kicad-integration --lib -- --ignored --nocapture kicads_own`.
#[cfg(test)]
mod kicads_own {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore = "reads all 430 MB of KiCad's libraries"]
    fn every_kicad_library_reads() {
        let Some(share) = crate::installed_share_dir(&Default::default()) else {
            eprintln!("KiCad is not installed; skipped");
            return;
        };
        let started = Instant::now();
        let (mut symbols, mut bytes) = (0usize, 0usize);
        for entry in std::fs::read_dir(share.join("symbols")).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "kicad_sym") {
                let data = std::fs::read(&path).unwrap();
                bytes += data.len();
                let summary = read_symbol_file(&data)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                symbols += summary.symbols.len();
            }
        }
        let symbol_time = started.elapsed();
        let started = Instant::now();
        let mut footprints = 0usize;
        for library in std::fs::read_dir(share.join("footprints"))
            .unwrap()
            .flatten()
        {
            for entry in std::fs::read_dir(library.path()).unwrap().flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "kicad_mod") {
                    let data = std::fs::read(&path).unwrap();
                    bytes += data.len();
                    read_footprint_file(&data)
                        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                    footprints += 1;
                }
            }
        }
        eprintln!(
            "{symbols} symbols in {symbol_time:?}, {footprints} footprints in {:?}, {} MB",
            started.elapsed(),
            bytes / 1_000_000
        );
        assert!(symbols > 10_000 && footprints > 10_000);
    }
}

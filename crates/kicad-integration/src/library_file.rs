//! Reads saved KiCad library files: the entries of a `.kicad_sym` symbol
//! library, each with its exact byte span and whether it is in the BOM, and
//! a `.kicad_mod` footprint's version, 3D model references and whether it
//! is excluded from the BOM.
//!
//! Only as much s-expression syntax as that needs: lists, atoms and quoted
//! strings with backslash escapes. Spans are byte ranges of the input, so a
//! caller can store one symbol's bytes exactly as they were saved.

use std::fmt;
use std::ops::Range;

/// A library file that is not well-formed enough to read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryFileError(pub String);

impl fmt::Display for LibraryFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LibraryFileError {}

/// A `.kicad_sym` library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolLibrary {
    /// The library's `(version ...)`.
    pub version: Option<String>,
    /// Every top-level `(symbol ...)`, in file order.
    pub entries: Vec<SymbolEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolEntry {
    pub name: String,
    /// From the entry's `(` to its `)`, inclusive.
    pub span: Range<usize>,
    /// The parent this symbol `(extends ...)`, if any.
    pub extends: Option<String>,
    /// The symbol's own `(in_bom yes|no)`; `None` when it does not say.
    pub in_bom: Option<bool>,
}

/// The result of looking a symbol up by name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolLookup<'a> {
    Found(&'a SymbolEntry),
    Missing,
    /// More than one top-level entry has the name.
    Ambiguous,
}

impl SymbolLibrary {
    /// Reads a `.kicad_sym` file.
    pub fn parse(bytes: &[u8]) -> Result<Self, LibraryFileError> {
        let (head, children) = root_children(bytes)?;
        if head != "kicad_symbol_lib" {
            return Err(LibraryFileError(format!(
                "a symbol library starts with `kicad_symbol_lib`, not `{head}`"
            )));
        }
        let mut entries = Vec::new();
        for child in children.iter().filter(|child| child.head == "symbol") {
            let name = child
                .first
                .clone()
                .ok_or_else(|| LibraryFileError("a symbol has no name".into()))?;
            let (_, inner) = root_children(&bytes[child.span.clone()])?;
            let extends = inner
                .iter()
                .find(|item| item.head == "extends")
                .and_then(|item| item.first.clone());
            let in_bom =
                inner
                    .iter()
                    .find(|item| item.head == "in_bom")
                    .and_then(|item| match item.first.as_deref() {
                        Some("yes") => Some(true),
                        Some("no") => Some(false),
                        _ => None,
                    });
            entries.push(SymbolEntry {
                name,
                span: child.span.clone(),
                extends,
                in_bom,
            });
        }
        Ok(Self {
            version: file_version(&children),
            entries,
        })
    }

    /// The top-level symbol named exactly `name`.
    pub fn find(&self, name: &str) -> SymbolLookup<'_> {
        let mut matches = self.entries.iter().filter(|entry| entry.name == name);
        match (matches.next(), matches.next()) {
            (None, _) => SymbolLookup::Missing,
            (Some(entry), None) => SymbolLookup::Found(entry),
            (Some(_), Some(_)) => SymbolLookup::Ambiguous,
        }
    }
}

/// A `.kicad_mod` footprint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FootprintFile {
    /// The footprint's own name, the first word after `footprint`.
    pub name: Option<String>,
    /// The footprint's `(version ...)`.
    pub version: Option<String>,
    /// Every `(model "...")` path, in file order, exactly as written.
    pub model_references: Vec<String>,
    /// The footprint's `(attr ...)` includes `exclude_from_bom`.
    pub exclude_from_bom: bool,
}

impl FootprintFile {
    /// Reads a `.kicad_mod` file (`footprint`, or the older `module`).
    pub fn parse(bytes: &[u8]) -> Result<Self, LibraryFileError> {
        let (head, children) = root_children(bytes)?;
        if head != "footprint" && head != "module" {
            return Err(LibraryFileError(format!(
                "a footprint starts with `footprint` or `module`, not `{head}`"
            )));
        }
        let mut exclude_from_bom = false;
        for attr in children.iter().filter(|child| child.head == "attr") {
            let text = &bytes[attr.span.clone()];
            exclude_from_bom |= tokens(text)?.iter().any(|token| {
                matches!(token, Token::Atom(range) if &text[range.clone()] == b"exclude_from_bom")
            });
        }
        let name = match tokens(bytes)?.get(2) {
            Some(Token::Atom(range)) => {
                Some(String::from_utf8_lossy(&bytes[range.clone()]).into_owned())
            }
            Some(Token::Text(value)) => Some(value.clone()),
            _ => None,
        };
        Ok(Self {
            name,
            version: file_version(&children),
            model_references: model_references(&String::from_utf8_lossy(bytes)),
            exclude_from_bom,
        })
    }
}

/// Whether the bytes of a symbol CAD item revision (one or more top-level
/// `(symbol ...)` entries) have an entry that says `(in_bom no)`. Bytes that
/// do not read as symbol entries say nothing. The team server's
/// catalog_lib.js symbolSaysNotInBom reads them the same way.
pub fn symbol_entries_say_not_in_bom(bytes: &[u8]) -> bool {
    let mut library = b"(kicad_symbol_lib ".to_vec();
    library.extend_from_slice(bytes);
    library.push(b')');
    SymbolLibrary::parse(&library).is_ok_and(|library| {
        library
            .entries
            .iter()
            .any(|entry| entry.in_bom == Some(false))
    })
}

/// The value of a symbol's own `(property "Reference" "...")`, such as `R`
/// or `U?`, from the bytes of one top-level `(symbol ...)` entry. `None`
/// when the bytes do not read as one symbol or it has no Reference
/// property (a symbol that extends another inherits its parent's).
pub fn symbol_reference(entry: &[u8]) -> Option<String> {
    let (head, children) = root_children(entry).ok()?;
    if head != "symbol" {
        return None;
    }
    let property = children
        .iter()
        .find(|child| child.head == "property" && child.first.as_deref() == Some("Reference"))?;
    // (property "Reference" "VALUE" ...): the value is the fourth token.
    match tokens(&entry[property.span.clone()]).ok()?.get(3)? {
        Token::Text(value) => Some(value.clone()),
        Token::Atom(range) => Some(
            String::from_utf8_lossy(&entry[property.span.start + range.start..][..range.len()])
                .into_owned(),
        ),
        _ => None,
    }
}

/// Whether the bytes of a footprint (a whole `.kicad_mod`) have
/// `exclude_from_bom` in an `(attr ...)`. Bytes that do not read as a
/// footprint say nothing. catalog_lib.js footprintExcludedFromBom reads them
/// the same way.
pub fn footprint_says_excluded_from_bom(bytes: &[u8]) -> bool {
    FootprintFile::parse(bytes).is_ok_and(|footprint| footprint.exclude_from_bom)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    Open(usize),
    Close(usize),
    Atom(Range<usize>),
    /// A quoted string, unescaped.
    Text(String),
}

fn tokens(bytes: &[u8]) -> Result<Vec<Token>, LibraryFileError> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => {
                tokens.push(Token::Open(index));
                index += 1;
            }
            b')' => {
                tokens.push(Token::Close(index));
                index += 1;
            }
            b'"' => {
                let mut value = Vec::new();
                index += 1;
                loop {
                    match bytes.get(index) {
                        None => return Err(LibraryFileError("unterminated string".into())),
                        Some(b'\\') => {
                            if let Some(next) = bytes.get(index + 1) {
                                value.push(*next);
                            }
                            index += 2;
                        }
                        Some(b'"') => {
                            index += 1;
                            break;
                        }
                        Some(byte) => {
                            value.push(*byte);
                            index += 1;
                        }
                    }
                }
                tokens.push(Token::Text(String::from_utf8_lossy(&value).into_owned()));
            }
            byte if byte.is_ascii_whitespace() => index += 1,
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'(' | b')' | b'"')
                {
                    index += 1;
                }
                tokens.push(Token::Atom(start..index));
            }
        }
    }
    Ok(tokens)
}

/// A direct child list of the root list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Child {
    pub(crate) head: String,
    /// The first argument after the head, if it is an atom or a string.
    pub(crate) first: Option<String>,
    pub(crate) span: Range<usize>,
}

/// The root list's head and its direct child lists.
pub(crate) fn root_children(bytes: &[u8]) -> Result<(String, Vec<Child>), LibraryFileError> {
    let invalid = |message: &str| LibraryFileError(message.to_owned());
    let tokens = tokens(bytes)?;
    let atom = |token: Option<&Token>| match token {
        Some(Token::Atom(range)) => {
            Some(String::from_utf8_lossy(&bytes[range.clone()]).into_owned())
        }
        Some(Token::Text(value)) => Some(value.clone()),
        _ => None,
    };
    if !matches!(tokens.first(), Some(Token::Open(_))) {
        return Err(invalid("the file does not start with a list"));
    }
    let root_head = atom(tokens.get(1)).ok_or_else(|| invalid("the root list has no head"))?;
    let mut children = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<(usize, usize)> = None;
    for (position, token) in tokens.iter().enumerate() {
        match token {
            Token::Open(offset) => {
                depth += 1;
                if depth == 2 {
                    open = Some((position, *offset));
                }
            }
            Token::Close(offset) => {
                if depth == 0 {
                    return Err(invalid("unbalanced `)`"));
                }
                if depth == 2 {
                    let (start_token, start) = open.take().expect("a depth-2 list was opened");
                    children.push(Child {
                        head: atom(tokens.get(start_token + 1)).unwrap_or_default(),
                        first: atom(tokens.get(start_token + 2)),
                        span: start..offset + 1,
                    });
                }
                depth -= 1;
                if depth == 0 && position + 1 != tokens.len() {
                    return Err(invalid("content after the root list"));
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(invalid("unbalanced `(`"));
    }
    Ok((root_head, children))
}

fn file_version(children: &[Child]) -> Option<String> {
    children
        .iter()
        .find(|child| child.head == "version")
        .and_then(|child| child.first.clone())
}

/// Every 3D model path in a footprint, in file order, extracted exactly as
/// the CSV catalog fixture (`tooling/csv-catalog-fixture`) does, so the two
/// always agree on what a footprint references.
pub fn model_references(content: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for suffix in content.split("(model").skip(1) {
        let value = suffix.trim_start();
        if let Some(value) = value.strip_prefix('"') {
            let mut escaped = false;
            let mut output = String::new();
            for ch in value.chars() {
                if !escaped && ch == '"' {
                    break;
                }
                escaped = !escaped && ch == '\\';
                output.push(ch);
                if ch != '\\' {
                    escaped = false;
                }
            }
            paths.push(output);
        }
    }
    paths
}

/// One node of a KiCad s-expression: a list, or a word (an atom or a quoted
/// string, unescaped).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Sexp {
    List(Vec<Sexp>),
    Word(String),
}

/// The deepest nesting [`Sexp::parse`] reads. KiCad's files nest a few
/// levels; a downloaded file nested deeper is refused, because walking or
/// dropping a deeper tree could overflow the stack.
pub const MAX_DEPTH: usize = 256;

impl Sexp {
    /// Reads `bytes` as exactly one list, nested at most [`MAX_DEPTH`]
    /// deep.
    pub fn parse(bytes: &[u8]) -> Result<Self, LibraryFileError> {
        let tokens = tokens(bytes)?;
        let mut stack: Vec<Vec<Sexp>> = Vec::new();
        let mut done = None;
        for token in tokens {
            if done.is_some() {
                return Err(LibraryFileError("content after the root list".into()));
            }
            match token {
                Token::Open(_) if stack.len() >= MAX_DEPTH => {
                    return Err(LibraryFileError(format!(
                        "the file is nested too deeply (more than {MAX_DEPTH} levels) to be a KiCad file"
                    )));
                }
                Token::Open(_) => stack.push(Vec::new()),
                Token::Close(_) => {
                    let list = stack
                        .pop()
                        .ok_or_else(|| LibraryFileError("unbalanced `)`".into()))?;
                    match stack.last_mut() {
                        Some(parent) => parent.push(Sexp::List(list)),
                        None => done = Some(Sexp::List(list)),
                    }
                }
                Token::Atom(range) => stack
                    .last_mut()
                    .ok_or_else(|| LibraryFileError("the file does not start with a list".into()))?
                    .push(Sexp::Word(
                        String::from_utf8_lossy(&bytes[range]).into_owned(),
                    )),
                Token::Text(text) => stack
                    .last_mut()
                    .ok_or_else(|| LibraryFileError("the file does not start with a list".into()))?
                    .push(Sexp::Word(text)),
            }
        }
        done.ok_or_else(|| LibraryFileError("unbalanced `(`".into()))
    }

    /// The word, if this is one.
    pub fn word(&self) -> Option<&str> {
        match self {
            Self::Word(word) => Some(word),
            Self::List(_) => None,
        }
    }

    /// The items of a list; empty for a word.
    pub fn items(&self) -> &[Sexp] {
        match self {
            Self::List(items) => items,
            Self::Word(_) => &[],
        }
    }

    /// The list's head word, such as `pad` for `(pad "1" smd ...)`.
    pub fn head(&self) -> Option<&str> {
        self.items().first().and_then(Sexp::word)
    }

    /// The word at `index` of a list (0 is the head).
    pub fn word_at(&self, index: usize) -> Option<&str> {
        self.items().get(index).and_then(Sexp::word)
    }

    /// The direct child lists whose head is `head`.
    pub fn children<'a>(&'a self, head: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.items()
            .iter()
            .filter(move |item| item.head() == Some(head))
    }

    /// Every list at any depth below this one whose head is `head`.
    pub fn descendants<'a>(&'a self, head: &str, found: &mut Vec<&'a Sexp>) {
        for item in self.items() {
            if item.head() == Some(head) {
                found.push(item);
            }
            item.descendants(head, found);
        }
    }
}

/// The `(property "NAME" "VALUE" ...)` pairs of one top-level `(symbol ...)`
/// entry, in file order. Empty when the bytes are not a symbol.
pub fn symbol_properties(entry: &[u8]) -> Vec<(String, String)> {
    let Ok(tree) = Sexp::parse(entry) else {
        return Vec::new();
    };
    if tree.head() != Some("symbol") {
        return Vec::new();
    }
    tree.children("property")
        .filter_map(|property| {
            Some((
                property.word_at(1)?.to_owned(),
                property.word_at(2)?.to_owned(),
            ))
        })
        .collect()
}

/// The distinct pin numbers of one top-level `(symbol ...)` entry, across
/// all its units and body styles. Pins without a number are left out.
pub fn symbol_pin_numbers(entry: &[u8]) -> std::collections::BTreeSet<String> {
    let mut numbers = std::collections::BTreeSet::new();
    let Ok(tree) = Sexp::parse(entry) else {
        return numbers;
    };
    let mut pins = Vec::new();
    tree.descendants("pin", &mut pins);
    for pin in pins {
        if let Some(number) = pin.children("number").find_map(|number| number.word_at(1)) {
            if !number.trim().is_empty() {
                numbers.insert(number.to_owned());
            }
        }
    }
    numbers
}

/// One `(pad ...)` of a footprint.
#[derive(Clone, Debug, PartialEq)]
pub struct Pad {
    /// Blank for an unnumbered pad, such as a mechanical one.
    pub number: String,
    /// `smd`, `thru_hole`, `np_thru_hole` or `connect`.
    pub kind: String,
    /// `rect`, `roundrect`, `circle`, `oval`, `trapezoid` or `custom`.
    pub shape: String,
    /// The pad's position, in millimetres.
    pub at: (f64, f64),
    /// The pad's rotation, in degrees (the third number of `at`).
    pub rotation: f64,
    /// The pad's size, in millimetres.
    pub size: (f64, f64),
    /// The drill, as written: its shape (`oval`) if any, then its sizes in
    /// millimetres; empty for a pad without a hole.
    pub drill: Vec<String>,
}

/// Every pad of a footprint (a whole `.kicad_mod`), in file order. Empty
/// when the bytes are not a footprint.
pub fn footprint_pads(bytes: &[u8]) -> Vec<Pad> {
    let Ok(tree) = Sexp::parse(bytes) else {
        return Vec::new();
    };
    if !matches!(tree.head(), Some("footprint" | "module")) {
        return Vec::new();
    }
    let pair = |pad: &Sexp, head: &str| {
        pad.children(head)
            .next()
            .map(|item| {
                let number = |index| {
                    item.word_at(index)
                        .and_then(|word| word.parse::<f64>().ok())
                        .unwrap_or(0.0)
                };
                (number(1), number(2))
            })
            .unwrap_or((0.0, 0.0))
    };
    tree.children("pad")
        .map(|pad| Pad {
            number: pad.word_at(1).unwrap_or_default().to_owned(),
            kind: pad.word_at(2).unwrap_or_default().to_owned(),
            shape: pad.word_at(3).unwrap_or_default().to_owned(),
            at: pair(pad, "at"),
            rotation: pad
                .children("at")
                .next()
                .and_then(|at| at.word_at(3))
                .and_then(|word| word.parse::<f64>().ok())
                .unwrap_or(0.0),
            drill: pad
                .children("drill")
                .next()
                .map(|drill| {
                    drill.items()[1..]
                        .iter()
                        .filter_map(Sexp::word)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            size: pair(pad, "size"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_nesting_is_refused_not_overflowed() {
        // A hostile file nested far deeper than any KiCad file.
        let deep = |head: &str| {
            let mut text = format!("({head} \"X\" ");
            text.push_str(&"(a ".repeat(100_000));
            text.push_str(&")".repeat(100_000));
            text.push(')');
            text
        };
        let footprint = deep("footprint");
        let error = Sexp::parse(footprint.as_bytes()).unwrap_err();
        assert!(error.0.contains("nested too deeply"), "{error}");
        assert!(footprint_pads(footprint.as_bytes()).is_empty());
        let symbol = deep("symbol");
        assert!(symbol_pin_numbers(symbol.as_bytes()).is_empty());
        assert!(symbol_properties(symbol.as_bytes()).is_empty());
        // KiCad's own depth is fine.
        let normal = format!("(footprint \"X\" {}{})", "(a ".repeat(200), ")".repeat(200));
        assert!(Sexp::parse(normal.as_bytes()).is_ok());
    }

    #[test]
    fn reads_properties_pins_and_pads() {
        let entry = "(symbol \"U\" (property \"Reference\" \"U\") (property \"MPN\" \"LM317\")\n\t(symbol \"U_1_1\" (pin input line (at 0 0 0) (number \"1\")) (pin input line (number \"2\")))\n\t(symbol \"U_2_1\" (pin power_in line (number \"2\")) (pin passive line (number \"\"))))";
        assert_eq!(
            symbol_properties(entry.as_bytes()),
            [
                ("Reference".to_owned(), "U".to_owned()),
                ("MPN".to_owned(), "LM317".to_owned())
            ]
        );
        let pins: Vec<String> = symbol_pin_numbers(entry.as_bytes()).into_iter().collect();
        assert_eq!(pins, ["1", "2"]);
        let footprint = "(footprint \"F\" (pad \"1\" smd roundrect (at -0.8 0) (size 0.9 0.95)) (pad \"\" np_thru_hole circle (at 0 2) (size 1 1)) (pad 2 smd rect (at 0.8 0 90) (size 0.9 0.95)))";
        let pads = footprint_pads(footprint.as_bytes());
        assert_eq!(pads.len(), 3);
        assert_eq!(pads[0].number, "1");
        assert_eq!(pads[0].shape, "roundrect");
        assert_eq!(pads[0].at, (-0.8, 0.0));
        assert_eq!(pads[1].kind, "np_thru_hole");
        assert_eq!(pads[2].number, "2");
        assert_eq!(pads[2].size, (0.9, 0.95));
        assert_eq!(pads[2].rotation, 90.0);
        assert!(pads[0].drill.is_empty());
        assert!(footprint_pads(b"(kicad_symbol_lib)").is_empty());
        assert!(symbol_pin_numbers(b"(symbol").is_empty());
    }

    const LIBRARY: &str = "(kicad_symbol_lib\n\t(version 20241209)\n\t(generator \"x\")\n\t(symbol \"C\"\n\t\t(property \"Value\" \"a (b\\\" c\")\n\t)\n\t(symbol \"R_US\"\n\t\t(symbol \"R_US_0_1\")\n\t)\n\t(symbol \"R_Child\"\n\t\t(extends \"R_US\")\n\t)\n)\n";

    #[test]
    fn reads_a_symbols_own_reference() {
        let entry = "(symbol \"R_US\"\n\t\t(property \"Value\" \"R\")\n\t\t(property \"Reference\" \"U?\" (at 0 0 0))\n\t)";
        assert_eq!(symbol_reference(entry.as_bytes()).as_deref(), Some("U?"));
        // A symbol without one, or bytes that are not a symbol, say nothing.
        assert_eq!(
            symbol_reference(b"(symbol \"C\" (property \"Value\" \"C\"))"),
            None
        );
        assert_eq!(symbol_reference(b"(footprint \"R\")"), None);
        assert_eq!(symbol_reference(b"(symbol \"C\""), None);
    }

    #[test]
    fn cuts_top_level_symbols_exactly() {
        let library = SymbolLibrary::parse(LIBRARY.as_bytes()).unwrap();
        assert_eq!(library.version.as_deref(), Some("20241209"));
        let names: Vec<&str> = library.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["C", "R_US", "R_Child"]);
        let SymbolLookup::Found(entry) = library.find("R_US") else {
            panic!("R_US is in the library");
        };
        assert_eq!(
            &LIBRARY[entry.span.clone()],
            "(symbol \"R_US\"\n\t\t(symbol \"R_US_0_1\")\n\t)"
        );
        assert_eq!(entry.extends, None);
        assert_eq!(
            &LIBRARY[library.entries[0].span.clone()],
            "(symbol \"C\"\n\t\t(property \"Value\" \"a (b\\\" c\")\n\t)"
        );
        let SymbolLookup::Found(child) = library.find("R_Child") else {
            panic!("R_Child is in the library");
        };
        assert_eq!(child.extends.as_deref(), Some("R_US"));
        // Units nested inside a symbol are not top-level entries.
        assert_eq!(library.find("R_US_0_1"), SymbolLookup::Missing);
        let twice = "(kicad_symbol_lib (symbol \"A\") (symbol \"A\"))";
        assert_eq!(
            SymbolLibrary::parse(twice.as_bytes()).unwrap().find("A"),
            SymbolLookup::Ambiguous
        );
    }

    #[test]
    fn rejects_malformed_files() {
        for bad in [&b"(a (b)"[..], b"(a))", b"(a) (b)", b"(a \"x)", b"x"] {
            assert!(
                root_children(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(SymbolLibrary::parse(b"(footprint \"x\")").is_err());
        assert!(FootprintFile::parse(b"(kicad_symbol_lib)").is_err());
    }

    #[test]
    fn reads_footprint_versions_and_model_references_like_the_oracle() {
        let footprint = "(footprint \"R\"\n (version 20241229)\n (model \"${KICAD9_3DMODEL_DIR}/R.step\"\n (offset)) (model \"${GITPLM_PARTS}/3d-models/x.step\"))";
        let file = FootprintFile::parse(footprint.as_bytes()).unwrap();
        assert_eq!(file.name.as_deref(), Some("R"));
        assert_eq!(file.version.as_deref(), Some("20241229"));
        assert!(!file.exclude_from_bom);
        assert_eq!(
            file.model_references,
            vec![
                "${KICAD9_3DMODEL_DIR}/R.step".to_owned(),
                "${GITPLM_PARTS}/3d-models/x.step".to_owned()
            ]
        );
    }

    #[test]
    fn reads_whether_symbols_and_footprints_are_in_the_bom() {
        let library = "(kicad_symbol_lib (version 1) (symbol \"TP\" (in_bom no) (on_board yes)) (symbol \"R\" (in_bom yes)) (symbol \"C\"))";
        let library = SymbolLibrary::parse(library.as_bytes()).unwrap();
        let in_bom: Vec<Option<bool>> = library.entries.iter().map(|entry| entry.in_bom).collect();
        assert_eq!(in_bom, [Some(false), Some(true), None]);
        for (footprint, excluded) in [
            (
                "(footprint \"F\" (attr smd exclude_from_pos_files exclude_from_bom))",
                true,
            ),
            ("(footprint \"F\" (attr exclude_from_bom))", true),
            (
                "(footprint \"F\" (attr smd) (fp_text value \"exclude_from_bom\"))",
                false,
            ),
            ("(footprint \"F\")", false),
        ] {
            let file = FootprintFile::parse(footprint.as_bytes()).unwrap();
            assert_eq!(file.exclude_from_bom, excluded, "{footprint}");
        }
    }
}

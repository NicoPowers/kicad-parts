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
        Ok(Self {
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
struct Child {
    head: String,
    /// The first argument after the head, if it is an atom or a string.
    first: Option<String>,
    span: Range<usize>,
}

/// The root list's head and its direct child lists.
fn root_children(bytes: &[u8]) -> Result<(String, Vec<Child>), LibraryFileError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = "(kicad_symbol_lib\n\t(version 20241209)\n\t(generator \"x\")\n\t(symbol \"C\"\n\t\t(property \"Value\" \"a (b\\\" c\")\n\t)\n\t(symbol \"R_US\"\n\t\t(symbol \"R_US_0_1\")\n\t)\n\t(symbol \"R_Child\"\n\t\t(extends \"R_US\")\n\t)\n)\n";

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

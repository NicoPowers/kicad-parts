//! Just enough of KiCad's s-expression syntax to cut one symbol out of a
//! library file byte-for-byte and to read file versions and 3D model
//! references.

use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    Open(usize),
    Close(usize),
    Atom(Range<usize>),
    /// A quoted string: its full span and its unescaped value.
    Text(String),
}

fn tokens(bytes: &[u8]) -> Result<Vec<Token>, String> {
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
                        None => return Err("unterminated string".into()),
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

/// A direct child list of the file's root list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Child {
    pub head: String,
    /// The first argument after the head, unescaped, if it is an atom or a
    /// string.
    pub first: Option<String>,
    /// From its `(` to its `)`, inclusive.
    pub span: Range<usize>,
}

/// The root list's head and its direct child lists.
pub fn root_children(bytes: &[u8]) -> Result<(String, Vec<Child>), String> {
    let tokens = tokens(bytes)?;
    let atom = |token: Option<&Token>| match token {
        Some(Token::Atom(range)) => {
            Some(String::from_utf8_lossy(&bytes[range.clone()]).into_owned())
        }
        Some(Token::Text(value)) => Some(value.clone()),
        _ => None,
    };
    if !matches!(tokens.first(), Some(Token::Open(_))) {
        return Err("the file does not start with a list".into());
    }
    let root_head = atom(tokens.get(1)).ok_or("the root list has no head")?;
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
                    return Err("unbalanced `)`".into());
                }
                if depth == 2 {
                    let (start_token, start) = open.take().expect("a depth-2 list was opened");
                    let head = atom(tokens.get(start_token + 1)).unwrap_or_default();
                    let first = atom(tokens.get(start_token + 2));
                    children.push(Child {
                        head,
                        first,
                        span: start..offset + 1,
                    });
                }
                depth -= 1;
                if depth == 0 && position + 1 != tokens.len() {
                    return Err("content after the root list".into());
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unbalanced `(`".into());
    }
    Ok((root_head, children))
}

/// The root list's `(version ...)`, if any.
pub fn file_version(children: &[Child]) -> Option<String> {
    children
        .iter()
        .find(|child| child.head == "version")
        .and_then(|child| child.first.clone())
}

/// Every 3D model path in a footprint, in file order, extracted exactly as
/// the import oracle does (`tooling/import-fixture`), so the two always
/// agree on what a footprint references.
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

    const LIBRARY: &str = "(kicad_symbol_lib\n\t(version 20241209)\n\t(generator \"x\")\n\t(symbol \"C\"\n\t\t(property \"Value\" \"a (b\\\" c\")\n\t)\n\t(symbol \"R_US\"\n\t\t(symbol \"R_US_0_1\")\n\t)\n)\n";

    #[test]
    fn cuts_top_level_symbols_exactly() {
        let (head, children) = root_children(LIBRARY.as_bytes()).unwrap();
        assert_eq!(head, "kicad_symbol_lib");
        assert_eq!(file_version(&children).as_deref(), Some("20241209"));
        let symbols: Vec<_> = children
            .iter()
            .filter(|child| child.head == "symbol")
            .collect();
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[1].first.as_deref(), Some("R_US"));
        assert_eq!(
            &LIBRARY[symbols[1].span.clone()],
            "(symbol \"R_US\"\n\t\t(symbol \"R_US_0_1\")\n\t)"
        );
        assert_eq!(
            &LIBRARY[symbols[0].span.clone()],
            "(symbol \"C\"\n\t\t(property \"Value\" \"a (b\\\" c\")\n\t)"
        );
    }

    #[test]
    fn rejects_unbalanced_files() {
        assert!(root_children(b"(a (b)").is_err());
        assert!(root_children(b"(a))").is_err());
        assert!(root_children(b"(a) (b)").is_err());
        assert!(root_children(b"(a \"x)").is_err());
    }

    #[test]
    fn finds_model_references_like_the_oracle() {
        let footprint = "(footprint \"R\"\n (model \"${KICAD9_3DMODEL_DIR}/R.step\"\n (offset)) (model \"${GITPLM_PARTS}/3d-models/x.step\"))";
        assert_eq!(
            model_references(footprint),
            vec![
                "${KICAD9_3DMODEL_DIR}/R.step".to_owned(),
                "${GITPLM_PARTS}/3d-models/x.step".to_owned()
            ]
        );
    }
}

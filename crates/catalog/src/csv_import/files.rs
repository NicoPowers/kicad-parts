//! Read-only, fail-closed access to CSV catalog files and their exact
//! CSV records.

use super::ImportError;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Reads `relative` (a `/`-separated repository path) under `root`.
///
/// Like the import oracle, it accepts only regular files whose every path
/// component exists with exactly this case, and it refuses links and
/// Windows reparse points before reading anything through them.
pub fn read_input(root: &Path, relative: &str) -> Result<Vec<u8>, ImportError> {
    let path = exact_file(root, relative)?
        .ok_or_else(|| ImportError(format!("`{relative}` does not exist")))?;
    fs::read(&path).map_err(|io| ImportError(format!("cannot read `{relative}`: {io}")))
}

/// True if `relative` names an exact-case regular file under `root`.
pub fn input_exists(root: &Path, relative: &str) -> Result<bool, ImportError> {
    Ok(exact_file(root, relative)?.is_some())
}

/// The file at `relative`, if it exists with exactly this case.
pub fn exact_file(root: &Path, relative: &str) -> Result<Option<PathBuf>, ImportError> {
    let unsafe_path = || ImportError(format!("`{relative}` is not a safe repository path"));
    if relative.is_empty() || relative.contains('\\') || relative.contains(':') {
        return Err(unsafe_path());
    }
    let components: Vec<_> = Path::new(relative).components().collect();
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(unsafe_path());
        };
        let entries = fs::read_dir(&current)
            .map_err(|io| ImportError(format!("cannot list `{}`: {io}", current.display())))?;
        let mut found = None;
        for entry in entries {
            let entry = entry
                .map_err(|io| ImportError(format!("cannot list `{}`: {io}", current.display())))?;
            if entry.file_name() == *name {
                found = Some(entry.path());
                break;
            }
        }
        let Some(next) = found else {
            return Ok(None);
        };
        let metadata = fs::symlink_metadata(&next)
            .map_err(|io| ImportError(format!("cannot inspect `{relative}`: {io}")))?;
        if is_link(&metadata) {
            return Err(ImportError(format!(
                "`{relative}` passes through a link, which imports refuse"
            )));
        }
        if index + 1 == components.len() {
            return Ok(metadata.is_file().then_some(next));
        }
        if !metadata.is_dir() {
            return Ok(None);
        }
        current = next;
    }
    Ok(None)
}

#[cfg(windows)]
fn is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// One CSV record with the exact bytes it was parsed from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CsvRecord {
    /// 1-based position among the data rows (the header is row 0).
    pub row_number: usize,
    /// The fields as parsed, without padding.
    pub fields: Vec<String>,
    /// From the record's first byte to the next record's first byte (or the
    /// end of the file): line terminator and any skipped blank lines
    /// included. Concatenating the header and every record gives back the
    /// file exactly.
    pub raw: String,
}

/// A parsed CSV file. Parsing matches the import oracle: RFC 4180, UTF-8,
/// rows may be shorter than the header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CsvTable {
    pub headers: Vec<String>,
    /// The header line's exact bytes, terminator included.
    pub header_raw: String,
    pub records: Vec<CsvRecord>,
}

pub fn parse_csv(relative: &str, bytes: &[u8]) -> Result<CsvTable, ImportError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ImportError(format!("`{relative}` is not UTF-8")))?;
    let invalid = |error: csv::Error| ImportError(format!("cannot parse `{relative}`: {error}"));
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(bytes);
    let headers = reader
        .headers()
        .map_err(invalid)?
        .iter()
        .map(str::to_owned)
        .collect();
    let mut starts = Vec::new();
    let mut parsed = Vec::new();
    let mut record = csv::StringRecord::new();
    while reader.read_record(&mut record).map_err(invalid)? {
        let mut start = record
            .position()
            .map(|position| position.byte() as usize)
            .ok_or_else(|| ImportError(format!("`{relative}`: a record has no position")))?;
        // The reader reports a record after `\r\n` as starting at the `\n`;
        // terminator bytes belong to the line they end.
        while matches!(bytes.get(start), Some(b'\r' | b'\n')) {
            start += 1;
        }
        starts.push(start);
        parsed.push(record.iter().map(str::to_owned).collect::<Vec<_>>());
    }
    let first = starts.first().copied().unwrap_or(text.len());
    let mut records = Vec::new();
    for (index, fields) in parsed.into_iter().enumerate() {
        let start = starts[index];
        let end = starts.get(index + 1).copied().unwrap_or(text.len());
        let raw = text.get(start..end).ok_or_else(|| {
            ImportError(format!(
                "`{relative}`: record {} splits a character",
                index + 1
            ))
        })?;
        records.push(CsvRecord {
            row_number: index + 1,
            fields,
            raw: raw.to_owned(),
        });
    }
    Ok(CsvTable {
        headers,
        header_raw: text[..first].to_owned(),
        records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_keep_their_exact_bytes() {
        let text = "\"A\",\"B\",\"C\"\r\n\"1\",\"x, y\",\"\"\n2,\"multi\nline\"\n\n3\n";
        let table = parse_csv("t.csv", text.as_bytes()).unwrap();
        assert_eq!(table.headers, ["A", "B", "C"]);
        assert_eq!(table.header_raw, "\"A\",\"B\",\"C\"\r\n");
        assert_eq!(table.records.len(), 3);
        assert_eq!(table.records[0].fields, ["1", "x, y", ""]);
        assert_eq!(table.records[1].fields, ["2", "multi\nline"]);
        assert_eq!(table.records[1].raw, "2,\"multi\nline\"\n\n");
        assert_eq!(table.records[2].fields, ["3"]);
        let rebuilt: String = std::iter::once(table.header_raw.as_str())
            .chain(table.records.iter().map(|record| record.raw.as_str()))
            .collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn a_header_only_file_has_no_records() {
        let table = parse_csv("s.csv", b"IPN,MPN\n").unwrap();
        assert_eq!(table.header_raw, "IPN,MPN\n");
        assert!(table.records.is_empty());
    }

    #[test]
    fn lookups_are_exact_case_and_refuse_unsafe_paths() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("symbols")).unwrap();
        fs::write(root.path().join("symbols/g-Case.kicad_sym"), "x").unwrap();
        assert!(input_exists(root.path(), "symbols/g-Case.kicad_sym").unwrap());
        assert!(!input_exists(root.path(), "symbols/g-case.kicad_sym").unwrap());
        assert!(!input_exists(root.path(), "symbols").unwrap());
        for bad in ["../x", "/x", "C:/x", "a\\b", ""] {
            assert!(input_exists(root.path(), bad).is_err(), "{bad}");
        }
        assert_eq!(
            read_input(root.path(), "symbols/g-Case.kicad_sym").unwrap(),
            b"x"
        );
    }
}

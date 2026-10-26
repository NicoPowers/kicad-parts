//! Read-only view of the CSV catalog (`database/g-*.csv`).
//!
//! The canonical, revisioned catalog belongs to a later bounded context. This
//! crate only reads the migration input so the desktop shell can browse real
//! parts; it never writes to `database/`.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum CatalogError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Csv {
        path: PathBuf,
        error: csv::Error,
    },
    Invalid(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "cannot read {}: {error}", path.display()),
            Self::Csv { path, error } => write!(f, "cannot parse {}: {error}", path.display()),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CatalogError {}

impl CatalogError {
    /// Whether the file could not be opened or read at all, as when another
    /// program has it locked or an online-only file is not downloaded yet,
    /// rather than read but not understood as a CSV file.
    pub fn is_unopened_file(&self) -> bool {
        match self {
            Self::Io { .. } => true,
            Self::Csv { error, .. } => matches!(error.kind(), csv::ErrorKind::Io(_)),
            Self::Invalid(_) => false,
        }
    }
}

/// One catalog row. Well-known columns are lifted out; every non-blank column
/// is also kept in source order in `fields`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogPart {
    pub ipn: String,
    /// Category table name, such as `g-res`.
    pub category: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub value: String,
    /// Library reference such as `g-res:R_US`, if the row assigns one.
    pub symbol: Option<String>,
    /// Library reference such as `g-Resistor_SMD:R_0603_1608Metric`, if assigned.
    pub footprint: Option<String>,
    pub datasheet: String,
    pub fields: Vec<(String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CategorySummary {
    pub name: String,
    pub rows: usize,
}

/// The loaded catalog plus a search index over it.
#[derive(Debug)]
pub struct CsvCatalog {
    parts: Vec<CatalogPart>,
    categories: Vec<CategorySummary>,
    index: Vec<SearchText>,
}

/// What a search matches a part on: its identifying text, lower case, made
/// once so each search only compares. Every list of parts in the app ranks
/// through [`SearchText::rank`], so they all rank alike.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchText {
    ipn: String,
    mpn: String,
    text: String,
}

/// A search as typed, lower case and split into terms once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query {
    whole: String,
    terms: Vec<String>,
}

impl Query {
    pub fn new(query: &str) -> Self {
        let whole = query.trim().to_lowercase();
        let terms = whole.split_whitespace().map(str::to_owned).collect();
        Self { whole, terms }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchResults {
    /// Number of rows matching the query, before `limit` is applied.
    pub total: usize,
    /// Indices into [`CsvCatalog::parts`], best match first.
    pub hits: Vec<usize>,
    /// The rank of each hit: 0 for an exact IPN or MPN, 1 for an IPN or
    /// MPN that starts with the first term, 2 for any other match.
    pub ranks: Vec<u8>,
}

impl CsvCatalog {
    /// Reads every `database/g-*.csv` table under `root`, in file-name order.
    /// A folder with no `database/` folder, such as the sample library, has
    /// no CSV catalog: it is an empty catalog, not an error.
    pub fn load(root: &Path) -> Result<Self, CatalogError> {
        let database = root.join("database");
        if !database.exists() && root.is_dir() {
            return Ok(Self {
                parts: Vec::new(),
                categories: Vec::new(),
                index: Vec::new(),
            });
        }
        let entries = fs::read_dir(&database).map_err(|error| CatalogError::Io {
            path: database.clone(),
            error,
        })?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| CatalogError::Io {
                path: database.clone(),
                error,
            })?;
            let path = entry.path();
            let is_table = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("g-") && name.ends_with(".csv"));
            if is_table && path.is_file() {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return Err(CatalogError::Invalid(format!(
                "no catalog tables (g-*.csv) found in {}",
                database.display()
            )));
        }
        paths.sort();

        let mut parts = Vec::new();
        let mut categories = Vec::new();
        for path in paths {
            let category = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            let rows = read_table(&path, &category)?;
            categories.push(CategorySummary {
                name: category,
                rows: rows.len(),
            });
            parts.extend(rows);
        }
        Ok(Self::from_parts(parts, categories))
    }

    /// Builds a catalog from already-parsed rows. Benchmarks and tests use this
    /// to search synthetic catalogs without touching the repository data.
    pub fn from_parts(parts: Vec<CatalogPart>, categories: Vec<CategorySummary>) -> Self {
        let index = parts.iter().map(SearchText::new).collect();
        Self {
            parts,
            categories,
            index,
        }
    }

    pub fn parts(&self) -> &[CatalogPart] {
        &self.parts
    }

    pub fn categories(&self) -> &[CategorySummary] {
        &self.categories
    }

    pub fn find(&self, ipn: &str) -> Option<&CatalogPart> {
        self.parts.iter().find(|part| part.ipn == ipn)
    }

    /// What a search matches the part at `position` in [`Self::parts`] on.
    pub fn search_text(&self, position: usize) -> &SearchText {
        &self.index[position]
    }

    /// Case-insensitive search. Every whitespace-separated term must occur in
    /// the part's identifying text. Exact IPN or MPN matches rank first, then
    /// IPN or MPN prefix matches, then catalog order. An empty query matches
    /// every row.
    pub fn search(&self, query: &str, limit: usize) -> SearchResults {
        let query = Query::new(query);
        let mut ranked: Vec<(u8, usize)> = self
            .index
            .iter()
            .enumerate()
            .filter_map(|(position, key)| Some((key.rank(&query)?, position)))
            .collect();
        let total = ranked.len();
        // Positions are already ascending, so a stable sort keeps catalog order
        // within each rank.
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.truncate(limit);
        let (ranks, hits) = ranked.into_iter().unzip();
        SearchResults { total, hits, ranks }
    }
}

impl SearchText {
    pub fn new(part: &CatalogPart) -> Self {
        let mut text = String::new();
        for value in [
            part.ipn.as_str(),
            part.mpn.as_str(),
            part.manufacturer.as_str(),
            part.description.as_str(),
            part.value.as_str(),
            part.category.as_str(),
            part.symbol.as_deref().unwrap_or_default(),
            part.footprint.as_deref().unwrap_or_default(),
        ] {
            text.push_str(&value.to_lowercase());
            // A separator no query term can contain keeps terms from matching
            // across field boundaries.
            text.push('\u{1f}');
        }
        Self {
            ipn: part.ipn.to_lowercase(),
            mpn: part.mpn.to_lowercase(),
            text,
        }
    }

    /// How well the part matches `query`, or `None` when it does not: every
    /// term must occur in its text. 0 for an exact IPN or MPN, 1 for an
    /// IPN or MPN that starts with the first term, 2 for any other match.
    /// An empty query matches every part, at 2.
    pub fn rank(&self, query: &Query) -> Option<u8> {
        if !query
            .terms
            .iter()
            .all(|term| self.text.contains(term.as_str()))
        {
            return None;
        }
        let Some(first) = query.terms.first() else {
            return Some(2);
        };
        Some(if self.ipn == query.whole || self.mpn == query.whole {
            0
        } else if self.ipn.starts_with(first.as_str()) || self.mpn.starts_with(first.as_str()) {
            1
        } else {
            2
        })
    }
}

/// The IPN of every row of one CSV file, such as `database/g-res.csv`, read
/// as [`CsvCatalog::load`] reads them: trimmed, and rows without one left
/// out.
pub fn table_ipns(path: &Path) -> Result<Vec<String>, CatalogError> {
    Ok(read_table(path, "")?
        .into_iter()
        .map(|part| part.ipn)
        .collect())
}

fn read_table(path: &Path, category: &str) -> Result<Vec<CatalogPart>, CatalogError> {
    let csv_error = |error| CatalogError::Csv {
        path: path.to_path_buf(),
        error,
    };
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .map_err(csv_error)?;
    let headers: Vec<String> = reader
        .headers()
        .map_err(csv_error)?
        .iter()
        .map(|header| header.trim_start_matches('\u{feff}').to_owned())
        .collect();
    if !headers.iter().any(|header| header == "IPN") {
        return Err(CatalogError::Invalid(format!(
            "{} has no IPN column",
            path.display()
        )));
    }

    let mut parts = Vec::new();
    for record in reader.records() {
        let record = record.map_err(csv_error)?;
        let fields: Vec<(String, String)> = headers
            .iter()
            .zip(record.iter())
            .filter(|(_, value)| !value.trim().is_empty())
            .map(|(header, value)| (header.clone(), value.trim().to_owned()))
            .collect();
        let get = |name: &str| {
            fields
                .iter()
                .find(|(header, _)| header == name)
                .map(|(_, value)| value.clone())
        };
        let Some(ipn) = get("IPN") else {
            continue;
        };
        parts.push(CatalogPart {
            ipn,
            category: category.to_owned(),
            mpn: get("MPN").unwrap_or_default(),
            manufacturer: get("Manufacturer").unwrap_or_default(),
            description: get("Description").unwrap_or_default(),
            value: get("Value").unwrap_or_default(),
            symbol: get("Symbol"),
            footprint: get("Footprint"),
            datasheet: get("Datasheet").unwrap_or_default(),
            fields,
        });
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(ipn: &str, mpn: &str, description: &str) -> CatalogPart {
        CatalogPart {
            ipn: ipn.into(),
            category: "g-test".into(),
            mpn: mpn.into(),
            manufacturer: String::new(),
            description: description.into(),
            value: String::new(),
            symbol: None,
            footprint: None,
            datasheet: String::new(),
            fields: Vec::new(),
        }
    }

    #[test]
    fn a_folder_without_database_has_an_empty_catalog() {
        let folder = tempfile::tempdir().unwrap();
        let catalog = CsvCatalog::load(folder.path()).unwrap();
        assert!(catalog.parts().is_empty() && catalog.categories().is_empty());
        // A database folder without CSV files is still a mistake.
        fs::create_dir(folder.path().join("database")).unwrap();
        assert!(CsvCatalog::load(folder.path()).is_err());
        // So is a catalog folder that is not there.
        assert!(CsvCatalog::load(&folder.path().join("nope")).is_err());
    }

    #[test]
    fn one_search_text_ranks_any_part_the_same_way() {
        let text = SearchText::new(&part("RES-0000-0470", "RC0603FR-0747RL", "47R 0603"));
        assert_eq!(text.rank(&Query::new(" rc0603fr-0747rl ")), Some(0));
        assert_eq!(text.rank(&Query::new("RES-0000")), Some(1));
        assert_eq!(text.rank(&Query::new("0603 47r")), Some(2));
        assert_eq!(text.rank(&Query::new("")), Some(2));
        assert_eq!(text.rank(&Query::new("0805")), None);
    }

    #[test]
    fn exact_and_prefix_matches_rank_before_text_matches() {
        let catalog = CsvCatalog::from_parts(
            vec![
                part("A-1", "X", "mentions abc"),
                part("A-2", "ABCD", "prefix"),
                part("A-3", "ABC", "exact"),
            ],
            Vec::new(),
        );
        let results = catalog.search("abc", 10);
        assert_eq!(results.total, 3);
        assert_eq!(results.hits, vec![2, 1, 0]);
    }

    #[test]
    fn all_terms_must_match_and_limit_keeps_total() {
        let catalog = CsvCatalog::from_parts(
            vec![
                part("R-1", "", "res 10k 0603"),
                part("R-2", "", "res 10k 0402"),
                part("R-3", "", "res 1k 0603"),
            ],
            Vec::new(),
        );
        let results = catalog.search("0603 RES", 1);
        assert_eq!(results.total, 2);
        assert_eq!(results.hits, vec![0]);
        assert_eq!(catalog.search("  ", 10).total, 3);
    }

    #[test]
    fn terms_do_not_match_across_field_boundaries() {
        let catalog = CsvCatalog::from_parts(vec![part("AB", "CD", "")], Vec::new());
        assert_eq!(catalog.search("bc", 10).total, 0);
    }
}

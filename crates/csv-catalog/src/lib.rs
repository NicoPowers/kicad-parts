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
    index: Vec<SearchKey>,
}

#[derive(Debug)]
struct SearchKey {
    ipn: String,
    mpn: String,
    text: String,
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
    pub fn load(root: &Path) -> Result<Self, CatalogError> {
        let database = root.join("database");
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
        let index = parts.iter().map(SearchKey::new).collect();
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

    /// Case-insensitive search. Every whitespace-separated term must occur in
    /// the part's identifying text. Exact IPN or MPN matches rank first, then
    /// IPN or MPN prefix matches, then catalog order. An empty query matches
    /// every row.
    pub fn search(&self, query: &str, limit: usize) -> SearchResults {
        self.search_replacing(query, limit, |_| None)
    }

    /// Like [`Self::search`], but a part for which `replace` returns a part
    /// is matched on that part's values instead (for example the team
    /// catalog's current details), so a search finds what the app shows.
    pub fn search_replacing(
        &self,
        query: &str,
        limit: usize,
        replace: impl Fn(&CatalogPart) -> Option<CatalogPart>,
    ) -> SearchResults {
        self.search_with_extra(query, limit, replace, &[])
    }

    /// Like [`Self::search_replacing`], but `extra` parts (for example
    /// ones only the team catalog has) are searched too, after the
    /// catalog's own. A hit at `parts().len() + n` is `extra[n]`.
    pub fn search_with_extra(
        &self,
        query: &str,
        limit: usize,
        replace: impl Fn(&CatalogPart) -> Option<CatalogPart>,
        extra: &[CatalogPart],
    ) -> SearchResults {
        let query = query.trim().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        let mut ranked: Vec<(u8, usize)> = Vec::new();
        let extra_keys: Vec<SearchKey> = extra.iter().map(SearchKey::new).collect();
        let keys = self
            .index
            .iter()
            .enumerate()
            .map(|(position, indexed)| {
                let replaced = replace(&self.parts[position]).map(|part| SearchKey::new(&part));
                (position, replaced, Some(indexed))
            })
            .chain(
                extra_keys
                    .iter()
                    .enumerate()
                    .map(|(at, key)| (self.parts.len() + at, None, Some(key))),
            );
        for (position, replaced, indexed) in keys {
            let Some(key) = replaced.as_ref().or(indexed) else {
                continue;
            };
            if !terms.iter().all(|term| key.text.contains(term)) {
                continue;
            }
            let rank = if terms.is_empty() {
                2
            } else if key.ipn == query || key.mpn == query {
                0
            } else if key.ipn.starts_with(terms[0]) || key.mpn.starts_with(terms[0]) {
                1
            } else {
                2
            };
            ranked.push((rank, position));
        }
        let total = ranked.len();
        // Positions are already ascending, so a stable sort keeps catalog order
        // within each rank.
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.truncate(limit);
        let (ranks, hits) = ranked.into_iter().unzip();
        SearchResults { total, hits, ranks }
    }
}

impl SearchKey {
    fn new(part: &CatalogPart) -> Self {
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
    fn extra_parts_are_searched_after_the_catalogs_own() {
        let catalog = CsvCatalog::from_parts(
            vec![part("A-1", "X", "abc"), part("A-2", "Y", "other")],
            Vec::new(),
        );
        let extra = [part("N-1", "ABC", "added in the app")];
        let results = catalog.search_with_extra("abc", 10, |_| None, &extra);
        // The extra part is an exact MPN match, so it ranks first; its hit
        // is past the catalog's own parts.
        assert_eq!(results.hits, vec![2, 0]);
        assert_eq!(results.total, 2);
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
    fn replaced_parts_match_on_their_replacement_values() {
        let catalog = CsvCatalog::from_parts(
            vec![part("R-1", "", "res 47"), part("R-2", "", "res 10k")],
            Vec::new(),
        );
        let replace = |found: &CatalogPart| {
            (found.ipn == "R-1").then(|| CatalogPart {
                description: "thick film 47R".into(),
                ..found.clone()
            })
        };
        assert_eq!(catalog.search_replacing("47r", 10, replace).hits, vec![0]);
        assert_eq!(catalog.search("47r", 10).total, 0);
        // The old value no longer finds the replaced part.
        assert_eq!(catalog.search_replacing("res", 10, replace).hits, vec![1]);
        assert_eq!(
            catalog.search_replacing("10k", 10, replace).hits,
            vec![1],
            "other parts match as before"
        );
    }

    #[test]
    fn terms_do_not_match_across_field_boundaries() {
        let catalog = CsvCatalog::from_parts(vec![part("AB", "CD", "")], Vec::new());
        assert_eq!(catalog.search("bc", 10).total, 0);
    }
}

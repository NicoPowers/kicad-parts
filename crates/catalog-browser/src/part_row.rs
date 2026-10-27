//! One part as the part list shows it, and what a search matches it on.

/// One part's values as the list and the part panel show them. Well-known
/// fields are lifted out; every non-blank field is also kept in order in
/// `fields`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PartRow {
    pub ipn: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub value: String,
    /// Library reference such as `g-res:R_US`, if the part has one.
    pub symbol: Option<String>,
    /// Library reference such as `g-Resistor_SMD:R_0603_1608Metric`, if
    /// the part has one; several choices are separated by `;`.
    pub footprint: Option<String>,
    pub datasheet: String,
    pub fields: Vec<(String, String)>,
}

/// What a search matches a part on: its identifying text, lower case, made
/// once so each search only compares. Every list of parts in the app ranks
/// through [`SearchText::rank`], so they all rank alike.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SearchText {
    ipn: String,
    mpn: String,
    text: String,
}

/// A search as typed, lower case and split into terms once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Query {
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

impl SearchText {
    pub fn new(part: &PartRow) -> Self {
        let mut text = String::new();
        for value in [
            part.ipn.as_str(),
            part.mpn.as_str(),
            part.manufacturer.as_str(),
            part.description.as_str(),
            part.value.as_str(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn part(ipn: &str, mpn: &str, description: &str) -> PartRow {
        PartRow {
            ipn: ipn.into(),
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
    fn one_search_text_ranks_any_part_the_same_way() {
        let text = SearchText::new(&part("RES-0000-0470", "RC0603FR-0747RL", "47R 0603"));
        assert_eq!(text.rank(&Query::new(" rc0603fr-0747rl ")), Some(0));
        assert_eq!(text.rank(&Query::new("RES-0000")), Some(1));
        assert_eq!(text.rank(&Query::new("0603 47r")), Some(2));
        assert_eq!(text.rank(&Query::new("")), Some(2));
        assert_eq!(text.rank(&Query::new("0805")), None);
    }

    #[test]
    fn terms_do_not_match_across_field_boundaries() {
        let text = SearchText::new(&part("AB", "CD", ""));
        assert_eq!(text.rank(&Query::new("bc")), None);
        assert_eq!(text.rank(&Query::new("cd")), Some(0));
    }
}

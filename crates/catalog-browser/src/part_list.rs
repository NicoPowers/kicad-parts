//! The part list's search (kicad-parts-8a71): one list and one search over
//! every signed-in team's parts, and the CSV catalog's parts no team has.
//!
//! A part is named by its team and its IPN ([`PartKey`]). Two teams may
//! both have RES-0000-0470, and each shows as its own row. A part the CSV
//! catalog has shows with each team's current values, as that team's part.
//! The CSV row itself shows only while no team has its IPN. So a person
//! with one team sees the list exactly as before.
//!
//! Rows rank as the CSV catalog's search ranks parts ([`SearchText::rank`]).
//! Within a rank they keep catalog order: the CSV catalog's order, then the
//! parts only teams have, by IPN. The same IPN in two teams goes by team
//! order, as in the Teams menu.
//!
//! Searching makes no request. Each team's parts come from its
//! [`KnownParts`], and are made into rows once per change of that team, so
//! a search over 10 teams of a few thousand parts each only compares text.

use crate::team::KnownParts;
use crate::{CatalogBrowser, PartValues, part_uses, summary};
use csv_catalog::{CatalogPart, CsvCatalog, Query, SearchText};
use desktop_contract::{
    GroupBy, PartKey, PartSummary, SearchRequest, SearchResponse, SearchSection, team_section,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Largest page of search results the UI may request.
pub const MAX_SEARCH_LIMIT: usize = 500;

/// What a section of only the CSV catalog's parts is called, when grouping
/// by team.
pub(crate) const CSV_SECTION: &str = "CSV files";

/// One team's parts made into rows: each part as the list shows it, and
/// what a search matches it on.
pub(crate) struct ListedTeam {
    /// What they were made from, so they are made again only when it
    /// changes.
    source: Arc<KnownParts>,
    /// In IPN order.
    parts: Vec<ListedPart>,
    /// Where each IPN is in `parts`.
    at: HashMap<String, usize>,
}

struct ListedPart {
    part: CatalogPart,
    /// The part's category code; blank for none.
    category: String,
    text: SearchText,
    /// The CSV catalog has this IPN too.
    in_csv: bool,
}

impl ListedTeam {
    fn new(source: Arc<KnownParts>, csv: &CsvRows<'_>) -> Self {
        let parts: Vec<ListedPart> = source
            .values
            .iter()
            .map(|(ipn, values)| listed_part(ipn, values, csv.find(ipn)))
            .collect();
        let at = parts
            .iter()
            .enumerate()
            .map(|(at, listed)| (listed.part.ipn.clone(), at))
            .collect();
        Self { source, parts, at }
    }
}

/// The team part `ipn` with `values`: over the CSV catalog's `row` when it
/// has the IPN, as the part header shows it.
fn listed_part(ipn: &str, values: &PartValues, row: Option<&CatalogPart>) -> ListedPart {
    let part = match row {
        Some(row) => values.apply(row),
        None => values.part(ipn),
    };
    let category = values
        .category
        .clone()
        .or_else(|| row.map(|row| catalog::category::category_code(&row.category).to_owned()))
        .unwrap_or_default();
    ListedPart {
        text: SearchText::new(&part),
        part,
        category,
        in_csv: row.is_some(),
    }
}

/// The CSV catalog this app loaded, and where each IPN's first row is.
pub(crate) struct CsvRows<'a> {
    catalog: Option<&'a CsvCatalog>,
    at: &'a HashMap<String, usize>,
}

impl CsvRows<'_> {
    fn find(&self, ipn: &str) -> Option<&CatalogPart> {
        let catalog = self.catalog?;
        self.at.get(ipn).map(|&at| &catalog.parts()[at])
    }
}

/// Where each IPN's first row is in `catalog`. The CSV catalog can repeat
/// an IPN; the team import keeps the first row.
pub(crate) fn csv_rows_by_ipn(catalog: Option<&CsvCatalog>) -> HashMap<String, usize> {
    let mut at = HashMap::new();
    for (position, part) in catalog
        .map(CsvCatalog::parts)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        at.entry(part.ipn.clone()).or_insert(position);
    }
    at
}

/// Every team's rows, kept until that team's parts change.
#[derive(Default)]
pub(crate) struct ListedTeams(Mutex<HashMap<String, Arc<ListedTeam>>>);

impl ListedTeams {
    /// The rows of each team in `teams`, in order. A team's rows are made
    /// again only when its parts changed: [`KnownParts`] are kept and
    /// given out again until then. Teams no longer given are forgotten.
    fn rows_of(&self, teams: &[Arc<KnownParts>], csv: &CsvRows<'_>) -> Vec<Arc<ListedTeam>> {
        let mut known = self.0.lock().unwrap_or_else(|e| e.into_inner());
        known.retain(|team, _| teams.iter().any(|given| given.team == *team));
        teams
            .iter()
            .map(|source| match known.get(&source.team) {
                Some(listed) if Arc::ptr_eq(&listed.source, source) => listed.clone(),
                _ => {
                    let listed = Arc::new(ListedTeam::new(source.clone(), csv));
                    known.insert(source.team.clone(), listed.clone());
                    listed
                }
            })
            .collect()
    }
}

/// One row of the part list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Row {
    /// The CSV catalog's part at this position, which no team has.
    Csv(usize),
    /// The part at `part` of the team at `team` (in team order).
    Team { team: usize, part: usize },
}

/// The teams a list draws from, in team order, and the CSV catalog.
struct Sources<'a> {
    csv: Option<&'a CsvCatalog>,
    teams: Vec<Arc<ListedTeam>>,
}

impl Sources<'_> {
    /// Every row of the list, in catalog order: the CSV catalog's order,
    /// with each team having the IPN in team order (or the CSV row when no
    /// team has it), then the parts only teams have, by IPN and then team
    /// order. With `only`, just that team's rows.
    fn rows(&self, only: Option<&str>) -> Vec<Row> {
        let shown = |team: usize| only.is_none_or(|only| self.teams[team].source.team == only);
        let mut rows = Vec::new();
        if let Some(csv) = self.csv {
            for (position, part) in csv.parts().iter().enumerate() {
                let mut held = false;
                for (team, listed) in self.teams.iter().enumerate() {
                    if let Some(&at) = listed.at.get(&part.ipn) {
                        held = true;
                        if shown(team) {
                            rows.push(Row::Team { team, part: at });
                        }
                    }
                }
                if !held && only.is_none() {
                    rows.push(Row::Csv(position));
                }
            }
        }
        let mut team_only: Vec<(&str, usize, usize)> = Vec::new();
        for (team, listed) in self
            .teams
            .iter()
            .enumerate()
            .filter(|(team, _)| shown(*team))
        {
            for (at, part) in listed.parts.iter().enumerate() {
                if !part.in_csv {
                    team_only.push((&part.part.ipn, team, at));
                }
            }
        }
        // Each team's parts are in IPN order already, so this only merges.
        team_only.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        rows.extend(
            team_only
                .into_iter()
                .map(|(_, team, part)| Row::Team { team, part }),
        );
        rows
    }

    fn part(&self, row: Row) -> &CatalogPart {
        match row {
            Row::Csv(position) => &self.csv_catalog().parts()[position],
            Row::Team { team, part } => &self.teams[team].parts[part].part,
        }
    }

    fn text(&self, row: Row) -> &SearchText {
        match row {
            Row::Csv(position) => self.csv_catalog().search_text(position),
            Row::Team { team, part } => &self.teams[team].parts[part].text,
        }
    }

    fn team(&self, row: Row) -> &str {
        match row {
            Row::Csv(_) => "",
            Row::Team { team, .. } => &self.teams[team].source.team,
        }
    }

    fn category(&self, row: Row) -> String {
        match row {
            Row::Csv(position) => {
                let part = &self.csv_catalog().parts()[position];
                catalog::category::category_code(&part.category).to_owned()
            }
            Row::Team { team, part } => self.teams[team].parts[part].category.clone(),
        }
    }

    fn key(&self, row: Row) -> PartKey {
        PartKey {
            team: self.team(row).to_owned(),
            ipn: self.part(row).ipn.clone(),
        }
    }

    fn summary(&self, row: Row) -> PartSummary {
        PartSummary {
            team: self.team(row).to_owned(),
            ..summary(self.part(row))
        }
    }

    /// The section `row` is in.
    fn section(&self, row: Row, group_by: GroupBy) -> String {
        match group_by {
            GroupBy::Category => self.category(row),
            GroupBy::Team => team_section(self.team(row)),
        }
    }

    fn csv_catalog(&self) -> &CsvCatalog {
        self.csv.expect("a CSV row comes from the CSV catalog")
    }

    /// Every category's plain name, by code: from the first team, in team
    /// order, that names it.
    fn category_names(&self) -> BTreeMap<String, String> {
        let mut names = BTreeMap::new();
        for listed in self.teams.iter().rev() {
            names.extend(listed.source.category_names.clone());
        }
        names
    }

    /// Each team's section, in team order, with its name; the CSV
    /// catalog's section last.
    fn team_sections(&self) -> Vec<(String, String)> {
        self.teams
            .iter()
            .map(|listed| {
                (
                    team_section(&listed.source.team),
                    listed.source.label.clone(),
                )
            })
            .chain(std::iter::once((team_section(""), CSV_SECTION.to_owned())))
            .collect()
    }
}

impl CatalogBrowser {
    /// The search behind the part list, over every team in `teams` (in
    /// team order) and the CSV catalog. The matches are put into sections,
    /// by category or by team, each with at most `request.limit` parts plus
    /// the selected part (0 for counts only).
    ///
    /// Without a search, category sections are in name order; while
    /// searching, the section with the best match comes first. Team
    /// sections are always in team order. With `request.section`, only that
    /// section is answered; with `request.team`, only that team's parts.
    pub fn search_request(
        &self,
        request: &SearchRequest,
        teams: &[Arc<KnownParts>],
    ) -> SearchResponse {
        let started = Instant::now();
        let mut response = answer(&self.sources(teams), request);
        response.elapsed_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        response
    }

    /// Every part the list would show for `only` (every team when `None`),
    /// with its team, for the Libraries browser's "used by".
    pub(crate) fn listed_parts(
        &self,
        teams: &[Arc<KnownParts>],
        only: Option<&str>,
    ) -> Vec<(String, CatalogPart)> {
        let sources = self.sources(teams);
        sources
            .rows(only)
            .into_iter()
            .map(|row| (sources.team(row).to_owned(), sources.part(row).clone()))
            .collect()
    }

    fn sources(&self, teams: &[Arc<KnownParts>]) -> Sources<'_> {
        let catalog = self.catalog.as_ref().ok();
        let at = self
            .csv_rows_by_ipn
            .get_or_init(|| csv_rows_by_ipn(catalog));
        Sources {
            csv: catalog,
            teams: self.listed_teams.rows_of(teams, &CsvRows { catalog, at }),
        }
    }
}

/// The part list's answer to `request` from `sources`; see
/// [`CatalogBrowser::search_request`].
fn answer(sources: &Sources<'_>, request: &SearchRequest) -> SearchResponse {
    let query = Query::new(&request.query);
    // Only the parts using the reference, compared exactly.
    let uses = request
        .uses
        .as_deref()
        .map(str::trim)
        .filter(|reference| !reference.is_empty());
    let mut ranked: Vec<(u8, Row)> = sources
        .rows(request.team.as_deref())
        .into_iter()
        .filter_map(|row| Some((sources.text(row).rank(&query)?, row)))
        .filter(|(_, row)| uses.is_none_or(|reference| part_uses(sources.part(*row), reference)))
        .collect();
    // Rows are in catalog order already, so a stable sort keeps it within
    // each rank.
    ranked.sort_by_key(|(rank, _)| *rank);
    let total = ranked.len();
    let selected_row = request.selected.as_ref().and_then(|selected| {
        ranked
            .iter()
            .map(|(_, row)| *row)
            .find(|row| sources.key(*row) == *selected)
    });
    let names = sources.category_names();
    let team_sections = sources.team_sections();
    let groups = group_rows(
        ranked
            .into_iter()
            .map(|(rank, row)| Ranked {
                section: sources.section(row, request.group_by),
                row,
                rank,
            })
            .filter(|ranked| {
                request
                    .section
                    .as_ref()
                    .is_none_or(|wanted| *wanted == ranked.section)
            }),
        &Grouping {
            limit: request.limit.min(MAX_SEARCH_LIMIT),
            searching: !query_is_empty(&request.query) || uses.is_some(),
            selected: selected_row,
            sections: match request.group_by {
                GroupBy::Category => Sections::Categories(&names),
                GroupBy::Team => Sections::Teams(&team_sections),
            },
        },
    );
    SearchResponse {
        query: request.query.clone(),
        uses: request.uses.clone(),
        total,
        sections: groups
            .into_iter()
            .map(|group| SearchSection {
                key: group.key,
                name: group.name,
                total: group.total,
                hits: group
                    .rows
                    .into_iter()
                    .map(|row| sources.summary(row))
                    .collect(),
            })
            .collect(),
        selected: selected_row.map(|row| sources.section(row, request.group_by)),
        elapsed_micros: 0,
    }
}

fn query_is_empty(query: &str) -> bool {
    query.trim().is_empty()
}

/// One match of a search, with its section and rank (see
/// [`SearchText::rank`]).
#[derive(Clone, Debug)]
struct Ranked {
    section: String,
    row: Row,
    rank: u8,
}

/// How [`group_rows`] groups.
struct Grouping<'a> {
    /// Most rows kept per section; 0 keeps none (counts only).
    limit: usize,
    /// Whether this is a search, which orders category sections by best
    /// match.
    searching: bool,
    /// The selected part's row, always kept in its section.
    selected: Option<Row>,
    sections: Sections<'a>,
}

/// What the sections are, and how they are named and ordered.
enum Sections<'a> {
    /// By category code, named by the teams' names for them.
    Categories(&'a BTreeMap<String, String>),
    /// Each team's section key and name, in team order.
    Teams(&'a [(String, String)]),
}

/// The matches of one section of the part list.
#[derive(Debug, Eq, PartialEq)]
struct Group {
    key: String,
    name: String,
    /// Every match in this section.
    total: usize,
    /// The rank of its best match.
    best_rank: u8,
    /// The first `limit` of them, best first, and the selected part.
    rows: Vec<Row>,
}

/// Groups ranked matches, given best first, into one group per section.
/// Each group keeps the rank order and at most `limit` rows (plus the
/// selected part), but counts every match.
///
/// Category groups are in name order; while searching, the group with the
/// best match comes first, ties by name. "Other parts" (no category) is
/// always last. A category without matches makes no section, so an empty
/// CSV file (g-mec.csv) has none: 21 CSV files, 20 sections. Team groups
/// are in team order, the CSV catalog's last.
fn group_rows(ranked: impl IntoIterator<Item = Ranked>, how: &Grouping<'_>) -> Vec<Group> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for Ranked { section, row, rank } in ranked {
        let group = groups.entry(section).or_insert_with_key(|key| Group {
            name: section_name(key, &how.sections),
            key: key.clone(),
            total: 0,
            best_rank: rank,
            rows: Vec::new(),
        });
        group.total += 1;
        group.best_rank = group.best_rank.min(rank);
        if group.rows.len() < how.limit || how.selected == Some(row) {
            group.rows.push(row);
        }
    }
    let mut groups: Vec<Group> = groups.into_values().collect();
    match &how.sections {
        Sections::Categories(_) => groups.sort_by(|a, b| {
            let rank = |g: &Group| if how.searching { g.best_rank } else { 0 };
            (a.key.is_empty(), rank(a))
                .cmp(&(b.key.is_empty(), rank(b)))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.key.cmp(&b.key))
        }),
        Sections::Teams(teams) => groups.sort_by_key(|group| {
            teams
                .iter()
                .position(|(key, _)| *key == group.key)
                .unwrap_or(usize::MAX)
        }),
    }
    groups
}

/// A section's plain name. A category's is the teams' name for it, else
/// the one it starts with, else its code; a part without a category is in
/// "Other parts". A team's is its code, or its address.
fn section_name(key: &str, sections: &Sections<'_>) -> String {
    match sections {
        Sections::Categories(names) => category_name(key, names),
        Sections::Teams(teams) => teams
            .iter()
            .find(|(team, _)| team == key)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| key.to_owned()),
    }
}

fn category_name(code: &str, names: &BTreeMap<String, String>) -> String {
    if code.is_empty() {
        return "Other parts".to_owned();
    }
    names
        .get(code)
        .cloned()
        .or_else(|| catalog::category::starting_category(code).map(|start| start.name.to_owned()))
        .unwrap_or_else(|| code.to_owned())
}

#[cfg(test)]
mod tests;

//! The part list's search (kicad-parts-8a71): one list and one search over
//! every signed-in team's parts.
//!
//! A part is named by its team and its IPN ([`PartKey`]). Two teams may
//! both have RES-0000-0470, and each shows as its own row.
//!
//! Rows rank by [`SearchText::rank`]. Within a rank they are in IPN order;
//! the same IPN in two teams goes by team order, as in the Teams menu.
//!
//! Searching makes no request. Each team's parts come from its
//! [`KnownParts`], and are made into rows once per change of that team, so
//! a search over 10 teams of a few thousand parts each only compares text.

use crate::part_row::{PartRow, Query, SearchText};
use crate::team::KnownParts;
use crate::{CatalogBrowser, detail, part_uses, summary};
use desktop_contract::{
    GroupBy, PartDetail, PartKey, PartSummary, SearchRequest, SearchResponse, SearchSection,
    team_section,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Largest page of search results the UI may request.
pub const MAX_SEARCH_LIMIT: usize = 500;

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
    part: PartRow,
    /// The part's category code; blank for none.
    category: String,
    text: SearchText,
}

impl ListedTeam {
    fn new(source: Arc<KnownParts>) -> Self {
        let parts: Vec<ListedPart> = source
            .values
            .iter()
            .map(|(ipn, values)| {
                let part = values.part(ipn);
                ListedPart {
                    text: SearchText::new(&part),
                    part,
                    category: values.category.clone().unwrap_or_default(),
                }
            })
            .collect();
        let at = parts
            .iter()
            .enumerate()
            .map(|(at, listed)| (listed.part.ipn.clone(), at))
            .collect();
        Self { source, parts, at }
    }
}

/// Every team's rows, kept until that team's parts change.
#[derive(Default)]
pub(crate) struct ListedTeams(Mutex<HashMap<String, Arc<ListedTeam>>>);

impl ListedTeams {
    /// The rows of each team in `teams`, in order. A team's rows are made
    /// again only when its parts changed: [`KnownParts`] are kept and
    /// given out again until then. Teams no longer given are forgotten.
    fn rows_of(&self, teams: &[Arc<KnownParts>]) -> Vec<Arc<ListedTeam>> {
        let mut known = self.0.lock().unwrap_or_else(|e| e.into_inner());
        known.retain(|team, _| teams.iter().any(|given| given.team == *team));
        teams
            .iter()
            .map(|source| match known.get(&source.team) {
                Some(listed) if Arc::ptr_eq(&listed.source, source) => listed.clone(),
                _ => {
                    let listed = Arc::new(ListedTeam::new(source.clone()));
                    known.insert(source.team.clone(), listed.clone());
                    listed
                }
            })
            .collect()
    }
}

/// One row of the part list: the part at `part` of the team at `team` (in
/// team order).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Row {
    team: usize,
    part: usize,
}

/// The teams a list draws from, in team order.
struct Sources {
    teams: Vec<Arc<ListedTeam>>,
}

impl Sources {
    /// Every row of the list, by IPN and then team order. With `only`,
    /// just that team's rows.
    fn rows(&self, only: Option<&str>) -> Vec<Row> {
        let mut rows: Vec<(&str, Row)> = Vec::new();
        for (team, listed) in self.teams.iter().enumerate() {
            if only.is_some_and(|only| listed.source.team != only) {
                continue;
            }
            for (at, part) in listed.parts.iter().enumerate() {
                rows.push((&part.part.ipn, Row { team, part: at }));
            }
        }
        // Each team's parts are in IPN order already, so this only merges.
        rows.sort_by(|a, b| (a.0, a.1.team).cmp(&(b.0, b.1.team)));
        rows.into_iter().map(|(_, row)| row).collect()
    }

    fn listed(&self, row: Row) -> &ListedPart {
        &self.teams[row.team].parts[row.part]
    }

    fn part(&self, row: Row) -> &PartRow {
        &self.listed(row).part
    }

    fn text(&self, row: Row) -> &SearchText {
        &self.listed(row).text
    }

    fn team(&self, row: Row) -> &str {
        &self.teams[row.team].source.team
    }

    fn category(&self, row: Row) -> String {
        self.listed(row).category.clone()
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

    /// Every category's plain name, by code: from the first team, in team
    /// order, that names it.
    fn category_names(&self) -> BTreeMap<String, String> {
        let mut names = BTreeMap::new();
        for listed in self.teams.iter().rev() {
            names.extend(listed.source.category_names.clone());
        }
        names
    }

    /// Each team's section, in team order, with its name.
    fn team_sections(&self) -> Vec<(String, String)> {
        self.teams
            .iter()
            .map(|listed| {
                (
                    team_section(&listed.source.team),
                    listed.source.label.clone(),
                )
            })
            .collect()
    }
}

impl CatalogBrowser {
    /// The search behind the part list, over every team in `teams` (in
    /// team order). The matches are put into sections,
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

    /// The part `key` as the part list shows it: the team's row, from its
    /// last-known parts in `teams`. So the part panel and the list always
    /// agree, also while the team's server cannot be reached.
    pub fn part_in_list(&self, teams: &[Arc<KnownParts>], key: &PartKey) -> Option<PartDetail> {
        let sources = self.sources(teams);
        let team = sources
            .teams
            .iter()
            .find(|listed| listed.source.team == key.team)?;
        let part = &team.parts[*team.at.get(&key.ipn)?].part;
        Some(detail(part))
    }

    /// Every part the list would show for `only` (every team when `None`),
    /// with its team, for the Libraries browser's "used by".
    pub(crate) fn listed_parts(
        &self,
        teams: &[Arc<KnownParts>],
        only: Option<&str>,
    ) -> Vec<(String, PartRow)> {
        let sources = self.sources(teams);
        sources
            .rows(only)
            .into_iter()
            .map(|row| (sources.team(row).to_owned(), sources.part(row).clone()))
            .collect()
    }

    fn sources(&self, teams: &[Arc<KnownParts>]) -> Sources {
        Sources {
            teams: self.listed_teams.rows_of(teams),
        }
    }
}

/// The part list's answer to `request` from `sources`; see
/// [`CatalogBrowser::search_request`].
fn answer(sources: &Sources, request: &SearchRequest) -> SearchResponse {
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
    // Rows are in IPN order already, so a stable sort keeps it within
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
        selected_part: selected_row.map(|row| sources.key(row)),
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
/// always last. A category without matches makes no section. Team groups
/// are in team order.
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

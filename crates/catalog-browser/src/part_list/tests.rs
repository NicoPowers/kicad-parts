use super::*;

const MA: &str = "https://ma.example.com";
const AC: &str = "https://ac.example.com";

/// A CSV catalog row: (IPN, MPN, CSV table such as `g-res`).
fn csv_part(ipn: &str, mpn: &str, table: &str) -> CatalogPart {
    CatalogPart {
        ipn: ipn.into(),
        category: table.into(),
        mpn: mpn.into(),
        manufacturer: String::new(),
        description: format!("{mpn} from the CSV files"),
        value: String::new(),
        symbol: None,
        footprint: None,
        datasheet: String::new(),
        fields: Vec::new(),
    }
}

fn csv(rows: &[(&str, &str, &str)]) -> CsvCatalog {
    CsvCatalog::from_parts(
        rows.iter()
            .map(|(ipn, mpn, table)| csv_part(ipn, mpn, table))
            .collect(),
        Vec::new(),
    )
}

/// A team's parts: (IPN, MPN, category code).
fn team(server: &str, label: &str, parts: &[(&str, &str, &str)]) -> Arc<KnownParts> {
    let values = parts
        .iter()
        .map(|(ipn, mpn, category)| {
            (
                (*ipn).to_owned(),
                PartValues {
                    category: Some((*category).to_owned()),
                    mpn: Some((*mpn).to_owned()),
                    description: Some(format!("{mpn} in {label}")),
                    ..PartValues::default()
                },
            )
        })
        .collect();
    Arc::new(KnownParts::new(server, label, values, BTreeMap::new()))
}

/// The answer to `request` over `catalog` and `teams`, as the app gives it.
fn search(
    catalog: &CsvCatalog,
    teams: &[Arc<KnownParts>],
    request: &SearchRequest,
) -> SearchResponse {
    let at = csv_rows_by_ipn(Some(catalog));
    let listed = ListedTeams::default();
    let csv = CsvRows {
        catalog: Some(catalog),
        at: &at,
    };
    let sources = Sources {
        csv: Some(catalog),
        teams: listed.rows_of(teams, &csv),
    };
    answer(&sources, request)
}

fn request(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        limit: 100,
        selected: None,
        section: None,
        uses: None,
        team: None,
        group_by: GroupBy::Category,
    }
}

/// Every row shown, section by section, as `team code:IPN` (`CSV:IPN` for
/// a part only the CSV catalog has).
fn rows(response: &SearchResponse) -> Vec<String> {
    response
        .hits()
        .map(|hit| {
            let team = match hit.team.as_str() {
                MA => "MA",
                AC => "AC",
                "" => "CSV",
                other => other,
            };
            format!("{team}:{}", hit.ipn)
        })
        .collect()
}

#[test]
fn every_teams_parts_are_in_one_list_with_the_same_ipn_once_per_team() {
    let catalog = csv(&[
        ("RES-1", "R1", "g-res"),
        ("RES-2", "R2", "g-res"),
        ("RES-3", "R3", "g-res"),
    ]);
    let ma = team(MA, "MA", &[("RES-1", "R1", "res"), ("RES-9", "R9", "res")]);
    let ac = team(
        AC,
        "AC",
        &[("RES-1", "R1-AC", "res"), ("RES-5", "R5", "res")],
    );
    let found = search(&catalog, &[ma, ac], &request(""));
    // RES-1 is in both teams: one row each, in team order. RES-2 and RES-3
    // only the CSV catalog has. Then the parts only teams have, by IPN.
    assert_eq!(
        rows(&found),
        [
            "MA:RES-1",
            "AC:RES-1",
            "CSV:RES-2",
            "CSV:RES-3",
            "AC:RES-5",
            "MA:RES-9"
        ]
    );
    assert_eq!(found.total, 6);
    // Each team's row shows that team's own values.
    let mpns: Vec<&str> = found.hits().take(2).map(|hit| hit.mpn.as_str()).collect();
    assert_eq!(mpns, ["R1", "R1-AC"]);
}

#[test]
fn with_one_team_the_list_is_as_before() {
    // Before several teams, the CSV catalog's rows showed in their order,
    // the team's values over them, then the team's own parts.
    let catalog = csv(&[("RES-2", "R2", "g-res"), ("RES-1", "R1", "g-res")]);
    let ma = team(
        MA,
        "MA",
        &[("RES-1", "R1-new", "res"), ("RES-0", "R0", "res")],
    );
    let found = search(&catalog, &[ma], &request(""));
    assert_eq!(rows(&found), ["CSV:RES-2", "MA:RES-1", "MA:RES-0"]);
    assert_eq!(found.sections[0].hits[1].mpn, "R1-new");
    // A team's part over a CSV row keeps the CSV file it came from.
    assert_eq!(found.sections[0].hits[1].csv_file, "g-res.csv");
    assert_eq!(found.sections[0].hits[2].csv_file, "Team catalog");
}

#[test]
fn with_two_teams_a_team_part_never_takes_the_csv_rows_values() {
    // Whose CSV folder this is is not known yet (kicad-parts-e9a3), so a
    // client team's RES-1 must not show our CSV file's data.
    let catalog = csv(&[("RES-1", "R1", "g-res")]);
    let mut ac_values = (*team(AC, "AC", &[("RES-1", "R1-AC", "res")])).clone();
    ac_values.values.get_mut("RES-1").unwrap().symbol = Some("AC-Lib:R".into());
    let ma = team(MA, "MA", &[("RES-1", "R1", "res")]);
    let found = search(&catalog, &[ma, Arc::new(ac_values.clone())], &request(""));
    let ac_row = found.hits().find(|hit| hit.team == AC).unwrap();
    assert_eq!(ac_row.csv_file, "Team catalog");
    assert_eq!(ac_row.description, "R1-AC in AC");
    // With one team, the CSV row fills it in, as before several teams.
    let found = search(&catalog, &[Arc::new(ac_values)], &request(""));
    assert_eq!(found.hits().next().unwrap().csv_file, "g-res.csv");
}

#[test]
fn search_ranks_as_before_and_ties_on_one_part_go_by_team_order() {
    let catalog = csv(&[("CAP-1", "C100N", "g-cap")]);
    let ma = team(
        MA,
        "MA",
        &[("CAP-1", "C100N", "cap"), ("CAP-7", "C100N-X", "cap")],
    );
    let ac = team(
        AC,
        "AC",
        &[("CAP-1", "C100N", "cap"), ("CAP-3", "XC100N", "cap")],
    );
    let found = search(&catalog, &[ma, ac], &request("c100n"));
    // Exact MPN first (in team order), then an MPN prefix, then any match.
    assert_eq!(
        rows(&found),
        ["MA:CAP-1", "AC:CAP-1", "MA:CAP-7", "AC:CAP-3"]
    );
    // The same search with the teams the other way round.
    let ma = team(
        MA,
        "MA",
        &[("CAP-1", "C100N", "cap"), ("CAP-7", "C100N-X", "cap")],
    );
    let ac = team(
        AC,
        "AC",
        &[("CAP-1", "C100N", "cap"), ("CAP-3", "XC100N", "cap")],
    );
    let found = search(&catalog, &[ac, ma], &request("c100n"));
    assert_eq!(
        rows(&found),
        ["AC:CAP-1", "MA:CAP-1", "MA:CAP-7", "AC:CAP-3"]
    );
}

#[test]
fn the_team_filter_shows_only_that_teams_parts() {
    let catalog = csv(&[("RES-1", "R1", "g-res"), ("RES-2", "R2", "g-res")]);
    let ma = team(MA, "MA", &[("RES-1", "R1", "res")]);
    let ac = team(AC, "AC", &[("RES-1", "R1", "res"), ("RES-5", "R5", "res")]);
    let only_ac = SearchRequest {
        team: Some(AC.into()),
        ..request("")
    };
    let found = search(&catalog, &[ma.clone(), ac.clone()], &only_ac);
    // No CSV rows: they belong to no team.
    assert_eq!(rows(&found), ["AC:RES-1", "AC:RES-5"]);
    let only_ma = SearchRequest {
        team: Some(MA.into()),
        ..request("")
    };
    assert_eq!(rows(&search(&catalog, &[ma, ac], &only_ma)), ["MA:RES-1"]);
}

#[test]
fn grouping_by_team_makes_one_section_per_team_in_team_order() {
    let catalog = csv(&[("RES-2", "R2", "g-res")]);
    let ma = team(MA, "MA", &[("RES-1", "R1", "res"), ("CAP-1", "C1", "cap")]);
    let ac = team(AC, "AC", &[("RES-1", "R1", "res")]);
    let by_team = SearchRequest {
        group_by: GroupBy::Team,
        ..request("")
    };
    let found = search(&catalog, &[ac, ma], &by_team);
    let sections: Vec<(&str, &str, usize)> = found
        .sections
        .iter()
        .map(|section| (section.key.as_str(), section.name.as_str(), section.total))
        .collect();
    assert_eq!(
        sections,
        [
            ("team:https://ac.example.com", "AC", 1),
            ("team:https://ma.example.com", "MA", 2),
            ("team:", CSV_SECTION, 1),
        ]
    );
    // The same parts grouped by category.
    let found = search(
        &catalog,
        &[team(
            MA,
            "MA",
            &[("RES-1", "R1", "res"), ("CAP-1", "C1", "cap")],
        )],
        &request(""),
    );
    let names: Vec<&str> = found.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Capacitors", "Resistors"]);
}

#[test]
fn the_selected_part_is_found_in_its_own_team() {
    let catalog = csv(&[]);
    let parts: Vec<(String, String)> = (0..30)
        .map(|n| (format!("RES-{n:02}"), format!("R{n}")))
        .collect();
    let same: Vec<(&str, &str, &str)> = parts
        .iter()
        .map(|(ipn, mpn)| (ipn.as_str(), mpn.as_str(), "res"))
        .collect();
    // Both teams have the same 30 IPNs.
    let ma = team(MA, "MA", &same);
    let ac = team(AC, "AC", &same);
    let wanted = PartKey {
        team: AC.into(),
        ipn: "RES-29".into(),
    };
    let found = search(
        &catalog,
        &[ma, ac],
        &SearchRequest {
            limit: 2,
            selected: Some(wanted.clone()),
            group_by: GroupBy::Team,
            ..request("")
        },
    );
    assert_eq!(
        found.selected.as_deref(),
        Some("team:https://ac.example.com")
    );
    let ac_section = &found.sections[1];
    assert!(ac_section.hits.iter().any(|hit| hit.key() == wanted));
    // The other team's RES-29 is not shown beyond the limit.
    assert_eq!(found.sections[0].hits.len(), 2);
}

#[test]
fn a_part_selected_while_signed_out_becomes_the_first_teams_part() {
    let catalog = csv(&[("RES-1", "R1", "g-res"), ("RES-2", "R2", "g-res")]);
    let ac = team(AC, "AC", &[("RES-1", "R1", "res")]);
    let ma = team(MA, "MA", &[("RES-1", "R1", "res")]);
    let asked = |ipn: &str| SearchRequest {
        selected: Some(PartKey {
            team: String::new(),
            ipn: ipn.into(),
        }),
        ..request("")
    };
    let found = search(&catalog, &[ac.clone(), ma], &asked("RES-1"));
    assert_eq!(
        found.selected_part,
        Some(PartKey {
            team: AC.into(),
            ipn: "RES-1".into()
        })
    );
    // A part still only in the CSV files stays as it is.
    let found = search(&catalog, &[ac], &asked("RES-2"));
    assert_eq!(
        found.selected_part.map(|part| part.team),
        Some(String::new())
    );
}

#[test]
fn a_team_with_new_parts_makes_its_rows_again_and_others_keep_theirs() {
    let catalog = csv(&[]);
    let at = csv_rows_by_ipn(Some(&catalog));
    let csv = CsvRows {
        catalog: Some(&catalog),
        at: &at,
    };
    let listed = ListedTeams::default();
    let ma = team(MA, "MA", &[("RES-1", "R1", "res")]);
    let ac = team(AC, "AC", &[("RES-1", "R1", "res")]);
    let first = listed.rows_of(&[ma.clone(), ac.clone()], &csv);
    // The same parts give the same rows: nothing is made again.
    let again = listed.rows_of(&[ma.clone(), ac.clone()], &csv);
    assert!(Arc::ptr_eq(&first[0], &again[0]) && Arc::ptr_eq(&first[1], &again[1]));
    // A teammate saved on AC: only AC's rows are made again.
    let ac_saved = team(AC, "AC", &[("RES-1", "R1-new", "res")]);
    let after = listed.rows_of(&[ma, ac_saved], &csv);
    assert!(Arc::ptr_eq(&first[0], &after[0]));
    assert!(!Arc::ptr_eq(&first[1], &after[1]));
    assert_eq!(after[1].parts[0].part.mpn, "R1-new");
}

#[test]
fn ten_teams_of_three_thousand_parts_search_quickly() {
    let catalog = csv(&[]);
    let teams: Vec<Arc<KnownParts>> = (0..10)
        .map(|n| {
            let parts: Vec<(String, String)> = (0..3000)
                .map(|p| (format!("RES-{p:04}-0001"), format!("MPN{n}-{p}")))
                .collect();
            let rows: Vec<(&str, &str, &str)> = parts
                .iter()
                .map(|(ipn, mpn)| (ipn.as_str(), mpn.as_str(), "res"))
                .collect();
            team(
                &format!("https://t{n}.example.com"),
                &format!("T{n}"),
                &rows,
            )
        })
        .collect();
    let at = csv_rows_by_ipn(Some(&catalog));
    let csv = CsvRows {
        catalog: Some(&catalog),
        at: &at,
    };
    let listed = ListedTeams::default();
    let sources = Sources {
        csv: Some(&catalog),
        teams: listed.rows_of(&teams, &csv),
    };
    // Rows are made once; each keystroke only compares text.
    let started = Instant::now();
    for query in ["m", "mp", "mpn", "mpn5", "mpn5-", "mpn5-29"] {
        let found = answer(&sources, &request(query));
        assert!(found.total > 0);
    }
    let per_search = started.elapsed() / 6;
    // Generous, for unoptimised test builds on a busy machine.
    assert!(per_search.as_millis() < 1500, "{per_search:?}");
}

// ---- Sections ----------------------------------------------------------

/// Ranked matches as (section, CSV position, rank), best first.
fn ranked(items: &[(&str, usize, u8)]) -> Vec<Ranked> {
    items
        .iter()
        .map(|(code, position, rank)| Ranked {
            section: (*code).to_owned(),
            row: Row::Csv(*position),
            rank: *rank,
        })
        .collect()
}

static NO_NAMES: BTreeMap<String, String> = BTreeMap::new();

fn grouping(limit: usize, searching: bool) -> Grouping<'static> {
    Grouping {
        limit,
        searching,
        selected: None,
        sections: Sections::Categories(&NO_NAMES),
    }
}

fn positions(group: &Group) -> Vec<usize> {
    group
        .rows
        .iter()
        .map(|row| match row {
            Row::Csv(position) => *position,
            Row::Team { part, .. } => *part,
        })
        .collect()
}

fn summary_of(groups: &[Group]) -> Vec<(&str, &str, usize, Vec<usize>)> {
    groups
        .iter()
        .map(|g| (g.key.as_str(), g.name.as_str(), g.total, positions(g)))
        .collect()
}

#[test]
fn without_a_search_sections_are_in_name_order_keeping_rank_order() {
    let groups = group_rows(
        ranked(&[
            ("res", 7, 2),
            ("cap", 3, 2),
            ("res", 1, 2),
            ("ana", 9, 2),
            ("cap", 0, 2),
        ]),
        &grouping(10, false),
    );
    assert_eq!(
        summary_of(&groups),
        [
            ("ana", "Analog ICs", 1, vec![9]),
            ("cap", "Capacitors", 2, vec![3, 0]),
            ("res", "Resistors", 2, vec![7, 1]),
        ]
    );
}

#[test]
fn while_searching_the_section_with_the_best_match_comes_first() {
    // res holds the exact match (rank 0); cap and ana tie on rank 1, so
    // they go by name; dio has only rank-2 matches.
    let groups = group_rows(
        ranked(&[
            ("res", 4, 0),
            ("cap", 3, 1),
            ("ana", 9, 1),
            ("dio", 2, 2),
            ("res", 1, 2),
        ]),
        &grouping(10, true),
    );
    let codes: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
    assert_eq!(codes, ["res", "ana", "cap", "dio"]);
}

#[test]
fn other_parts_always_come_last() {
    for searching in [false, true] {
        let groups = group_rows(
            ranked(&[("", 0, 0), ("res", 1, 2), ("zzz", 2, 2)]),
            &grouping(10, searching),
        );
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names.last(), Some(&"Other parts"), "searching: {searching}");
    }
}

#[test]
fn each_section_shows_at_most_the_limit_but_counts_every_match() {
    let many: Vec<Ranked> = (0..5)
        .map(|n| Ranked {
            section: "res".to_owned(),
            row: Row::Csv(n),
            rank: 2,
        })
        .collect();
    let groups = group_rows(many.clone(), &grouping(2, false));
    assert_eq!((groups[0].total, positions(&groups[0])), (5, vec![0, 1]));
    // Counts only: no parts at all.
    let groups = group_rows(many.clone(), &grouping(0, false));
    assert_eq!((groups[0].total, groups[0].rows.len()), (5, 0));
    // The selected part is always among its section's parts.
    let groups = group_rows(
        many,
        &Grouping {
            selected: Some(Row::Csv(4)),
            ..grouping(2, false)
        },
    );
    assert_eq!(positions(&groups[0]), [0, 1, 4]);
}

#[test]
fn team_names_win_then_starting_names_then_the_code() {
    let names: BTreeMap<String, String> = [("res".to_owned(), "Fixed resistors".to_owned())].into();
    let groups = group_rows(
        ranked(&[("res", 0, 2), ("cap", 1, 2), ("zzz", 2, 2), ("", 3, 2)]),
        &Grouping {
            sections: Sections::Categories(&names),
            ..grouping(10, false)
        },
    );
    let shown: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(
        shown,
        ["Capacitors", "Fixed resistors", "zzz", "Other parts"]
    );
}

#[test]
fn the_first_team_to_name_a_category_names_its_section() {
    let mut ma = (*team(MA, "MA", &[("RES-1", "R1", "res")])).clone();
    ma.category_names = [("res".to_owned(), "MA resistors".to_owned())].into();
    let mut ac = (*team(AC, "AC", &[("RES-2", "R2", "res")])).clone();
    ac.category_names = [("res".to_owned(), "AC resistors".to_owned())].into();
    let found = search(&csv(&[]), &[Arc::new(ma), Arc::new(ac)], &request(""));
    assert_eq!(found.sections[0].name, "MA resistors");
}

#[test]
fn nothing_matching_makes_no_sections() {
    assert!(group_rows(Vec::new(), &grouping(10, true)).is_empty());
}

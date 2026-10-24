//! How the part list tells teams apart (kicad-parts-8a71): each row's team
//! mark, the team filter, and the note about teams whose parts are hidden.
//!
//! A team's mark shows its code as text, such as `MA`, in a chip of the
//! team's colour: never colour alone. With only one team showing parts,
//! the list shows no marks and no filter, so it looks as it always did.
//!
//! Features that work on one team at a time, such as Add part or the
//! library check, pick their team with [`team_for`].

use crate::teams::{Standing, standing_of};
use desktop_contract::{GroupBy, LiveUpdates, TeamSession, TeamStatus, TeamsStatus};
use leptos::prelude::*;
use std::collections::BTreeMap;

/// What a row shows of its team.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamMark {
    /// The team's code, such as `MA`, or a short form of its address.
    pub(crate) label: String,
    /// One of `TEAM_COLOURS`.
    pub(crate) colour: String,
    /// Its server cannot be reached now: its parts are the ones last seen,
    /// and cannot be edited until it is back.
    pub(crate) offline: bool,
}

/// The marks of every team that shows parts, by server address, and
/// whether rows show them (two teams or more).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TeamMarks {
    pub(crate) by_team: BTreeMap<String, TeamMark>,
    pub(crate) shown: bool,
}

impl TeamMarks {
    pub(crate) fn of(status: &TeamsStatus) -> Self {
        let by_team: BTreeMap<String, TeamMark> = status
            .teams
            .iter()
            .filter(|team| shows_parts(team))
            .map(|team| {
                (
                    team.server_url.clone(),
                    TeamMark {
                        label: team.label(),
                        colour: team.colour.clone(),
                        offline: standing_of(team) == Standing::Offline,
                    },
                )
            })
            .collect();
        Self {
            shown: by_team.len() >= 2,
            by_team,
        }
    }

    /// The mark of the team at `team`, while rows show marks.
    pub(crate) fn mark(&self, team: &str) -> Option<&TeamMark> {
        self.by_team.get(team).filter(|_| self.shown)
    }
}

/// Whether `team` shows parts in the list: it has a session, live or
/// offline. A team asked to sign in again shows none.
pub(crate) fn shows_parts(team: &TeamStatus) -> bool {
    match &team.session {
        TeamSession::SignedIn { live, .. } => !matches!(live, LiveUpdates::SignInAgain { .. }),
        _ => false,
    }
}

/// The team the part list shows: the one chosen in the filter, while it
/// shows parts and the filter is shown (two teams or more); otherwise
/// every team (`None`).
pub(crate) fn list_filter(status: &TeamsStatus) -> Option<String> {
    let marks = TeamMarks::of(status);
    let chosen = &status.choices.filter;
    (marks.shown && marks.by_team.contains_key(chosen)).then(|| chosen.clone())
}

/// How the part list groups: as the person chose while two teams or more
/// show parts; by category with one, where a team section would be all of
/// it.
pub(crate) fn list_group_by(status: &TeamsStatus) -> GroupBy {
    if TeamMarks::of(status).shown {
        status.choices.group_by
    } else {
        GroupBy::Category
    }
}

/// Every team that shows parts, by server address, in team order.
pub(crate) fn showing_teams(status: &TeamsStatus) -> Vec<String> {
    status
        .teams
        .iter()
        .filter(|team| shows_parts(team))
        .map(|team| team.server_url.clone())
        .collect()
}

/// Every team the person can add and change parts in now: signed in with
/// edit access, and its server reachable. A new part or a library check
/// needs the server, so an offline team is not one of them.
pub(crate) fn editable_teams(status: &TeamsStatus) -> Vec<String> {
    status
        .teams
        .iter()
        .filter(|team| {
            matches!(
                &team.session,
                TeamSession::SignedIn {
                    can_edit: true,
                    live: LiveUpdates::Live | LiveUpdates::Connecting,
                    ..
                }
            )
        })
        .map(|team| team.server_url.clone())
        .collect()
}

/// The note under the count line about teams whose parts are not shown,
/// and what to do; `None` when every team's parts are shown.
pub(crate) fn hidden_teams_note(status: &TeamsStatus) -> Option<String> {
    let count = |standing: Standing| {
        status
            .teams
            .iter()
            .filter(|team| standing_of(team) == standing && !shows_parts(team))
            .count()
    };
    let (again, unreachable) = (count(Standing::SignInAgain), count(Standing::Offline));
    let teams = |n: usize| {
        if n == 1 {
            "1 team".to_owned()
        } else {
            format!("{n} teams")
        }
    };
    let mut parts = Vec::new();
    if again > 0 {
        let (need, their) = if again == 1 {
            ("needs", "its")
        } else {
            ("need", "their")
        };
        parts.push(format!(
            "{} {need} you to sign in again, so {their} parts are not shown. Sign in from Teams at the top right.",
            teams(again)
        ));
    }
    if unreachable > 0 {
        let their = if unreachable == 1 { "its" } else { "their" };
        parts.push(format!(
            "{} could not be reached, so {their} parts are not shown yet. The app keeps trying.",
            teams(unreachable)
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The team a feature that works on one team at a time uses, among
/// `candidates` (server addresses, in team order): the team chosen in the
/// part list's filter, else the team the feature last used, else the
/// first. `None` when there is no candidate.
pub(crate) fn team_for(
    filter: Option<&str>,
    last_used: Option<&str>,
    candidates: &[String],
) -> Option<String> {
    let listed = |team: &&str| candidates.iter().any(|candidate| candidate == team);
    filter
        .filter(listed)
        .or_else(|| last_used.filter(listed))
        .map(str::to_owned)
        .or_else(|| candidates.first().cloned())
}

/// Each team in `teams` (server addresses), with its code, for a list box.
pub(crate) fn team_options(status: &TeamsStatus, teams: &[String]) -> Vec<(String, String)> {
    status
        .teams
        .iter()
        .filter(|team| teams.contains(&team.server_url))
        .map(|team| (team.server_url.clone(), team.label()))
        .collect()
}

/// The small Team list box of a feature that works on one team at a time,
/// such as Add part. Shown only with two teams or more to choose from.
/// The options and the chosen team are kept apart, so a choice only moves
/// the selection, and the keyboard focus stays on it (mx-b28a31).
#[component]
pub(crate) fn TeamSelect(
    /// The element ID, which the label names.
    id: &'static str,
    /// Each team to choose from, by server address, with its code.
    options: Memo<Vec<(String, String)>>,
    /// The team chosen, by server address.
    #[prop(into)]
    chosen: Signal<String>,
    changed: Callback<String>,
) -> impl IntoView {
    move || {
        let options = options.get();
        (options.len() >= 2).then(|| {
            view! {
                <p class="team-select">
                    <label for=id>"Team"</label>
                    <select id=id on:change:target=move |event| changed.run(event.target().value())>
                        {options
                            .into_iter()
                            .map(|(server, label)| {
                                let this = server.clone();
                                view! {
                                    <option value=server prop:selected=move || chosen.get() == this>
                                        {label}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </p>
            }
        })
    }
}

/// A row's team mark: the team's code in its colour, and "offline" in
/// words while its server cannot be reached.
#[component]
pub(crate) fn TeamChip(mark: TeamMark) -> impl IntoView {
    let class = format!("team-chip team-colour-{}", mark.colour);
    view! {
        <span class=class>
            <span class="visually-hidden">"Team "</span>
            {mark.label}
            {mark.offline.then(|| view! { <span class="team-chip-state">" · offline"</span> })}
        </span>
    }
}

/// The mark of a part only the CSV files have, beside team marks.
#[component]
pub(crate) fn CsvChip() -> impl IntoView {
    view! {
        <span class="team-chip team-chip-csv">
            <span class="visually-hidden">"From the "</span>
            "CSV"
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_contract::{SignInsKept, TeamChoices};

    fn team(code: &str, colour: &str, session: TeamSession) -> TeamStatus {
        TeamStatus {
            server_url: format!("https://{}.example.com", code.to_lowercase()),
            email: "ada@example.test".into(),
            code: code.into(),
            colour: colour.into(),
            session,
            note: None,
            libraries_made_from: None,
        }
    }

    fn signed_in(live: LiveUpdates) -> TeamSession {
        TeamSession::SignedIn {
            name: "Ada".into(),
            email: "ada@example.test".into(),
            can_edit: true,
            live,
            checkpoint: 1,
            imports: 1,
        }
    }

    fn status(teams: Vec<TeamStatus>, filter: &str) -> TeamsStatus {
        TeamsStatus {
            teams,
            choices: TeamChoices {
                filter: filter.into(),
                ..TeamChoices::default()
            },
            kept: SignInsKept::AfterRestart,
            form_server: String::new(),
            form_email: String::new(),
            problem: None,
            note: None,
        }
    }

    const MA: &str = "https://ma.example.com";
    const AC: &str = "https://ac.example.com";

    #[test]
    fn one_team_shows_no_marks_and_no_filter() {
        let ma = team("MA", "blue", signed_in(LiveUpdates::Live));
        let out = team("AC", "green", TeamSession::SignedOut { problem: None });
        let one = status(vec![ma.clone(), out], MA);
        let marks = TeamMarks::of(&one);
        assert!(!marks.shown);
        assert_eq!(marks.mark(MA), None);
        assert_eq!(list_filter(&one), None, "the list shows every part");
    }

    #[test]
    fn two_teams_mark_every_row_with_code_colour_and_offline() {
        let ma = team("MA", "blue", signed_in(LiveUpdates::Live));
        let offline = signed_in(LiveUpdates::Reconnecting {
            message: "refused".into(),
        });
        let ac = team("AC", "green", offline);
        let marks = TeamMarks::of(&status(vec![ma, ac], ""));
        assert!(marks.shown);
        assert_eq!(
            marks.mark(MA),
            Some(&TeamMark {
                label: "MA".into(),
                colour: "blue".into(),
                offline: false
            })
        );
        assert!(marks.mark(AC).unwrap().offline);
        // A team whose code is not known yet shows its address.
        let unnamed = team("", "teal", signed_in(LiveUpdates::Live));
        let named = team("MA", "blue", signed_in(LiveUpdates::Live));
        let marks = TeamMarks::of(&status(vec![named, unnamed], ""));
        assert_eq!(
            marks.mark("https://.example.com").unwrap().label,
            ".example.com"
        );
    }

    #[test]
    fn the_filter_shows_the_chosen_team_while_it_shows_parts() {
        let ma = team("MA", "blue", signed_in(LiveUpdates::Live));
        let ac = team("AC", "green", signed_in(LiveUpdates::Live));
        assert_eq!(
            list_filter(&status(vec![ma.clone(), ac.clone()], AC)),
            Some(AC.to_owned())
        );
        assert_eq!(list_filter(&status(vec![ma.clone(), ac.clone()], "")), None);
        // A chosen team that signed out: every team is shown meanwhile.
        let out = team("AC", "green", TeamSession::SignedOut { problem: None });
        let be = team("BE", "teal", signed_in(LiveUpdates::Live));
        assert_eq!(list_filter(&status(vec![ma, out, be], AC)), None);
    }

    #[test]
    fn grouping_by_team_needs_two_teams() {
        let ma = team("MA", "blue", signed_in(LiveUpdates::Live));
        let ac = team("AC", "green", signed_in(LiveUpdates::Live));
        let mut both = status(vec![ma.clone(), ac], "");
        both.choices.group_by = GroupBy::Team;
        assert_eq!(list_group_by(&both), GroupBy::Team);
        let mut one = status(vec![ma], "");
        one.choices.group_by = GroupBy::Team;
        assert_eq!(list_group_by(&one), GroupBy::Category);
    }

    #[test]
    fn only_reachable_teams_where_you_can_edit_take_new_parts() {
        let ma = team("MA", "blue", signed_in(LiveUpdates::Live));
        let offline = team(
            "AC",
            "green",
            signed_in(LiveUpdates::Reconnecting {
                message: "refused".into(),
            }),
        );
        let mut viewer = team("BE", "teal", signed_in(LiveUpdates::Live));
        if let TeamSession::SignedIn { can_edit, .. } = &mut viewer.session {
            *can_edit = false;
        }
        let all = status(vec![ma, offline, viewer], "");
        assert_eq!(editable_teams(&all), [MA.to_owned()]);
        assert_eq!(showing_teams(&all).len(), 3);
    }

    #[test]
    fn hidden_teams_are_counted_with_what_to_do() {
        let ma = team("MA", "blue", signed_in(LiveUpdates::Live));
        assert_eq!(hidden_teams_note(&status(vec![ma.clone()], "")), None);
        let again = |code: &str| {
            team(
                code,
                "green",
                TeamSession::SignedOut {
                    problem: Some("Sign in again.".into()),
                },
            )
        };
        assert_eq!(
            hidden_teams_note(&status(vec![ma.clone(), again("AC")], "")).as_deref(),
            Some(
                "1 team needs you to sign in again, so its parts are not shown. Sign in from Teams at the top right."
            )
        );
        let unreachable = team(
            "BE",
            "teal",
            TeamSession::Reaching {
                problem: Some("refused".into()),
            },
        );
        assert_eq!(
            hidden_teams_note(&status(
                vec![ma.clone(), again("AC"), again("CE"), unreachable],
                ""
            ))
            .as_deref(),
            Some(
                "2 teams need you to sign in again, so their parts are not shown. Sign in from Teams at the top right. 1 team could not be reached, so its parts are not shown yet. The app keeps trying."
            )
        );
        // Offline after it was reached: its parts are still shown.
        let offline = team(
            "AC",
            "green",
            signed_in(LiveUpdates::Reconnecting {
                message: "refused".into(),
            }),
        );
        assert_eq!(hidden_teams_note(&status(vec![ma, offline], "")), None);
        // Signed out on purpose: nothing to say.
        let out = team("DE", "rose", TeamSession::SignedOut { problem: None });
        assert_eq!(hidden_teams_note(&status(vec![out], "")), None);
    }

    /// The value of the colour token `name` in the stylesheet, such as
    /// `#dbe7f6`.
    fn token(name: &str) -> String {
        let styles = include_str!("../styles.css");
        let at = styles
            .find(&format!("--{name}:"))
            .unwrap_or_else(|| panic!("no --{name} in styles.css"));
        let rest = &styles[at + name.len() + 3..];
        rest[..rest.find(';').unwrap()].trim().to_owned()
    }

    /// WCAG 2 relative luminance of `#rrggbb`.
    fn luminance(colour: &str) -> f64 {
        let channel = |at: usize| {
            let value = f64::from(u8::from_str_radix(&colour[at..at + 2], 16).unwrap()) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn every_team_colour_passes_contrast_where_it_shows() {
        let styles = include_str!("../styles.css");
        let (white, selected, navy) = (token("panel"), token("accent-soft"), token("brand"));
        for colour in desktop_contract::TEAM_COLOURS {
            let (bg, ink) = (
                token(&format!("team-{colour}-bg")),
                token(&format!("team-{colour}-ink")),
            );
            assert!(
                styles.contains(&format!(".team-colour-{colour} {{")),
                "{colour}"
            );
            // The code reads on its chip, and the chip's border on any row.
            assert!(contrast(&ink, &bg) >= 4.5, "{colour} text on its chip");
            assert!(contrast(&ink, &white) >= 4.5, "{colour} border on a row");
            assert!(
                contrast(&ink, &selected) >= 4.5,
                "{colour} on a selected row"
            );
            // The chip stands out on the dark header too.
            assert!(contrast(&bg, &navy) >= 3.0, "{colour} chip on the header");
        }
        // The CSV mark's muted text reads on white.
        assert!(contrast(&token("muted"), &white) >= 4.5);
    }

    #[test]
    fn a_feature_uses_the_filters_team_else_its_last_else_the_first() {
        let teams = vec![MA.to_owned(), AC.to_owned()];
        assert_eq!(team_for(Some(AC), Some(MA), &teams).as_deref(), Some(AC));
        assert_eq!(team_for(None, Some(AC), &teams).as_deref(), Some(AC));
        assert_eq!(team_for(None, None, &teams).as_deref(), Some(MA));
        // A team that is no longer a candidate is passed over.
        let gone = "https://gone.example.com";
        assert_eq!(
            team_for(Some(gone), Some(gone), &teams).as_deref(),
            Some(MA)
        );
        assert_eq!(team_for(None, None, &[]), None);
    }
}

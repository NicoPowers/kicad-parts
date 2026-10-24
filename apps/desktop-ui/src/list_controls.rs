//! The two choices above the part list, shown with two or more teams
//! (kicad-parts-8a71): which team's parts to show (All teams, the default,
//! or one team), and how to group them (by category, the default, or by
//! team). Both are kept for the next start.
//!
//! Each is a native list box, so it works with the keyboard and screen
//! readers as people expect. The options and the chosen value are kept
//! apart, so a choice only moves the selection: the list box is never
//! drawn again under the person's focus (mx-b28a31).

use crate::backend;
use crate::team_marks::{list_filter, shows_parts};
use desktop_contract::{GroupBy, TeamChoices, TeamsStatus};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The value of the "All teams" option.
const ALL_TEAMS: &str = "";

#[component]
pub(crate) fn ListControls(
    /// The teams' status, without checkpoints, so saves do not redraw it.
    teams: Memo<Option<TeamsStatus>>,
    set_teams: RwSignal<Option<TeamsStatus>>,
) -> impl IntoView {
    // Each team that shows parts, by server address and code, in order.
    let options = Memo::new(move |_| {
        teams.with(|status| {
            status.as_ref().map_or_else(Vec::new, |status| {
                status
                    .teams
                    .iter()
                    .filter(|team| shows_parts(team))
                    .map(|team| (team.server_url.clone(), team.label()))
                    .collect::<Vec<_>>()
            })
        })
    });
    let filter = Memo::new(move |_| {
        teams.with(|status| status.as_ref().and_then(list_filter).unwrap_or_default())
    });
    let group_by = Memo::new(move |_| {
        teams.with(|status| {
            status
                .as_ref()
                .map(|status| status.choices.group_by)
                .unwrap_or_default()
        })
    });
    // Shows the change at once, then keeps it for the next start.
    let choose = move |change: Box<dyn FnOnce(&mut TeamChoices) + Send>| {
        let Some(mut choices) =
            teams.with_untracked(|status| status.as_ref().map(|s| s.choices.clone()))
        else {
            return;
        };
        change(&mut choices);
        set_teams.update(|status| {
            if let Some(status) = status {
                status.choices = choices.clone();
            }
        });
        spawn_local(async move {
            if let Ok(status) = backend::team_choose(choices).await {
                set_teams.set(Some(status));
            }
        });
    };
    move || {
        let options = options.get();
        (options.len() >= 2).then(|| {
            view! {
                <div class="list-controls">
                    <label for="team-filter">"Team"</label>
                    <select
                        id="team-filter"
                        on:change:target=move |event| {
                            let value = event.target().value();
                            choose(Box::new(move |choices| choices.filter = value));
                        }
                    >
                        <option value=ALL_TEAMS prop:selected=move || filter.with(String::is_empty)>
                            "All teams"
                        </option>
                        {options
                            .into_iter()
                            .map(|(server, label)| {
                                let this = server.clone();
                                view! {
                                    <option value=server prop:selected=move || filter.get() == this>
                                        {label}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                    <label for="group-by">"Group by"</label>
                    <select
                        id="group-by"
                        on:change:target=move |event| {
                            let by_team = event.target().value() == "team";
                            choose(Box::new(move |choices| {
                                choices.group_by = if by_team { GroupBy::Team } else { GroupBy::Category };
                            }));
                        }
                    >
                        <option value="category" prop:selected=move || group_by.get() == GroupBy::Category>
                            "Category"
                        </option>
                        <option value="team" prop:selected=move || group_by.get() == GroupBy::Team>
                            "Team"
                        </option>
                    </select>
                </div>
            }
        })
    }
}

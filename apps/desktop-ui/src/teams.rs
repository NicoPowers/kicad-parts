//! The Teams area in the header: every team the person works with, each
//! signed in, offline or needing a sign-in, with Sign out, Sign in and
//! Remove, and a form that adds a team or signs in again. Each team's name
//! is in its colour, as its mark in the part list is. The part list shows
//! every team's parts at once (kicad-parts-8a71).
//!
//! A team whose KiCad libraries were made from another server address asks,
//! on its row, whether the team moved (kicad-parts-092b). Each row also
//! shows the team's library repo (kicad-parts-ab30, see [`crate::library_repo`]).

use crate::as_of::{Said, SaidView};
use crate::backend;
use crate::focus::{Scrolling, focus_found_later, focus_later};
use crate::library_repo::library_repo_part;
use desktop_contract::{
    LibrariesMadeFrom, LiveUpdates, SignInRequest, SignInsKept, TeamSession, TeamStatus,
    TeamsStatus, UseNewServerRequest, two_servers_claim_a_team,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

const READY: &str = "kicad-badge kicad-badge-ready";
const OFF: &str = "kicad-badge team-badge-off";
const PROBLEM: &str = "kicad-badge kicad-badge-problem";

/// The ID of Cancel in the question before removing a team.
const REMOVE_CANCEL: &str = "team-remove-cancel";

/// The ID of the sign-in form's password box.
const PASSWORD: &str = "team-password";

/// The Teams menu in the header.
///
/// `teams` is the status without checkpoints, so new Changes do not
/// re-render (and close) the menu. What the person typed and whether the
/// menu is open live here, outside the part that re-renders.
#[component]
pub fn TeamsMenu(
    teams: Memo<Option<TeamsStatus>>,
    set_teams: RwSignal<Option<TeamsStatus>>,
) -> impl IntoView {
    let server_url = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    let busy = RwSignal::new(false);
    // True after Add a team or Sign in, until a sign-in works.
    let adding = RwSignal::new(false);
    // Fill the form once from the suggested or saved values.
    Effect::new(move |filled: Option<bool>| {
        if filled == Some(true) {
            return true;
        }
        let Some(status) = teams.get() else {
            return false;
        };
        server_url.set(status.form_server.clone());
        email.set(status.form_email.clone());
        true
    });
    let toggled = move |event: leptos::ev::Event| {
        let is_open = event
            .target()
            .and_then(|target| js_sys::Reflect::get(&target, &"open".into()).ok())
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        open.set(is_open);
    };
    let sign_in = move || {
        busy.set(true);
        let request = SignInRequest {
            server_url: server_url.get_untracked(),
            email: email.get_untracked(),
            password: password.get_untracked(),
        };
        password.set(String::new());
        spawn_local(async move {
            let result = backend::team_sign_in(request).await;
            busy.set(false);
            if let Ok(status) = result {
                if status.problem.is_none() {
                    adding.set(false);
                    open.set(false);
                }
                set_teams.set(Some(status));
            }
        });
    };
    let act = move |action: Action, server: String| {
        spawn_local(async move {
            let result = match action {
                Action::SignOut => backend::team_sign_out(&server).await,
                Action::Remove => backend::team_remove(&server).await,
            };
            if let Ok(status) = result {
                set_teams.set(Some(status));
            }
        });
    };
    // Sign in again to one team: its address and email fill in the form.
    let sign_in_again = move |team: TeamStatus| {
        server_url.set(team.server_url);
        email.set(team.email);
        adding.set(true);
        focus_later(PASSWORD);
    };
    // Only the rows follow every status change. The form is drawn once
    // while it shows, so a team going offline never takes the focus away
    // from what the person is typing.
    let anyone_signed_in = Memo::new(move |_| {
        teams.with(|status| {
            status
                .as_ref()
                .is_some_and(|status| status.teams.iter().any(has_session))
        })
    });
    let has_teams = Memo::new(move |_| {
        teams.with(|status| {
            status
                .as_ref()
                .is_some_and(|status| !status.teams.is_empty())
        })
    });
    let show_form = Memo::new(move |_| adding.get() || !anyone_signed_in.get());
    let badge = Memo::new(move |_| teams.with(|status| status.as_ref().map(summary)));
    let until_closed = Memo::new(move |_| {
        teams.with(|status| match status.as_ref().map(|status| &status.kept) {
            Some(SignInsKept::UntilClosed { reason }) => Some(format!(
                "You stay signed in only until the app closes, because {reason}. After a restart, the app asks for each password again."
            )),
            _ => None,
        })
    });
    let problem = Memo::new(move |_| {
        teams.with(|status| status.as_ref().and_then(|status| status.problem.clone()))
    });
    let note = Memo::new(move |_| {
        teams.with(|status| status.as_ref().and_then(|status| status.note.clone()))
    });
    // The team asking whether to remove it, by server address.
    let removing = RwSignal::new(None::<String>);
    let servers = Memo::new(move |_| {
        teams.with(|status| {
            status.as_ref().map_or_else(Vec::new, |status| {
                status
                    .teams
                    .iter()
                    .map(|team| team.server_url.clone())
                    .collect::<Vec<_>>()
            })
        })
    });
    let loaded = Memo::new(move |_| badge.with(Option::is_some));
    let look = move || badge.with(|badge| badge.as_ref().map(|badge| badge.1).unwrap_or_default());
    let words = move || {
        badge.with(|badge| {
            badge
                .as_ref()
                .map(|badge| badge.0.clone())
                .unwrap_or_default()
        })
    };
    move || {
        loaded.get().then(|| view! {
            <details class="team-settings" prop:open=move || open.get() on:toggle=toggled>
                <summary class=look>{words}</summary>
                <div class="kicad-settings-body team-settings-body">
                    <Show when=move || has_teams.get()>
                        <h2 class="teams-heading" id="teams-heading">"Your teams"</h2>
                        <ul class="team-list" aria-labelledby="teams-heading">
                            <For
                                each=move || servers.get()
                                key=|server| server.clone()
                                children=move |server| team_row(server, teams, set_teams, removing, act, sign_in_again)
                            />
                        </ul>
                    </Show>
                    {move || until_closed.get().map(|text| view! { <p class="hint" id="sign-ins-until-closed">{text}</p> })}
                    {move || note.get().map(|text| view! { <p class="hint" id="teams-note" role="status">{text}</p> })}
                    <Show
                        when=move || show_form.get()
                        fallback=move || view! {
                            <button
                                type="button"
                                id="team-add"
                                on:click=move |_| {
                                    server_url.set(String::new());
                                    email.set(String::new());
                                    adding.set(true);
                                    focus_later("team-server");
                                }
                            >
                                "Add a team"
                            </button>
                        }
                    >
                        <form
                            class="team-sign-in"
                            aria-label="Sign in to the team catalog"
                            on:submit=move |event| {
                                event.prevent_default();
                                sign_in();
                            }
                        >
                            <p>
                                {move || if has_teams.get() {
                                    "Sign in to a team, or add one. Each client's team has its own server."
                                } else {
                                    "Sign in to see each part's history and to edit parts with your team."
                                }}
                            </p>
                            <label for="team-server">"Team server address"</label>
                            // Text, not url: the address may leave out https://,
                            // which a url input would refuse before the app sees it.
                            <input
                                id="team-server"
                                type="text"
                                inputmode="url"
                                spellcheck="false"
                                placeholder="https://team.example.com"
                                prop:value=move || server_url.get()
                                on:input:target=move |event| server_url.set(event.target().value())
                            />
                            <label for="team-email">"Email"</label>
                            <input
                                id="team-email"
                                type="email"
                                autocomplete="username"
                                spellcheck="false"
                                prop:value=move || email.get()
                                on:input:target=move |event| email.set(event.target().value())
                            />
                            <label for=PASSWORD>"Password"</label>
                            <input
                                id=PASSWORD
                                type="password"
                                autocomplete="current-password"
                                prop:value=move || password.get()
                                on:input:target=move |event| password.set(event.target().value())
                            />
                            <p class="hint">
                                {move || if until_closed.with(Option::is_some) {
                                    "The app never saves your password."
                                } else {
                                    "You stay signed in after a restart. The app never saves your password."
                                }}
                            </p>
                            {move || problem.get().map(|message| view! { <p class="problem" role="alert">{message}</p> })}
                            <div class="team-actions">
                                <button type="submit" class="primary" disabled=move || busy.get()>
                                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                                </button>
                                {move || anyone_signed_in.get().then(|| view! {
                                    <button type="button" on:click=move |_| adding.set(false)>"Cancel"</button>
                                })}
                            </div>
                        </form>
                    </Show>
                </div>
            </details>
        })
    }
}

/// One team's row in the Teams menu: its name, where it stands, and what
/// the person can do. It stays in place while the team's status changes,
/// so the focus stays on its buttons. Remove asks once, in place.
fn team_row(
    server: String,
    teams: Memo<Option<TeamsStatus>>,
    set_teams: RwSignal<Option<TeamsStatus>>,
    removing: RwSignal<Option<String>>,
    act: impl Fn(Action, String) + Copy + Send + Sync + 'static,
    sign_in_again: impl Fn(TeamStatus) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let this = server.clone();
    let team = Memo::new(move |_| {
        teams.with(|status| {
            status
                .as_ref()
                .and_then(|status| status.teams.iter().find(|team| team.server_url == this))
                .cloned()
        })
    });
    let label = move || team.with(|team| team.as_ref().map(TeamStatus::label).unwrap_or_default());
    // The team's name in its colour, as the part list marks its parts.
    let chip_class = move || {
        team.with(|team| {
            let colour = team
                .as_ref()
                .map(|team| team.colour.as_str())
                .unwrap_or_default();
            format!("team-chip team-colour-{colour}")
        })
    };
    let address = move || {
        team.with(|team| {
            let team = team.as_ref()?;
            (team.label() != address_of(&team.server_url)).then(|| team.server_url.clone())
        })
    };
    // Drawn again only when the words change, not at every status poll.
    let said = Memo::new(move |_| {
        team.with(|team| {
            team.as_ref()
                .map_or_else(|| Said::plain(String::new()), state_words)
        })
    });
    let words = move || view! { <SaidView said=said.get() /> };
    let note = move || team.with(|team| team.as_ref().and_then(|team| team.note.clone()));
    let can_sign_out = move || team.with(|team| team.as_ref().is_some_and(has_session));
    let needs_password = move || {
        team.with(|team| {
            team.as_ref()
                .is_some_and(|team| matches!(team.session, TeamSession::SignedOut { .. }))
        })
    };
    let asking = {
        let server = server.clone();
        Memo::new(move |_| removing.with(|removing| removing.as_deref() == Some(server.as_str())))
    };
    let row_id = format!("team-row-{}", id_part(&server));
    let question = new_server_question(server.clone(), row_id.clone(), team, set_teams);
    let library_repo = library_repo_part(server.clone(), row_id.clone(), team, set_teams);
    let (sign_out_server, remove_server, ask_server) = (server.clone(), server.clone(), server);
    view! {
        <li class="team-row" id=row_id>
            <p class="team-name">
                <strong class=chip_class>{label}</strong>
                {move || address().map(|address| view! { <span class="team-server">{address}</span> })}
            </p>
            <p class="team-state">{words}</p>
            {move || note().map(|note| view! { <p class="team-state hint">{note}</p> })}
            {question}
            <Show
                when=move || asking.get()
                fallback=move || {
                    let (sign_out_server, ask_server) = (sign_out_server.clone(), ask_server.clone());
                    view! {
                        <div class="team-actions">
                            {move || needs_password().then(|| view! {
                                <button
                                    type="button"
                                    on:click=move |_| {
                                        if let Some(team) = team.get_untracked() {
                                            sign_in_again(team);
                                        }
                                    }
                                >
                                    "Sign in"
                                </button>
                            })}
                            {
                                let sign_out_server = sign_out_server.clone();
                                move || {
                                    let server = sign_out_server.clone();
                                    can_sign_out().then(|| view! {
                                        <button type="button" on:click=move |_| act(Action::SignOut, server.clone())>
                                            "Sign out"
                                        </button>
                                    })
                                }
                            }
                            <button
                                type="button"
                                on:click={
                                    let server = ask_server.clone();
                                    move |_| {
                                        removing.set(Some(server.clone()));
                                        focus_later(REMOVE_CANCEL);
                                    }
                                }
                            >
                                "Remove"
                            </button>
                        </div>
                    }
                }
            >
                <div class="team-confirm" role="group" aria-labelledby="team-remove-question">
                    <p id="team-remove-question">
                        {move || format!(
                            "Remove {} from your teams? You'll need the address, email and password to add it again.",
                            label()
                        )}
                    </p>
                    <div class="team-actions">
                        <button
                            type="button"
                            id="team-remove-confirm"
                            on:click={
                                let server = remove_server.clone();
                                move |_| {
                                    removing.set(None);
                                    act(Action::Remove, server.clone());
                                }
                            }
                        >
                            "Remove"
                        </button>
                        <button
                            type="button"
                            id=REMOVE_CANCEL
                            on:click=move |_| removing.set(None)
                        >
                            "Cancel"
                        </button>
                    </div>
                </div>
            </Show>
            {library_repo}
        </li>
    }
}

/// What a team's row says when its KiCad libraries come from another
/// server address. Another server could claim the team, so the app writes
/// them only once the person says the team moved.
///
/// While that other server is in the person's teams too, two servers say
/// they are the team: the row says so and offers no answer, so one click
/// can't hand the libraries back and forth. Otherwise it asks once. "Not
/// now" leaves them as they are, and keeps the answer one button away.
fn new_server_question(
    server: String,
    row_id: String,
    team: Memo<Option<TeamStatus>>,
    set_teams: RwSignal<Option<TeamsStatus>>,
) -> impl IntoView {
    // Only the question's own words redraw it, so the focus stays on its
    // buttons while the rest of the team's status changes.
    let asked = Memo::new(move |_| {
        team.with(|team| {
            team.as_ref()
                .and_then(|team| team.libraries_made_from.clone())
        })
    });
    let not_now = RwSignal::new(false);
    let problem = RwSignal::new(None::<String>);
    let question_id = format!("{row_id}-libraries-question");
    // After an answer, the keyboard goes on to the row's first button.
    let focus_row = move |row_id: String| {
        focus_found_later(
            move || {
                document()
                    .query_selector(&format!("#{row_id} .team-actions button"))
                    .ok()
                    .flatten()
            },
            Scrolling::IntoView,
        );
    };
    // Sends what the question showed, so an answer to a question that
    // changed meanwhile is refused.
    let use_new_server = {
        let (server, row_id) = (server.clone(), row_id.clone());
        move |made: LibrariesMadeFrom| {
            let request = UseNewServerRequest {
                server_url: server.clone(),
                code: made.code,
                made_from: made.server,
            };
            let row_id = row_id.clone();
            spawn_local(async move {
                match backend::kicad_libraries_use_new_server(request).await {
                    Ok(status) => {
                        problem.set(None);
                        set_teams.set(Some(status));
                        focus_row(row_id);
                    }
                    Err(message) => problem.set(Some(message)),
                }
            });
        }
    };
    let problem_view = move || {
        problem
            .get()
            .map(|message| view! { <p class="problem" role="alert">{message}</p> })
    };
    move || {
        let made = asked.get()?;
        let code = made.code.clone();
        if made.in_your_teams {
            let text = two_servers_claim_a_team(&code, &made.server, &server);
            // Each address has its own element, so a journey can blank out
            // its port, which changes from run to run.
            let parts = split_out(&text, &[&made.server, &server])
                .into_iter()
                .map(|(part, is_address)| {
                    if is_address {
                        view! { <span class="team-question-server">{part}</span> }.into_any()
                    } else {
                        part.into_any()
                    }
                })
                .collect_view();
            return Some(
                view! {
                    <div class="team-confirm team-question">
                        <p class="team-two-servers">{parts}</p>
                    </div>
                }
                .into_any(),
            );
        }
        let answer = {
            let (use_new_server, made) = (use_new_server.clone(), made.clone());
            move |_| use_new_server(made.clone())
        };
        if not_now.get() {
            // Asked once; the answer stays one button away.
            let text = format!(
                "The app does not update the KiCad libraries for {code} until you choose Use the new address."
            );
            return Some(
                view! {
                    <div class="team-question">
                        <p class="team-state hint">{text}</p>
                        <button type="button" class="team-use-new-server" on:click=answer>
                            "Use the new address"
                        </button>
                        {problem_view}
                    </div>
                }
                .into_any(),
            );
        }
        // The new address has its own element, so a journey can blank out
        // its port, which changes from run to run.
        let (text, new_server) = (
            format!(
                "The KiCad libraries for {code} come from {}. Update them from ",
                made.server
            ),
            server.clone(),
        );
        let (labelled_by, question_id) = (question_id.clone(), question_id.clone());
        let row_id = row_id.clone();
        Some(
            view! {
                <div class="team-confirm team-question" role="group" aria-labelledby=labelled_by>
                    <p id=question_id>
                        {text}
                        <span class="team-question-server">{new_server}</span>
                        " instead? Only do this if the team moved to this new address."
                    </p>
                    {problem_view}
                    <div class="team-actions">
                        <button type="button" class="team-use-new-server" on:click=answer>
                            "Use the new address"
                        </button>
                        <button
                            type="button"
                            on:click=move |_| {
                                not_now.set(true);
                                focus_row(row_id.clone());
                            }
                        >
                            "Not now"
                        </button>
                    </div>
                </div>
            }
            .into_any(),
        )
    }
}

/// `text` cut into parts, each with whether it is one of `addresses`. At
/// one place, the longest address wins, so `http://h:80` never takes the
/// start of `http://h:8090`.
fn split_out(text: &str, addresses: &[&str]) -> Vec<(String, bool)> {
    let mut parts = Vec::new();
    let mut rest = text;
    loop {
        let next = addresses
            .iter()
            .filter(|address| !address.is_empty())
            .filter_map(|address| rest.find(address).map(|at| (at, *address)))
            .min_by_key(|(at, address)| (*at, std::cmp::Reverse(address.len())));
        let Some((at, address)) = next else {
            break;
        };
        if at > 0 {
            parts.push((rest[..at].to_owned(), false));
        }
        parts.push((address.to_owned(), true));
        rest = &rest[at + address.len()..];
    }
    if !rest.is_empty() {
        parts.push((rest.to_owned(), false));
    }
    parts
}

/// `text` as letters, digits and `_`, for use in an element ID. Every
/// other character, `_` too, becomes `_` and its code in hexadecimal, such
/// as `_3a_` for `:`. So two different addresses never give the same ID.
fn id_part(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_string()
            } else {
                format!("_{:x}_", u32::from(ch))
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Action {
    SignOut,
    Remove,
}

/// Whether the team has a session, so its parts can be shown: signed in,
/// or offline after it was.
fn has_session(team: &TeamStatus) -> bool {
    matches!(team.session, TeamSession::SignedIn { .. })
}

/// `https://parts.example.com/acme` as `parts.example.com/acme`.
fn address_of(server_url: &str) -> &str {
    server_url
        .split_once("://")
        .map_or(server_url, |(_, rest)| rest)
}

/// The header badge: its words and its look.
pub(crate) fn summary(status: &TeamsStatus) -> (String, &'static str) {
    let count = |standing: Standing| {
        status
            .teams
            .iter()
            .filter(|team| standing_of(team) == standing)
            .count()
    };
    let (live, connecting, offline, again) = (
        count(Standing::Live),
        count(Standing::Connecting),
        count(Standing::Offline),
        count(Standing::SignInAgain),
    );
    // The team showing parts, while only one does: the badge names it.
    let showing: Vec<(&TeamStatus, &String)> = status
        .teams
        .iter()
        .filter_map(|team| match &team.session {
            TeamSession::SignedIn { name, .. } => Some((team, name)),
            _ => None,
        })
        .collect();
    // Teams that show parts kept from an earlier run while their server
    // cannot be reached (kicad-parts-ed46) show parts too, with no session.
    let kept_only = status
        .teams
        .iter()
        .filter(|team| !has_session(team) && team.parts_as_of.is_some())
        .count();
    let shown = match showing.as_slice() {
        [only] if kept_only == 0 => Some(*only),
        _ => None,
    };
    // Teams whose KiCad libraries wait for the person (kicad-parts-092b):
    // for an answer, or for them to remove one of two servers.
    let waiting = status
        .teams
        .iter()
        .filter(|team| team.libraries_made_from.is_some())
        .count();
    let needs_a_look = |words: String| match waiting {
        0 => words,
        _ if status.teams.len() < 2 => format!("{words} · needs a look"),
        1 => format!("{words} · 1 team needs a look"),
        many => format!("{words} · {many} teams need a look"),
    };
    let look = if offline + again + waiting > 0 {
        PROBLEM
    } else if connecting > 0 || showing.is_empty() {
        OFF
    } else {
        READY
    };
    if status.teams.len() < 2 {
        // One team reads as it always did.
        let words = match shown {
            Some((team, _)) if standing_of(team) == Standing::SignInAgain => {
                "Team catalog: sign in again".to_owned()
            }
            Some((team, name)) if standing_of(team) == Standing::Offline => {
                format!("Team catalog · {name} · offline")
            }
            Some((_, name)) => format!("Team catalog · {name}"),
            None if connecting > 0 => "Team catalog: connecting…".to_owned(),
            None if offline > 0 => "Team catalog: offline".to_owned(),
            None if again > 0 => "Team catalog: sign in again".to_owned(),
            None => "Team catalog: not signed in".to_owned(),
        };
        return (needs_a_look(words), look);
    }
    // Several teams: name the team showing parts, if only one does, then
    // count each team once by where it stands. Teams signed out on purpose
    // are not counted.
    let counts: Vec<String> = [
        (live, "live"),
        (connecting, "connecting"),
        (offline, "offline"),
        (again, "to sign in again"),
    ]
    .into_iter()
    .filter(|(number, _)| *number > 0)
    .map(|(number, words)| format!("{number} {words}"))
    .collect();
    let words = match (shown, counts.is_empty()) {
        (Some((team, name)), _) => format!("{} · {name} · {}", team.label(), counts.join(", ")),
        (None, true) => "Team catalog: not signed in".to_owned(),
        (None, false) => format!("Teams · {}", counts.join(", ")),
    };
    (needs_a_look(words), look)
}

/// Where one team stands, for the header badge and the part list. Each
/// team has exactly one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Standing {
    Live,
    Connecting,
    Offline,
    SignInAgain,
    /// Signed out on purpose: needs nothing.
    SignedOut,
}

pub(crate) fn standing_of(team: &TeamStatus) -> Standing {
    match &team.session {
        TeamSession::SignedIn { live, .. } => match live {
            LiveUpdates::Live => Standing::Live,
            LiveUpdates::Connecting => Standing::Connecting,
            LiveUpdates::Reconnecting { .. } => Standing::Offline,
            LiveUpdates::SignInAgain { .. } => Standing::SignInAgain,
        },
        TeamSession::Reaching { problem: None } => Standing::Connecting,
        TeamSession::Reaching { problem: Some(_) } => Standing::Offline,
        TeamSession::SignedOut { problem: Some(_) } => Standing::SignInAgain,
        TeamSession::SignedOut { problem: None } => Standing::SignedOut,
    }
}

/// What a team's row says about it. A team showing parts that may be out
/// of date says when they last matched its server: offline, in one
/// wording whatever the reason, or while it connects.
pub(crate) fn state_words(team: &TeamStatus) -> Said {
    let Some(as_of) = team.parts_as_of else {
        return Said::plain(words_without_time(team));
    };
    let offline = |problem: &str| {
        Said::parts_as_of(
            "Offline. ",
            as_of,
            &format!(" The app reconnects by itself. Last problem: {problem}"),
        )
    };
    match &team.session {
        TeamSession::SignedIn {
            live: LiveUpdates::Reconnecting { message },
            ..
        } => offline(message),
        TeamSession::Reaching {
            problem: Some(problem),
        } => offline(problem),
        // Connecting, with the parts file showing until the catch-up.
        _ => Said::parts_as_of(&format!("{} ", words_without_time(team)), as_of, ""),
    }
}

/// What a team's row says about it, with no time in it.
fn words_without_time(team: &TeamStatus) -> String {
    match &team.session {
        TeamSession::SignedIn {
            name,
            email,
            can_edit,
            live,
            ..
        } => {
            let access = if *can_edit {
                "You can edit parts."
            } else {
                "You can view parts but not edit them."
            };
            let signed_in = format!("Signed in as {name} ({email}). {access}");
            match live {
                LiveUpdates::Live => {
                    format!("{signed_in} Changes others make appear here by themselves.")
                }
                LiveUpdates::Connecting => {
                    format!("{signed_in} Connecting so that changes others make appear here…")
                }
                LiveUpdates::Reconnecting { message } => format!(
                    "Offline: showing the parts last seen. The app reconnects by itself. Last problem: {message}"
                ),
                LiveUpdates::SignInAgain { .. } => {
                    "The team server no longer accepts this sign-in. Sign in again.".to_owned()
                }
            }
        }
        TeamSession::Reaching { problem: None } => {
            format!("Signing in as {} with your kept sign-in…", team.email)
        }
        TeamSession::Reaching {
            problem: Some(problem),
        } => format!("Offline. The app reconnects by itself. Last problem: {problem}"),
        TeamSession::SignedOut {
            problem: Some(problem),
        } => problem.clone(),
        TeamSession::SignedOut { problem: None } => format!("Signed out ({}).", team.email),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-24 09:12 UTC: tests read times in UTC.
    const AS_OF: u64 = 1_792_833_120_000;
    const PROBLEM_TEXT: &str = "The team server could not be reached.";

    fn team(code: &str, session: TeamSession) -> TeamStatus {
        TeamStatus {
            server_url: format!("https://{}.example.com", code.to_lowercase()),
            email: "ada@example.test".into(),
            code: code.into(),
            colour: "blue".into(),
            session,
            note: None,
            libraries_made_from: None,
            library_repo: None,
            parts_as_of: None,
        }
    }

    fn signed_in(live: LiveUpdates) -> TeamSession {
        TeamSession::SignedIn {
            name: "Ada Editor".into(),
            email: "ada@example.test".into(),
            can_edit: true,
            live,
            checkpoint: 1,
            imports: 1,
        }
    }

    fn status(teams: Vec<TeamStatus>) -> TeamsStatus {
        TeamsStatus {
            teams,
            choices: desktop_contract::TeamChoices::default(),
            kept: SignInsKept::AfterRestart,
            form_server: String::new(),
            form_email: String::new(),
            problem: None,
            note: None,
        }
    }

    #[test]
    fn one_team_reads_as_before() {
        assert_eq!(
            summary(&status(Vec::new())),
            ("Team catalog: not signed in".into(), OFF)
        );
        let ma = team("MA", signed_in(LiveUpdates::Live));
        assert_eq!(
            summary(&status(vec![ma])),
            ("Team catalog · Ada Editor".into(), READY)
        );
        let rejected = team(
            "MA",
            signed_in(LiveUpdates::SignInAgain {
                message: "401".into(),
            }),
        );
        assert_eq!(
            summary(&status(vec![rejected])),
            ("Team catalog: sign in again".into(), PROBLEM)
        );
    }

    #[test]
    fn one_team_offline_is_said_without_a_count() {
        let offline = team(
            "MA",
            signed_in(LiveUpdates::Reconnecting {
                message: "refused".into(),
            }),
        );
        assert_eq!(
            summary(&status(vec![offline])),
            ("Team catalog · Ada Editor · offline".into(), PROBLEM)
        );
    }

    #[test]
    fn several_teams_count_each_team_once() {
        let ma = team("MA", signed_in(LiveUpdates::Live));
        let ac = team("AC", signed_in(LiveUpdates::Live));
        // Both teams' parts are in the list: no team is "shown".
        assert_eq!(
            summary(&status(vec![ma.clone(), ac.clone()])),
            ("Teams · 2 live".into(), READY)
        );
        let offline_be = team(
            "BE",
            signed_in(LiveUpdates::Reconnecting {
                message: "refused".into(),
            }),
        );
        // An offline team with its session is counted once: offline.
        assert_eq!(
            summary(&status(vec![ma.clone(), offline_be.clone()])),
            ("Teams · 1 live, 1 offline".into(), PROBLEM)
        );
        let reaching = team(
            "BE",
            TeamSession::Reaching {
                problem: Some("refused".into()),
            },
        );
        let again = team(
            "CE",
            TeamSession::SignedOut {
                problem: Some("Enter your password.".into()),
            },
        );
        assert_eq!(
            summary(&status(vec![ma.clone(), ac, reaching.clone(), again])),
            (
                "Teams · 2 live, 1 offline, 1 to sign in again".into(),
                PROBLEM
            )
        );
        // With one team signed in, the code still says whose parts these
        // are; a team signed out on purpose is not counted.
        let out = team("DE", TeamSession::SignedOut { problem: None });
        assert_eq!(
            summary(&status(vec![ma.clone(), out.clone()])),
            ("MA · Ada Editor · 1 live".into(), READY)
        );
        // No team shown.
        assert_eq!(
            summary(&status(vec![out.clone(), reaching])),
            ("Teams · 1 offline".into(), PROBLEM)
        );
        assert_eq!(
            summary(&status(vec![out.clone(), out])),
            ("Team catalog: not signed in".into(), OFF)
        );
    }

    #[test]
    fn one_team_without_a_session_says_where_it_stands() {
        let offline = team(
            "BE",
            TeamSession::Reaching {
                problem: Some("refused".into()),
            },
        );
        assert_eq!(
            summary(&status(vec![offline])),
            ("Team catalog: offline".into(), PROBLEM)
        );
        let connecting = team("MA", TeamSession::Reaching { problem: None });
        assert_eq!(
            summary(&status(vec![connecting])),
            ("Team catalog: connecting…".into(), OFF)
        );
    }

    #[test]
    fn addresses_are_split_out_of_a_sentence() {
        let parts = |text: &str| -> Vec<(String, bool)> {
            split_out(text, &["http://h:80", "http://h:8090"])
        };
        assert_eq!(
            parts("From http://h:8090, not http://h:80."),
            vec![
                ("From ".to_owned(), false),
                ("http://h:8090".to_owned(), true),
                (", not ".to_owned(), false),
                ("http://h:80".to_owned(), true),
                (".".to_owned(), false),
            ]
        );
        assert_eq!(
            parts("No address."),
            vec![("No address.".to_owned(), false)]
        );
    }

    #[test]
    fn row_ids_never_clash_for_different_addresses() {
        assert_eq!(id_part("https://a.b:1"), "https_3a__2f__2f_a_2e_b_3a_1");
        // Addresses that differ only in their punctuation get different IDs.
        assert_ne!(id_part("https://a-b.c"), id_part("https://a.b-c"));
        assert_ne!(id_part("https://a_b"), id_part("https://a-b"));
    }

    #[test]
    fn a_team_whose_libraries_wait_shows_in_the_badge() {
        let mut ma = team("MA", signed_in(LiveUpdates::Live));
        ma.libraries_made_from = Some(LibrariesMadeFrom {
            code: "MA".into(),
            server: "https://old.example.com".into(),
            in_your_teams: false,
        });
        assert_eq!(
            summary(&status(vec![ma.clone()])),
            ("Team catalog · Ada Editor · needs a look".into(), PROBLEM)
        );
        let ac = team("AC", signed_in(LiveUpdates::Live));
        assert_eq!(
            summary(&status(vec![ma.clone(), ac.clone()])),
            ("Teams · 2 live · 1 team needs a look".into(), PROBLEM)
        );
        // Two servers claiming one team have no answer to give, but need a
        // look all the same.
        let mut claimed = ac;
        claimed.libraries_made_from = Some(LibrariesMadeFrom {
            code: "AC".into(),
            server: "https://ma.example.com".into(),
            in_your_teams: true,
        });
        assert_eq!(
            summary(&status(vec![ma, claimed])),
            ("Teams · 2 live · 2 teams need a look".into(), PROBLEM)
        );
    }

    #[test]
    fn each_row_says_plainly_where_the_team_stands() {
        let ma = team("MA", signed_in(LiveUpdates::Live));
        let words = |team: &TeamStatus| state_words(team).text(AS_OF + 60_000);
        assert_eq!(
            words(&ma),
            "Signed in as Ada Editor (ada@example.test). You can edit parts. Changes others make appear here by themselves."
        );
        let mut offline = team(
            "MA",
            signed_in(LiveUpdates::Reconnecting {
                message: "The team server could not be reached.".into(),
            }),
        );
        assert!(words(&offline).starts_with("Offline: showing the parts last seen."));
        // With the time the parts last matched the server, it says when.
        offline.parts_as_of = Some(AS_OF);
        assert_eq!(
            words(&offline),
            "Offline. Parts as of 09:12. The app reconnects by itself. Last problem: The team server could not be reached."
        );
        let out = team("MA", TeamSession::SignedOut { problem: None });
        assert_eq!(words(&out), "Signed out (ada@example.test).");
    }

    #[test]
    fn a_team_showing_its_parts_file_counts_as_a_team_showing_parts() {
        let ma = team("MA", signed_in(LiveUpdates::Live));
        let mut be = team(
            "BE",
            TeamSession::Reaching {
                problem: Some(PROBLEM_TEXT.into()),
            },
        );
        be.parts_as_of = Some(AS_OF);
        // Two teams show parts, so the badge names neither.
        assert_eq!(
            summary(&status(vec![ma, be.clone()])),
            ("Teams · 1 live, 1 offline".into(), PROBLEM)
        );
        // Alone, it reads as one team offline.
        assert_eq!(
            summary(&status(vec![be])),
            ("Team catalog: offline".into(), PROBLEM)
        );
    }

    #[test]
    fn a_team_unreachable_at_start_says_when_its_parts_file_is_from() {
        let mut reaching = team(
            "MA",
            TeamSession::Reaching {
                problem: Some("The team server could not be reached.".into()),
            },
        );
        let words = |team: &TeamStatus| state_words(team).text(AS_OF + 60_000);
        // With no parts file, the same words, with no time.
        assert_eq!(
            words(&reaching),
            "Offline. The app reconnects by itself. Last problem: The team server could not be reached."
        );
        reaching.parts_as_of = Some(AS_OF);
        // The same words as a team that drops during a run.
        assert_eq!(
            words(&reaching),
            "Offline. Parts as of 09:12. The app reconnects by itself. Last problem: The team server could not be reached."
        );
    }

    #[test]
    fn a_team_connecting_with_its_parts_file_says_when_the_parts_are_from() {
        let words = |team: &TeamStatus| state_words(team).text(AS_OF + 60_000);
        let mut first_try = team("MA", TeamSession::Reaching { problem: None });
        first_try.parts_as_of = Some(AS_OF);
        assert_eq!(
            words(&first_try),
            "Signing in as ada@example.test with your kept sign-in… Parts as of 09:12."
        );
        let mut catching_up = team("MA", signed_in(LiveUpdates::Connecting));
        catching_up.parts_as_of = Some(AS_OF);
        assert!(
            words(&catching_up).ends_with(
                "Connecting so that changes others make appear here… Parts as of 09:12."
            ),
            "{}",
            words(&catching_up)
        );
    }
}

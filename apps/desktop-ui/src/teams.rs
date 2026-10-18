//! The Teams area in the header: every team the person works with, each
//! signed in, offline or needing a sign-in, with Sign out, Sign in and
//! Remove, and a form that adds a team or signs in again. With two or more
//! teams signed in, a switcher beside it chooses the team the catalog view
//! shows; switching needs no sign-in.

use crate::backend;
use crate::focus::focus_later;
use desktop_contract::{
    LiveUpdates, SignInRequest, SignInsKept, TeamSession, TeamStatus, TeamsStatus,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

const READY: &str = "kicad-badge kicad-badge-ready";
const OFF: &str = "kicad-badge team-badge-off";
const PROBLEM: &str = "kicad-badge kicad-badge-problem";

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
    move || {
        let Some(status) = teams.get() else {
            return ().into_any();
        };
        let (summary, badge) = summary(&status);
        let anyone_signed_in = status.teams.iter().any(has_session);
        let show_form = adding.get() || !anyone_signed_in;
        let rows = status
            .teams
            .iter()
            .cloned()
            .map(|team| {
                let server = team.server_url.clone();
                let can_sign_out = has_session(&team);
                let needs_password = matches!(team.session, TeamSession::SignedOut { .. });
                let again = team.clone();
                let (sign_out_server, remove_server) = (server.clone(), server.clone());
                let label = team.label();
                let address = (label != address_of(&team.server_url)).then(|| team.server_url.clone());
                view! {
                    <li class="team-row">
                        <p class="team-name">
                            <strong>{label}</strong>
                            {address.map(|address| view! { <span class="team-server">{address}</span> })}
                        </p>
                        <p class="team-state">{state_words(&team)}</p>
                        <div class="team-actions">
                            {needs_password.then(|| view! {
                                <button type="button" on:click=move |_| sign_in_again(again.clone())>
                                    "Sign in"
                                </button>
                            })}
                            {can_sign_out.then(|| view! {
                                <button type="button" on:click=move |_| act(Action::SignOut, sign_out_server.clone())>
                                    "Sign out"
                                </button>
                            })}
                            <button type="button" on:click=move |_| act(Action::Remove, remove_server.clone())>
                                "Remove"
                            </button>
                        </div>
                    </li>
                }
            })
            .collect_view();
        let has_teams = !status.teams.is_empty();
        let until_closed = match &status.kept {
            SignInsKept::AfterRestart => None,
            SignInsKept::UntilClosed { reason } => Some(format!(
                "You stay signed in only until the app closes, because {reason}. After a restart, the app asks for each password again."
            )),
        };
        let hint = if until_closed.is_some() {
            "The app never saves your password."
        } else {
            "You stay signed in after a restart. The app never saves your password."
        };
        let problem = status.problem.clone();
        view! {
            <details class="team-settings" open=open.get_untracked() on:toggle=toggled>
                <summary class=badge>{summary}</summary>
                <div class="kicad-settings-body team-settings-body">
                    {has_teams.then(|| view! {
                        <h2 class="teams-heading" id="teams-heading">"Your teams"</h2>
                        <ul class="team-list" aria-labelledby="teams-heading">{rows}</ul>
                    })}
                    {until_closed.map(|text| view! { <p class="hint" id="sign-ins-until-closed">{text}</p> })}
                    {if show_form {
                        view! {
                            <form
                                class="team-sign-in"
                                aria-label="Sign in to the team catalog"
                                on:submit=move |event| {
                                    event.prevent_default();
                                    sign_in();
                                }
                            >
                                <p>
                                    {if has_teams {
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
                                <p class="hint">{hint}</p>
                                {problem.map(|message| view! { <p class="problem" role="alert">{message}</p> })}
                                <div class="team-actions">
                                    <button type="submit" class="primary" disabled=move || busy.get()>
                                        {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                                    </button>
                                    {anyone_signed_in.then(|| view! {
                                        <button type="button" on:click=move |_| adding.set(false)>"Cancel"</button>
                                    })}
                                </div>
                            </form>
                        }
                        .into_any()
                    } else {
                        view! {
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
                        .into_any()
                    }}
                </div>
            </details>
        }
        .into_any()
    }
}

/// Chooses the team the catalog view shows, when two or more teams are
/// signed in. A native list box, so it works with the keyboard and screen
/// readers as people expect.
#[component]
pub fn TeamSwitcher(
    teams: Memo<Option<TeamsStatus>>,
    set_teams: RwSignal<Option<TeamsStatus>>,
) -> impl IntoView {
    let show = move |server: String| {
        spawn_local(async move {
            if let Ok(status) = backend::team_show(&server).await {
                set_teams.set(Some(status));
            }
        });
    };
    // The choices and the shown team are kept apart, so switching only
    // moves the selection: the list box is never drawn again under the
    // person's focus.
    let choices = Memo::new(move |_| {
        teams.with(|status| {
            status.as_ref().map_or_else(Vec::new, |status| {
                status
                    .teams
                    .iter()
                    .filter(|team| has_session(team))
                    .map(|team| (team.server_url.clone(), team.label()))
                    .collect::<Vec<_>>()
            })
        })
    });
    let shown = Memo::new(move |_| {
        teams.with(|status| {
            status
                .as_ref()
                .map(|status| status.shown.clone())
                .unwrap_or_default()
        })
    });
    move || {
        let choices = choices.get();
        if choices.len() < 2 {
            return None;
        }
        Some(view! {
            <div class="team-switcher">
                <label for="team-shown">"Team"</label>
                <select
                    id="team-shown"
                    on:change:target=move |event| show(event.target().value())
                >
                    {choices
                        .into_iter()
                        .map(|(server, label)| {
                            let this = server.clone();
                            view! {
                                <option value=server prop:selected=move || shown.get() == this>
                                    {label}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </div>
        })
    }
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

/// Whether the team needs the person: offline, or asking for a password
/// after the server stopped accepting its sign-in.
fn is_offline(team: &TeamStatus) -> bool {
    matches!(
        team.session,
        TeamSession::Reaching { problem: Some(_) }
            | TeamSession::SignedIn {
                live: LiveUpdates::Reconnecting { .. },
                ..
            }
    )
}

fn must_sign_in_again(team: &TeamStatus) -> bool {
    matches!(
        team.session,
        TeamSession::SignedOut { problem: Some(_) }
            | TeamSession::SignedIn {
                live: LiveUpdates::SignInAgain { .. },
                ..
            }
    )
}

/// The header badge: its words and its look.
pub(crate) fn summary(status: &TeamsStatus) -> (String, &'static str) {
    let signed_in: Vec<&TeamStatus> = status.teams.iter().filter(|t| has_session(t)).collect();
    let offline = status.teams.iter().filter(|t| is_offline(t)).count();
    let again = status
        .teams
        .iter()
        .filter(|t| must_sign_in_again(t))
        .count();
    let connecting = status.teams.iter().any(|team| {
        matches!(
            team.session,
            TeamSession::Reaching { problem: None }
                | TeamSession::SignedIn {
                    live: LiveUpdates::Connecting,
                    ..
                }
        )
    });
    let mut notes = Vec::new();
    if again > 0 {
        notes.push(format!("{again} to sign in again"));
    }
    if offline > 0 {
        notes.push(format!("{offline} offline"));
    }
    let look = if !notes.is_empty() {
        PROBLEM
    } else if connecting || signed_in.is_empty() {
        OFF
    } else {
        READY
    };
    let words = match signed_in[..] {
        [] if connecting && notes.is_empty() => "Team catalog: connecting…".to_owned(),
        [] if again > 0 && offline == 0 => "Team catalog: sign in again".to_owned(),
        [] if offline > 0 => "Team catalog: offline".to_owned(),
        [] => "Team catalog: not signed in".to_owned(),
        [only] => {
            let TeamSession::SignedIn { name, .. } = &only.session else {
                unreachable!("has_session")
            };
            if must_sign_in_again(only) && status.teams.len() == 1 {
                "Team catalog: sign in again".to_owned()
            } else {
                with_notes(format!("Team catalog · {name}"), &notes)
            }
        }
        _ => with_notes(format!("Teams · {} signed in", signed_in.len()), &notes),
    };
    (words, look)
}

fn with_notes(words: String, notes: &[String]) -> String {
    if notes.is_empty() {
        words
    } else {
        format!("{words} · {}", notes.join(", "))
    }
}

/// What a team's row says about it.
pub(crate) fn state_words(team: &TeamStatus) -> String {
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
        } => format!(
            "Offline: the team server could not be reached. The app keeps trying, and signs you in when it is back. Last problem: {problem}"
        ),
        TeamSession::SignedOut {
            problem: Some(problem),
        } => problem.clone(),
        TeamSession::SignedOut { problem: None } => format!("Signed out ({}).", team.email),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(code: &str, session: TeamSession) -> TeamStatus {
        TeamStatus {
            server_url: format!("https://{}.example.com", code.to_lowercase()),
            email: "ada@example.test".into(),
            code: code.into(),
            session,
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
            shown: teams
                .first()
                .map(|team| team.server_url.clone())
                .unwrap_or_default(),
            teams,
            kept: SignInsKept::AfterRestart,
            form_server: String::new(),
            form_email: String::new(),
            problem: None,
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
    fn several_teams_say_how_many_are_signed_in_and_which_need_the_person() {
        let ma = team("MA", signed_in(LiveUpdates::Live));
        let ac = team("AC", signed_in(LiveUpdates::Live));
        assert_eq!(
            summary(&status(vec![ma.clone(), ac.clone()])),
            ("Teams · 2 signed in".into(), READY)
        );
        let offline = team(
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
            summary(&status(vec![ma.clone(), ac, offline.clone(), again])),
            (
                "Teams · 2 signed in · 1 to sign in again, 1 offline".into(),
                PROBLEM
            )
        );
        // A team signed out on purpose needs nothing.
        let out = team("DE", TeamSession::SignedOut { problem: None });
        assert_eq!(
            summary(&status(vec![ma.clone(), out])),
            ("Team catalog · Ada Editor".into(), READY)
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
    fn each_row_says_plainly_where_the_team_stands() {
        let ma = team("MA", signed_in(LiveUpdates::Live));
        assert_eq!(
            state_words(&ma),
            "Signed in as Ada Editor (ada@example.test). You can edit parts. Changes others make appear here by themselves."
        );
        let offline = team(
            "MA",
            signed_in(LiveUpdates::Reconnecting {
                message: "The team server could not be reached.".into(),
            }),
        );
        assert!(state_words(&offline).starts_with("Offline: showing the parts last seen."));
        let out = team("MA", TeamSession::SignedOut { problem: None });
        assert_eq!(state_words(&out), "Signed out (ada@example.test).");
    }
}

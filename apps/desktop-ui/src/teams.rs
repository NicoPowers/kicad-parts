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
                                children=move |server| team_row(server, teams, removing, act, sign_in_again)
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
    let address = move || {
        team.with(|team| {
            let team = team.as_ref()?;
            (team.label() != address_of(&team.server_url)).then(|| team.server_url.clone())
        })
    };
    let words = move || team.with(|team| team.as_ref().map(state_words).unwrap_or_default());
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
    let (sign_out_server, remove_server, ask_server) = (server.clone(), server.clone(), server);
    view! {
        <li class="team-row">
            <p class="team-name">
                <strong>{label}</strong>
                {move || address().map(|address| view! { <span class="team-server">{address}</span> })}
            </p>
            <p class="team-state">{words}</p>
            {move || note().map(|note| view! { <p class="team-state hint">{note}</p> })}
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
        </li>
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
    let shown = status.shown_team().and_then(|team| match &team.session {
        TeamSession::SignedIn { name, .. } => Some((team, name)),
        _ => None,
    });
    let look = if offline + again > 0 {
        PROBLEM
    } else if connecting > 0 || shown.is_none() {
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
        return (words, look);
    }
    // Several teams: say whose parts are shown, then count each team once
    // by where it stands. Teams signed out on purpose are not counted.
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
    (words, look)
}

/// Where one team stands, for the header badge. Each team has exactly one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Standing {
    Live,
    Connecting,
    Offline,
    SignInAgain,
    /// Signed out on purpose: needs nothing.
    SignedOut,
}

fn standing_of(team: &TeamStatus) -> Standing {
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
            note: None,
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
    fn several_teams_say_whose_parts_are_shown_and_count_each_team_once() {
        let ma = team("MA", signed_in(LiveUpdates::Live));
        let ac = team("AC", signed_in(LiveUpdates::Live));
        assert_eq!(
            summary(&status(vec![ma.clone(), ac.clone()])),
            ("MA · Ada Editor · 2 live".into(), READY)
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
            ("MA · Ada Editor · 1 live, 1 offline".into(), PROBLEM)
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
                "MA · Ada Editor · 2 live, 1 offline, 1 to sign in again".into(),
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

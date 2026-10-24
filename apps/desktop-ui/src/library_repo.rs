//! A team's library repo on its row in the Teams menu (kicad-parts-ab30):
//! the git repo that holds the team's own KiCad library.
//!
//! None yet: "Add…" takes a folder with the person's clone of the repo, or
//! the repo's address. Linked: the repo's short name and host, what the
//! app can do with it in one line, and "Check again" and "Remove". A
//! problem keeps git's own words under "Details".

use crate::as_of::{Said, SaidView};
use crate::backend;
use crate::focus::{Scrolling, focus_found_later, focus_later};
use desktop_contract::{
    LibraryRepoLinking, LibraryRepoRequest, LibraryRepoStatus, RepoAccess, RepoProblem, RepoRemote,
    TeamStatus, TeamsStatus,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The library repo part of the row with ID `row_id`, for the team at
/// `server`. What the person types lives here, so status polls never take
/// it away.
pub(crate) fn library_repo_part(
    server: String,
    row_id: String,
    team: Memo<Option<TeamStatus>>,
    set_teams: RwSignal<Option<TeamsStatus>>,
) -> impl IntoView {
    let repo = Memo::new(move |_| {
        team.with(|team| team.as_ref().and_then(|team| team.library_repo.clone()))
    });
    // Each part is drawn again only when its own words change.
    let which = Memo::new(move |_| {
        repo.with(|repo| {
            repo.as_ref()
                .map(|repo| (repo.name.clone(), repo.host.clone()))
        })
    });
    let unusable =
        Memo::new(move |_| repo.with(|repo| repo.as_ref().is_some_and(|repo| repo.unusable)));
    let said = Memo::new(move |_| repo.with(|repo| repo.as_ref().map(access_words)));
    let details = Memo::new(move |_| repo.with(|repo| repo.as_ref().and_then(details_of)));
    // A problem reads in the danger colour too; its words say it first.
    let state_class = move || {
        let problem = repo.with(|repo| {
            repo.as_ref().is_some_and(|repo| {
                matches!(
                    repo.access,
                    RepoAccess::CanReadOnly { .. } | RepoAccess::CannotRead { .. }
                )
            })
        });
        if problem {
            "team-state team-repo-state team-repo-problem"
        } else {
            "team-state team-repo-state"
        }
    };
    let adding = RwSignal::new(false);
    let place = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let refused = RwSignal::new(None::<RepoProblem>);
    let remotes = RwSignal::new(Vec::<RepoRemote>::new());
    let label_id = format!("{row_id}-repo");
    let input_id = format!("{row_id}-repo-place");
    // After an action, the keyboard goes on to the part's first button.
    let focus_first_button = {
        let row_id = row_id.clone();
        move || {
            let selector = format!("#{row_id} .team-repo .team-actions button");
            focus_found_later(
                move || document().query_selector(&selector).ok().flatten(),
                Scrolling::IntoView,
            );
        }
    };
    let link = {
        let (server, focus_first_button) = (server.clone(), focus_first_button.clone());
        move |chosen: String| {
            busy.set(true);
            refused.set(None);
            let request = LibraryRepoRequest {
                server_url: server.clone(),
                place: chosen,
            };
            let focus_first_button = focus_first_button.clone();
            spawn_local(async move {
                let linking = backend::library_repo_link(request).await;
                busy.set(false);
                match linking {
                    Ok(LibraryRepoLinking::Linked { teams }) => {
                        adding.set(false);
                        place.set(String::new());
                        remotes.set(Vec::new());
                        set_teams.set(Some(teams));
                        focus_first_button();
                    }
                    Ok(LibraryRepoLinking::ChooseRemote { remotes: found }) => remotes.set(found),
                    Ok(LibraryRepoLinking::Refused { problem }) => refused.set(Some(problem)),
                    Err(message) => refused.set(Some(RepoProblem {
                        summary: message,
                        details: String::new(),
                        offline: false,
                    })),
                }
            });
        }
    };
    let act = {
        let (server, focus_first_button) = (server.clone(), focus_first_button.clone());
        move |unlink: bool| {
            let (server, focus_first_button) = (server.clone(), focus_first_button.clone());
            spawn_local(async move {
                let result = if unlink {
                    backend::library_repo_unlink(&server).await
                } else {
                    backend::library_repo_check(&server).await
                };
                if let Ok(status) = result {
                    set_teams.set(Some(status));
                    if unlink {
                        focus_first_button();
                    }
                }
            });
        }
    };
    let actions = {
        let input_id = input_id.clone();
        move || {
            let input_id = input_id.clone();
            let (check, remove) = (act.clone(), act.clone());
            if which.with(Option::is_none) {
                return view! {
                    <div class="team-actions">
                        <button
                            type="button"
                            on:click=move |_| {
                                adding.set(true);
                                focus_later(&input_id);
                            }
                        >
                            "Add…"
                        </button>
                    </div>
                }
                .into_any();
            }
            // An address that can't be used can only be removed.
            view! {
                <div class="team-actions">
                    {(!unusable.get()).then(|| view! {
                        <button type="button" on:click=move |_| check(false)>"Check again"</button>
                    })}
                    <button type="button" on:click=move |_| remove(true)>"Remove repo"</button>
                </div>
            }
            .into_any()
        }
    };
    let form = {
        let (input_id, link) = (input_id.clone(), link.clone());
        move || {
            let (input_id, submit_link, remote_link) =
                (input_id.clone(), link.clone(), link.clone());
            let label_for = input_id.clone();
            view! {
                <form
                    class="team-repo-form"
                    aria-label="Add a library repo"
                    on:submit=move |event| {
                        event.prevent_default();
                        submit_link(place.get_untracked());
                    }
                >
                    <label for=label_for>"A folder with your clone of the repo, or the repo's address"</label>
                    // Text, not url: a folder or git@host:path is no URL.
                    <input
                        id=input_id
                        type="text"
                        spellcheck="false"
                        autocomplete="off"
                        placeholder="git@gitlab.com:client/kicad-library.git"
                        prop:value=move || place.get()
                        on:input:target=move |event| place.set(event.target().value())
                    />
                    <p class="hint">
                        "The app uses your own git and SSH setup, and keeps a copy of its own. It never changes your folder."
                    </p>
                    {move || refused.get().map(|problem| view! {
                        <div class="problem" role="alert">
                            <p>{problem.summary.clone()}</p>
                            {details_view(problem.details)}
                        </div>
                    })}
                    {
                        let remote_link = remote_link.clone();
                        move || {
                            let found = remotes.get();
                            let remote_link = remote_link.clone();
                            (!found.is_empty()).then(|| view! {
                                <fieldset class="team-repo-remotes">
                                    <legend>"This clone has several remotes. Which one holds the library?"</legend>
                                    {found
                                        .into_iter()
                                        .map(|remote| {
                                            let remote_link = remote_link.clone();
                                            let address = remote.address.clone();
                                            // A refused remote is said, not offered: its
                                            // address may hold a token, so it is never shown.
                                            match remote.refused {
                                                Some(why) => view! {
                                                    <p class="team-repo-remote-refused">
                                                        <strong>{remote.name}</strong>
                                                        {format!(": {why}")}
                                                    </p>
                                                }
                                                .into_any(),
                                                None => view! {
                                                    <button type="button" on:click=move |_| remote_link(address.clone())>
                                                        <strong>{remote.name}</strong>
                                                        " "
                                                        <span class="team-repo-remote-address">{remote.address}</span>
                                                    </button>
                                                }
                                                .into_any(),
                                            }
                                        })
                                        .collect_view()}
                                </fieldset>
                            })
                        }
                    }
                    <div class="team-actions">
                        // While the person chooses a remote, the remote is the
                        // way on: Add would only ask again.
                        <button
                            type="submit"
                            class="primary"
                            hidden=move || remotes.with(|found| !found.is_empty())
                            disabled=move || busy.get()
                        >
                            {move || if busy.get() { "Checking…" } else { "Add" }}
                        </button>
                        <button
                            type="button"
                            on:click=move |_| {
                                adding.set(false);
                                refused.set(None);
                                remotes.set(Vec::new());
                            }
                        >
                            "Cancel"
                        </button>
                    </div>
                </form>
            }
        }
    };
    let labelled_by = label_id.clone();
    view! {
        <div class="team-repo" role="group" aria-labelledby=labelled_by>
            <p class="team-repo-name">
                // Spaces between the parts, so a screen reader and the
                // text read "Library repo: acme/lib on gitlab.com".
                <span class="team-repo-label" id=label_id>"Library repo:"</span>
                " "
                {move || match which.get() {
                    Some((name, host)) => view! {
                        <span class="team-repo-which">{name}</span>
                        {(!host.is_empty()).then(|| view! {
                            " "
                            <span class="team-repo-host">{format!("on {host}")}</span>
                        })}
                    }
                    .into_any(),
                    None => view! { <span class="hint">"None yet"</span> }.into_any(),
                }}
            </p>
            {move || said.get().map(|said| view! {
                <p class=state_class role="status"><SaidView said=said /></p>
            })}
            {move || details.get().map(details_view)}
            <Show when=move || adding.get() fallback=actions.clone()>
                {form.clone()}
            </Show>
        </div>
    }
}

/// git's own words, under a closed "Details".
fn details_view(details: String) -> Option<impl IntoView> {
    (!details.trim().is_empty()).then(|| {
        view! {
            <details class="team-repo-details">
                <summary>"Details"</summary>
                <pre>{details}</pre>
            </details>
        }
    })
}

/// What the app can do with the repo, in one line. While its host cannot
/// be reached, the line says the app reads from its copy, and from when.
pub(crate) fn access_words(repo: &LibraryRepoStatus) -> Said {
    match &repo.access {
        RepoAccess::Checking { copying: true } => Said::plain(
            "Copying the repo to this computer. A library with many 3D models can take a few minutes."
                .to_owned(),
        ),
        RepoAccess::Checking { copying: false } => Said::plain("Checking access…".to_owned()),
        RepoAccess::CanReadAndWrite => Said::plain("Can read and write.".to_owned()),
        RepoAccess::CanRead => Said::plain(
            "Can read. Choose Check again to see whether you can push.".to_owned(),
        ),
        RepoAccess::CanReadEmpty => Said::plain(
            "Can read. The repo has no commits yet, so the app can't try a push.".to_owned(),
        ),
        RepoAccess::CanReadOnly { problem } => Said::plain(problem.summary.clone()),
        // Offline with a copy: reading goes on from the copy, so the line
        // says that instead of "Can't read the repo".
        RepoAccess::CannotRead { problem } => match repo.copied_at {
            Some(at) if problem.offline => Said {
                before: format!("Can't reach {}. The app uses its copy from ", repo.host),
                as_of_ms: Some(at),
                after: ". Choose Check again when you're online.".to_owned(),
            },
            _ => Said::plain(problem.summary.clone()),
        },
    }
}

/// git's own words about the last problem, if there was one.
fn details_of(repo: &LibraryRepoStatus) -> Option<String> {
    match &repo.access {
        RepoAccess::CanReadOnly { problem } | RepoAccess::CannotRead { problem } => {
            Some(problem.details.clone()).filter(|details| !details.trim().is_empty())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-24 09:12 UTC: tests read times in UTC.
    const AS_OF: u64 = 1_792_833_120_000;

    fn repo(access: RepoAccess, copied_at: Option<u64>) -> LibraryRepoStatus {
        LibraryRepoStatus {
            address: "git@gitlab.com:acme/kicad-lib.git".into(),
            name: "acme/kicad-lib".into(),
            host: "gitlab.com".into(),
            access,
            copied_at,
            unusable: false,
            layout: None,
        }
    }

    fn problem(summary: &str, offline: bool) -> RepoProblem {
        RepoProblem {
            summary: summary.into(),
            details: "ssh: connect to host gitlab.com port 22: Connection refused".into(),
            offline,
        }
    }

    fn words(repo: &LibraryRepoStatus) -> String {
        access_words(repo).text(AS_OF + 60_000)
    }

    #[test]
    fn each_access_reads_as_one_line() {
        assert_eq!(
            words(&repo(RepoAccess::CanReadAndWrite, Some(AS_OF))),
            "Can read and write."
        );
        assert_eq!(
            words(&repo(RepoAccess::Checking { copying: false }, None)),
            "Checking access…"
        );
        assert!(
            words(&repo(RepoAccess::Checking { copying: true }, None))
                .starts_with("Copying the repo")
        );
        let read_only = repo(
            RepoAccess::CanReadOnly {
                problem: problem(
                    "Can read, but can't push: permission denied. Check that your SSH key or account has write access on gitlab.com.",
                    false,
                ),
            },
            Some(AS_OF),
        );
        assert_eq!(
            words(&read_only),
            "Can read, but can't push: permission denied. Check that your SSH key or account has write access on gitlab.com."
        );
        assert!(
            details_of(&read_only)
                .unwrap()
                .contains("Connection refused")
        );
        assert!(details_of(&repo(RepoAccess::CanReadAndWrite, None)).is_none());
    }

    #[test]
    fn an_unreachable_host_says_the_app_reads_its_copy_meanwhile() {
        let offline = problem(
            "Can't read the repo: can't reach gitlab.com. Check your internet connection, or try again later.",
            true,
        );
        assert_eq!(
            words(&repo(
                RepoAccess::CannotRead {
                    problem: offline.clone()
                },
                Some(AS_OF)
            )),
            "Can't reach gitlab.com. The app uses its copy from 09:12. Choose Check again when you're online."
        );
        // With no copy yet, there is nothing to read meanwhile.
        assert_eq!(
            words(&repo(RepoAccess::CannotRead { problem: offline }, None)),
            "Can't read the repo: can't reach gitlab.com. Check your internet connection, or try again later."
        );
        // A repo that is gone is not offline: no copy is offered.
        let gone = problem("Can't read the repo: the repo was not found.", false);
        assert_eq!(
            words(&repo(RepoAccess::CannotRead { problem: gone }, Some(AS_OF))),
            "Can't read the repo: the repo was not found."
        );
    }
}

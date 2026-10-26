//! KiCad Parts desktop UI: search the catalog, select a part, and see the
//! exact KiCad symbol (every unit) and footprint exported by kicad-cli.
//! Signed in to the team catalog, a part it has also shows its history and
//! can be edited there.

mod add_part;
mod as_of;
mod backend;
mod cad_choice;
mod edit_cad;
mod focus;
mod kicad_libraries;
mod libraries;
mod library_check;
mod library_layout;
mod library_repo;
mod list_controls;
mod list_focus;
mod part_list;
mod team;
mod team_marks;
mod teams;
mod viewer;

use add_part::AddPartView;
use as_of::{Said, SaidView};
use desktop_contract::{
    AppStatus, KicadStatus, LibraryCheckView, ManufacturerPart, PartDetail, PartKey, PreviewKind,
    PreviewOutcome, PreviewProblem, PreviewProblemReason, PreviewRequest, PreviewSet,
    SearchRequest, TeamPart, TeamSession, TeamStatus, TeamsStatus, unit_label,
};
use focus::focus_later;
use kicad_libraries::KicadLibrariesSection;
use leptos::prelude::*;
use leptos::task::spawn_local;
use libraries::{BrowseFor, BrowserActions, LibrariesDialog};
use library_check::{LibraryCheckLine, LibraryCheckPanel};
use list_controls::{ListControls, remember_team};
use part_list::{ResultList, SEARCH_ROWS, Searched};
use std::time::Duration;
use team::{Notice, TeamColumn, TeamUi};
use team_marks::{ListTeams, shows_parts};
use teams::{Standing, TeamsMenu, standing_of};
use viewer::{CloseUp, CloseUpDialog, Picture, Viewer};

/// The ID of the search box above the part list.
pub(crate) const SEARCH_BOX: &str = "part-search";

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// How often the UI asks whether the team catalog has new Changes. The app
/// itself hears about them through PocketBase realtime.
const TEAM_POLL: Duration = Duration::from_millis(500);
/// How often the UI asks how the KiCad libraries are doing.
const LIBRARIES_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq)]
enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let status = RwSignal::new(Load::<AppStatus>::Loading);
    // Bumped whenever KiCad discovery changes, so previews reload.
    let kicad_epoch = RwSignal::new(0u32);
    let query = RwSignal::new(String::new());
    // Only the parts using this symbol or footprint, chosen in the
    // Libraries browser; shown as a chip that removes it.
    let uses = RwSignal::new(None::<String>);
    let results = RwSignal::new(Load::<Searched>::Loading);
    // The selected part, named by its team and IPN.
    let selected = RwSignal::new(None::<PartKey>);
    let search_seq = StoredValue::new(0u64);
    // The kicad-cli path being typed; kept here so it survives the panel
    // re-rendering when a path is rejected.
    let override_input = RwSignal::new(String::new());

    spawn_local(async move {
        let loaded = into_load(backend::app_status().await);
        if let Load::Ready(app) = &loaded {
            override_input.set(app.kicad_override.clone().unwrap_or_default());
        }
        status.set(loaded);
    });

    // Every team in the person's list; the part list shows every team's
    // parts at once.
    let teams = RwSignal::new(None::<TeamsStatus>);
    // Features that remember their team save it through this.
    provide_context(teams);
    // The header does not need the checkpoints; leaving them out keeps the
    // menu from re-rendering whenever someone saves a Change.
    let teams_badge = Memo::new(move |_| {
        teams.get().map(|mut status| {
            for team in &mut status.teams {
                if let TeamSession::SignedIn {
                    checkpoint,
                    imports,
                    ..
                } = &mut team.session
                {
                    *checkpoint = 0;
                    *imports = 0;
                }
            }
            status
        })
    });
    // Each row's team mark, the filter and grouping, and each one-team
    // feature's team.
    let list_teams = ListTeams::new(teams_badge, move |feature, team| {
        remember_team(teams, feature, &team)
    });
    provide_context(list_teams);
    let ListTeams {
        filter,
        group_by,
        hidden_note,
        ..
    } = list_teams;
    let kicad_team_url =
        Memo::new(move |_| list_teams.kicad_libraries.team.get().unwrap_or_default());
    // The selected part's team, while it has one and it is in the list.
    let part_team = Memo::new(move |_| {
        let team = selected.with(|part| part.as_ref().map(|part| part.team.clone()))?;
        teams.with(|status| {
            status
                .as_ref()?
                .teams
                .iter()
                .find(|listed| !team.is_empty() && listed.server_url == team)
                .cloned()
        })
    });
    // True while the Add part form is open in the part panel.
    let adding = RwSignal::new(false);
    // Add part is offered while the person can add parts to some team.
    let can_add = Memo::new(move |_| list_teams.add_part.team.with(Option::is_some));
    // True while the Library check view is open in the part panel.
    let checking = RwSignal::new(false);
    // The Libraries browser, while it is open, and what it is open for.
    let libraries_open = RwSignal::new(None::<BrowseFor>);
    // Review library layout, while it is open (kicad-parts-e9a3).
    let layout_open = RwSignal::new(None::<library_layout::LayoutOpen>);
    provide_context(library_layout::LayoutOpener(layout_open));
    // Edit part and Add part open it to choose a symbol or footprint, and
    // offer our libraries for a file (kicad-parts-bc6f).
    provide_context(cad_choice::LibrariesOpener(libraries_open));
    let our_libraries = RwSignal::new(None);
    provide_context(cad_choice::OurLibrariesList(our_libraries));
    spawn_local(async move {
        if let Ok(found) = backend::our_libraries().await {
            our_libraries.set(Some(found));
        }
    });
    // The browser's Add part from it, for the part panel to start.
    let library_add = RwSignal::new(None::<desktop_contract::TeamLibraryChoice>);
    Effect::new(move |_| {
        if !can_add.get() {
            adding.set(false);
            checking.set(false);
        }
    });
    let team_changes =
        Memo::new(move |_| teams.with(|status| status.as_ref().and_then(list_version)));
    let refresh_team = move || {
        spawn_local(async move {
            if let Ok(latest) = backend::team_status().await {
                if teams.with_untracked(|known| known.as_ref() != Some(&latest)) {
                    teams.set(Some(latest));
                }
            }
        });
    };
    refresh_team();
    set_interval(refresh_team, TEAM_POLL);
    let kicad_libraries = RwSignal::new(None::<desktop_contract::KicadLibrariesStatus>);
    let refresh_libraries = move || {
        let team = kicad_team_url.get_untracked();
        spawn_local(async move {
            if let Ok(latest) = backend::kicad_libraries_status(&team).await {
                // A late answer about the team shown before is dropped.
                let current = kicad_team_url.get_untracked() == latest.team;
                if current
                    && kicad_libraries.with_untracked(|known| known.as_ref() != Some(&latest))
                {
                    kicad_libraries.set(Some(latest));
                }
            }
        });
    };
    refresh_libraries();
    set_interval(refresh_libraries, LIBRARIES_POLL);
    // Another team's libraries show at once, not at the next poll.
    Effect::new(move |previous: Option<String>| {
        let team = kicad_team_url.get();
        if previous.is_some_and(|previous| previous != team) {
            refresh_libraries();
        }
        team
    });

    // The library check, for people who can edit: worked out by the app
    // once per change of the team catalog (who is signed in, a new Change
    // or a finished import), never per keystroke.
    let check_team = list_teams.library_check.team;
    let check_position = Memo::new(move |_| {
        let team = check_team.get()?;
        teams.with(|status| {
            let status = status
                .as_ref()?
                .teams
                .iter()
                .find(|t| t.server_url == team)?;
            match &status.session {
                TeamSession::SignedIn {
                    can_edit: true,
                    email,
                    checkpoint,
                    imports,
                    ..
                } => Some((format!("{team}\n{email}"), *checkpoint, *imports)),
                _ => None,
            }
        })
    });
    let library_check = RwSignal::new(None::<LibraryCheckView>);
    // The position the shown check was worked out for, so the journeys
    // can wait until it is current.
    let checked_at = RwSignal::new(None::<(String, u64, u64)>);
    // Why the check could not run, said in place of its line.
    let check_problem = RwSignal::new(None::<String>);
    // Asks for the check for what the app follows now; the backend works
    // it out again only when the team catalog or the folder changed.
    let refresh_check = move || {
        let Some(position) = check_position.get_untracked() else {
            library_check.set(None);
            check_problem.set(None);
            checked_at.set(None);
            return;
        };
        let team = check_team.get_untracked().unwrap_or_default();
        spawn_local(async move {
            let result = backend::team_library_check(&team).await;
            // Only the answer for the latest position is kept; the last
            // one stays shown while a newer one is worked out.
            if check_position.get_untracked().as_ref() != Some(&position) {
                return;
            }
            match result {
                Ok(found) => {
                    check_problem.set(None);
                    library_check.set(found);
                }
                Err(message) => check_problem.set(Some(message)),
            }
            checked_at.set(Some(position));
        });
    };
    Effect::new(move |_| {
        let _ = check_position.get();
        refresh_check();
    });
    let check_state = move || match check_position.get() {
        None => "off",
        Some(position) if checked_at.get().as_ref() == Some(&position) => "ready",
        Some(_) => "loading",
    };
    Effect::new(move |_| {
        let text = query.get();
        let only_using = uses.get();
        // Search again when anyone saves a Change, or on sign-in and
        // sign-out, so team parts show their current details.
        // (Reading the value, not just tracking it, makes the memo follow
        // the team status.)
        let version = team_changes.get();
        // The answer says which section holds the selected part, and
        // always includes it there.
        let chosen = selected.get();
        let (team, group_by) = (filter.get(), group_by.get());
        let seq = search_seq.get_value() + 1;
        search_seq.set_value(seq);
        set_timeout(
            move || {
                if search_seq.get_value() != seq {
                    return;
                }
                spawn_local(async move {
                    // Without a search every section starts closed, so only
                    // the counts are needed; open sections load their parts.
                    let limit = if text.trim().is_empty() && only_using.is_none() {
                        0
                    } else {
                        SEARCH_ROWS
                    };
                    let request = SearchRequest {
                        query: text,
                        limit,
                        selected: chosen,
                        section: None,
                        uses: only_using,
                        team,
                        group_by,
                    };
                    let response = backend::search(request.clone()).await;
                    // Ignore answers to queries the user has already replaced.
                    if search_seq.get_value() == seq {
                        // A part only the CSV files had, selected before
                        // signing in, follows its IPN to the team's part.
                        let moved = response.as_ref().ok().and_then(|response| {
                            let now = response.selected_part.clone()?;
                            let asked = request.selected.as_ref()?;
                            (asked.team.is_empty() && *asked != now).then_some(now)
                        });
                        if let Some(part) = moved {
                            selected.set(Some(part));
                            return;
                        }
                        results.set(into_load(response).map(|response| Searched {
                            response,
                            request,
                            version,
                        }));
                    }
                });
            },
            SEARCH_DEBOUNCE,
        );
    });

    view! {
        <div class="app">
            <header class="topbar">
                <h1 class="brand">"KiCad Parts"</h1>
                <CatalogBadge status=status />
                <button
                    type="button"
                    id="libraries-open"
                    class="libraries-open"
                    on:click=move |_| libraries_open.set(Some(BrowseFor::default()))
                >
                    "Libraries"
                </button>
                <TeamsMenu teams=teams_badge set_teams=teams />
                <KicadBadge
                    status=status
                    kicad_epoch=kicad_epoch
                    override_input=override_input
                    libraries=kicad_libraries
                />
            </header>
            <KicadProblem status=status kicad_epoch=kicad_epoch override_input=override_input />
            <SettingsProblem status=status />
            <main class="workspace">
                <section class="search-pane" aria-label="Part search" data-library-check=check_state>
                    <div class="search-head">
                        <label class="search-label" for=SEARCH_BOX>
                            "Search parts"
                        </label>
                        {move || can_add.get().then(|| view! {
                            <button
                                type="button"
                                id="add-part-open"
                                class="add-part-open"
                                on:click=move |_| {
                                    checking.set(false);
                                    adding.set(true);
                                }
                            >
                                "Add part"
                            </button>
                        })}
                    </div>
                    <input
                        id=SEARCH_BOX
                        class="search-input"
                        type="search"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="IPN, MPN, manufacturer, value or footprint"
                        aria-describedby="result-count"
                        prop:value=move || query.get()
                        on:input:target=move |event| query.set(event.target().value())
                    />
                    {move || uses.get().map(|reference| view! {
                        <p class="uses-chip" id="uses-chip">
                            <span>"Parts using "<code>{cad_reference(&reference)}</code></span>
                            <button
                                type="button"
                                id="uses-chip-remove"
                                class="link-button"
                                on:click=move |_| {
                                    uses.set(None);
                                    focus_later(SEARCH_BOX);
                                }
                            >
                                "Show all parts"
                            </button>
                        </p>
                    })}
                    <ListControls teams=teams_badge set_teams=teams />
                    <LibraryCheckLine
                        check=library_check
                        problem=check_problem
                        team=Signal::derive(move || {
                            // The team of the results shown, while two teams
                            // or more show parts.
                            library_check.with(|check| {
                                check.as_ref().and_then(|check| list_teams.named(&check.team))
                            })
                        })
                        open=Callback::new(move |_| {
                            adding.set(false);
                            checking.set(true);
                            // The folder may have changed since (a fix
                            // made in KiCad), so look again.
                            refresh_check();
                        })
                    />
                    <ResultList
                        results=results
                        selected=selected
                        adding=adding
                        checking=checking
                        hidden_note=hidden_note
                    />
                </section>
                <PartPanel
                    selected=selected
                    kicad_epoch=kicad_epoch
                    team=part_team
                    adding=adding
                    checking=checking
                    library_check=library_check
                    library_add=library_add
                />
            </main>
            {move || layout_open.with(Option::is_some).then(|| view! {
                <library_layout::LayoutDialog open=layout_open />
            })}
            {move || libraries_open.with(Option::is_some).then(|| view! {
                <LibrariesDialog
                    open=libraries_open
                    library_check=library_check
                    actions=BrowserActions {
                        team: filter.into(),
                        can_add,
                        add_from: Callback::new(move |choice| library_add.set(Some(choice))),
                        show_parts: Callback::new(move |reference: String| {
                            query.set(String::new());
                            uses.set(Some(reference));
                            focus_later(SEARCH_BOX);
                        }),
                        open_part: Callback::new(move |part: PartKey| {
                            checking.set(false);
                            adding.set(false);
                            selected.set(Some(part));
                        }),
                    }
                />
            })}
        </div>
    }
}

/// One number for how far the app has followed every team that shows
/// parts, so the search runs again when anyone saves a Change on any team,
/// or a team signs in or out. `None` while no team shows parts. Each team's
/// address goes in too: two teams may share a checkpoint.
fn list_version(status: &TeamsStatus) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut any = false;
    // Only teams that show parts: one asked to sign in again leaves the
    // list at once.
    for team in status.teams.iter().filter(|team| shows_parts(team)) {
        // A team with no session shows parts kept from an earlier run
        // (kicad-parts-ed46). Its catch-up moves the checkpoint on, so the
        // list changes when the server's parts replace them. Going offline
        // during a run changes nothing in the list.
        let checkpoint = match &team.session {
            TeamSession::SignedIn { checkpoint, .. } => Some(*checkpoint),
            _ => None,
        };
        // The code too: a team named once its code is known shows its name
        // at once.
        (&team.server_url, &team.code, checkpoint).hash(&mut hasher);
        any = true;
    }
    any.then(|| hasher.finish())
}

/// Why a part can't be edited while its team's server cannot be reached,
/// in plain words, naming the team by `label` when there are several, and
/// saying when its parts last matched the server, if known.
fn offline_words(label: Option<&str>, as_of_ms: Option<u64>) -> Said {
    let team = label.map_or_else(|| "The team catalog".to_owned(), str::to_owned);
    let cannot = format!("{team} is offline, so this part can't be edited now. ");
    match as_of_ms {
        Some(as_of) => Said::parts_as_of(&cannot, as_of, " The app reconnects by itself."),
        None => Said::plain(format!(
            "{cannot}You see it as it was last seen. The app reconnects by itself."
        )),
    }
}

/// `20000` as `20,000`.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut text = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// `1 part`, `2 parts`, `20,000 parts`.
fn counted(n: usize, singular: &str, plural: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { singular } else { plural })
}

impl<T> Load<T> {
    fn map<U>(self, f: impl FnOnce(T) -> U) -> Load<U> {
        match self {
            Load::Loading => Load::Loading,
            Load::Ready(value) => Load::Ready(f(value)),
            Load::Failed(error) => Load::Failed(error),
        }
    }
}

fn into_load<T>(result: Result<T, String>) -> Load<T> {
    match result {
        Ok(value) => Load::Ready(value),
        Err(error) => Load::Failed(error),
    }
}

/// The header's words about where the list's parts come from. With two
/// teams or more showing parts, the CSV count alone would not match the
/// list, so it says the teams instead of a number. A catalog folder with no
/// CSV files gets no words, unless two teams or more show parts.
fn catalog_words(csv_parts: usize, csv_files: usize, teams: usize) -> String {
    if csv_files == 0 {
        return if teams >= 2 {
            format!("Parts from {teams} teams")
        } else {
            String::new()
        };
    }
    let files = counted(csv_files, "CSV file", "CSV files");
    if teams >= 2 {
        format!("Parts from {teams} teams and {files}")
    } else {
        format!("{} from {files}", counted(csv_parts, "part", "parts"))
    }
}

#[component]
fn CatalogBadge(status: RwSignal<Load<AppStatus>>) -> impl IntoView {
    let marks = use_context::<ListTeams>().map(|list| list.marks);
    let teams = move || {
        marks
            .map(|marks| marks.with(|marks| if marks.shown { marks.by_team.len() } else { 0 }))
            .unwrap_or_default()
    };
    move || match status.get() {
        Load::Ready(status) => match status.catalog.error {
            None => {
                let words =
                    move || catalog_words(status.catalog.parts, status.catalog.csv_files, teams());
                view! {
                    <Show when=move || !words().is_empty()>
                        <p class="catalog-summary">{words}</p>
                    </Show>
                }
                .into_any()
            }
            Some(error) => view! {
                <p class="catalog-summary catalog-error" role="alert">
                    {format!("Catalog not loaded: {error}")}
                </p>
            }
            .into_any(),
        },
        _ => ().into_any(),
    }
}

/// Header status: the KiCad version in use, with a form to change it.
#[component]
fn KicadBadge(
    status: RwSignal<Load<AppStatus>>,
    kicad_epoch: RwSignal<u32>,
    override_input: RwSignal<String>,
    libraries: RwSignal<Option<desktop_contract::KicadLibrariesStatus>>,
) -> impl IntoView {
    move || match status.get() {
        Load::Loading => view! { <p class="kicad-badge">"Looking for KiCad…"</p> }.into_any(),
        Load::Failed(error) => view! {
            <p class="kicad-badge kicad-badge-problem" role="alert">{error}</p>
        }
        .into_any(),
        Load::Ready(app) => match &app.kicad {
            KicadStatus::Ready {
                version,
                cli_path,
                from_override,
            } => {
                let source = if *from_override {
                    "your chosen path"
                } else {
                    "auto-detected"
                };
                view! {
                    <details class="kicad-settings">
                        <summary class="kicad-badge kicad-badge-ready">
                            {format!("KiCad {version}")}
                        </summary>
                        <div class="kicad-settings-body">
                            <p>
                                "Previews are exported by "
                                <code>{cli_path.clone()}</code>
                                {format!(" ({source}).")}
                            </p>
                            <OverrideForm
                                status=status
                                kicad_epoch=kicad_epoch
                                input=override_input
                                saved=app.kicad_override.is_some()
                                rejected=app.override_rejected.clone()
                            />
                            <KicadLibrariesSection status=libraries />
                        </div>
                    </details>
                }
                .into_any()
            }
            _ => view! {
                <p class="kicad-badge kicad-badge-problem">"KiCad needs attention"</p>
            }
            .into_any(),
        },
    }
}

/// A prominent, actionable panel whenever KiCad is not usable.
#[component]
fn KicadProblem(
    status: RwSignal<Load<AppStatus>>,
    kicad_epoch: RwSignal<u32>,
    override_input: RwSignal<String>,
) -> impl IntoView {
    move || {
        let Load::Ready(app) = status.get() else {
            return ().into_any();
        };
        let (title, explanation, details): (String, String, Vec<String>) = match &app.kicad {
            KicadStatus::Ready { .. } => return ().into_any(),
            KicadStatus::Missing { searched } => (
                "KiCad 10 was not found".into(),
                "Symbol and footprint previews are drawn by KiCad's own kicad-cli. Install \
                 KiCad 10 from kicad.org/download and choose Search again, or enter where \
                 kicad-cli is installed."
                    .into(),
                searched
                    .iter()
                    .map(|path| format!("Searched {path}"))
                    .chain(std::iter::once("Searched the folders on PATH".to_owned()))
                    .collect(),
            ),
            KicadStatus::Unsupported {
                version,
                cli_path,
                supported_major,
            } => (
                format!("KiCad {version} is not supported"),
                format!(
                    "This app needs KiCad {supported_major}. Install KiCad {supported_major} \
                     from kicad.org/download, or enter the path of a KiCad \
                     {supported_major} kicad-cli."
                ),
                vec![format!("Found {cli_path}")],
            ),
            // The saved path itself is already in the input below.
            KicadStatus::OverrideInvalid { problem, .. } => (
                "The kicad-cli location you entered does not work".into(),
                "Correct the path below, or clear it to search the standard install \
                 locations again."
                    .into(),
                vec![problem.clone()],
            ),
            KicadStatus::Broken { cli_path, message } => (
                "kicad-cli did not respond".into(),
                "Reinstall or repair KiCad 10, or enter the path of a working kicad-cli.".into(),
                vec![format!("{cli_path}: {message}")],
            ),
            // KiCad may be fine: the app's own folder for kicad-cli failed.
            KicadStatus::SettingsFolder { cli_path, message } => (
                "kicad-cli could not be started".into(),
                "This app keeps kicad-cli's settings in a folder of its own, and could not \
                 use it. Check that you can write to that folder, then choose Search again."
                    .into(),
                vec![message.clone(), format!("Found {cli_path}")],
            ),
        };
        view! {
            <section class="kicad-problem" role="alert" aria-labelledby="kicad-problem-title">
                <h2 id="kicad-problem-title">{title}</h2>
                <p>{explanation}</p>
                <ul class="kicad-problem-details">
                    {details.into_iter().map(|line| view! { <li>{line}</li> }).collect_view()}
                </ul>
                <OverrideForm
                    status=status
                    kicad_epoch=kicad_epoch
                    input=override_input
                    saved=app.kicad_override.is_some()
                    rejected=app.override_rejected.clone()
                />
            </section>
        }
        .into_any()
    }
}

#[component]
fn OverrideForm(
    status: RwSignal<Load<AppStatus>>,
    kicad_epoch: RwSignal<u32>,
    /// The path being typed.
    input: RwSignal<String>,
    /// Whether a path is saved (offers "Clear saved path").
    saved: bool,
    /// Why the last entered path was not saved.
    rejected: Option<String>,
) -> impl IntoView {
    let path = input;
    let busy = RwSignal::new(false);
    let apply = move |result: Result<AppStatus, String>| {
        busy.set(false);
        status.set(into_load(result));
        kicad_epoch.update(|epoch| *epoch += 1);
    };
    let save = move |value: Option<String>| {
        busy.set(true);
        if value.is_none() {
            path.set(String::new());
        }
        spawn_local(async move { apply(backend::set_kicad_override(value).await) });
    };
    let has_override = saved;
    view! {
        <form
            class="override-form"
            aria-label="kicad-cli location"
            on:submit=move |event| {
                event.prevent_default();
                save(Some(path.get_untracked()));
            }
        >
            <label for="kicad-cli-path">"kicad-cli path or KiCad install folder"</label>
            <div class="override-row">
                <input
                    id="kicad-cli-path"
                    type="text"
                    spellcheck="false"
                    placeholder="C:\\Program Files\\KiCad\\10.0\\bin\\kicad-cli.exe"
                    prop:value=move || path.get()
                    on:input:target=move |event| path.set(event.target().value())
                />
                <button type="submit" class="primary" disabled=move || busy.get()>
                    "Use this path"
                </button>
                <button
                    type="button"
                    disabled=move || busy.get()
                    on:click=move |_| {
                        busy.set(true);
                        spawn_local(async move { apply(backend::rediscover_kicad().await) });
                    }
                >
                    "Search again"
                </button>
                {has_override
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                disabled=move || busy.get()
                                on:click=move |_| save(None)
                            >
                                "Clear saved path"
                            </button>
                        }
                    })}
            </div>
            {rejected
                .map(|message| view! {
                    <p class="problem override-rejected" role="alert">
                        {format!("Not saved: {message}")}
                    </p>
                })}
        </form>
    }
}

/// Shown when the saved settings file exists but cannot be used.
#[component]
fn SettingsProblem(status: RwSignal<Load<AppStatus>>) -> impl IntoView {
    move || {
        let Load::Ready(app) = status.get() else {
            return ().into_any();
        };
        let Some(error) = app.settings_error else {
            return ().into_any();
        };
        view! {
            <section class="kicad-problem" role="alert" aria-labelledby="settings-problem-title">
                <h2 id="settings-problem-title">"Saved settings could not be read"</h2>
                <p>
                    "Default settings are in use. Saving a kicad-cli path replaces the file."
                </p>
                <ul class="kicad-problem-details">
                    <li>{error}</li>
                </ul>
            </section>
        }
        .into_any()
    }
}

#[component]
fn PartPanel(
    selected: RwSignal<Option<PartKey>>,
    kicad_epoch: RwSignal<u32>,
    /// The selected part's team, if it has one.
    team: Memo<Option<TeamStatus>>,
    adding: RwSignal<bool>,
    checking: RwSignal<bool>,
    library_check: RwSignal<Option<LibraryCheckView>>,
    /// Add part from it, chosen in the Libraries browser.
    library_add: RwSignal<Option<desktop_contract::TeamLibraryChoice>>,
) -> impl IntoView {
    let list_teams = expect_context::<ListTeams>();
    // Whether any team shows parts.
    let any_team = Memo::new(move |_| list_teams.marks.with(|marks| !marks.by_team.is_empty()));
    let detail = RwSignal::new(None::<Load<Option<PartDetail>>>);
    let symbol = RwSignal::new(Load::<PreviewOutcome>::Loading);
    let footprint = RwSignal::new(Load::<PreviewOutcome>::Loading);
    let seq = StoredValue::new(0u64);
    let preview_seq = StoredValue::new(0u64);
    // Tells the backend which previews are for the newest selection, so it
    // can drop waiting exports for older ones. Based on the clock, so it
    // keeps growing if the page reloads.
    let selection = StoredValue::new(0u64);
    // The team catalog's view of the selected part: None while signed out
    // or when the team catalog does not have it.
    let team_part = RwSignal::new(None::<TeamPart>);
    let team_checked = RwSignal::new(false);
    let team_seq = StoredValue::new(0u64);
    let ui = TeamUi::new();
    // Set by Add part for the part it opens: what to say once it is shown,
    // and a manufacturer part to add to it in Edit part.
    let pending_notice = StoredValue::new(None::<String>);
    let pending_part = StoredValue::new(None::<(PartKey, ManufacturerPart)>);
    // Add part's form, kept while another part is looked at.
    let add_form = add_part::Form::new();
    // Changes whenever someone saves a Change, a CSV import run finishes
    // (it may find CSV changes to review), or on sign-in and sign-out.
    let team_checkpoint = Memo::new(move |_| {
        team.with(|status| match status {
            Some(TeamStatus {
                server_url,
                session:
                    TeamSession::SignedIn {
                        checkpoint,
                        imports,
                        ..
                    },
                ..
            }) => Some((server_url.clone(), *checkpoint, *imports)),
            _ => None,
        })
    });
    // Who is signed in, to which team; another person or another team
    // starts from a clean column.
    let signed_in_as = Memo::new(move |_| {
        team.with(|status| match status {
            Some(TeamStatus {
                server_url,
                session: TeamSession::SignedIn { email, .. },
                ..
            }) => Some((server_url.clone(), email.clone())),
            _ => None,
        })
    });
    Effect::new(move |previous: Option<Option<(String, String)>>| {
        let now = signed_in_as.get();
        if previous.is_some_and(|previous| previous != now) {
            ui.reset();
            // Nothing typed for Add part by one person is kept for another.
            add_form.reset();
        }
        now
    });
    // Why the part's team can't take edits now, while its server cannot be
    // reached: it shows the parts last seen.
    let offline_note = Memo::new(move |_| {
        // With one team the note says "the team catalog", as the rest of
        // the one-team view does.
        let several = list_teams.marks.with(|marks| marks.shown);
        team.with(|status| {
            let status = status.as_ref()?;
            // Offline with a session, or at start with parts kept from an
            // earlier run.
            (standing_of(status) == Standing::Offline && shows_parts(status)).then(|| {
                offline_words(
                    several.then(|| status.label()).as_deref(),
                    status.parts_as_of,
                )
            })
        })
    });
    Effect::new(move |_| ui.offline.set(offline_note.with(Option::is_some)));
    // The part's team code, said in its Source while several teams show
    // parts, as its row's mark says it.
    let source_team = Memo::new(move |_| {
        let several = list_teams.marks.with(|marks| marks.shown);
        team.with(|status| status.as_ref().filter(|_| several).map(TeamStatus::label))
    });
    let can_edit = Memo::new(move |_| {
        let may_edit = team.with(|status| {
            matches!(
                status.as_ref().map(|status| &status.session),
                Some(TeamSession::SignedIn { can_edit: true, .. })
            )
        });
        may_edit && offline_note.with(Option::is_none)
    });
    // The selected part's team, which every request about it names.
    let team_url = Memo::new(move |_| {
        selected.with(|part| {
            part.as_ref()
                .map(|part| part.team.clone())
                .unwrap_or_default()
        })
    });
    Effect::new(move |_| ui.team.set(team_url.get()));
    // For a part the team catalog has: the revision shown, whose exact CAD
    // item revisions the previews are drawn from, and its Symbol and
    // Footprint in case those cannot be fetched. Otherwise the CSV catalog's.
    let preview_source = Memo::new(move |_| {
        team_part.with(|part| {
            part.as_ref().map(|part| {
                let field = |name: &str| part.revision.field(name).unwrap_or_default().to_owned();
                (part.revision.number, field("Symbol"), field("Footprint"))
            })
        })
    });

    Effect::new(move |_| {
        let Some(PartKey { team, ipn }) = selected.get() else {
            detail.set(None);
            return;
        };
        let current = seq.get_value() + 1;
        seq.set_value(current);
        detail.set(Some(Load::Loading));
        team_part.set(None);
        team_checked.set(false);
        ui.reset();
        if let Some(message) = pending_notice.try_update_value(Option::take).flatten() {
            ui.notice.set(Some(Notice::Done(message)));
        }
        spawn_local(async move {
            let result = backend::part_detail(&team, &ipn).await;
            if seq.get_value() == current {
                detail.set(Some(into_load(result)));
            }
        });
    });

    Effect::new(move |_| {
        let signed_in = team_checkpoint.get().is_some();
        let Some(PartKey { team, ipn }) = selected.get() else {
            return;
        };
        if !signed_in {
            team_part.set(None);
            team_checked.set(false);
            return;
        }
        let current = team_seq.get_value() + 1;
        team_seq.set_value(current);
        let asked = PartKey {
            team: team.clone(),
            ipn: ipn.clone(),
        };
        spawn_local(async move {
            let result = backend::team_part(&team, &ipn).await;
            if team_seq.get_value() == current && selected.get_untracked() == Some(asked) {
                // Keep showing the last good view if a refresh fails, and a
                // newer one than this answer: a refresh started before a
                // save of this app's own can arrive after the save's own
                // view, with older history.
                let newest =
                    |p: &TeamPart| (p.ipn.clone(), p.history.first().map(|entry| entry.seq));
                let older = match &result {
                    Ok(Some(fetched)) => team_part.with_untracked(|shown| {
                        shown.as_ref().is_some_and(|shown| {
                            let ((shown_ipn, shown_seq), (fetched_ipn, fetched_seq)) =
                                (newest(shown), newest(fetched));
                            shown_ipn == fetched_ipn && fetched_seq < shown_seq
                        })
                    }),
                    _ => false,
                };
                if let (Ok(part), false) = (result, older) {
                    // A change someone else saved makes an earlier "saved"
                    // message out of date.
                    let numbers = |p: &TeamPart| (p.details.number, p.revision.number);
                    let shown = team_part.with_untracked(|shown| shown.as_ref().map(numbers));
                    let fetched = part.as_ref().map(numbers);
                    if shown.is_some() && shown != fetched {
                        ui.notice.set(None);
                    }
                    team_part.set(part);
                }
                team_checked.set(true);
            }
        });
    });

    // Add part's "Add as a manufacturer part of it instead": once the part
    // is shown, Edit part opens with the manufacturer part added.
    Effect::new(move |_| {
        let Some(part) = team_part.get() else {
            return;
        };
        // The part of the team Add part was adding to, not the same IPN
        // in another team.
        let shown = selected.get_untracked();
        let wanted = pending_part.with_value(|pending| {
            pending
                .as_ref()
                .filter(|(key, _)| key.ipn == part.ipn && Some(key) == shown.as_ref())
                .map(|(_, added)| added.clone())
        });
        if let Some(added) = wanted {
            pending_part.set_value(None);
            let mut draft = team::draft_of(&part);
            draft.parts.push(added);
            ui.start_editing(draft);
        }
    });

    Effect::new(move |_| {
        kicad_epoch.track();
        // Signing in again (or as someone else) refetches the same revision,
        // so the source is unchanged; draw again anyway, so a "Sign in to see
        // this symbol" card from the signed-out moment never stays.
        signed_in_as.track();
        let source = preview_source.get();
        let Some(PartKey { team, ipn }) = selected.get() else {
            return;
        };
        let current = preview_seq.get_value() + 1;
        preview_seq.set_value(current);
        let now = js_sys::Date::now() as u64;
        let this_selection = now.max(selection.get_value() + 1);
        selection.set_value(this_selection);
        symbol.set(Load::Loading);
        footprint.set(Load::Loading);
        let (team_revision, symbol_field, footprint_field) = match source {
            Some((revision, symbol, footprint)) => (Some(revision), Some(symbol), Some(footprint)),
            None => (None, None, None),
        };
        for (kind, target, field) in [
            (PreviewKind::Symbol, symbol, symbol_field),
            (PreviewKind::Footprint, footprint, footprint_field),
        ] {
            let request = PreviewRequest {
                ipn: ipn.clone(),
                kind,
                field,
                team_revision,
                selection: Some(this_selection),
            };
            let team = team.clone();
            spawn_local(async move {
                let replaced = |result: &Result<PreviewOutcome, String>| {
                    matches!(
                        result,
                        Ok(PreviewOutcome::Problem(problem))
                            if problem.reason == PreviewProblemReason::Replaced
                    )
                };
                let mut result = backend::preview(&team, request.clone()).await;
                // Only the newest selection's answer is shown, even if an
                // older export finishes later.
                if preview_seq.get_value() != current {
                    return;
                }
                // The backend knows a newer selection number than this one
                // (for example after the clock moved back), yet this is
                // still what the person selected: ask again without a
                // selection, which is never replaced, rather than loading
                // forever.
                if replaced(&result) {
                    result = backend::preview(
                        &team,
                        PreviewRequest {
                            selection: None,
                            ..request
                        },
                    )
                    .await;
                }
                if preview_seq.get_value() == current {
                    target.set(into_load(result));
                }
            });
        }
    });

    let close_adding = Callback::new(move |_| adding.set(false));
    let close_checking = Callback::new(move |_| {
        checking.set(false);
        focus_later("library-check-open");
    });
    let added = Callback::new(move |ipn: String| {
        pending_notice.set_value(Some(format!("Added {ipn} as revision 1.")));
        adding.set(false);
        selected.set(Some(PartKey {
            team: add_form.team.get_untracked(),
            ipn,
        }));
    });
    // The Library check's Add part from it: Add part opens with the symbol
    // or footprint chosen in our libraries, for the team checked.
    let add_from_check = Callback::new(move |choice: desktop_contract::TeamLibraryChoice| {
        add_form.start_from_library(choice);
        // The team of the results shown, which may not be the team chosen
        // since, while its check is still worked out.
        let checked =
            library_check.with_untracked(|check| check.as_ref().map(|check| check.team.clone()));
        if let Some(team) = checked.filter(|team| !team.is_empty()) {
            add_form.start_in(team);
        }
        checking.set(false);
        adding.set(true);
    });
    // The Libraries browser's Add part from it, for Add part's own team.
    let add_from = Callback::new(move |choice: desktop_contract::TeamLibraryChoice| {
        add_form.start_from_library(choice);
        checking.set(false);
        adding.set(true);
    });
    // The Libraries browser's Add part from it.
    Effect::new(move |_| {
        if let Some(choice) = library_add.get() {
            library_add.set(None);
            add_from.run(choice);
        }
    });
    // A part of the team Add part was adding to.
    let open_part = Callback::new(move |(ipn, part): (String, Option<ManufacturerPart>)| {
        let key = PartKey {
            team: add_form.team.get_untracked(),
            ipn,
        };
        pending_part.set_value(part.map(|part| (key.clone(), part)));
        adding.set(false);
        selected.set(Some(key));
    });
    view! {
        <section
            class="part-panel"
            aria-label=move || if adding.get() {
                "New part"
            } else if checking.get() {
                "Library check"
            } else {
                "Selected part"
            }
        >
            {move || adding.get().then(|| view! {
                <AddPartView
                    form=add_form
                    start_team=Signal::derive(move || {
                        list_teams.add_part.team.get().unwrap_or_default()
                    })
                    close=close_adding
                    done=added
                    open_part=open_part
                />
            })}
            {move || (checking.get() && !adding.get()).then(|| view! {
                <LibraryCheckPanel
                    check=library_check
                    teams=list_teams.library_check.options
                    team=Signal::derive(move || {
                        library_check.with(|check| check.as_ref().map(|check| check.team.clone()).unwrap_or_default())
                    })
                    choose_team=list_teams.library_check.choose
                    close=close_checking
                    add_from=add_from_check
                />
            })}
            {move || if adding.get() || checking.get() { ().into_any() } else { match detail.get() {
                None => view! {
                    <div class="empty-state">
                        <h2>"No part selected"</h2>
                        <p>"Select a part to see its KiCad symbol, every unit, and its footprint."</p>
                    </div>
                }
                .into_any(),
                Some(Load::Loading) => view! {
                    <p class="loading" aria-busy="true">"Loading part…"</p>
                }
                .into_any(),
                Some(Load::Failed(error)) => view! {
                    <p class="problem" role="alert">{format!("Could not load the part: {error}")}</p>
                }
                .into_any(),
                Some(Load::Ready(None)) => view! {
                    <p class="problem" role="alert">"This part is no longer in the catalog."</p>
                }
                .into_any(),
                Some(Load::Ready(Some(part))) => view! {
                    <PartView
                        part=part
                        symbol=symbol
                        footprint=footprint
                        team_part=team_part
                        team_checked=team_checked
                        in_no_team=Signal::derive(move || {
                            any_team.get() && team_url.with(String::is_empty)
                        })
                        offline_note=offline_note
                        source_team=source_team
                        can_edit=can_edit
                        ui=ui
                    />
                }
                .into_any(),
            }}}
        </section>
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn PartView(
    part: PartDetail,
    symbol: RwSignal<Load<PreviewOutcome>>,
    footprint: RwSignal<Load<PreviewOutcome>>,
    team_part: RwSignal<Option<TeamPart>>,
    team_checked: RwSignal<bool>,
    /// The part is only in the CSV files while a team shows parts.
    in_no_team: Signal<bool>,
    /// Why the part can't be edited now, while its team is offline.
    offline_note: Memo<Option<Said>>,
    /// The part's team code, while several teams show parts.
    source_team: Memo<Option<String>>,
    can_edit: Memo<bool>,
    ui: TeamUi,
) -> impl IntoView {
    let in_team = Memo::new(move |_| team_part.with(Option::is_some));
    let summary = part.summary.clone();
    // The header: from the team catalog's current details and revision when
    // it has the part, otherwise from the CSV catalog.
    let header = move || {
        let team = team_part.get();
        let field = |name: &str| {
            team.as_ref()
                .and_then(|part| part.details.field(name).map(str::to_owned))
        };
        let main = team
            .as_ref()
            .and_then(|part| part.details.manufacturer_parts.first().cloned());
        let (mpn, manufacturer) = match (&team, main) {
            (Some(_), Some(main)) => (main.mpn, main.manufacturer),
            (Some(_), None) => (String::new(), String::new()),
            (None, _) => (summary.mpn.clone(), summary.manufacturer.clone()),
        };
        let heading = if mpn.is_empty() {
            summary.ipn.clone()
        } else {
            mpn
        };
        let description = field("Description").unwrap_or_else(|| summary.description.clone());
        let value = field("Value").unwrap_or_else(|| summary.value.clone());
        // The IPN prefix is the letters before the first dash, such as `RES`.
        let ipn_prefix = summary
            .ipn
            .split_once('-')
            .map(|(prefix, _)| prefix.to_owned())
            .unwrap_or_default();
        let catalog = match source_team.get() {
            Some(code) => format!("{code} team catalog"),
            None => "Team catalog".to_owned(),
        };
        let source = match &team {
            Some(part) => format!("{catalog}, revision {} of this part", part.revision.number),
            None if summary.csv_file == "Team catalog" => catalog,
            None => summary.csv_file.clone(),
        };
        let facts: Vec<(&str, String)> = [
            ("IPN", summary.ipn.clone()),
            ("Manufacturer", manufacturer),
            ("Value", value),
            ("IPN prefix", ipn_prefix),
            ("Source", source),
        ]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .collect();
        view! {
            <h2 id="part-title">{heading}</h2>
            <p class="part-description">{description}</p>
            <dl class="facts">
                {facts
                    .into_iter()
                    .map(|(name, value)| view! {
                        <div class="fact">
                            <dt>{name}</dt>
                            <dd>{value}</dd>
                        </div>
                    })
                    .collect_view()}
            </dl>
        }
    };
    let fields = part.fields.clone();
    let field_count = fields.len();
    // Previews stack on the left and every catalog field stays visible on
    // the right, so a typical part needs no scrolling.
    view! {
        <div class="part-view">
            <header class="part-header">{header}</header>
            <div class="part-body" class:team=move || in_team.get()>
                // Each column scrolls on its own when a part needs more room,
                // so both are focusable for keyboard scrolling.
                <div class="part-previews" role="region" aria-label="Symbol and footprint" tabindex="0">
                    <PreviewSection kind=PreviewKind::Symbol state=symbol />
                    <PreviewSection kind=PreviewKind::Footprint state=footprint />
                </div>
                // The open edit form replaces the details and their heading.
                <section
                    class="part-fields"
                    aria-labelledby=move || {
                        if in_team.get() && ui.editing.with(Option::is_some) {
                            "edit-heading"
                        } else {
                            "fields-heading"
                        }
                    }
                    tabindex="0"
                >
                    {move || {
                        let offline = move || offline_note.get().map(|note| view! {
                            <p class="hint team-offline-note" role="status"><SaidView said=note /></p>
                        });
                        if in_team.get() {
                            // Drawn again when edit access changes, such as
                            // when the team goes offline or comes back.
                            return view! {
                                {offline}
                                <TeamColumn part=team_part can_edit=can_edit.get() ui=ui />
                            }
                            .into_any();
                        }
                        let fields = fields.clone();
                        // An offline team's part is shown as last seen,
                        // which the offline note says, not "not in the
                        // team catalog".
                        let is_offline = offline_note.with(Option::is_some);
                        let not_in_team = in_no_team.get() || (team_checked.get() && !is_offline);
                        view! {
                            {offline}
                            <h3 id="fields-heading">{format!("Catalog fields ({field_count})")}</h3>
                            {not_in_team.then(|| view! {
                                <p class="hint">"This part is not in the team catalog yet, so it has no history."</p>
                            })}
                            <table aria-labelledby="fields-heading">
                                <thead>
                                    <tr><th scope="col">"Field"</th><th scope="col">"Value"</th></tr>
                                </thead>
                                <tbody>
                                    {fields
                                        .into_iter()
                                        .map(|field| {
                                            // Long URLs stay on one line; hovering shows all of it.
                                            let is_url = field.value.starts_with("http://")
                                                || field.value.starts_with("https://");
                                            let title = is_url.then(|| field.value.clone());
                                            let value = if is_cad_field(&field.name) {
                                                cad_reference(&field.value)
                                            } else {
                                                field.value.into_any()
                                            };
                                            view! {
                                                <tr>
                                                    <th scope="row">{field.name}</th>
                                                    <td class:field-url=is_url title=title>{value}</td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                        }
                        .into_any()
                    }}
                </section>
            </div>
        </div>
    }
}

/// A CAD reference such as `g-Resistor_SMD:R_0603_1608Metric`. The library
/// part up to the colon never breaks across lines (not even after its `g-`),
/// and the line may break right after the colon. A value with several
/// choices, such as `g-Resistor_SMD:R_0603;g-TestPoint:TP`, shows each choice
/// this way and may also break after each `;`. The spans and `<wbr>` add no
/// characters, so copying gives the exact value.
pub(crate) fn cad_reference(reference: &str) -> AnyView {
    let mut choices = reference.split(';').peekable();
    let mut parts = Vec::new();
    while let Some(choice) = choices.next() {
        parts.push(one_reference(choice));
        if choices.peek().is_some() {
            parts.push(view! { ";"<wbr/> }.into_any());
        }
    }
    parts.collect_view().into_any()
}

/// One choice of [`cad_reference`].
fn one_reference(reference: &str) -> AnyView {
    match reference.split_once(':') {
        Some((library, name)) if !library.is_empty() && !name.is_empty() => view! {
            <span class="lib">{format!("{library}:")}</span><wbr/><span>{name.to_owned()}</span>
        }
        .into_any(),
        _ => reference.to_owned().into_any(),
    }
}

/// `text` with the first copy of `reference` in it shown by [`cad_reference`].
pub(crate) fn text_with_reference(text: &str, reference: Option<&str>) -> AnyView {
    match reference
        .filter(|r| !r.is_empty())
        .and_then(|r| text.find(r).map(|at| (r, at)))
    {
        Some((reference, at)) => {
            let before = text[..at].to_owned();
            let after = text[at + reference.len()..].to_owned();
            view! { {before}{cad_reference(reference)}{after} }.into_any()
        }
        None => text.to_owned().into_any(),
    }
}

/// Whether a field holds a symbol or footprint reference: its label starts
/// with `Symbol` or `Footprint`, as in `Footprint revision (choice 2)`.
pub(crate) fn is_cad_field(name: &str) -> bool {
    matches!(name.split_whitespace().next(), Some("Symbol" | "Footprint"))
}

fn kind_title(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::Symbol => "Symbol",
        PreviewKind::Footprint => "Footprint",
    }
}

#[component]
fn PreviewSection(
    kind: PreviewKind,
    state: RwSignal<Load<PreviewOutcome>>,
    /// Put before every id, so a second drawing on the page (the Libraries
    /// browser's) has ids of its own.
    #[prop(optional)]
    id_prefix: &'static str,
    /// Show each drawing in a viewer that zooms and moves, with an
    /// Enlarge button, instead of a picture that opens the close-up.
    #[prop(optional)]
    interactive: bool,
) -> impl IntoView {
    let title = kind_title(kind);
    let heading_id = format!("{id_prefix}{}-heading", title.to_lowercase());
    let labelled_by = heading_id.clone();
    view! {
        <section class="preview" aria-labelledby=labelled_by>
            <h3 id=heading_id>{title}</h3>
            {move || match state.get() {
                Load::Loading => view! {
                    <p class="loading" aria-busy="true">
                        {format!("Exporting the {} with kicad-cli…", title.to_lowercase())}
                    </p>
                }
                .into_any(),
                Load::Failed(error) => view! {
                    <div class="problem" role="alert"><p>{error}</p></div>
                }
                .into_any(),
                Load::Ready(PreviewOutcome::Ready(set)) => view! { <PreviewImages set=set interactive=interactive /> }.into_any(),
                Load::Ready(PreviewOutcome::Problem(problem)) => {
                    view! { <PreviewProblemView problem=problem /> }.into_any()
                }
            }}
        </section>
    }
}

#[component]
fn PreviewImages(set: PreviewSet, #[prop(optional)] interactive: bool) -> impl IntoView {
    let title = kind_title(set.kind);
    let total = set.images.len();
    let units = {
        let mut units: Vec<u32> = set.images.iter().filter_map(|image| image.unit).collect();
        units.dedup();
        units.len()
    };
    let source = match set.cad_item_revision {
        // The CAD item's own revision, not the part's.
        Some(number) => match set.kind {
            PreviewKind::Symbol => format!("team catalog, symbol revision {number}"),
            PreviewKind::Footprint => format!("team catalog, footprint revision {number}"),
        },
        None if set.from_kicad_bundle => "KiCad library".to_owned(),
        None if set.installed_library => "installed library".to_owned(),
        None => "project library".to_owned(),
    };
    // Hovering shows the library file, or the stored bytes' SHA-256.
    let drawn_from = if set.library_path.is_empty() {
        format!("Stored bytes with SHA-256 {}", set.library_sha256)
    } else {
        set.library_path.clone()
    };
    let note = set.note.clone();
    // Each part stays on one line; the line breaks only between parts.
    let provenance = [
        source,
        format!("kicad-cli {}", set.kicad_version),
        if set.from_cache {
            "from cache"
        } else {
            "exported now"
        }
        .to_owned(),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, part)| {
        let separator = if index == 0 { "" } else { " · " };
        view! { {separator}<span class="provenance-part">{part}</span> }
    })
    .collect_view();
    let reference = set.reference.clone();
    let alternates = set.alternates.clone();
    let canvas = match set.kind {
        PreviewKind::Symbol => "canvas canvas-symbol",
        PreviewKind::Footprint => "canvas canvas-footprint",
    };
    // One close-up per preview section, showing the clicked image.
    let close_up = CloseUp::new();
    let footprint = set.kind == PreviewKind::Footprint;
    view! {
        <p class="preview-meta">
            <code class="reference">{cad_reference(&set.reference)}</code>
            <span class="provenance" title=drawn_from>{provenance}</span>
        </p>
        {note.map(|note| view! { <p class="preview-note">{text_with_reference(&note, Some(&set.reference))}</p> })}
        {(!alternates.is_empty())
            .then(|| view! {
                <p class="alternates">
                    "Also listed: "
                    {alternates
                        .iter()
                        .enumerate()
                        .map(|(index, alternate)| {
                            let separator = if index == 0 { "" } else { ", " };
                            view! { {separator}{cad_reference(alternate)} }
                        })
                        .collect_view()}
                </p>
            })}
        <div class={if total > 1 { "preview-grid multi" } else { "preview-grid" }}>
            {set
                .images
                .into_iter()
                .map(|image| {
                    let body = if image.demorgan { ", De Morgan" } else { "" };
                    let (caption, alt) = match image.unit {
                        Some(unit) if total > 1 => {
                            let letter = unit_label(unit);
                            (
                                format!("Unit {letter}{body} ({unit} of {units})"),
                                format!("{title} {reference}, unit {letter}{body} ({unit} of {units})"),
                            )
                        }
                        _ => (title.to_owned(), format!("{title} {reference}")),
                    };
                    let picture = Picture::new(&image, alt.clone(), footprint);
                    let drawing = if interactive {
                        let enlarged = picture.clone();
                        let enlarge = Callback::new(move |()| close_up.open(enlarged.clone()));
                        view! { <Viewer picture=picture enlarge=enlarge /> }.into_any()
                    } else {
                        view! {
                            <button
                                type="button"
                                class=canvas
                                title="Enlarge"
                                on:click=move |_| close_up.open(picture.clone())
                            >
                                <img src=image.data_uri alt=alt decoding="async" />
                            </button>
                        }
                        .into_any()
                    };
                    view! {
                        <figure class="preview-figure">
                            {drawing}
                            {(total > 1).then(|| view! { <figcaption>{caption}</figcaption> })}
                        </figure>
                    }
                })
                .collect_view()}
        </div>
        <CloseUpDialog close_up=close_up />
    }
}

/// An element's left, right, top and bottom on the screen
/// (`getBoundingClientRect()`).
pub(crate) fn bounds(element: &wasm_bindgen::JsValue) -> Option<[f64; 4]> {
    use wasm_bindgen::JsCast;
    let method = js_sys::Reflect::get(element, &"getBoundingClientRect".into()).ok()?;
    let rect = method.dyn_ref::<js_sys::Function>()?.call0(element).ok()?;
    let side = |name: &str| js_sys::Reflect::get(&rect, &name.into()).ok()?.as_f64();
    Some([side("left")?, side("right")?, side("top")?, side("bottom")?])
}

/// `HTMLDialogElement.showModal()`, which traps focus and closes on Escape.
pub(crate) fn show_modal(dialog: &wasm_bindgen::JsValue) {
    use wasm_bindgen::JsCast;
    if let Ok(method) = js_sys::Reflect::get(dialog, &"showModal".into()) {
        if let Some(method) = method.dyn_ref::<js_sys::Function>() {
            let _ = method.call0(dialog);
        }
    }
}

#[component]
fn PreviewProblemView(problem: PreviewProblem) -> impl IntoView {
    let noun = kind_title(problem.kind).to_lowercase();
    let (headline, hint) = match problem.reason {
        PreviewProblemReason::Unassigned => (format!("No {noun} assigned"), String::new()),
        PreviewProblemReason::Unresolved => (
            format!("The {noun} library was not found"),
            format!(
                "Check the {noun} reference in the catalog, or add the library to the \
                 project's {noun}s folder."
            ),
        ),
        PreviewProblemReason::TeamSignInNeeded => {
            (format!("Sign in to see this {noun}"), String::new())
        }
        PreviewProblemReason::Replaced => {
            (format!("The {noun} preview was replaced"), String::new())
        }
        PreviewProblemReason::KicadUnavailable => (
            "Previews need KiCad 10".to_owned(),
            "Fix the KiCad problem shown above; previews load automatically.".to_owned(),
        ),
        PreviewProblemReason::ExportFailed => (
            format!("kicad-cli could not export this {noun}"),
            format!(
                "Check that the library really contains this {noun}, or correct the \
                 catalog reference."
            ),
        ),
        PreviewProblemReason::Unexportable => (
            format!("kicad-cli cannot export this {noun}"),
            format!("Rename the {noun} in its library so the name does not begin with '-'."),
        ),
        PreviewProblemReason::UnknownPart | PreviewProblemReason::Internal => {
            (format!("The {noun} preview failed"), String::new())
        }
    };
    // A missing assignment is not an error, and a KiCad problem is already
    // announced by the panel at the top; neither needs another alert.
    let quiet = matches!(
        problem.reason,
        PreviewProblemReason::Unassigned
            | PreviewProblemReason::KicadUnavailable
            | PreviewProblemReason::TeamSignInNeeded
            | PreviewProblemReason::Replaced
    );
    // The headline and reference already say what failed to export.
    let message = (problem.reason != PreviewProblemReason::ExportFailed).then_some(problem.message);
    view! {
        <div class={if quiet { "problem quiet" } else { "problem" }} role={(!quiet).then_some("alert")}>
            <p class="problem-title">{headline}</p>
            {problem.reference.clone().map(|reference| view! { <p><code class="reference">{cad_reference(&reference)}</code></p> })}
            {message.map(|message| view! { <p>{text_with_reference(&message, problem.reference.as_deref())}</p> })}
            {problem
                .cli_message
                .map(|message| view! {
                    <p class="cli-label">"kicad-cli said:"</p>
                    <pre class="cli-message">{message}</pre>
                })}
            {(!hint.is_empty()).then(|| view! { <p class="hint">{hint}</p> })}
            {problem
                .command
                .map(|command| view! {
                    <details class="command">
                        <summary>"Command"</summary>
                        <pre>{command}</pre>
                    </details>
                })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_header_says_where_the_lists_parts_come_from() {
        assert_eq!(
            super::catalog_words(178, 21, 0),
            "178 parts from 21 CSV files"
        );
        assert_eq!(
            super::catalog_words(178, 21, 1),
            "178 parts from 21 CSV files"
        );
        assert_eq!(
            super::catalog_words(178, 21, 2),
            "Parts from 2 teams and 21 CSV files"
        );
        // A folder with no CSV catalog, such as the sample library.
        assert_eq!(super::catalog_words(0, 0, 0), "");
        assert_eq!(super::catalog_words(0, 0, 1), "");
        assert_eq!(super::catalog_words(0, 0, 2), "Parts from 2 teams");
    }
}

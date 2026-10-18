//! The "KiCad libraries" section of the KiCad settings panel
//! (kicad-parts-f397): the team code that names the libraries, whether the
//! signed-in team's libraries are up to date, Update now and Open folder,
//! what to add to KiCad once, and other teams' libraries on this computer.

use crate::backend;
use desktop_contract::{
    KicadLibrariesState, KicadLibrariesStatus, KicadLibrary, OdbcDriver, TeamCode,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The browser's clipboard: `navigator.clipboard.writeText`.
    #[wasm_bindgen(js_namespace = ["navigator", "clipboard"], js_name = writeText, catch)]
    async fn write_text(text: &str) -> Result<JsValue, JsValue>;
}

/// How long "Copied" shows on a Copy button.
const COPIED_FOR: Duration = Duration::from_secs(2);

/// `count` with `singular` or `plural`.
fn counted(count: u32, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// A time in milliseconds since 1970 as local hours and minutes, such as
/// `14:02`, in a `<time>` with the full UTC time for machines.
fn local_time(at_ms: u64) -> impl IntoView {
    let date = js_sys::Date::new(&JsValue::from_f64(at_ms as f64));
    let shown = format!("{:02}:{:02}", date.get_hours(), date.get_minutes());
    let datetime: String = date.to_iso_string().into();
    view! { <time datetime=datetime>{shown}</time> }
}

/// `a`, `a and b`, `a, b and c`.
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[component]
pub fn KicadLibrariesSection(status: RwSignal<Option<KicadLibrariesStatus>>) -> impl IntoView {
    let asked = RwSignal::new(false);
    // Which Copy button just copied, and what the live region says.
    let copied = RwSignal::new(None::<&'static str>);
    let announced = RwSignal::new(String::new());
    let copy_failed = RwSignal::new(false);
    let update_now = move |_| {
        asked.set(true);
        spawn_local(async move {
            if let Ok(latest) = backend::kicad_libraries_update().await {
                status.set(Some(latest));
            }
            asked.set(false);
        });
    };
    let open_folder = move |_| {
        spawn_local(async move {
            let _ = backend::kicad_libraries_open_folder().await;
        });
    };
    let copy = move |which: &'static str, what: &'static str, text: String| {
        spawn_local(async move {
            let worked = write_text(&text).await.is_ok();
            copy_failed.set(!worked);
            copied.set(worked.then_some(which));
            announced.set(if worked {
                format!("Copied the {what}.")
            } else {
                String::new()
            });
            if worked {
                let kind = if which == "symbol" {
                    "symbols"
                } else {
                    "footprints"
                };
                if let Ok(latest) = backend::kicad_libraries_copied(kind).await {
                    status.set(Some(latest));
                }
            }
            set_timeout(
                move || {
                    copied.set(None);
                    announced.set(String::new());
                },
                COPIED_FOR,
            );
        });
    };
    let state_line = move || {
        let Some(known) = status.get() else {
            return view! { <p class="kicad-libraries-status" role="status">"Looking…"</p> }
                .into_any();
        };
        let restart = known
            .restart_kicad
            .then_some(" The groups or fields changed, so restart KiCad to see them.");
        match known.state {
            KicadLibrariesState::SignedOut => view! {
                <p class="kicad-libraries-status" role="status" data-state="signed-out">
                    "Sign in to the team catalog to make the KiCad libraries."
                </p>
            }
            .into_any(),
            KicadLibrariesState::Waiting => view! {
                <p class="kicad-libraries-status" role="status" data-state="waiting">
                    "Waiting for the team catalog…"
                </p>
            }
            .into_any(),
            KicadLibrariesState::NeedsCode => view! {
                <p class="kicad-libraries-status" role="status" data-state="needs-code">
                    "No KiCad libraries are made until the team has a code."
                </p>
            }
            .into_any(),
            KicadLibrariesState::Updating => view! {
                <p class="kicad-libraries-status" role="status" data-state="updating">
                    "Updating…"
                </p>
            }
            .into_any(),
            KicadLibrariesState::UpToDate {
                at_ms,
                current,
                older,
                archived,
            } => view! {
                <p
                    class="kicad-libraries-status"
                    role="status"
                    data-state="up-to-date"
                    data-at=at_ms.to_string()
                >
                    {format!(
                        "Up to date: {}, {} and {}.",
                        counted(current, "part", "parts"),
                        counted(older, "older revision", "older revisions"),
                        counted(archived, "archived revision", "archived revisions"),
                    )}
                    {restart}
                    // The time comes last, so nothing after it moves with
                    // its width.
                    " Updated at "
                    {local_time(at_ms)}
                    "."
                </p>
            }
            .into_any(),
            // Not a failure: it waits its turn.
            KicadLibrariesState::Busy { at_ms } => view! {
                <p
                    class="kicad-libraries-status"
                    role="status"
                    data-state="busy"
                    data-at=at_ms.to_string()
                >
                    "Another update of these libraries is running, from this app or another one, so this one waits. It tries again in 30 seconds. Tried at "
                    {local_time(at_ms)}
                    "."
                </p>
            }
            .into_any(),
            KicadLibrariesState::CodeTaken {
                code,
                other_server,
                server,
            } => view! {
                <p
                    class="kicad-libraries-status kicad-libraries-failed"
                    role="status"
                    data-state="code-taken"
                >
                    {format!(
                        "Two teams on this computer use the code {code}, and KiCad can't hold both. The {code} libraries here belong to the team on {other_server}, so none are made for this team. To fix this, a superuser of {server} changes this team's code before any board uses it. Then sign out and sign in again."
                    )}
                </p>
            }
            .into_any(),
            KicadLibrariesState::Failed { at_ms, reason } => view! {
                <p
                    class="kicad-libraries-status kicad-libraries-failed"
                    role="status"
                    data-state="failed"
                    data-at=at_ms.to_string()
                >
                    "The last update failed. "
                    {reason}
                    " Tried at "
                    {local_time(at_ms)}
                    "."
                </p>
            }
            .into_any(),
        }
    };
    let can_update = move || {
        status.with(|known| {
            known.as_ref().is_some_and(|known| {
                !matches!(
                    known.state,
                    KicadLibrariesState::SignedOut
                        | KicadLibrariesState::NeedsCode
                        | KicadLibrariesState::CodeTaken { .. }
                )
            })
        })
    };
    let new_libraries = move || {
        let new: Vec<KicadLibrary> = status.with(|known| {
            known
                .as_ref()
                .map(|known| known.new_libraries.clone())
                .unwrap_or_default()
        });
        (!new.is_empty()).then(|| {
            let names: Vec<String> = new.into_iter().map(|library| library.nickname).collect();
            view! {
                <p class="kicad-libraries-new" role="status">
                    {format!(
                        "New {}: {}. Copy the libraries below again and paste them into KiCad.",
                        if names.len() == 1 { "library" } else { "libraries" },
                        listed(&names)
                    )}
                </p>
            }
        })
    };
    let problems = move || {
        let problems = status.with(|known| {
            known
                .as_ref()
                .map(|known| known.problems.clone())
                .unwrap_or_default()
        });
        (!problems.is_empty()).then(|| {
            view! {
                <details class="kicad-libraries-problems">
                    <summary>{format!(
                        "{} to look at",
                        counted(problems.len() as u32, "item", "items")
                    )}</summary>
                    <ul>
                        {problems.into_iter().map(|problem| view! { <li>{problem}</li> }).collect_view()}
                    </ul>
                </details>
            }
        })
    };
    let driver = move || {
        let driver = status.with(|known| known.as_ref().map(|known| known.odbc_driver))?;
        Some(match driver {
            OdbcDriver::Installed => view! {
                <p class="kicad-libraries-driver">
                    "KiCad reads the parts through the SQLite3 ODBC driver, which is installed."
                </p>
            }
            .into_any(),
            OdbcDriver::Missing => view! {
                <p class="kicad-libraries-driver kicad-libraries-missing">
                    "KiCad reads the parts through the SQLite3 ODBC driver, which is not installed. Install the 64-bit SQLite3 ODBC driver, then restart KiCad."
                </p>
            }
            .into_any(),
            OdbcDriver::Unknown => view! {
                <p class="kicad-libraries-driver">
                    "KiCad reads the parts through the SQLite3 ODBC driver. Check that it is installed."
                </p>
            }
            .into_any(),
        })
    };
    let setup = move || {
        let known = status.get()?;
        if known.libraries.is_empty() {
            return None;
        }
        let table = move |which: &'static str,
                          what: &'static str,
                          title: &'static str,
                          text: String| {
            let heading = format!("kicad-{which}-libraries-heading");
            let copied_text = text.clone();
            view! {
                <div class="kicad-library-table">
                    <div class="kicad-library-table-head">
                        <h4 id=heading.clone()>{title}</h4>
                        <button
                            type="button"
                            class="kicad-library-copy"
                            id=format!("kicad-{which}-libraries-copy")
                            on:click=move |_| copy(which, what, copied_text.clone())
                        >
                            {move || if copied.get() == Some(which) { "Copied" } else { "Copy" }}
                            <span class="visually-hidden">{format!(" the {what}")}</span>
                        </button>
                    </div>
                    <pre
                        id=format!("kicad-{which}-libraries")
                        tabindex="0"
                        aria-labelledby=heading
                    >
                        {text}
                    </pre>
                </div>
            }
        };
        Some(view! {
            <h3>"Add them to KiCad once"</h3>
            <ol class="kicad-libraries-steps">
                <li>
                    "Copy the symbol libraries. In KiCad, choose Preferences, then Manage Symbol Libraries. On the Global Libraries tab, right-click the table, choose Paste, then choose OK."
                </li>
                <li>
                    "Copy the footprint libraries, and paste them the same way in Preferences, then Manage Footprint Libraries."
                </li>
            </ol>
            <p class="kicad-libraries-note">
                {format!(
                    "New parts and revisions reach KiCad within about {} seconds: close and open the Add Symbol window to see them.",
                    known.refresh_seconds
                )}
            </p>
            {move || copy_failed.get().then(|| view! {
                <p class="kicad-libraries-copy-failed">
                    "Copying did not work. Select the text and press Ctrl+C instead."
                </p>
            })}
            {table("symbol", "symbol libraries", "Symbol libraries", known.symbol_table.clone())}
            {table("footprint", "footprint libraries", "Footprint libraries", known.footprint_table.clone())}
        })
    };
    let other_teams = move || {
        let others = status.with(|known| {
            known
                .as_ref()
                .map(|known| known.other_teams.clone())
                .unwrap_or_default()
        });
        (!others.is_empty()).then(|| {
            view! {
                <h3>"Other teams on this computer"</h3>
                <p>"Their libraries stay as they are, so they keep working in KiCad."</p>
                <ul class="kicad-libraries-other-teams">
                    {others
                        .into_iter()
                        .map(|other| {
                            let updated = other.updated_at_ms;
                            view! {
                                <li>
                                    <strong>{other.code}</strong>
                                    {format!(", from {}", other.server)}
                                    {updated.map(|at| view! { ", updated at " {local_time(at)} })}
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            }
        })
    };
    view! {
        <section class="kicad-libraries" aria-labelledby="kicad-libraries-heading">
            <h2 id="kicad-libraries-heading">"KiCad libraries"</h2>
            <p>
                "The team's parts, one entry for each revision, so a design never switches to newer CAD by itself. They are in "
                <code class="kicad-libraries-folder">
                    {move || status.with(|known| known.as_ref().map(|known| known.folder.clone()))}
                </code>
                "."
            </p>
            <TeamCodeChoice status=status />
            {state_line}
            {new_libraries}
            <div class="override-row">
                <button
                    type="button"
                    id="kicad-libraries-update"
                    disabled=move || !can_update() || asked.get()
                    on:click=update_now
                >
                    "Update now"
                </button>
                <button type="button" id="kicad-libraries-open" on:click=open_folder>
                    "Open folder"
                </button>
            </div>
            <p class="visually-hidden" role="status" aria-live="polite" id="kicad-libraries-copied">
                {move || announced.get()}
            </p>
            {problems}
            {driver}
            {setup}
            {other_teams}
        </section>
    }
}

/// The team code: read-only once chosen; otherwise a short form for people
/// who can edit, with a confirmation, or who to ask.
#[component]
fn TeamCodeChoice(status: RwSignal<Option<KicadLibrariesStatus>>) -> impl IntoView {
    let typed = RwSignal::new(String::new());
    // The code waiting for confirmation, and any warning about it.
    let confirming = RwSignal::new(None::<(String, Option<String>)>);
    let problem = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let check = move || {
        let code = typed.get_untracked().trim().to_uppercase();
        busy.set(true);
        spawn_local(async move {
            match backend::team_code_check(&code).await {
                Ok(found) => match found.problem {
                    Some(why) => problem.set(Some(why)),
                    None => {
                        problem.set(None);
                        confirming.set(Some((code, found.warning)));
                        crate::focus::focus_later("team-code-confirm");
                    }
                },
                Err(why) => problem.set(Some(why)),
            }
            busy.set(false);
        });
    };
    let save = move |code: String| {
        busy.set(true);
        spawn_local(async move {
            match backend::team_code_set(&code).await {
                Ok(latest) => {
                    confirming.set(None);
                    problem.set(None);
                    status.set(Some(latest));
                }
                Err(why) => {
                    confirming.set(None);
                    problem.set(Some(why));
                }
            }
            busy.set(false);
        });
    };
    // Drawn again only when the code changes, so typing is never cut off.
    let team_code = Memo::new(move |_| {
        status.with(|known| known.as_ref().map(|known| known.team_code.clone()))
    });
    move || {
        match team_code.get() {
            None | Some(TeamCode::Unknown) => ().into_any(),
            Some(TeamCode::Set { code, set_by }) => view! {
                <p class="team-code" id="team-code">
                    "Team code: " <strong>{code}</strong>
                    {(!set_by.is_empty()).then(|| format!(", chosen by {set_by}"))}
                    ". It names the team's libraries and never changes."
                </p>
            }
            .into_any(),
            Some(TeamCode::Missing {
                can_set: false,
                mut editors,
                unnamed_editors,
            }) => view! {
                <p class="team-code" id="team-code">
                    {
                        // Members are named, never shown by email.
                        match unnamed_editors {
                            0 => {}
                            1 => editors.push(
                                "a member without a name in the team catalog".to_owned(),
                            ),
                            many => editors.push(format!(
                                "{many} members without a name in the team catalog"
                            )),
                        }
                        if editors.is_empty() {
                            "The team has no code yet. Ask someone who can edit the team catalog to choose it.".to_owned()
                        } else {
                            format!(
                                "The team has no code yet. Ask someone who can edit the team catalog to choose it: {}.",
                                listed(&editors)
                            )
                        }
                    }
                </p>
            }
            .into_any(),
            Some(TeamCode::Missing { can_set: true, .. }) => view! {
                <form
                    class="team-code-form"
                    aria-label="Team code"
                    on:submit=move |event| {
                        event.prevent_default();
                        check();
                    }
                >
                    <label for="team-code-input">"Team code"</label>
                    <p class="hint" id="team-code-hint">
                        "2 or 3 capital letters or digits, such as MA. It names the team's KiCad libraries, such as MA-Parts, and boards save those names, so it can never change."
                    </p>
                    <div class="override-row">
                        <input
                            id="team-code-input"
                            type="text"
                            maxlength="3"
                            spellcheck="false"
                            autocomplete="off"
                            aria-describedby="team-code-hint"
                            prop:value=move || typed.get()
                            on:input:target=move |event| {
                                typed.set(event.target().value().to_uppercase());
                                confirming.set(None);
                            }
                        />
                        <button type="submit" disabled=move || busy.get()>"Choose code…"</button>
                    </div>
                    {move || problem.get().map(|why| view! {
                        <p class="team-code-problem" role="alert">{why}</p>
                    })}
                    {move || confirming.get().map(|(code, warning)| {
                        let chosen = code.clone();
                        view! {
                            <div class="team-code-confirm" role="group" aria-labelledby="team-code-confirm-text">
                                <p id="team-code-confirm-text">
                                    {format!(
                                        "Use {code} as the team code? Every member's KiCad libraries will be named {code}-Parts, {code}-Resistor_SMD and so on, and boards save those names. It can never be changed in the app."
                                    )}
                                </p>
                                {warning.map(|warning| view! {
                                    <p class="team-code-warning">{warning}</p>
                                })}
                                <div class="override-row">
                                    <button
                                        type="button"
                                        class="primary"
                                        id="team-code-confirm"
                                        disabled=move || busy.get()
                                        on:click=move |_| save(chosen.clone())
                                    >
                                        {format!("Use {code}")}
                                    </button>
                                    <button type="button" on:click=move |_| confirming.set(None)>
                                        "Cancel"
                                    </button>
                                </div>
                            </div>
                        }
                    })}
                </form>
            }
            .into_any(),
        }
    }
}

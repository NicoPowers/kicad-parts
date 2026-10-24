//! Review library layout (kicad-parts-e9a3): one screen that says where a
//! team's library repo keeps its libraries, 3D models and path variables,
//! as the app's scan found them. The person fixes only what the scan got
//! wrong (a path variable's folder, a library to leave out), then chooses
//! "Use this layout".
//!
//! Library names stay exactly as the repo gives them: boards store them.

use crate::backend;
use crate::focus::focus_later;
use crate::team_marks::{TeamChip, TeamMark};
use crate::{counted, grouped};
use desktop_contract::{
    LayoutChoices, LayoutDoubt, LayoutLibrary, LayoutUseRequest, LibraryLayoutReview, TeamsStatus,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::{BTreeMap, BTreeSet};

/// How many examples each kind of doubt lists before "and N more".
const EXAMPLES: usize = 4;

/// Which team's layout the dialog reviews, and the button that gets the
/// keyboard back when it closes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LayoutOpen {
    pub server_url: String,
    pub return_focus: String,
}

/// Opens the dialog; provided by the app.
#[derive(Clone, Copy)]
pub(crate) struct LayoutOpener(pub RwSignal<Option<LayoutOpen>>);

/// The counts line at the top, such as "2 symbol libraries, 1 footprint
/// library and 14 3D models in 1 folder."
pub(crate) fn summary_text(review: &LibraryLayoutReview) -> String {
    let layout = &review.layout;
    let mut parts = vec![
        counted(
            layout.symbol_libraries.len(),
            "symbol library",
            "symbol libraries",
        ),
        counted(
            layout.footprint_libraries.len(),
            "footprint library",
            "footprint libraries",
        ),
        format!(
            "{} in {}",
            counted(layout.model_files, "3D model", "3D models"),
            counted(layout.model_folders.len(), "folder", "folders")
        ),
    ];
    if !layout.database_libraries.is_empty() {
        parts.push(counted(
            layout.database_libraries.len(),
            "database library",
            "database libraries",
        ));
    }
    let last = parts.pop().unwrap_or_default();
    if parts.is_empty() {
        format!("{last}.")
    } else {
        format!("{} and {last}.", parts.join(", "))
    }
}

/// One kind of doubt: a heading and one line per case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DoubtGroup {
    pub heading: String,
    pub lines: Vec<String>,
    /// Cases not listed.
    pub more: usize,
}

/// The doubts by kind, in plain words, each with a few examples.
pub(crate) fn doubt_groups(doubts: &[LayoutDoubt]) -> Vec<DoubtGroup> {
    let mut groups: BTreeMap<u8, (String, Vec<String>)> = BTreeMap::new();
    for doubt in doubts {
        let (order, heading, line) = match doubt {
            LayoutDoubt::VariableNotFound { name } => (
                0,
                "Path variables with no folder",
                format!("No folder matches the links of {name}. Choose its folder below."),
            ),
            LayoutDoubt::NicknameClash { nickname, paths } => (
                1,
                "Libraries with the same name",
                format!(
                    "{} libraries are named {nickname}: {}. KiCad uses only one, so keep the one boards use. The app first keeps the one a library table names.",
                    paths.len(),
                    paths.join(", ")
                ),
            ),
            LayoutDoubt::MissingModel { footprint, link } => (
                2,
                "Missing 3D models",
                format!("{footprint} links {link}, but the repo has no such file."),
            ),
            LayoutDoubt::CaseDiffers {
                footprint,
                link,
                file,
            } => (
                3,
                "3D model names that differ in capitals",
                format!(
                    "{footprint} links {link}, but the file is {file}. That works on Windows and macOS, not on Linux."
                ),
            ),
            LayoutDoubt::GitLfs { path } => (
                4,
                "Files stored with Git LFS",
                format!("{path}: the app sees only a small pointer file, not the file itself."),
            ),
            LayoutDoubt::TableUnreadable { table, why } => (
                5,
                "Library tables that can't be read",
                format!("{table}: {why}."),
            ),
        };
        let group = groups
            .entry(order)
            .or_insert_with(|| (heading.to_owned(), Vec::new()));
        group.1.push(line);
    }
    groups
        .into_values()
        .map(|(heading, lines)| {
            let more = lines.len().saturating_sub(EXAMPLES);
            DoubtGroup {
                heading: format!("{heading} ({})", grouped(lines.len())),
                lines: lines.into_iter().take(EXAMPLES).collect(),
                more,
            }
        })
        .collect()
}

/// How a folder reads in the list of folders: the repo's top folder by
/// name, since it has none.
pub(crate) fn folder_label(folder: &str) -> String {
    if folder.is_empty() {
        "The repo's top folder".to_owned()
    } else {
        format!("{folder}/")
    }
}

/// The dialog, while `open` names a team.
#[component]
pub(crate) fn LayoutDialog(open: RwSignal<Option<LayoutOpen>>) -> impl IntoView {
    let teams = use_context::<RwSignal<Option<TeamsStatus>>>();
    let LayoutOpen {
        server_url,
        return_focus,
    } = open.get_untracked().unwrap_or(LayoutOpen {
        server_url: String::new(),
        return_focus: String::new(),
    });
    let dialog = NodeRef::<leptos::html::Dialog>::new();
    let review = RwSignal::new(None::<Result<LibraryLayoutReview, String>>);
    let folders = RwSignal::new(BTreeMap::<String, String>::new());
    let left_out = RwSignal::new(BTreeSet::<String>::new());
    let busy = RwSignal::new(true);
    let refused = RwSignal::new(None::<String>);
    let mark = {
        let server_url = server_url.clone();
        move || {
            teams.and_then(|teams| {
                teams.with(|status| {
                    let team = status
                        .as_ref()?
                        .teams
                        .iter()
                        .find(|team| team.server_url == server_url)?;
                    Some(TeamMark {
                        label: team.label(),
                        colour: team.colour.clone(),
                        offline: false,
                    })
                })
            })
        }
    };

    // Scans, then fills in the choices: the ones made before, or the
    // first ones. A scan again keeps what the person changed meanwhile.
    let scan = {
        let server_url = server_url.clone();
        move |fetch: bool| {
            let server_url = server_url.clone();
            busy.set(true);
            refused.set(None);
            spawn_local(async move {
                let found = backend::library_layout_review(&server_url, fetch).await;
                if let Ok(found) = &found {
                    let first = review.with_untracked(|review| !matches!(review, Some(Ok(_))));
                    if first {
                        folders.set(found.choices.variable_folders.clone());
                        left_out.set(found.choices.left_out.iter().cloned().collect());
                    }
                }
                let first_answer = review.with_untracked(Option::is_none);
                review.set(Some(found));
                busy.set(false);
                // Once the scan is shown, the keyboard starts at its
                // summary, so a screen reader reads what was found first.
                if first_answer {
                    focus_later("layout-summary");
                }
            });
        }
    };
    scan(false);

    Effect::new(move |_| {
        if let Some(dialog) = dialog.get() {
            crate::show_modal(&dialog);
        }
    });
    let close = move || {
        if let Some(dialog) = dialog.get_untracked() {
            dialog.close();
        }
    };
    let use_layout = {
        let server_url = server_url.clone();
        move || {
            let Some(Ok(found)) = review.get_untracked() else {
                return;
            };
            let request = LayoutUseRequest {
                server_url: server_url.clone(),
                commit: found.layout.commit.clone(),
                choices: LayoutChoices {
                    variable_folders: folders.get_untracked(),
                    left_out: left_out.get_untracked().into_iter().collect(),
                },
            };
            busy.set(true);
            refused.set(None);
            spawn_local(async move {
                match backend::library_layout_use(request).await {
                    Ok(status) => {
                        if let Some(teams) = teams {
                            teams.set(Some(status));
                        }
                        busy.set(false);
                        if let Some(dialog) = dialog.get_untracked() {
                            dialog.close();
                        }
                    }
                    Err(message) => {
                        busy.set(false);
                        refused.set(Some(message));
                    }
                }
            });
        }
    };

    let body = move || match review.get() {
        None => view! { <p class="hint" role="status">"Scanning the repo…"</p> }.into_any(),
        Some(Err(message)) => view! {
            <div class="problem" role="alert"><p>{message}</p></div>
        }
        .into_any(),
        Some(Ok(found)) => layout_view(found, folders, left_out).into_any(),
    };
    let title = move || {
        review.with(|review| match review {
            Some(Ok(found)) => format!("Library layout of {}", found.repo_name),
            _ => "Library layout".to_owned(),
        })
    };
    let commit_line = move || {
        review.with(|review| match review {
            Some(Ok(found)) => {
                let short = &found.layout.commit[..12.min(found.layout.commit.len())];
                let mut line = format!(
                    "As the main branch was at commit {short}, scanned from the app's copy."
                );
                if let Some(note) = &found.note {
                    line.push(' ');
                    line.push_str(note);
                }
                line
            }
            _ => String::new(),
        })
    };
    let can_use = move || review.with(|review| matches!(review, Some(Ok(_))));
    let (scan_again, use_now) = (scan.clone(), use_layout.clone());

    view! {
        <dialog
            node_ref=dialog
            class="layout-dialog"
            aria-labelledby="layout-title"
            on:close=move |_| {
                open.set(None);
                if !return_focus.is_empty() {
                    focus_later(&return_focus);
                }
            }
        >
            <header class="layout-header">
                {move || mark().map(|mark| view! { <TeamChip mark=mark /> })}
                <h2 id="layout-title">{title}</h2>
                <button type="button" id="layout-close" on:click=move |_| close()>"Close"</button>
            </header>
            <p class="layout-commit hint">{commit_line}</p>
            <div class="layout-body">{body}</div>
            <footer class="layout-footer">
                {move || refused.get().map(|message| view! {
                    <p class="layout-refused" role="alert">{message}</p>
                })}
                <p class="hint layout-footer-hint">
                    "The app reads these libraries from its own copy. Library names stay as the repo gives them."
                </p>
                <button
                    type="button"
                    disabled=move || busy.get()
                    on:click=move |_| scan_again(true)
                >
                    "Scan again"
                </button>
                <button
                    type="button"
                    class="primary"
                    id="layout-use"
                    disabled=move || busy.get() || !can_use()
                    on:click=move |_| use_now()
                >
                    {move || if busy.get() && can_use() { "Saving…" } else { "Use this layout" }}
                </button>
            </footer>
        </dialog>
    }
}

/// The scan and the person's choices, in two columns: what to check and
/// change on the left, the libraries on the right.
fn layout_view(
    found: LibraryLayoutReview,
    folders: RwSignal<BTreeMap<String, String>>,
    left_out: RwSignal<BTreeSet<String>>,
) -> impl IntoView {
    let layout = found.layout.clone();
    let doubts = doubt_groups(&layout.doubts);
    let all_folders = layout.folders.clone();
    let variables = layout.path_variables.clone();
    view! {
        <div class="layout-columns">
            <div class="layout-column">
                <p class="layout-summary" id="layout-summary" tabindex="-1">{summary_text(&found)}</p>
                {(!doubts.is_empty()).then(|| view! {
                    <section class="layout-section layout-doubts" aria-labelledby="layout-doubts-title">
                        <h3 id="layout-doubts-title">"Check these"</h3>
                        {doubts.into_iter().map(|group| view! {
                            <h4>{group.heading}</h4>
                            <ul>
                                {group.lines.into_iter().map(|line| view! { <li>{line}</li> }).collect_view()}
                                {(group.more > 0).then(|| view! {
                                    <li class="hint">{format!("and {} more", grouped(group.more))}</li>
                                })}
                            </ul>
                        }).collect_view()}
                    </section>
                })}
                <section class="layout-section" aria-labelledby="layout-variables-title">
                    <h3 id="layout-variables-title">
                        {format!("Path variables ({})", variables.len())}
                    </h3>
                    {if variables.is_empty() {
                        view! { <p class="hint">"No footprint links a 3D model through a path variable."</p> }.into_any()
                    } else {
                        view! {
                            <p class="hint">"Set each one in KiCad's Configure Paths to this folder of your clone."</p>
                            <ul class="layout-variables">
                                {variables.into_iter().map(|variable| {
                                    let id = format!("layout-variable-{}", variable.name);
                                    let name = variable.name.clone();
                                    let found_folder = variable.folder.clone();
                                    let chosen = {
                                        let (name, found_folder) = (name.clone(), found_folder.clone());
                                        move || folders.with(|folders| folders.get(&name).cloned().or_else(|| found_folder.clone()))
                                    };
                                    let options = all_folders.clone();
                                    let set_name = name.clone();
                                    view! {
                                        <li>
                                            <label for=id.clone()>
                                                <code>{format!("${{{}}}", variable.name)}</code>
                                                <span class="hint">{format!(" {}", counted(variable.links, "link", "links"))}</span>
                                            </label>
                                            <select
                                                id=id
                                                on:change:target=move |event| {
                                                    let value = event.target().value();
                                                    folders.update(|folders| {
                                                        folders.insert(set_name.clone(), value);
                                                    });
                                                }
                                            >
                                                {found_folder.is_none().then(|| {
                                                    let chosen = chosen.clone();
                                                    view! {
                                                        <option value="" selected=move || chosen().is_none() disabled=true>"Choose a folder…"</option>
                                                    }
                                                })}
                                                {options.into_iter().map(|folder| {
                                                    let label = folder_label(&folder);
                                                    let this = folder.clone();
                                                    let chosen = chosen.clone();
                                                    view! {
                                                        <option value=folder selected=move || chosen().as_deref() == Some(this.as_str())>{label}</option>
                                                    }
                                                }).collect_view()}
                                            </select>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }}
                </section>
                <section class="layout-section" aria-labelledby="layout-models-title">
                    <h3 id="layout-models-title">{format!("3D model folders ({})", layout.model_folders.len())}</h3>
                    <ul class="layout-paths">
                        {layout.model_folders.iter().map(|folder| view! { <li>{folder_label(folder)}</li> }).collect_view()}
                    </ul>
                    {(!layout.database_libraries.is_empty()).then(|| view! {
                        <h3>{format!("Database libraries ({})", layout.database_libraries.len())}</h3>
                        <ul class="layout-paths">
                            {layout.database_libraries.iter().map(|path| view! { <li>{path.clone()}</li> }).collect_view()}
                        </ul>
                    })}
                </section>
            </div>
            <div class="layout-column layout-libraries">
                {library_list("symbols", "Symbol libraries", layout.symbol_libraries.clone(), left_out)}
                {library_list("footprints", "Footprint libraries", layout.footprint_libraries.clone(), left_out)}
            </div>
        </div>
    }
}

/// One kind of library, each with a box to keep it in or leave it out.
fn library_list(
    id: &'static str,
    title: &'static str,
    libraries: Vec<LayoutLibrary>,
    left_out: RwSignal<BTreeSet<String>>,
) -> impl IntoView {
    let heading = format!("layout-{id}-title");
    let labelled = heading.clone();
    let count = libraries.len();
    view! {
        <section class="layout-section" aria-labelledby=labelled>
            <h3 id=heading>{format!("{title} ({count})")}</h3>
            {(count == 0).then(|| view! { <p class="hint">"None found."</p> })}
            <ul class="layout-library-list">
                {libraries.into_iter().enumerate().map(|(at, library)| {
                    let box_id = format!("layout-{id}-{at}");
                    let (path, toggled) = (library.path.clone(), library.path.clone());
                    view! {
                        <li>
                            <input
                                type="checkbox"
                                id=box_id.clone()
                                prop:checked=move || left_out.with(|left_out| !left_out.contains(&path))
                                on:change:target=move |event| {
                                    let keep = event.target().checked();
                                    left_out.update(|left_out| {
                                        if keep {
                                            left_out.remove(&toggled);
                                        } else {
                                            left_out.insert(toggled.clone());
                                        }
                                    });
                                }
                            />
                            <label for=box_id>
                                <span class="layout-nickname">{library.nickname.clone()}</span>
                                <span class="layout-path">{library.path.clone()}</span>
                                {library.from_table.then(|| view! {
                                    <span class="hint layout-from-table">"named in a library table"</span>
                                })}
                            </label>
                        </li>
                    }
                }).collect_view()}
            </ul>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_contract::LibraryLayout;

    fn review(layout: LibraryLayout) -> LibraryLayoutReview {
        LibraryLayoutReview {
            server_url: "https://acme.example.test".into(),
            repo_name: "acme/kicad-lib".into(),
            layout,
            choices: LayoutChoices::default(),
            reviewed_before: false,
            note: None,
        }
    }

    fn library(nickname: &str) -> LayoutLibrary {
        LayoutLibrary {
            nickname: nickname.into(),
            path: format!("{nickname}.kicad_sym"),
            from_table: false,
        }
    }

    #[test]
    fn the_summary_counts_what_the_scan_found() {
        let layout = LibraryLayout {
            symbol_libraries: vec![library("A"), library("B")],
            footprint_libraries: vec![library("F")],
            model_files: 14,
            model_folders: vec!["3d".into()],
            ..LibraryLayout::default()
        };
        assert_eq!(
            summary_text(&review(layout.clone())),
            "2 symbol libraries, 1 footprint library and 14 3D models in 1 folder."
        );
        let with_database = LibraryLayout {
            database_libraries: vec!["a.kicad_dbl".into()],
            ..layout
        };
        assert_eq!(
            summary_text(&review(with_database)),
            "2 symbol libraries, 1 footprint library, 14 3D models in 1 folder and 1 database library."
        );
    }

    #[test]
    fn doubts_are_grouped_with_a_few_examples_each() {
        let mut doubts: Vec<LayoutDoubt> = (0..6)
            .map(|n| LayoutDoubt::MissingModel {
                footprint: format!("A.pretty/F{n}.kicad_mod"),
                link: format!("${{ACME_3D}}/F{n}.step"),
            })
            .collect();
        doubts.push(LayoutDoubt::VariableNotFound {
            name: "HOME_3D".into(),
        });
        let groups = doubt_groups(&doubts);
        assert_eq!(groups.len(), 2);
        // What the person can fix here comes first.
        assert_eq!(groups[0].heading, "Path variables with no folder (1)");
        assert_eq!(
            groups[0].lines,
            ["No folder matches the links of HOME_3D. Choose its folder below."]
        );
        assert_eq!(groups[1].heading, "Missing 3D models (6)");
        assert_eq!(groups[1].lines.len(), EXAMPLES);
        assert_eq!(groups[1].more, 2);
        assert_eq!(
            groups[1].lines[0],
            "A.pretty/F0.kicad_mod links ${ACME_3D}/F0.step, but the repo has no such file."
        );
    }

    #[test]
    fn the_top_folder_reads_by_name() {
        assert_eq!(folder_label(""), "The repo's top folder");
        assert_eq!(folder_label("lib/3d"), "lib/3d/");
    }
}

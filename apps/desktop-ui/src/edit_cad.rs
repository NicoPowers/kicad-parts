//! Edit part's Symbol and Footprint (kicad-parts-bc6f). Nothing is typed:
//! each shows the chosen reference read only, with Choose... (the
//! Libraries browser for that role, on the current item) and, for the
//! symbol and the first footprint, Use a file... for a downloaded file. A
//! file, or a copy of an installed library's item, goes into one of our
//! libraries chosen from a list. A part with several footprint choices
//! keeps them as a list, with Add another... and Remove.
//!
//! Each choice is a row; every change writes the Symbol and Footprint
//! fields and the files to send into the draft, so a conflict's form and a
//! retry see them as saved.

use crate::backend;
use crate::cad_choice::{
    ChooseButton, CopyModels, Drawing, LibrarySelect, draw_item, noun, pick_problems, use_words,
};
use crate::team::{FilePreview, TeamUi, accepted, base64, read_chosen};
use desktop_contract::{
    FileKind, FilePreviewRequest, PickUse, PreviewKind, TeamFile, TeamFileRequest, TeamPick,
    TeamPickRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// What happened to one choice since the form opened.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Change {
    /// As the part has it (or as chosen before a conflict opened the form
    /// again).
    None,
    /// Chosen in the Libraries browser and used by reference: the
    /// Catalog's, ours or KiCad's.
    Pick(Box<TeamPick>),
    /// A file chosen with Use a file..., or the copy of an installed
    /// library's item.
    File(Box<FileChoice>),
}

/// A file a choice sends, as read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FileChoice {
    /// The symbol library (with the symbol named) or the footprint file.
    pub file: TeamFile,
    /// The symbols in a symbol library, or the footprint's name.
    pub names: Vec<String>,
    /// The library it goes into; blank until one is chosen.
    pub library: String,
    /// Catalog 3D models the footprint names that the Catalog lacks.
    pub missing_models: Vec<String>,
    /// Files chosen for those 3D models.
    pub model_files: Vec<TeamFile>,
    /// For the copy of an installed library's item: the choice it came
    /// from, with the 3D models copied with it.
    pub copy: Option<TeamPick>,
    /// For a copied footprint: No 3D model.
    pub no_models: bool,
}

impl FileChoice {
    /// The symbol or footprint's own name.
    fn name(&self) -> String {
        self.file
            .symbol
            .clone()
            .or_else(|| self.names.first().cloned())
            .unwrap_or_default()
    }

    /// `LIBRARY:NAME`, or just the name while no library is chosen.
    fn reference(&self) -> String {
        if self.library.is_empty() {
            self.name()
        } else {
            format!("{}:{}", self.library, self.name())
        }
    }
}

/// One symbol or footprint choice.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// Stays the same while rows are added and removed.
    pub key: u32,
    /// What the field says for this choice.
    pub reference: String,
    /// What it said when the form opened; blank for a choice added since.
    pub before: String,
    pub change: Change,
}

/// The rows of one field.
#[derive(Clone, Copy)]
struct Rows {
    kind: PreviewKind,
    /// The field's place in the draft's CAD fields.
    index: usize,
    rows: RwSignal<Vec<Row>>,
}

impl Rows {
    fn row(&self, key: u32) -> Option<Row> {
        self.rows
            .with_untracked(|rows| rows.iter().find(|row| row.key == key).cloned())
    }

    fn set(&self, key: u32, update: impl FnOnce(&mut Row)) {
        self.rows.update(|rows| {
            if let Some(row) = rows.iter_mut().find(|row| row.key == key) {
                update(row);
            }
        });
    }
}

fn file_kind(kind: PreviewKind) -> FileKind {
    match kind {
        PreviewKind::Symbol => FileKind::Symbol,
        PreviewKind::Footprint => FileKind::Footprint,
    }
}

/// Writes the rows into the draft: the field values and the files.
fn sync(ui: TeamUi, symbol: Option<Rows>, footprints: Option<Rows>) {
    let mut files = Vec::new();
    let mut unchosen = None;
    let mut collect = |rows: &[Row], kind: PreviewKind| {
        for row in rows {
            if let Change::File(choice) = &row.change {
                if choice.library.is_empty() && unchosen.is_none() {
                    unchosen = Some(format!(
                        "Choose a library for the {} {} before you save.",
                        noun(kind),
                        choice.name()
                    ));
                }
                let mut file = choice.file.clone();
                file.replaces = Some(row.before.clone());
                file.no_models = choice.copy.is_some() && choice.no_models;
                files.push(file);
                if let Some(copy) = choice.copy.as_ref().filter(|_| !choice.no_models) {
                    files.extend(copy.models.iter().cloned());
                }
                files.extend(choice.model_files.iter().cloned());
            }
        }
    };
    let symbol_value = symbol.map(|rows| {
        rows.rows.with_untracked(|list| {
            collect(list, PreviewKind::Symbol);
            list.first()
                .map(|row| row.reference.clone())
                .unwrap_or_default()
        })
    });
    let footprint_value = footprints.map(|rows| {
        rows.rows.with_untracked(|list| {
            collect(list, PreviewKind::Footprint);
            list.iter()
                .map(|row| row.reference.trim().to_owned())
                .filter(|reference| !reference.is_empty())
                .collect::<Vec<_>>()
                .join(";")
        })
    });
    ui.draft.update_value(|draft| {
        if let (Some(rows), Some(value)) = (symbol, symbol_value) {
            draft.cad_fields[rows.index].value = value;
        }
        if let (Some(rows), Some(value)) = (footprints, footprint_value) {
            draft.cad_fields[rows.index].value = value;
        }
        draft.files = files;
        draft.unchosen_library = unchosen;
    });
}

/// The Symbol and Footprint of Edit part, from the draft. `before` holds
/// the part's Symbol and Footprint as they are now.
#[component]
pub(crate) fn CadFields(ui: TeamUi, ipn: String, before: Vec<(String, String)>) -> impl IntoView {
    let draft = ui.draft.get_value();
    let next_key = StoredValue::new(0u32);
    let new_key = move || {
        next_key.update_value(|key| *key += 1);
        next_key.get_value()
    };
    let was = |name: &str| {
        before
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let make = |name: &str, kind: PreviewKind| {
        let index = draft
            .cad_fields
            .iter()
            .position(|field| field.name == name)?;
        let value = draft.cad_fields[index].value.clone();
        let was = was(name);
        let choices: Vec<String> = match kind {
            PreviewKind::Symbol => vec![value.trim().to_owned()],
            PreviewKind::Footprint => value
                .split(';')
                .map(|choice| choice.trim().to_owned())
                .filter(|choice| !choice.is_empty())
                .collect(),
        };
        let earlier: Vec<String> = was
            .split(';')
            .map(|choice| choice.trim().to_owned())
            .collect();
        let mut rows: Vec<Row> = choices
            .into_iter()
            .enumerate()
            .map(|(at, reference)| Row {
                key: new_key(),
                before: earlier.get(at).cloned().unwrap_or_default(),
                reference,
                change: Change::None,
            })
            .collect();
        if rows.is_empty() {
            rows.push(Row {
                key: new_key(),
                reference: String::new(),
                before: String::new(),
                change: Change::None,
            });
        }
        Some(Rows {
            kind,
            index,
            rows: RwSignal::new(rows),
        })
    };
    let symbol = make("Symbol", PreviewKind::Symbol);
    let footprints = make("Footprint", PreviewKind::Footprint);
    let changed = Callback::new(move |_| sync(ui, symbol, footprints));
    let symbol_reference = Signal::derive(move || {
        symbol
            .and_then(|rows| {
                rows.rows
                    .with(|list| list.first().map(|row| row.reference.clone()))
            })
            .filter(|_| {
                // A file's symbol is not in a library yet, so its filters
                // cannot be used.
                symbol.is_some_and(|rows| {
                    rows.rows.with(|list| {
                        list.first()
                            .is_some_and(|row| !matches!(row.change, Change::File(_)))
                    })
                })
            })
    });

    // Files chosen before (the form opened again after a conflict) are
    // read again.
    let restore = |rows: Option<Rows>| {
        let Some(rows) = rows else { return };
        let kind = file_kind(rows.kind);
        for file in draft.files.iter().filter(|file| file.kind == kind) {
            let name = file.symbol.clone();
            // A symbol file goes with the symbol; a downloaded footprint file
            // with the first choice; a copied footprint with the choice named
            // like its file, as KiCad names a footprint's file.
            let wanted = match kind {
                FileKind::Footprint if file.installed.is_none() => None,
                FileKind::Footprint => file.file_name.strip_suffix(".kicad_mod").map(str::to_owned),
                _ => name.clone(),
            };
            let row = rows.rows.with_untracked(|list| {
                list.iter()
                    .find(|row| {
                        wanted
                            .as_deref()
                            .is_some_and(|wanted| row.reference.rsplit(':').next() == Some(wanted))
                    })
                    .map(|row| row.key)
            });
            let Some(key) = row.or_else(|| {
                rows.rows
                    .with_untracked(|list| list.first().map(|row| row.key))
            }) else {
                continue;
            };
            let copy = file.installed.as_ref().map(|_| TeamPick {
                kind: rows.kind,
                library: desktop_contract::LibraryKey {
                    group: desktop_contract::LibraryGroup::Installed,
                    nickname: file
                        .installed
                        .as_ref()
                        .map(|installed| installed.library.clone())
                        .unwrap_or_default(),
                },
                name: String::new(),
                reference: String::new(),
                use_as: PickUse::Copy,
                file: Some(file.clone()),
                models: draft
                    .files
                    .iter()
                    .filter(|model| {
                        model.kind == FileKind::Model3d
                            && model
                                .installed
                                .as_ref()
                                .is_some_and(|installed| installed.link.is_some())
                    })
                    .cloned()
                    .collect(),
                model_problems: Vec::new(),
                problems: Vec::new(),
                notes: Vec::new(),
            });
            let model_files: Vec<TeamFile> = draft
                .files
                .iter()
                .filter(|model| model.model_key.is_some())
                .cloned()
                .collect();
            read_file(
                ui,
                ipn.clone(),
                rows,
                key,
                file.clone(),
                copy,
                true,
                model_files,
                changed,
            );
        }
    };
    restore(symbol);
    restore(footprints);

    view! {
        {symbol.map(|rows| view! {
            <FieldRows ui=ui ipn=ipn.clone() rows=rows changed=changed for_symbol=symbol_reference new_key=Callback::new(move |_| new_key()) />
        })}
        {footprints.map(|rows| view! {
            <FieldRows ui=ui ipn=ipn.clone() rows=rows changed=changed for_symbol=symbol_reference new_key=Callback::new(move |_| new_key()) />
        })}
    }
}

/// Reads `file` for the row `key` of `rows` and makes it the row's change;
/// `keep_library` keeps the library the row's reference names (a file
/// chosen before), otherwise the one suggested.
#[allow(clippy::too_many_arguments)]
fn read_file(
    ui: TeamUi,
    ipn: String,
    rows: Rows,
    key: u32,
    file: TeamFile,
    copy: Option<TeamPick>,
    keep_library: bool,
    model_files: Vec<TeamFile>,
    changed: Callback<()>,
) {
    let Some(row) = rows.row(key) else { return };
    let current = row.reference.clone();
    let request = TeamFileRequest {
        ipn,
        kind: file.kind,
        file_name: file.file_name.clone(),
        base64: file.base64.clone(),
        current_reference: current.clone(),
    };
    let reading = ui.reading_files;
    reading.update(|count| *count += 1);
    spawn_local(async move {
        let result = backend::team_read_file(request).await;
        reading.update(|count| *count = count.saturating_sub(1));
        let found = match result {
            Ok(found) => found,
            Err(message) => {
                ui.cad_problem.set(Some((rows.kind, key, message)));
                return;
            }
        };
        let mut file = file;
        if file.kind == FileKind::Symbol {
            let name = file
                .symbol
                .clone()
                .filter(|name| found.names.contains(name))
                .unwrap_or(found.chosen.clone());
            file.symbol = Some(name);
        }
        let library = match current.split_once(':') {
            Some((library, _)) if keep_library => library.to_owned(),
            _ => found.library.clone(),
        };
        let choice = FileChoice {
            file,
            names: found.names,
            library,
            missing_models: found.missing_models.clone(),
            model_files: model_files
                .into_iter()
                .filter(|model| {
                    model
                        .model_key
                        .as_ref()
                        .is_some_and(|model_key| found.missing_models.contains(model_key))
                })
                .collect(),
            copy,
            no_models: false,
        };
        rows.set(key, |row| {
            row.reference = choice.reference();
            row.change = Change::File(Box::new(choice));
        });
        ui.cad_problem.set(None);
        changed.run(());
    });
}

/// The rows of one field, with Add another... for footprints.
#[component]
fn FieldRows(
    ui: TeamUi,
    ipn: String,
    rows: Rows,
    changed: Callback<()>,
    for_symbol: Signal<Option<String>>,
    new_key: Callback<(), u32>,
) -> impl IntoView {
    let keys = Memo::new(move |_| {
        rows.rows
            .with(|list| list.iter().map(|row| row.key).collect::<Vec<_>>())
    });
    let add = (rows.kind == PreviewKind::Footprint).then(|| {
        let ipn = ipn.clone();
        let chose = Callback::new(move |pick: TeamPickRequest| {
            let key = new_key.run(());
            rows.rows.update(|list| {
                list.push(Row {
                    key,
                    reference: format!("{}:{}", pick.library.nickname, pick.name),
                    before: String::new(),
                    change: Change::None,
                })
            });
            choose(ui, ipn.clone(), rows, key, pick, changed);
        });
        view! {
            <div class="edit-file cad-actions">
                <ChooseButton
                    id="edit-footprint-add".to_owned()
                    kind=PreviewKind::Footprint
                    what="another footprint choice".to_owned()
                    current=Signal::derive(|| None::<String>)
                    for_symbol=for_symbol
                    chose=chose
                    label="Add another…"
                />
            </div>
        }
    });
    view! {
        <For
            each=move || keys.get()
            key=|key| *key
            children=move |key| view! {
                <RowView ui=ui ipn=ipn.clone() rows=rows key=key changed=changed for_symbol=for_symbol />
            }
        />
        {add}
    }
}

/// Uses the item `pick` chosen in the Libraries browser for the row `key`.
fn choose(
    ui: TeamUi,
    ipn: String,
    rows: Rows,
    key: u32,
    pick: TeamPickRequest,
    changed: Callback<()>,
) {
    let reading = ui.reading_files;
    reading.update(|count| *count += 1);
    spawn_local(async move {
        let result = backend::team_pick(pick).await;
        reading.update(|count| *count = count.saturating_sub(1));
        let pick = match result {
            Ok(pick) => pick,
            Err(message) => {
                ui.cad_problem.set(Some((rows.kind, key, message)));
                return;
            }
        };
        ui.cad_problem.set(None);
        match (pick.use_as, pick.file.clone()) {
            (PickUse::Copy, Some(file)) => {
                read_file(
                    ui,
                    ipn,
                    rows,
                    key,
                    file,
                    Some(pick),
                    false,
                    Vec::new(),
                    changed,
                );
            }
            _ => {
                rows.set(key, |row| {
                    row.reference = pick.reference.clone();
                    row.change = Change::Pick(Box::new(pick));
                });
                changed.run(());
            }
        }
    });
}

/// One choice: its reference, read only, and what can be done with it.
#[component]
fn RowView(
    ui: TeamUi,
    ipn: String,
    rows: Rows,
    key: u32,
    changed: Callback<()>,
    for_symbol: Signal<Option<String>>,
) -> impl IntoView {
    let kind = rows.kind;
    let index = rows.index;
    let row = Memo::new(move |_| {
        rows.rows
            .with(|list| list.iter().find(|row| row.key == key).cloned())
    });
    let position = Memo::new(move |_| {
        rows.rows
            .with(|list| list.iter().position(|row| row.key == key).unwrap_or(0))
    });
    let count = Memo::new(move |_| rows.rows.with(Vec::len));
    // The first choice keeps the ids it always had.
    let id = move || match position.get() {
        0 => format!("edit-cad-{index}"),
        at => format!("edit-cad-{index}-{}", at + 1),
    };
    let label = move || match (kind, position.get()) {
        (PreviewKind::Symbol, _) => "Symbol".to_owned(),
        (PreviewKind::Footprint, 0) => "Footprint".to_owned(),
        (PreviewKind::Footprint, at) => format!("Footprint {}", at + 1),
    };
    let what = move || match (kind, position.get()) {
        (PreviewKind::Symbol, _) => "the symbol".to_owned(),
        (PreviewKind::Footprint, 0) => "the footprint".to_owned(),
        (PreviewKind::Footprint, at) => format!("footprint {}", at + 1),
    };
    let reference =
        Signal::derive(move || row.with(|row| row.as_ref().map(|row| row.reference.clone())));

    // The drawing of what was chosen; what the part has now is drawn on the
    // left, so the two show what replaces what.
    let drawing: Drawing = RwSignal::new(None);
    let drawing_busy = RwSignal::new(false);
    let draws = StoredValue::new(0u64);
    Effect::new(move |_| {
        let Some(row) = row.get() else { return };
        match &row.change {
            Change::None => {
                draws.update_value(|number| *number += 1);
                drawing.set(None);
                drawing_busy.set(false);
            }
            Change::Pick(pick) => draw_item(
                &TeamPickRequest {
                    kind,
                    library: pick.library.clone(),
                    name: pick.name.clone(),
                },
                drawing,
                drawing_busy,
                draws,
            ),
            Change::File(choice) => {
                draws.update_value(|number| *number += 1);
                let asked = draws.get_value();
                drawing_busy.set(true);
                let request = FilePreviewRequest {
                    file: choice.file.clone(),
                    reference: choice.reference(),
                    selection: None,
                };
                spawn_local(async move {
                    let result = backend::team_preview_file(request).await;
                    if draws.try_get_value() == Some(asked) {
                        drawing_busy.set(false);
                        drawing.set(Some(result));
                    }
                });
            }
        }
    });

    let chose = {
        let ipn = ipn.clone();
        Callback::new(move |pick: TeamPickRequest| {
            choose(ui, ipn.clone(), rows, key, pick, changed)
        })
    };
    let undo = move |_| {
        let Some(now) = rows.row(key) else { return };
        if now.before.is_empty() && position.get_untracked() > 0 {
            rows.rows.update(|list| list.retain(|row| row.key != key));
        } else {
            rows.set(key, |row| {
                row.reference = row.before.clone();
                row.change = Change::None;
            });
        }
        changed.run(());
    };
    let remove = move |_| {
        let at = position.get_untracked();
        rows.rows.update(|list| list.retain(|row| row.key != key));
        changed.run(());
        // Focus goes to the choice before, or to Add another...
        let next = match at {
            0 | 1 => format!("edit-cad-{index}-choose"),
            at => format!("edit-cad-{index}-{at}-choose"),
        };
        crate::team::focus_later(&next);
    };
    let file_input = {
        let ipn = ipn.clone();
        move |event: leptos::ev::Targeted<leptos::ev::Event, leptos::web_sys::HtmlInputElement>| {
            let input = event.target();
            let ipn = ipn.clone();
            spawn_local(async move {
                match read_chosen(input, file_kind(kind)).await {
                    Ok(Some((name, bytes))) => {
                        let file = TeamFile {
                            kind: file_kind(kind),
                            file_name: name,
                            base64: base64(&bytes),
                            symbol: None,
                            model_key: None,
                            replaces: None,
                            label: None,
                            installed: None,
                            no_models: false,
                        };
                        read_file(ui, ipn, rows, key, file, None, false, Vec::new(), changed);
                    }
                    Ok(None) => {}
                    Err(message) => ui.cad_problem.set(Some((kind, key, message))),
                }
            });
        }
    };
    let (extensions, _) = accepted(file_kind(kind));
    let accept = extensions.join(",");
    let field_name = if kind == PreviewKind::Symbol {
        "Symbol"
    } else {
        "Footprint"
    };
    let set_choice = move |update: Box<dyn FnOnce(&mut FileChoice)>| {
        rows.set(key, |row| {
            if let Change::File(choice) = &mut row.change {
                update(choice.as_mut());
                row.reference = choice.reference();
            }
        });
        changed.run(());
    };
    let set_choice = StoredValue::new(set_choice);

    view! {
        <div class="edit-row cad-row">
            <label for=id>{label}</label>
            <input
                id=id
                type="text"
                readonly
                spellcheck="false"
                class="cad-reference"
                title=move || reference.get().unwrap_or_default()
                prop:value=move || reference.get().unwrap_or_default()
            />
        </div>
        <div class="edit-file cad-actions">
            <div class="cad-buttons">
                <ChooseButton
                    id=Signal::derive(move || format!("{}-choose", id()))
                    kind=kind
                    what=what()
                    current=reference
                    for_symbol=for_symbol
                    chose=chose
                />
                {move || (position.get() == 0).then(|| {
                    let input_id = format!("edit-cad-file-{index}");
                    view! {
                        <input
                            type="file"
                            id=input_id.clone()
                            class="file-input visually-hidden"
                            accept=accept.clone()
                            on:change:target=file_input.clone()
                        />
                        <label for=input_id class="file-choice">
                            "Use a file…"
                            <span class="visually-hidden">{format!(" for the {field_name}")}</span>
                        </label>
                    }
                })}
                {move || (kind == PreviewKind::Footprint && count.get() > 1).then(|| view! {
                    <button type="button" id=format!("{}-remove", id()) class="file-remove" on:click=remove>
                        "Remove"
                        <span class="visually-hidden">{format!(" {}", what())}</span>
                    </button>
                })}
            </div>
            {move || ui.cad_problem.get().filter(|(problem_kind, problem_key, _)| *problem_kind == kind && *problem_key == key).map(|(_, _, message)| view! {
                <p class="problem file-problem" role="alert">{message}</p>
            })}
            {move || row.get().map(|now| match now.change {
                Change::None => ().into_any(),
                Change::Pick(pick) => {
                    let problems = pick_problems(&pick, false);
                    view! {
                        <div class="file-chosen">
                            <p class="file-name">
                                {use_words(&pick)}
                                " "
                                <button type="button" class="file-remove" on:click=undo>"Undo"</button>
                            </p>
                            {problems.into_iter().map(|problem| view! { <p class="problem file-problem" role="alert">{problem}</p> }).collect_view()}
                            <FilePreview kind=file_kind(kind) preview=drawing previewing=drawing_busy chosen=true />
                        </div>
                    }
                    .into_any()
                }
                Change::File(choice) => {
                    let symbol_id = format!("edit-cad-symbol-{index}");
                    let several = choice.names.len() > 1 && kind == PreviewKind::Symbol;
                    let current = choice.name();
                    let names = choice.names.clone();
                    let from = match &choice.copy {
                        Some(copy) => use_words(copy),
                        None => format!("From {}", choice.file.file_name),
                    };
                    let problems = choice
                        .copy
                        .as_ref()
                        .map(|copy| pick_problems(copy, choice.no_models))
                        .unwrap_or_default();
                    let copy_models = choice
                        .copy
                        .clone()
                        .filter(|_| kind == PreviewKind::Footprint)
                        .map(|copy| {
                            let no_models = choice.no_models;
                            view! {
                                <CopyModels
                                    id=format!("edit-cad-models-{index}-{}", position.get_untracked())
                                    pick=copy
                                    no_models=Signal::derive(move || no_models)
                                    changed=Callback::new(move |none: bool| set_choice.with_value(|set| set(Box::new(move |choice| choice.no_models = none))))
                                />
                            }
                        });
                    let undo_label = if choice.copy.is_some() { "Undo" } else { "Don't use the file" };
                    let missing = choice.missing_models.clone();
                    let chosen_models = choice.model_files.clone();
                    let library = choice.library.clone();
                    view! {
                        <div class="file-chosen">
                            <p class="file-name">
                                {from}
                                " "
                                <button type="button" class="file-remove" on:click=undo>{undo_label}</button>
                            </p>
                            {several.then(|| view! {
                                <div class="file-symbol">
                                    <label for=symbol_id.clone()>"Symbol in the file"</label>
                                    <select
                                        id=symbol_id
                                        on:change:target=move |event| {
                                            let name = event.target().value();
                                            set_choice.with_value(|set| set(Box::new(move |choice| choice.file.symbol = Some(name))));
                                        }
                                    >
                                        {names.iter().map(|name| {
                                            let selected = *name == current;
                                            view! { <option value=name.clone() selected=selected>{name.clone()}</option> }
                                        }).collect_view()}
                                    </select>
                                </div>
                            })}
                            <LibrarySelect
                                id=format!("edit-cad-library-{index}-{}", position.get_untracked())
                                kind=kind
                                chosen=Signal::derive(move || library.clone())
                                changed=Callback::new(move |library: String| set_choice.with_value(|set| set(Box::new(move |choice| choice.library = library))))
                            />
                            {copy_models}
                            {problems.into_iter().map(|problem| view! { <p class="problem file-problem" role="alert">{problem}</p> }).collect_view()}
                            <FilePreview kind=file_kind(kind) preview=drawing previewing=drawing_busy chosen=choice.copy.is_some() />
                            {missing.into_iter().enumerate().map(|(number, model_key)| {
                                let chosen_name = chosen_models
                                    .iter()
                                    .find(|file| file.model_key.as_deref() == Some(model_key.as_str()))
                                    .map(|file| file.file_name.clone());
                                view! {
                                    <ModelFileInput
                                        index=index
                                        number=number
                                        model_key=model_key
                                        chosen=chosen_name
                                        picked=Callback::new(move |file: TeamFile| set_choice.with_value(|set| set(Box::new(move |choice| {
                                            choice.model_files.retain(|known| known.model_key != file.model_key);
                                            choice.model_files.push(file);
                                        }))))
                                    />
                                }
                            }).collect_view()}
                        </div>
                    }
                    .into_any()
                }
            })}
        </div>
    }
}

/// A Catalog 3D model a chosen footprint names that the Catalog lacks: a
/// file input for it, or leave it out.
#[component]
fn ModelFileInput(
    index: usize,
    number: usize,
    model_key: String,
    chosen: Option<String>,
    picked: Callback<TeamFile>,
) -> impl IntoView {
    let chosen = RwSignal::new(chosen);
    let problem = RwSignal::new(None::<String>);
    let input_id = format!("edit-model-file-{index}-{number}");
    let (extensions, _) = accepted(FileKind::Model3d);
    let key = model_key.clone();
    let on_choose =
        move |event: leptos::ev::Targeted<leptos::ev::Event, leptos::web_sys::HtmlInputElement>| {
            let input = event.target();
            let key = key.clone();
            spawn_local(async move {
                match read_chosen(input, FileKind::Model3d).await {
                    Ok(Some((name, bytes))) => {
                        problem.set(None);
                        chosen.set(Some(name.clone()));
                        picked.run(TeamFile {
                            kind: FileKind::Model3d,
                            file_name: name,
                            base64: base64(&bytes),
                            symbol: None,
                            model_key: Some(key),
                            replaces: None,
                            label: None,
                            installed: None,
                            no_models: false,
                        });
                    }
                    Ok(None) => {}
                    Err(message) => problem.set(Some(message)),
                }
            });
        };
    let shown = model_key
        .rsplit('/')
        .next()
        .unwrap_or(&model_key)
        .to_owned();
    view! {
        <div class="model-file">
            <p>{format!("The footprint names the 3D model {model_key}, which the Catalog does not have.")}</p>
            <input
                type="file"
                id=input_id.clone()
                class="file-input visually-hidden"
                accept=extensions.join(",")
                on:change:target=on_choose
            />
            <label for=input_id class="file-choice">
                "Use a 3D model file…"
                <span class="visually-hidden">{format!(" for {shown}")}</span>
            </label>
            {move || match chosen.get() {
                Some(name) => view! { <p class="file-name">{format!("From {name}")}</p> }.into_any(),
                None => view! {
                    <p class="hint">"Or leave it out: the footprint is saved with a missing 3D model."</p>
                }
                .into_any(),
            }}
            {move || problem.get().map(|message| view! { <p class="problem file-problem" role="alert">{message}</p> })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_reference_is_its_own_name_in_the_library_chosen() {
        let mut choice = FileChoice {
            file: TeamFile {
                kind: FileKind::Symbol,
                file_name: "two.kicad_sym".into(),
                base64: String::new(),
                symbol: Some("R_Round".into()),
                model_key: None,
                replaces: None,
                label: None,
                installed: None,
                no_models: false,
            },
            names: vec!["R_Box".into(), "R_Round".into()],
            library: String::new(),
            missing_models: Vec::new(),
            model_files: Vec::new(),
            copy: None,
            no_models: false,
        };
        // No library chosen yet: just the name, which saving refuses.
        assert_eq!(choice.reference(), "R_Round");
        choice.library = "g-res".into();
        assert_eq!(choice.reference(), "g-res:R_Round");
        // A footprint file is named by the footprint in it.
        choice.file.symbol = None;
        choice.names = vec!["R_0603_Mine".into()];
        assert_eq!(choice.reference(), "g-res:R_0603_Mine");
    }
}

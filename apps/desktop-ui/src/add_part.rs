//! Add part (kicad-parts-9693): an engineer chooses the CAD files they
//! downloaded (a ZIP file, or the .kicad_sym, .kicad_mod and 3D model
//! files), sees the symbol and footprint drawn, confirms the category the
//! app guesses and the IPN it offers, looks at the checks, and saves the
//! new part with a reason.
//!
//! The form takes the part panel: the drawings on the left, the form on
//! the right, like Edit part.

use crate::backend;
use crate::team::{FilePreview, MAX_FILE_BYTES, RetrySlot, base64, shown};
use desktop_contract::{
    DownloadedFile, Field, FileKind, FilePreviewRequest, ManufacturerPart, PreviewKind,
    PreviewOutcome, PreviewRequest, PublishResult, TeamDownload, TeamDownloadRequest, TeamFile,
    TeamNewCategory, TeamNewPartRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The file types the file input offers.
const ACCEPTED: &str = ".zip,.kicad_sym,.kicad_mod,.step,.stp,.wrl";

type Drawing = RwSignal<Option<Result<PreviewOutcome, String>>>;

/// What is chosen and typed in the form. Values the app changes by itself
/// (the IPN offered, the references) are signals; the fields are kept here
/// so typing never redraws them.
#[derive(Clone, Copy)]
struct Form {
    reading: RwSignal<Option<TeamDownload>>,
    category: RwSignal<String>,
    ipn: RwSignal<String>,
    /// The IPN the app offered for the category, so choosing another
    /// category replaces the offer but not an IPN typed by hand.
    offered_ipn: StoredValue<String>,
    ipn_problem: RwSignal<Option<String>>,
    fields: StoredValue<Vec<Field>>,
    /// Bumped when the category changes the fields.
    fields_version: RwSignal<u32>,
    manufacturer: RwSignal<String>,
    mpn: RwSignal<String>,
    symbol: RwSignal<String>,
    offered_symbol: StoredValue<String>,
    footprint: RwSignal<String>,
    /// A footprint the Catalog or KiCad already has, used instead of the
    /// download's.
    known_footprint: RwSignal<Option<String>>,
    /// The download's 3D model the footprint uses, by its place.
    model: RwSignal<Option<usize>>,
    from: RwSignal<String>,
    reason: RwSignal<String>,
}

impl Form {
    fn new() -> Self {
        Self {
            reading: RwSignal::new(None),
            category: RwSignal::new(String::new()),
            ipn: RwSignal::new(String::new()),
            offered_ipn: StoredValue::new(String::new()),
            ipn_problem: RwSignal::new(None),
            fields: StoredValue::new(Vec::new()),
            fields_version: RwSignal::new(0),
            manufacturer: RwSignal::new(String::new()),
            mpn: RwSignal::new(String::new()),
            symbol: RwSignal::new(String::new()),
            offered_symbol: StoredValue::new(String::new()),
            footprint: RwSignal::new(String::new()),
            known_footprint: RwSignal::new(None),
            model: RwSignal::new(None),
            from: RwSignal::new(String::new()),
            reason: RwSignal::new(String::new()),
        }
    }

    fn category(&self, code: &str) -> Option<TeamNewCategory> {
        self.reading.with_untracked(|reading| {
            reading
                .as_ref()?
                .categories
                .iter()
                .find(|category| category.code == code)
                .cloned()
        })
    }

    /// Asks whether the IPN typed can be used; only the answer for the
    /// IPN still shown is kept.
    fn check_ipn(&self) {
        let form = *self;
        let ipn = form.ipn.get_untracked();
        if ipn.trim().is_empty() {
            form.ipn_problem.set(None);
            return;
        }
        spawn_local(async move {
            let answer = backend::team_check_ipn(ipn.trim().to_owned()).await;
            if form.ipn.get_untracked() == ipn {
                form.ipn_problem.set(match answer {
                    Ok(check) => check.problem,
                    Err(message) => Some(message),
                });
            }
        });
    }

    /// Moves the form to the category `code`: its fields (keeping a value
    /// typed for a field of the same name), its next free IPN and the
    /// symbol's library, unless the person typed their own.
    fn choose_category(&self, code: String) {
        let category = self.category(&code);
        let symbol_name = self.reading.with_untracked(|reading| {
            reading
                .as_ref()
                .map(|reading| reading.symbol_name.clone())
                .unwrap_or_default()
        });
        self.fields.update_value(|fields| {
            let before = std::mem::take(fields);
            *fields = category
                .as_ref()
                .map(|category| {
                    category
                        .fields
                        .iter()
                        .map(|field| {
                            before
                                .iter()
                                .find(|typed| typed.name == field.name && !typed.value.is_empty())
                                .cloned()
                                .unwrap_or_else(|| field.clone())
                        })
                        .collect()
                })
                .unwrap_or_default();
        });
        let offer = category
            .as_ref()
            .map(|category| category.next_ipn.clone())
            .unwrap_or_default();
        if self.ipn.get_untracked() == self.offered_ipn.get_value() {
            self.ipn.set(offer.clone());
        }
        self.offered_ipn.set_value(offer);
        let symbol = if code.is_empty() || symbol_name.is_empty() {
            symbol_name
        } else {
            format!("g-{code}:{symbol_name}")
        };
        if self.symbol.get_untracked() == self.offered_symbol.get_value() {
            self.symbol.set(symbol.clone());
        }
        self.offered_symbol.set_value(symbol);
        self.category.set(code);
        self.fields_version.update(|version| *version += 1);
        self.check_ipn();
    }
}

/// The files chosen in a file input, read: each name and its bytes.
/// Refuses, plainly, files too big to send together.
async fn read_files(
    input: leptos::web_sys::HtmlInputElement,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    use wasm_bindgen::JsCast;
    let unreadable = || "The files could not be read.".to_owned();
    let list = js_sys::Reflect::get(&input, &"files".into()).map_err(|_| unreadable())?;
    let count = js_sys::Reflect::get(&list, &"length".into())
        .ok()
        .and_then(|count| count.as_f64())
        .unwrap_or(0.0) as u32;
    let mut total = 0.0;
    let mut chosen = Vec::new();
    for index in 0..count {
        let file = js_sys::Reflect::get(&list, &index.into()).map_err(|_| unreadable())?;
        let name = js_sys::Reflect::get(&file, &"name".into())
            .ok()
            .and_then(|name| name.as_string())
            .unwrap_or_default();
        total += js_sys::Reflect::get(&file, &"size".into())
            .ok()
            .and_then(|size| size.as_f64())
            .unwrap_or(0.0);
        if total > MAX_FILE_BYTES {
            return Err(
                "The files are too big to send: together they can be at most 24 MB.".into(),
            );
        }
        let method = js_sys::Reflect::get(&file, &"arrayBuffer".into())
            .ok()
            .and_then(|method| method.dyn_into::<js_sys::Function>().ok())
            .ok_or_else(unreadable)?;
        let promise = method
            .call0(&file)
            .ok()
            .and_then(|promise| promise.dyn_into::<js_sys::Promise>().ok())
            .ok_or_else(unreadable)?;
        let buffer = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(|_| unreadable())?;
        chosen.push((name, js_sys::Uint8Array::new(&buffer).to_vec()));
    }
    Ok(chosen)
}

/// `Best guess: Capacitors, because footprint C_0603 matches C_*.`
fn guess_words(name: &str, reasons: &[String]) -> String {
    match reasons.first() {
        Some(reason) => format!("Best guess: {name}, because {reason}."),
        None => format!("Best guess: {name}."),
    }
}

/// The Add part form, with the drawings of the chosen files. `done` gets
/// the new part's IPN once it is saved; `open_part` an IPN to open instead,
/// with a manufacturer part to add to it in Edit part, if any.
#[component]
pub fn AddPartView(
    close: Callback<()>,
    done: Callback<String>,
    open_part: Callback<(String, Option<ManufacturerPart>)>,
) -> impl IntoView {
    let form = Form::new();
    let downloads = StoredValue::new(Vec::<DownloadedFile>::new());
    let reading_files = RwSignal::new(false);
    let read_problem = RwSignal::new(None::<String>);
    let problem = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let attempt = RetrySlot::<TeamNewPartRequest>::new();
    let symbol_drawing: Drawing = RwSignal::new(None);
    let footprint_drawing: Drawing = RwSignal::new(None);
    let symbol_busy = RwSignal::new(false);
    let footprint_busy = RwSignal::new(false);
    // Only the newest drawing asked for is shown.
    let draws = StoredValue::new(0u64);

    // Draws the chosen symbol, and the footprint: the download's, or the
    // one the Catalog or KiCad already has.
    let draw = move || {
        let Some(found) = form.reading.get_untracked() else {
            return;
        };
        draws.update_value(|number| *number += 1);
        let asked = draws.get_value();
        let symbol = form.symbol.get_untracked();
        let footprint = form.footprint.get_untracked();
        let known = form.known_footprint.get_untracked();
        match found.symbol_file.clone() {
            Some(file) => {
                symbol_busy.set(true);
                spawn_local(async move {
                    let result = backend::team_preview_file(FilePreviewRequest {
                        file,
                        reference: symbol,
                        selection: None,
                    })
                    .await;
                    if draws.get_value() == asked {
                        symbol_busy.set(false);
                        symbol_drawing.set(Some(result));
                    }
                });
            }
            None => symbol_drawing.set(None),
        }
        let footprint_file = found.footprint_file.clone();
        if known.is_none() && footprint_file.is_none() {
            footprint_drawing.set(None);
            return;
        }
        footprint_busy.set(true);
        spawn_local(async move {
            let result = match (known, footprint_file) {
                (Some(reference), _) => {
                    backend::preview(PreviewRequest {
                        ipn: "new part".into(),
                        kind: PreviewKind::Footprint,
                        field: Some(reference),
                        team_revision: None,
                        selection: None,
                    })
                    .await
                }
                (None, Some(file)) => {
                    backend::team_preview_file(FilePreviewRequest {
                        file,
                        reference: footprint,
                        selection: None,
                    })
                    .await
                }
                (None, None) => return,
            };
            if draws.get_value() == asked {
                footprint_busy.set(false);
                footprint_drawing.set(Some(result));
            }
        });
    };
    // Reads the chosen files with the symbol and footprint chosen in them;
    // `first` for newly chosen files, which also takes what the symbol
    // says and the best guess.
    let read = move |symbol: Option<usize>, footprint: Option<usize>, first: bool| {
        reading_files.set(true);
        read_problem.set(None);
        let request = TeamDownloadRequest {
            files: downloads.get_value(),
            symbol,
            footprint,
        };
        spawn_local(async move {
            let result = backend::team_read_download(request).await;
            reading_files.set(false);
            let found = match result {
                Ok(found) => found,
                Err(message) => {
                    read_problem.set(Some(message));
                    return;
                }
            };
            if first {
                form.manufacturer.set(found.manufacturer.clone());
                form.mpn.set(found.mpn.clone());
                form.from.set(found.from.clone());
                form.ipn.set(String::new());
                form.offered_ipn.set_value(String::new());
                form.fields.set_value(Vec::new());
            }
            form.symbol.set(found.symbol_name.clone());
            form.offered_symbol.set_value(found.symbol_name.clone());
            form.footprint.set(if found.footprint_library.is_empty() {
                found.footprint_name.clone()
            } else {
                format!("{}:{}", found.footprint_library, found.footprint_name)
            });
            form.known_footprint.set(None);
            form.model.set(found.model);
            let category = if first {
                found
                    .guesses
                    .first()
                    .map(|guess| guess.code.clone())
                    .unwrap_or_default()
            } else {
                form.category.get_untracked()
            };
            form.reading.set(Some(found));
            form.choose_category(category);
            draw();
        });
    };
    let on_choose =
        move |event: leptos::ev::Targeted<leptos::ev::Event, leptos::web_sys::HtmlInputElement>| {
            let input = event.target();
            spawn_local(async move {
                match read_files(input).await {
                    Ok(chosen) if chosen.is_empty() => {}
                    Ok(chosen) => {
                        downloads.set_value(
                            chosen
                                .into_iter()
                                .map(|(file_name, bytes)| DownloadedFile {
                                    file_name,
                                    base64: base64(&bytes),
                                })
                                .collect(),
                        );
                        problem.set(None);
                        attempt.open();
                        read(None, None, true);
                    }
                    Err(message) => read_problem.set(Some(message)),
                }
            });
        };
    let save = move || {
        let Some(found) = form.reading.get_untracked() else {
            return;
        };
        if form.category.get_untracked().is_empty() {
            problem.set(Some("Choose the part's category before you save.".into()));
            return;
        }
        if form.reason.get_untracked().trim().is_empty() {
            problem.set(Some(
                "Say why you are adding the part before you save.".into(),
            ));
            return;
        }
        let known = form.known_footprint.get_untracked();
        let mut files: Vec<TeamFile> = found.symbol_file.iter().cloned().collect();
        if known.is_none() {
            files.extend(found.footprint_file.iter().cloned());
            if let Some(model) = form
                .model
                .get_untracked()
                .and_then(|at| found.model_files.get(at))
            {
                files.push(model.clone());
            }
        }
        let mut request = TeamNewPartRequest {
            correlation_id: None,
            ipn: form.ipn.get_untracked().trim().to_owned(),
            reason: form.reason.get_untracked(),
            category: form.category.get_untracked(),
            fields: form.fields.get_value(),
            manufacturer_parts: vec![ManufacturerPart {
                manufacturer: form.manufacturer.get_untracked(),
                mpn: form.mpn.get_untracked(),
            }],
            symbol: form.symbol.get_untracked(),
            footprint: known.unwrap_or_else(|| form.footprint.get_untracked()),
            files,
            from: form.from.get_untracked(),
            downloads: downloads
                .with_value(|files| files.iter().map(|file| file.file_name.clone()).collect()),
        };
        request.correlation_id = Some(attempt.id_for(request.clone()));
        saving.set(true);
        problem.set(None);
        spawn_local(async move {
            let result = backend::team_add_part(request).await;
            saving.set(false);
            match result {
                Ok(PublishResult::Added { part }) => done.run(part.ipn),
                Ok(PublishResult::IpnTaken { message, next_free }) => {
                    form.ipn.set(next_free.clone());
                    form.offered_ipn.set_value(next_free);
                    form.check_ipn();
                    problem.set(Some(message));
                }
                Ok(
                    PublishResult::Refused { message }
                    | PublishResult::SavedNotShown { message }
                    | PublishResult::NothingToSave { message },
                ) => problem.set(Some(message)),
                Ok(_) => problem.set(Some(
                    "The team server gave an answer this app does not know.".into(),
                )),
                Err(message) => problem.set(Some(message)),
            }
        });
    };

    let previews = move || {
        let chosen = form.reading.with(Option::is_some);
        view! {
            <section class="preview" aria-labelledby="symbol-heading">
                <h3 id="symbol-heading">"Symbol"</h3>
                {(!chosen).then(|| view! { <p class="hint">"Choose the download to see its symbol here."</p> })}
                <FilePreview kind=FileKind::Symbol preview=symbol_drawing previewing=symbol_busy />
            </section>
            <section class="preview" aria-labelledby="footprint-heading">
                <h3 id="footprint-heading">"Footprint"</h3>
                {(!chosen).then(|| view! { <p class="hint">"Choose the download to see its footprint here."</p> })}
                <FilePreview kind=FileKind::Footprint preview=footprint_drawing previewing=footprint_busy />
            </section>
        }
    };

    view! {
        <div class="part-view add-part">
            <header class="part-header">
                <h2 id="part-title">"Add part"</h2>
                <p class="part-description">
                    "Add a part from the CAD files you downloaded from SnapEDA, Ultra Librarian, SamacSys or a manufacturer's site."
                </p>
            </header>
            <div class="part-body team">
                <div class="part-previews" role="region" aria-label="Symbol and footprint" tabindex="0">
                    {previews}
                </div>
                <section class="part-fields" aria-labelledby="add-heading" tabindex="0">
                    <form
                        class="edit-form add-form"
                        aria-labelledby="add-heading"
                        on:submit=move |event| {
                            event.prevent_default();
                            save();
                        }
                    >
                        <h3 id="add-heading" class="visually-hidden">"Add part"</h3>
                        <div class="add-files">
                            <input
                                type="file"
                                multiple
                                id="add-files"
                                class="file-input visually-hidden"
                                accept=ACCEPTED
                                on:change:target=on_choose
                            />
                            <label for="add-files" class="file-choice">
                                {move || if form.reading.with(Option::is_some) { "Choose other files…" } else { "Choose the download…" }}
                            </label>
                            <p class="hint">"A ZIP file, or its .kicad_sym, .kicad_mod and 3D model files."</p>
                            {move || reading_files.get().then(|| view! { <p class="hint" aria-busy="true">"Reading the files…"</p> })}
                            {move || read_problem.get().map(|message| view! { <p class="problem file-problem" role="alert">{message}</p> })}
                        </div>
                        {move || form.reading.get().map(|found| view! {
                            <DownloadForm
                                downloads=downloads.with_value(|files| files.iter().map(|file| file.file_name.clone()).collect())
                                found=found
                                form=form
                                read=Callback::new(move |(symbol, footprint)| read(symbol, footprint, false))
                                draw=Callback::new(move |_| draw())
                                open_part=open_part
                            />
                        })}
                        {move || problem.get().map(|message| view! { <p class="problem team-problem" role="alert">{message}</p> })}
                        <div class="form-actions">
                            <button
                                type="submit"
                                class="primary"
                                disabled=move || saving.get() || form.reading.with(Option::is_none)
                            >
                                {move || if saving.get() { "Saving…" } else { "Save" }}
                            </button>
                            <button type="button" on:click=move |_| close.run(())>"Cancel"</button>
                        </div>
                    </form>
                </section>
            </div>
        </div>
    }
}

/// The part of the form that follows what the download holds.
#[component]
fn DownloadForm(
    /// The names of the chosen files.
    downloads: Vec<String>,
    found: TeamDownload,
    form: Form,
    /// Reads the download again with another symbol and footprint chosen.
    read: Callback<(Option<usize>, Option<usize>)>,
    /// Draws the symbol and footprint again.
    draw: Callback<()>,
    open_part: Callback<(String, Option<ManufacturerPart>)>,
) -> impl IntoView {
    let names = {
        let used = found.symbol_file.iter().count()
            + found.footprint_file.iter().count()
            + found.model_files.len();
        let mut words = format!(
            "From {}: {used} KiCad {} used",
            downloads.join(", "),
            if used == 1 { "file" } else { "files" }
        );
        if found.ignored > 0 {
            words.push_str(&format!(
                ", {} other {} left out",
                found.ignored,
                if found.ignored == 1 { "file" } else { "files" }
            ));
        }
        words.push('.');
        words
    };
    let symbol_choice = (found.symbols.len() > 1).then(|| {
        let chosen = found.symbol.unwrap_or(0);
        let footprint = found.footprint;
        view! {
            <div class="edit-row">
                <label for="add-symbol-choice">"Symbol in the download"</label>
                <select
                    id="add-symbol-choice"
                    on:change:target=move |event| {
                        let at = event.target().value().parse::<usize>().ok();
                        read.run((at, footprint));
                    }
                >
                    {found.symbols.iter().enumerate().map(|(at, label)| view! {
                        <option value=at.to_string() selected=at == chosen>{label.clone()}</option>
                    }).collect_view()}
                </select>
            </div>
        }
    });
    let footprint_choice = (found.footprints.len() > 1).then(|| {
        let chosen = found.footprint.unwrap_or(0);
        let symbol = found.symbol;
        view! {
            <div class="edit-row">
                <label for="add-footprint-choice">"Footprint in the download"</label>
                <select
                    id="add-footprint-choice"
                    on:change:target=move |event| {
                        let at = event.target().value().parse::<usize>().ok();
                        read.run((symbol, at));
                    }
                >
                    {found.footprints.iter().enumerate().map(|(at, label)| view! {
                        <option value=at.to_string() selected=at == chosen>{label.clone()}</option>
                    }).collect_view()}
                </select>
            </div>
        }
    });
    let model_choice = (!found.models.is_empty() && found.footprint_file.is_some()).then(|| {
        let models = found.models.clone();
        view! {
            <div class="edit-row">
                <label for="add-model">"3D model"</label>
                <select
                    id="add-model"
                    disabled=move || form.known_footprint.with(Option::is_some)
                    on:change:target=move |event| {
                        form.model.set(event.target().value().parse::<usize>().ok());
                    }
                >
                    {models.iter().enumerate().map(|(at, label)| view! {
                        <option value=at.to_string() selected=move || form.model.get() == Some(at)>{label.clone()}</option>
                    }).collect_view()}
                    <option value="" selected=move || form.model.get().is_none()>"No 3D model"</option>
                </select>
            </div>
        }
    });

    // The checks before saving.
    let same_mpn = (!found.same_mpn.is_empty()).then(|| {
        let mpn = found.mpn.clone();
        let manufacturer = found.manufacturer.clone();
        view! {
            <li class="check check-warn" id="add-same-mpn">
                <p>{format!("The MPN {mpn} is already in the Catalog:")}</p>
                <ul>
                    {found.same_mpn.iter().map(|part| {
                        let ipn = part.ipn.clone();
                        let to_open = ipn.clone();
                        let to_add = ipn.clone();
                        let added = ManufacturerPart { manufacturer: manufacturer.clone(), mpn: mpn.clone() };
                        let listed = part.listed;
                        view! {
                            <li class="same-mpn">
                                <span>{format!("{ipn}: {}", shown(&part.description))}</span>
                                " "
                                <button type="button" on:click=move |_| open_part.run((to_open.clone(), None))>
                                    {format!("Open {ipn}")}
                                </button>
                                {(!listed).then(|| view! {
                                    <button
                                        type="button"
                                        on:click=move |_| open_part.run((to_add.clone(), Some(added.clone())))
                                    >
                                        "Add as a manufacturer part of it instead"
                                    </button>
                                })}
                            </li>
                        }
                    }).collect_view()}
                </ul>
            </li>
        }
    });
    let known = found.known_footprints.clone();
    let footprint_check = (!known.is_empty() && found.footprint_file.is_some()).then_some({
        move || match form.known_footprint.get() {
            Some(reference) => view! {
                <li class="check" id="add-known-footprint">
                    <p>{format!("Uses {reference} instead of the footprint in the download.")}</p>
                    <button
                        type="button"
                        on:click=move |_| {
                            form.known_footprint.set(None);
                            draw.run(());
                        }
                    >
                        "Use the download's footprint"
                    </button>
                </li>
            }
            .into_any(),
            None => view! {
                <li class="check check-warn" id="add-known-footprint">
                    <p>"The Catalog or KiCad already has a footprint like this one. Using it keeps the library free of near-copies:"</p>
                    <ul>
                        {known.iter().map(|footprint| {
                            let reference = footprint.reference.clone();
                            view! {
                                <li>
                                    <code>{crate::cad_reference(&footprint.reference)}</code>
                                    {format!(" ({}) ", footprint.why)}
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            form.known_footprint.set(Some(reference.clone()));
                                            draw.run(());
                                        }
                                    >
                                        "Use it"
                                    </button>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                </li>
            }
            .into_any(),
        }
    });
    let pin_check = match (found.pins, found.pads) {
        (Some(pins), Some(pads)) if pins != pads => Some(view! {
            <li class="check check-warn" id="add-pins">
                {format!("The symbol has {pins} pins but the footprint has {pads} numbered pads. Thermal and mounting pads can make this normal; check the pinout.")}
            </li>
        }.into_any()),
        (Some(pins), Some(_)) => Some(view! {
            <li class="check" id="add-pins">
                {format!("The symbol's {pins} pins match the footprint's numbered pads.")}
            </li>
        }.into_any()),
        _ => None,
    };
    let notes = found.notes.clone();

    // The category: the best guess with its reason, the next two as quick
    // choices, and every category.
    let guesses = found.guesses.clone();
    let guess_line = match guesses.first() {
        Some(best) => view! { <p class="hint" id="add-guess">{guess_words(&best.name, &best.reasons)}</p> }.into_any(),
        None => view! {
            <p class="hint" id="add-guess">"Nothing in the download says which category this part is in. Choose one."</p>
        }
        .into_any(),
    };
    let quick = (!guesses.is_empty()).then(|| {
        view! {
            <div class="guess-choices" role="group" aria-label="Likely categories">
                {guesses.iter().map(|guess| {
                    let code = guess.code.clone();
                    let pressed = code.clone();
                    let title = guess.reasons.join("; ");
                    view! {
                        <button
                            type="button"
                            class="guess-choice"
                            title=title
                            aria-pressed=move || if form.category.get() == pressed { "true" } else { "false" }
                            on:click=move |_| form.choose_category(code.clone())
                        >
                            {guess.name.clone()}
                        </button>
                    }
                }).collect_view()}
            </div>
        }
    });
    let categories = found.categories.clone();
    let category_select = view! {
        <div class="edit-row">
            <label for="add-category">"Category"</label>
            <select
                id="add-category"
                on:change:target=move |event| form.choose_category(event.target().value())
            >
                <option value="" selected=move || form.category.get().is_empty()>"Choose a category"</option>
                {categories.iter().map(|category| {
                    let code = category.code.clone();
                    view! {
                        <option value=category.code.clone() selected=move || form.category.get() == code>
                            {category.name.clone()}
                        </option>
                    }
                }).collect_view()}
            </select>
        </div>
    };
    let ipn_row = view! {
        <div class="edit-row">
            <label for="add-ipn">"IPN"</label>
            <input
                id="add-ipn"
                type="text"
                spellcheck="false"
                aria-describedby="add-ipn-hint"
                prop:value=move || form.ipn.get()
                on:input:target=move |event| {
                    form.ipn.set(event.target().value());
                    form.check_ipn();
                }
            />
        </div>
        {move || match form.ipn_problem.get() {
            Some(message) => view! { <p class="problem file-problem" id="add-ipn-hint" role="alert">{message}</p> }.into_any(),
            None => {
                let offered = form.ipn.get() == form.offered_ipn.get_value() && !form.ipn.get().is_empty();
                let text = if offered {
                    "The next free IPN in this category. An IPN is never reused."
                } else {
                    "Three capital letters, four digits and four letters or digits, such as RES-0000-4702."
                };
                view! { <p class="hint" id="add-ipn-hint">{text}</p> }.into_any()
            }
        }}
    };
    let fields = move || {
        form.fields_version.track();
        let fields = form.fields.get_value();
        if fields.is_empty() {
            return view! { <p class="hint">"Choose a category to see its fields."</p> }.into_any();
        }
        fields
            .into_iter()
            .enumerate()
            .map(|(index, field)| {
                let id = format!("add-field-{index}");
                view! {
                    <div class="edit-row">
                        <label for=id.clone()>{field.name}</label>
                        <input
                            id=id
                            type="text"
                            spellcheck="false"
                            prop:value=field.value
                            on:input:target=move |event| {
                                let value = event.target().value();
                                form.fields.update_value(|fields| {
                                    if let Some(field) = fields.get_mut(index) {
                                        field.value = value;
                                    }
                                });
                            }
                        />
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };
    let unused = move || {
        form.fields_version.track();
        let code = form.category.get_untracked();
        let unused = form
            .category(&code)
            .map(|category| category.unused)
            .unwrap_or_default();
        (!unused.is_empty()).then(|| view! {
            <p class="hint" id="add-unused-heading">"Also in the download (no field of this category holds it):"</p>
            <table class="add-unused" aria-labelledby="add-unused-heading">
                <tbody>
                    {unused.into_iter().map(|field| view! {
                        <tr><th scope="row">{field.name}</th><td>{field.value}</td></tr>
                    }).collect_view()}
                </tbody>
            </table>
        })
    };
    let library_hint = found.footprint_library.is_empty().then(|| view! {
        <p class="hint">"Type the footprint's library (its package group) before its name, such as g-Package_SO:NAME."</p>
    });
    let text_row = move |id: &'static str, label: &'static str, value: RwSignal<String>| {
        view! {
            <div class="edit-row">
                <label for=id>{label}</label>
                <input
                    id=id
                    type="text"
                    spellcheck="false"
                    prop:value=move || value.get()
                    on:input:target=move |event| value.set(event.target().value())
                />
            </div>
        }
    };
    view! {
        <p class="file-name" id="add-download">{names}</p>
        <section class="add-checks" aria-labelledby="add-checks-heading">
            <h4 id="add-checks-heading">"Before you save"</h4>
            <ul>
                {same_mpn}
                {footprint_check}
                {pin_check}
                <li class="check" id="add-license">"License: Unknown, needs review. Check the site's terms before you use the part."</li>
                {notes.into_iter().map(|note| view! { <li class="check">{note}</li> }).collect_view()}
            </ul>
        </section>
        <fieldset>
            <legend>"Category and IPN"</legend>
            {guess_line}
            {quick}
            {category_select}
            {ipn_row}
        </fieldset>
        <fieldset>
            <legend>"Part details"</legend>
            {fields}
            {unused}
        </fieldset>
        <fieldset>
            <legend>"Manufacturer part"</legend>
            {text_row("add-manufacturer", "Manufacturer", form.manufacturer)}
            {text_row("add-mpn", "MPN", form.mpn)}
        </fieldset>
        <fieldset>
            <legend>"Symbol and footprint"</legend>
            {symbol_choice}
            {text_row("add-symbol", "Symbol", form.symbol)}
            {footprint_choice}
            <div class="edit-row">
                <label for="add-footprint">"Footprint"</label>
                <input
                    id="add-footprint"
                    type="text"
                    spellcheck="false"
                    disabled=move || form.known_footprint.with(Option::is_some)
                    prop:value=move || form.known_footprint.get().unwrap_or_else(|| form.footprint.get())
                    on:input:target=move |event| form.footprint.set(event.target().value())
                />
            </div>
            {library_hint}
            {model_choice}
        </fieldset>
        {text_row("add-from", "Where from", form.from)}
        <label for="add-reason" class="reason-label">"Why are you adding it?"</label>
        <textarea
            id="add-reason"
            rows="2"
            prop:value=move || form.reason.get()
            on:input:target=move |event| form.reason.set(event.target().value())
        ></textarea>
    }
}

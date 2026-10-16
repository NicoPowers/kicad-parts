//! Add part (kicad-parts-9693): an engineer uploads the CAD files they
//! downloaded (a ZIP file, or the .kicad_sym, .kicad_mod and 3D model
//! files), or chooses a symbol or footprint From a library, sees the symbol
//! and footprint drawn, confirms the category the app guesses and the IPN it
//! offers, looks at the checks, and saves the new part with a reason.
//!
//! The form takes the part panel: the drawings on the left, the form on
//! the right, like Edit part.
//!
//! No CAD name is typed (kicad-parts-bc6f): Symbol and Footprint show the
//! chosen reference, read only, with Choose... (the Libraries browser for
//! that role). A downloaded or copied file goes into one of our libraries
//! chosen from a list. What a chosen item is used as depends on where it
//! lives: the Catalog's own is bound, ours the Catalog lacks is added as
//! committed (kicad-parts-37a7), KiCad's own is referred to, and another
//! installed library's is copied into our libraries, like a download. The
//! Library check view and the browser's Add part from it open the form
//! with one of our items chosen.

use crate::backend;
use crate::cad_choice::{
    ChooseButton, CopyModels, Drawing, LibrariesOpener, LibrarySelect, draw_item, pick_problems,
    use_words,
};
use crate::libraries::BrowseFor;
use crate::team::{FilePreview, MAX_FILE_BYTES, RetrySlot, base64, focus_later, shown};
use desktop_contract::{
    DownloadedFile, Field, FileKind, FilePreviewRequest, LibraryGroup, LibraryKey,
    ManufacturerPart, PickUse, PreviewKind, PreviewRequest, PublishResult, TeamDownload,
    TeamDownloadRequest, TeamFile, TeamLibraryChoice, TeamNewCategory, TeamNewPartRequest,
    TeamPick, TeamPickRequest, TeamSameMpn,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The file types the file input offers.
const ACCEPTED: &str = ".zip,.kicad_sym,.kicad_mod,.step,.stp,.wrl";

/// Another footprint choice, chosen in the Libraries browser.
#[derive(Clone, Debug, PartialEq)]
struct Extra {
    key: u32,
    pick: TeamPick,
    /// For a copy: the library it goes into.
    library: String,
    /// For a copy: No 3D model.
    no_models: bool,
}

impl Extra {
    fn reference(&self) -> String {
        if self.pick.use_as == PickUse::Copy {
            joined(&self.library, &self.pick.name)
        } else {
            self.pick.reference.clone()
        }
    }
}

/// `LIBRARY:NAME`, or just the name while no library is chosen.
fn joined(library: &str, name: &str) -> String {
    if library.is_empty() || name.is_empty() {
        name.to_owned()
    } else {
        format!("{library}:{name}")
    }
}

/// The item chosen in the Libraries browser for `kind`, if any.
fn picked(reading: &TeamDownload, kind: PreviewKind) -> Option<&TeamPick> {
    reading.picks.iter().find(|pick| pick.kind == kind)
}

/// The chosen item for `kind` that is used by reference (the Catalog's,
/// ours or KiCad's), if any.
fn referenced(reading: &TeamDownload, kind: PreviewKind) -> Option<&TeamPick> {
    picked(reading, kind).filter(|pick| pick.use_as != PickUse::Copy)
}

/// The Symbol reference: a chosen item's own, else the file's symbol in the
/// library chosen for it.
fn symbol_reference(reading: &TeamDownload, library: &str) -> String {
    match referenced(reading, PreviewKind::Symbol) {
        Some(pick) => pick.reference.clone(),
        None if reading.symbol_file.is_some() => joined(library, &reading.symbol_name),
        None => String::new(),
    }
}

/// The first footprint choice: a footprint the Catalog or KiCad already has
/// used instead, a chosen item's own, else the file's footprint in the
/// library chosen for it.
fn footprint_reference(reading: &TeamDownload, library: &str, known: Option<&str>) -> String {
    if let Some(known) = known {
        return known.to_owned();
    }
    match referenced(reading, PreviewKind::Footprint) {
        Some(pick) => pick.reference.clone(),
        None if reading.footprint_file.is_some() => joined(library, &reading.footprint_name),
        None => String::new(),
    }
}

/// What is chosen and typed in the form. Values the app changes by itself
/// (the IPN offered, the libraries) are signals; the fields are kept here
/// so typing never redraws them. The part panel keeps one, so selecting a
/// part while adding one keeps the form as it was; Add part shows it again.
///
/// For each value the app fills in, it remembers what it offered: reading
/// the files again (another symbol chosen) replaces only a value that is
/// still the app's own, never one the engineer typed or chose.
#[derive(Clone, Copy)]
pub struct Form {
    reading: RwSignal<Option<TeamDownload>>,
    downloads: StoredValue<Vec<DownloadedFile>>,
    reading_files: RwSignal<bool>,
    read_problem: RwSignal<Option<String>>,
    problem: RwSignal<Option<String>>,
    /// A part you already added with this IPN (a save whose answer was
    /// lost), which the form offers to open.
    already: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
    attempt: RetrySlot<TeamNewPartRequest>,
    symbol_drawing: Drawing,
    footprint_drawing: Drawing,
    symbol_busy: RwSignal<bool>,
    footprint_busy: RwSignal<bool>,
    /// Only the newest drawing asked for is shown.
    symbol_draws: StoredValue<u64>,
    footprint_draws: StoredValue<u64>,
    /// Parts that already have the MPN, and the MPN they were shown for.
    same_mpn: RwSignal<Vec<TeamSameMpn>>,
    mpn_shown: StoredValue<String>,
    mpn_checks: StoredValue<u64>,
    offered_manufacturer: StoredValue<String>,
    offered_mpn: StoredValue<String>,
    offered_from: StoredValue<String>,
    /// The fields of the chosen category as the app filled them.
    offered_fields: StoredValue<Vec<Field>>,
    category: RwSignal<String>,
    /// The category the app guessed, so another symbol chosen guesses again
    /// unless the person chose one.
    offered_category: StoredValue<String>,
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
    /// The libraries a downloaded or copied symbol and footprint go into.
    symbol_library: RwSignal<String>,
    offered_symbol_library: StoredValue<String>,
    footprint_library: RwSignal<String>,
    offered_footprint_library: StoredValue<String>,
    /// A footprint the Catalog or KiCad already has, used instead of the
    /// download's.
    known_footprint: RwSignal<Option<String>>,
    /// The download's 3D model the footprint uses, by its place; for a
    /// copied footprint, whether its 3D models are copied (`None` is No 3D
    /// model).
    model: RwSignal<Option<usize>>,
    /// For a footprint copied from an installed library: No 3D model.
    no_models: RwSignal<bool>,
    from: RwSignal<String>,
    reason: RwSignal<String>,
    /// The symbol and footprint chosen in the Libraries browser.
    picks: RwSignal<Vec<TeamPickRequest>>,
    /// Bumped for each new start from a library, and the last one read, so
    /// the form reads each once, also when the Library check view opens it.
    picks_asked: RwSignal<u32>,
    picks_read: StoredValue<u32>,
    /// More footprint choices.
    extras: RwSignal<Vec<Extra>>,
    extra_keys: StoredValue<u32>,
    /// The element to give keyboard focus once the files are read again,
    /// as the form is drawn anew.
    focus_after: StoredValue<Option<String>>,
}

impl Default for Form {
    fn default() -> Self {
        Self::new()
    }
}

impl Form {
    pub fn new() -> Self {
        Self {
            reading: RwSignal::new(None),
            downloads: StoredValue::new(Vec::new()),
            reading_files: RwSignal::new(false),
            read_problem: RwSignal::new(None),
            problem: RwSignal::new(None),
            already: RwSignal::new(None),
            saving: RwSignal::new(false),
            attempt: RetrySlot::new(),
            symbol_drawing: RwSignal::new(None),
            footprint_drawing: RwSignal::new(None),
            symbol_busy: RwSignal::new(false),
            footprint_busy: RwSignal::new(false),
            symbol_draws: StoredValue::new(0),
            footprint_draws: StoredValue::new(0),
            same_mpn: RwSignal::new(Vec::new()),
            mpn_shown: StoredValue::new(String::new()),
            mpn_checks: StoredValue::new(0),
            offered_manufacturer: StoredValue::new(String::new()),
            offered_mpn: StoredValue::new(String::new()),
            offered_from: StoredValue::new(String::new()),
            offered_fields: StoredValue::new(Vec::new()),
            category: RwSignal::new(String::new()),
            offered_category: StoredValue::new(String::new()),
            ipn: RwSignal::new(String::new()),
            offered_ipn: StoredValue::new(String::new()),
            ipn_problem: RwSignal::new(None),
            fields: StoredValue::new(Vec::new()),
            fields_version: RwSignal::new(0),
            manufacturer: RwSignal::new(String::new()),
            mpn: RwSignal::new(String::new()),
            symbol_library: RwSignal::new(String::new()),
            offered_symbol_library: StoredValue::new(String::new()),
            footprint_library: RwSignal::new(String::new()),
            offered_footprint_library: StoredValue::new(String::new()),
            known_footprint: RwSignal::new(None),
            model: RwSignal::new(None),
            no_models: RwSignal::new(false),
            from: RwSignal::new(String::new()),
            reason: RwSignal::new(String::new()),
            picks: RwSignal::new(Vec::new()),
            picks_asked: RwSignal::new(0),
            picks_read: StoredValue::new(0),
            extras: RwSignal::new(Vec::new()),
            extra_keys: StoredValue::new(0),
            focus_after: StoredValue::new(None),
        }
    }

    /// Starts again from `choice` in our libraries, as the Library check
    /// view's Add part from it does.
    pub fn start_from_library(&self, choice: TeamLibraryChoice) {
        let pick = |kind: PreviewKind, reference: &Option<String>| {
            let (library, name) = reference.as_deref()?.split_once(':')?;
            Some(TeamPickRequest {
                kind,
                library: LibraryKey {
                    group: LibraryGroup::Ours,
                    nickname: library.to_owned(),
                },
                name: name.to_owned(),
            })
        };
        let picks = [
            pick(PreviewKind::Symbol, &choice.symbol),
            pick(PreviewKind::Footprint, &choice.footprint),
        ]
        .into_iter()
        .flatten()
        .collect();
        self.start_from_picks(picks);
    }

    /// Starts again from items chosen in the Libraries browser (once the
    /// form is shown).
    fn start_from_picks(&self, picks: Vec<TeamPickRequest>) {
        self.reset();
        self.picks.set(picks);
        self.picks_asked.update(|asked| *asked += 1);
    }

    /// Empties the form, after a save or Cancel.
    pub fn reset(&self) {
        self.reading.set(None);
        self.downloads.set_value(Vec::new());
        self.read_problem.set(None);
        self.problem.set(None);
        self.already.set(None);
        self.symbol_drawing.set(None);
        self.footprint_drawing.set(None);
        self.same_mpn.set(Vec::new());
        self.category.set(String::new());
        for value in [
            self.ipn,
            self.manufacturer,
            self.mpn,
            self.symbol_library,
            self.footprint_library,
            self.from,
            self.reason,
        ] {
            value.set(String::new());
        }
        for offered in [
            self.offered_ipn,
            self.offered_symbol_library,
            self.offered_footprint_library,
            self.offered_manufacturer,
            self.offered_mpn,
            self.offered_from,
            self.offered_category,
            self.mpn_shown,
        ] {
            offered.set_value(String::new());
        }
        self.fields.set_value(Vec::new());
        self.offered_fields.set_value(Vec::new());
        self.known_footprint.set(None);
        self.model.set(None);
        self.no_models.set(false);
        self.picks.set(Vec::new());
        self.extras.set(Vec::new());
        self.focus_after.set_value(None);
        self.attempt.open();
    }

    /// Starts again for newly chosen files: nothing typed for the earlier
    /// files is kept, but the files themselves are, and the items chosen.
    fn reset_for_new_files(&self) {
        let downloads = self.downloads.get_value();
        let picks = self.picks.get_untracked();
        let focus = self.focus_after.get_value();
        self.reset();
        self.downloads.set_value(downloads);
        self.picks.set(picks);
        self.focus_after.set_value(focus);
    }

    /// Sets `value` to `new` if it is still what the app offered before (or
    /// blank), and remembers `new` as the app's offer.
    fn offer(value: RwSignal<String>, offered: StoredValue<String>, new: &str) {
        let now = value.get_untracked();
        if now == offered.get_value() || now.is_empty() {
            value.set(new.to_owned());
        }
        offered.set_value(new.to_owned());
    }

    /// Asks, a moment after the last key, which parts already have the MPN
    /// typed.
    fn check_mpn_soon(&self) {
        let form = *self;
        form.mpn_checks.update_value(|number| *number += 1);
        let asked = form.mpn_checks.get_value();
        set_timeout(
            move || {
                if form.mpn_checks.get_value() != asked {
                    return;
                }
                let mpn = form.mpn.get_untracked();
                let manufacturer = form.manufacturer.get_untracked();
                spawn_local(async move {
                    if let Ok(parts) = backend::team_check_mpn(mpn.clone(), manufacturer).await {
                        if form.mpn.get_untracked() == mpn {
                            form.mpn_shown.set_value(mpn);
                            form.same_mpn.set(parts);
                        }
                    }
                });
            },
            std::time::Duration::from_millis(300),
        );
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
        let prefix = form
            .category(&form.category.get_untracked())
            .map(|category| category.ipn_prefix)
            .unwrap_or_default();
        spawn_local(async move {
            let answer = backend::team_check_ipn(ipn.trim().to_owned(), prefix).await;
            if form.ipn.get_untracked() == ipn {
                form.ipn_problem.set(match answer {
                    Ok(check) => check.problem,
                    Err(message) => Some(message),
                });
            }
        });
    }

    /// Moves the form to the category `code`: its fields as the files fill
    /// them (keeping a value typed for a field of the same name), its next
    /// free IPN, and the symbol library and footprint group it suggests,
    /// unless the person chose their own.
    fn choose_category(&self, code: String) {
        let category = self.category(&code);
        let offered = self.offered_fields.get_value();
        self.fields.update_value(|fields| {
            let before = std::mem::take(fields);
            *fields = category
                .as_ref()
                .map(|category| {
                    category
                        .fields
                        .iter()
                        .map(|field| {
                            // A value the person typed is one that is not
                            // blank and not what the app filled in.
                            let typed = before.iter().find(|typed| {
                                typed.name == field.name
                                    && !typed.value.is_empty()
                                    && !offered.iter().any(|offer| {
                                        offer.name == typed.name && offer.value == typed.value
                                    })
                            });
                            typed.cloned().unwrap_or_else(|| field.clone())
                        })
                        .collect()
                })
                .unwrap_or_default();
        });
        self.offered_fields.set_value(
            category
                .as_ref()
                .map(|category| category.fields.clone())
                .unwrap_or_default(),
        );
        let offer = category
            .as_ref()
            .map(|category| category.next_ipn.clone())
            .unwrap_or_default();
        if self.ipn.get_untracked() == self.offered_ipn.get_value() {
            self.ipn.set(offer.clone());
        }
        self.offered_ipn.set_value(offer);
        // The category's symbol library, and the footprint group the files
        // or the category suggest.
        let symbol_library = if code.is_empty() {
            String::new()
        } else {
            format!("g-{code}")
        };
        Form::offer(
            self.symbol_library,
            self.offered_symbol_library,
            &symbol_library,
        );
        let footprint_library = category
            .as_ref()
            .map(|category| category.footprint_library.clone())
            .unwrap_or_default();
        Form::offer(
            self.footprint_library,
            self.offered_footprint_library,
            &footprint_library,
        );
        self.category.set(code);
        self.fields_version.update(|version| *version += 1);
        self.check_ipn();
    }

    /// Uses `pick` for its role, keeping the other role's choice.
    fn choose(&self, pick: TeamPickRequest) {
        let kind = pick.kind;
        self.picks.update(|picks| {
            picks.retain(|known| known.kind != kind);
            picks.push(pick);
        });
        if kind == PreviewKind::Footprint {
            self.known_footprint.set(None);
            self.no_models.set(false);
        }
        self.focus_after.set_value(Some(match kind {
            PreviewKind::Symbol => "add-symbol-choose".to_owned(),
            PreviewKind::Footprint => "add-footprint-choose".to_owned(),
        }));
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

/// Draws the symbol or footprint `file` as it would be saved as
/// `reference`, into `drawing`; only the newest drawing asked for through
/// `draws` is shown.
fn draw_file(
    file: TeamFile,
    reference: String,
    drawing: Drawing,
    busy: RwSignal<bool>,
    draws: StoredValue<u64>,
) {
    draws.update_value(|number| *number += 1);
    let asked = draws.get_value();
    busy.set(true);
    spawn_local(async move {
        let result = backend::team_preview_file(FilePreviewRequest {
            file,
            reference,
            selection: None,
        })
        .await;
        if draws.get_value() == asked {
            busy.set(false);
            drawing.set(Some(result));
        }
    });
}

/// The Add part form, with the drawings of the chosen files. `done` gets
/// the new part's IPN once it is saved; `open_part` an IPN to open instead,
/// with a manufacturer part to add to it in Edit part, if any.
#[component]
pub fn AddPartView(
    form: Form,
    close: Callback<()>,
    done: Callback<String>,
    open_part: Callback<(String, Option<ManufacturerPart>)>,
) -> impl IntoView {
    let Form {
        downloads,
        reading_files,
        read_problem,
        problem,
        saving,
        attempt,
        symbol_drawing,
        footprint_drawing,
        symbol_busy,
        footprint_busy,
        symbol_draws,
        footprint_draws,
        ..
    } = form;
    let opener = use_context::<LibrariesOpener>();
    // Keyboard and screen reader users land in the form.
    focus_later("add-files");

    // Draws the chosen symbol and footprint: an item of a library as it is
    // there, a file as it would be saved, or the footprint the Catalog or
    // KiCad already has.
    let draw = move || {
        let Some(found) = form.reading.get_untracked() else {
            return;
        };
        let symbol = symbol_reference(&found, &form.symbol_library.get_untracked());
        let known = form.known_footprint.get_untracked();
        let footprint = footprint_reference(
            &found,
            &form.footprint_library.get_untracked(),
            known.as_deref(),
        );
        match (
            referenced(&found, PreviewKind::Symbol),
            found.symbol_file.clone(),
        ) {
            (Some(pick), _) => draw_item(
                &TeamPickRequest {
                    kind: PreviewKind::Symbol,
                    library: pick.library.clone(),
                    name: pick.name.clone(),
                },
                pick.use_as == PickUse::Catalog,
                symbol_drawing,
                symbol_busy,
                symbol_draws,
            ),
            (None, Some(file)) => {
                draw_file(file, symbol, symbol_drawing, symbol_busy, symbol_draws)
            }
            (None, None) => {
                symbol_draws.update_value(|number| *number += 1);
                symbol_busy.set(false);
                symbol_drawing.set(None);
            }
        }
        let footprint_pick = referenced(&found, PreviewKind::Footprint).cloned();
        match (known, footprint_pick, found.footprint_file.clone()) {
            (Some(reference), _, _) => {
                footprint_draws.update_value(|number| *number += 1);
                let asked = footprint_draws.get_value();
                footprint_busy.set(true);
                spawn_local(async move {
                    let result = backend::preview(PreviewRequest {
                        ipn: "new part".into(),
                        kind: PreviewKind::Footprint,
                        field: Some(reference),
                        team_revision: None,
                        selection: None,
                    })
                    .await;
                    if footprint_draws.get_value() == asked {
                        footprint_busy.set(false);
                        footprint_drawing.set(Some(result));
                    }
                });
            }
            (None, Some(pick), _) => draw_item(
                &TeamPickRequest {
                    kind: PreviewKind::Footprint,
                    library: pick.library.clone(),
                    name: pick.name.clone(),
                },
                pick.use_as == PickUse::Catalog,
                footprint_drawing,
                footprint_busy,
                footprint_draws,
            ),
            (None, None, Some(file)) => draw_file(
                file,
                footprint,
                footprint_drawing,
                footprint_busy,
                footprint_draws,
            ),
            (None, None, None) => {
                footprint_draws.update_value(|number| *number += 1);
                footprint_busy.set(false);
                footprint_drawing.set(None);
            }
        }
    };
    // Reads the chosen files and items with the symbol and footprint chosen
    // in a download; `first` for newly chosen files, which also takes what
    // the symbol says and the best guess.
    let read = move |symbol: Option<usize>, footprint: Option<usize>, first: bool| {
        reading_files.set(true);
        read_problem.set(None);
        let request = TeamDownloadRequest {
            files: downloads.get_value(),
            symbol,
            footprint,
            // The 3D models do not change with the symbol or footprint.
            without_models: !first,
            picks: form.picks.get_untracked(),
        };
        spawn_local(async move {
            let result = backend::team_read_download(request).await;
            reading_files.set(false);
            let mut found = match result {
                Ok(found) => found,
                Err(message) => {
                    read_problem.set(Some(message));
                    return;
                }
            };
            let footprint_picked = picked(&found, PreviewKind::Footprint).is_some();
            if first {
                form.reset_for_new_files();
            } else if let Some(before) = form.reading.get_untracked() {
                // A download's 3D models were not sent again.
                if !footprint_picked && picked(&before, PreviewKind::Footprint).is_none() {
                    found.model_files = before.model_files;
                }
            }
            // What the reading chose stays chosen, such as the footprint a
            // chosen symbol names.
            form.picks.set(
                found
                    .picks
                    .iter()
                    .map(|pick| TeamPickRequest {
                        kind: pick.kind,
                        library: pick.library.clone(),
                        name: pick.name.clone(),
                    })
                    .collect(),
            );
            // With a download, what it says stays when another symbol or
            // footprint is chosen: it is still the same part.
            let keep = !first && !downloads.with_value(Vec::is_empty);
            // Only values that are still the app's own are replaced.
            if !keep {
                Form::offer(
                    form.manufacturer,
                    form.offered_manufacturer,
                    &found.manufacturer,
                );
                Form::offer(form.mpn, form.offered_mpn, &found.mpn);
                Form::offer(form.from, form.offered_from, &found.from);
            }
            // A known footprint chosen stays while it still matches.
            if form.known_footprint.with_untracked(|known| {
                known.as_ref().is_some_and(|known| {
                    !found
                        .known_footprints
                        .iter()
                        .any(|offer| &offer.reference == known)
                })
            }) {
                form.known_footprint.set(None);
            }
            if first
                || footprint_picked
                || form
                    .model
                    .get_untracked()
                    .is_none_or(|at| at >= found.models.len())
            {
                form.model.set(found.model);
            }
            if !keep {
                form.mpn_shown.set_value(form.mpn.get_untracked());
                if form.mpn.get_untracked() == found.mpn {
                    form.same_mpn.set(found.same_mpn.clone());
                } else {
                    form.check_mpn_soon();
                }
            }
            // The best guess, unless the person chose a category or a
            // download says what the part is.
            let guess = found
                .guesses
                .first()
                .map(|guess| guess.code.clone())
                .unwrap_or_default();
            let now = form.category.get_untracked();
            let guessed = first || now.is_empty() || now == form.offered_category.get_value();
            let category = if guessed && !keep { guess.clone() } else { now };
            if !keep {
                form.offered_category.set_value(guess);
            }
            form.reading.set(Some(found));
            if keep {
                // The fields typed or filled from the download stay.
                form.fields_version.update(|version| *version += 1);
            } else {
                form.choose_category(category);
            }
            draw();
            if let Some(id) = form.focus_after.try_update_value(Option::take).flatten() {
                focus_later(&id);
            }
        });
    };
    let on_choose =
        move |event: leptos::ev::Targeted<leptos::ev::Event, leptos::web_sys::HtmlInputElement>| {
            let input = event.target();
            spawn_local(async move {
                match read_files(input).await {
                    Ok(chosen) if chosen.is_empty() => {}
                    Ok(chosen) => {
                        // A download replaces the items chosen in a library.
                        form.reset();
                        downloads.set_value(
                            chosen
                                .into_iter()
                                .map(|(file_name, bytes)| DownloadedFile {
                                    file_name,
                                    base64: base64(&bytes),
                                })
                                .collect(),
                        );
                        read(None, None, true);
                    }
                    Err(message) => read_problem.set(Some(message)),
                }
            });
        };
    // From a library: the browser over every library, both tabs; the part
    // starts from the item chosen.
    let from_library = move |_| {
        if let Some(opener) = opener {
            opener.0.set(Some(BrowseFor {
                role: None,
                current: None,
                for_symbol: None,
                choose: Some(Callback::new(move |pick: TeamPickRequest| {
                    form.start_from_picks(vec![pick]);
                })),
                return_focus: Some("add-from-library".into()),
            }));
        }
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
        let symbol_library = form.symbol_library.get_untracked();
        let footprint_library = form.footprint_library.get_untracked();
        let extras = form.extras.get_untracked();
        // A file goes into one of our libraries, chosen from the list.
        let symbol_file = found
            .symbol_file
            .clone()
            .filter(|_| referenced(&found, PreviewKind::Symbol).is_none());
        let footprint_file = found
            .footprint_file
            .clone()
            .filter(|_| known.is_none() && referenced(&found, PreviewKind::Footprint).is_none());
        let unchosen = if symbol_file.is_some() && symbol_library.is_empty() {
            Some("symbol")
        } else if (footprint_file.is_some() && footprint_library.is_empty())
            || extras
                .iter()
                .any(|extra| extra.pick.use_as == PickUse::Copy && extra.library.is_empty())
        {
            Some("footprint")
        } else {
            None
        };
        if let Some(what) = unchosen {
            problem.set(Some(format!(
                "Choose a library for the {what} before you save."
            )));
            return;
        }
        let mut files: Vec<TeamFile> = symbol_file.into_iter().collect();
        let copied_footprint =
            picked(&found, PreviewKind::Footprint).is_some_and(|pick| pick.use_as == PickUse::Copy);
        if let Some(mut file) = footprint_file {
            let model = form.model.get_untracked();
            if copied_footprint {
                let no_models = form.no_models.get_untracked();
                file.no_models = no_models;
                // It is the first footprint choice.
                file.choice = Some(0);
                files.push(file);
                if !no_models {
                    files.extend(found.model_files.iter().cloned());
                }
            } else {
                files.push(file);
                if let Some(model) = model.and_then(|at| found.model_files.get(at)) {
                    files.push(model.clone());
                }
            }
        }
        let mut footprints = vec![footprint_reference(
            &found,
            &footprint_library,
            known.as_deref(),
        )];
        for extra in &extras {
            // Each copy names the choice it is for, counted as the field
            // lists them (a blank first choice is left out).
            let choice = footprints
                .iter()
                .filter(|reference| !reference.is_empty())
                .count();
            footprints.push(extra.reference());
            if let (PickUse::Copy, Some(mut file)) = (extra.pick.use_as, extra.pick.file.clone()) {
                file.no_models = extra.no_models;
                file.choice = Some(choice);
                files.push(file);
                if !extra.no_models {
                    files.extend(extra.pick.models.iter().cloned());
                }
            }
        }
        let from = if found.picks.is_empty() {
            form.from.get_untracked()
        } else {
            found.from.clone()
        };
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
            symbol: symbol_reference(&found, &symbol_library),
            footprint: footprints
                .into_iter()
                .filter(|reference| !reference.is_empty())
                .collect::<Vec<_>>()
                .join(";"),
            files,
            from,
            downloads: downloads
                .with_value(|files| files.iter().map(|file| file.file_name.clone()).collect()),
        };
        request.correlation_id = Some(attempt.id_for(request.clone()));
        saving.set(true);
        problem.set(None);
        form.already.set(None);
        spawn_local(async move {
            // Once more just before saving: a part with this MPN may be new,
            // or the MPN typed was not checked yet. Saving again goes on.
            let mpn = form.mpn.get_untracked().trim().to_owned();
            if !mpn.is_empty() && form.mpn_shown.get_value().trim() != mpn {
                let manufacturer = form.manufacturer.get_untracked();
                if let Ok(parts) = backend::team_check_mpn(mpn.clone(), manufacturer).await {
                    form.mpn_shown.set_value(mpn.clone());
                    if !parts.is_empty() {
                        form.same_mpn.set(parts);
                        saving.set(false);
                        problem.set(Some(format!(
                            "The MPN {mpn} is already in the Catalog; see Before you save. To add this part anyway, save again."
                        )));
                        return;
                    }
                    form.same_mpn.set(parts);
                }
            }
            let result = backend::team_add_part(request).await;
            saving.set(false);
            match result {
                Ok(PublishResult::Added { part }) => {
                    form.reset();
                    done.run(part.ipn);
                }
                Ok(PublishResult::IpnTaken {
                    message,
                    next_free,
                    ipn,
                    yours,
                }) => {
                    // Open the part you already added, or use the next
                    // free IPN for another part.
                    if yours {
                        form.already.set(Some(ipn));
                    }
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

    // What is said about a chosen item and stops the save turns Save off:
    // its problems, and a copied footprint's unreadable 3D models unless No
    // 3D model is chosen.
    let blocked = Memo::new(move |_| {
        let no_models = form.no_models.get();
        let chosen = form.reading.with(|reading| {
            reading.as_ref().is_some_and(|reading| {
                reading.picks.iter().any(|pick| {
                    !pick_problems(pick, pick.kind == PreviewKind::Footprint && no_models)
                        .is_empty()
                })
            })
        });
        chosen
            || form.extras.with(|extras| {
                extras
                    .iter()
                    .any(|extra| !pick_problems(&extra.pick, extra.no_models).is_empty())
            })
    });

    // Each new start from a library is read once.
    Effect::new(move |_| {
        let asked = form.picks_asked.get();
        if asked == form.picks_read.get_value() || form.picks.with_untracked(Vec::is_empty) {
            return;
        }
        form.picks_read.set_value(asked);
        read(None, None, true);
    });

    // The footprint's 3D view uses the 3D models saving would use: a
    // copy's own models, or the download's chosen model.
    let copied_models = Signal::derive(move || {
        form.reading.with(|found| {
            let Some(found) = found else {
                return Vec::new();
            };
            let copied = picked(found, PreviewKind::Footprint)
                .is_some_and(|pick| pick.use_as == PickUse::Copy);
            if copied && !form.no_models.get() {
                found.model_files.clone()
            } else {
                Vec::new()
            }
        })
    });
    let download_model = Signal::derive(move || {
        form.reading.with(|found| {
            let found = found.as_ref()?;
            if picked(found, PreviewKind::Footprint).is_some() {
                return None;
            }
            form.model
                .get()
                .and_then(|at| found.model_files.get(at).cloned())
        })
    });
    let from_download = Signal::derive(move || {
        form.reading.with(|found| {
            found
                .as_ref()
                .is_some_and(|found| picked(found, PreviewKind::Footprint).is_none())
        })
    });
    let previews = move || {
        let chosen = form.reading.with(Option::is_some);
        view! {
            <section class="preview" aria-labelledby="symbol-heading">
                <h3 id="symbol-heading">"Symbol"</h3>
                {(!chosen).then(|| view! { <p class="hint">"Upload a download or choose from a library to see its symbol here."</p> })}
                <FilePreview kind=FileKind::Symbol preview=symbol_drawing previewing=symbol_busy />
            </section>
            <section class="preview" aria-labelledby="footprint-heading">
                <h3 id="footprint-heading">"Footprint"</h3>
                {(!chosen).then(|| view! { <p class="hint">"Upload a download or choose from a library to see its footprint here."</p> })}
                <FilePreview kind=FileKind::Footprint preview=footprint_drawing previewing=footprint_busy models=copied_models download_model=download_model from_download=from_download />
            </section>
        }
    };

    view! {
        <div class="part-view add-part">
            <header class="part-header">
                <h2 id="part-title">"Add part"</h2>
                <p class="part-description">
                    "Add a part from the CAD files you downloaded from SnapEDA, Ultra Librarian, SamacSys or a manufacturer's site, or from a symbol or footprint in a library."
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
                            <div class="add-sources">
                                <label for="add-files" class="file-choice">
                                    {move || if form.reading.with(|reading| reading.is_some()) && !downloads.with_value(Vec::is_empty) { "Upload other files…" } else { "Upload a ZIP…" }}
                                </label>
                                <button type="button" id="add-from-library" on:click=from_library>
                                    "From a library…"
                                </button>
                            </div>
                            <p class="hint">"A ZIP file, or its .kicad_sym, .kicad_mod and 3D model files; or a symbol or footprint from our libraries, KiCad's or another installed library."</p>
                            {move || reading_files.get().then(|| view! { <p class="hint" aria-busy="true">"Reading…"</p> })}
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
                        {move || form.already.get().map(|ipn| {
                            let to_open = ipn.clone();
                            view! {
                                <p>
                                    <button
                                        type="button"
                                        id="add-open-already"
                                        on:click=move |_| {
                                            form.reset();
                                            open_part.run((to_open.clone(), None));
                                        }
                                    >
                                        {format!("Open {ipn}")}
                                    </button>
                                </p>
                            }
                        })}
                        {move || blocked.get().then(|| view! {
                            <p class="problem cad-blocked" id="add-blocked">
                                "Save is off until what is said in Before you save or under Footprint is fixed."
                            </p>
                        })}
                        <div class="form-actions">
                            <button
                                type="submit"
                                class="primary"
                                disabled=move || saving.get() || form.reading.with(Option::is_none) || blocked.get()
                            >
                                {move || if saving.get() { "Saving…" } else { "Save" }}
                            </button>
                            <button
                                type="button"
                                on:click=move |_| {
                                    form.reset();
                                    close.run(());
                                }
                            >
                                "Cancel"
                            </button>
                        </div>
                    </form>
                </section>
            </div>
        </div>
    }
}

/// `note` with the first of `references` it names shown as a reference that
/// breaks only after its colon.
fn note_with_reference(note: &str, references: &[String]) -> AnyView {
    match references
        .iter()
        .find_map(|reference| note.find(reference.as_str()).map(|at| (reference, at)))
    {
        Some((reference, at)) => {
            let before = note[..at].to_owned();
            let after = note[at + reference.len()..].to_owned();
            view! {
                {before}
                <span class="nowrap-reference">{crate::cad_reference(reference)}</span>
                {after}
            }
            .into_any()
        }
        None => note.to_owned().into_any(),
    }
}

/// The part of the form that follows what the files and items hold.
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
    let symbol_pick = picked(&found, PreviewKind::Symbol).cloned();
    let footprint_pick = picked(&found, PreviewKind::Footprint).cloned();
    let symbol_chosen = symbol_pick.is_some();
    let any_pick = !found.picks.is_empty();
    // The references break after their colon, never at a hyphen.
    let names = if any_pick && downloads.is_empty() {
        let items: Vec<String> = found
            .picks
            .iter()
            .map(|pick| pick.reference.clone())
            .collect();
        let count = items.len();
        view! {
            "From the libraries: "
            {items.into_iter().enumerate().map(|(at, item)| view! {
                {(at > 0).then_some(if at + 1 == count { " and " } else { ", " })}
                <span class="nowrap-reference">{crate::cad_reference(&item)}</span>
            }).collect_view()}
            "."
        }
        .into_any()
    } else {
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
        words.into_any()
    };
    let symbol_choice = (found.symbols.len() > 1 && !symbol_chosen).then(|| {
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
    let footprint_choice = (found.footprints.len() > 1 && footprint_pick.is_none()).then(|| {
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
    // A copied footprint's 3D models are all copied, or none (No 3D model).
    let copy_models = footprint_pick
        .clone()
        .filter(|pick| pick.use_as == PickUse::Copy)
        .map(|pick| {
            view! {
                <CopyModels
                    id="add-copy-models".to_owned()
                    pick=pick
                    no_models=Signal::derive(move || form.no_models.get())
                    changed=Callback::new(move |none: bool| form.no_models.set(none))
                />
            }
        });
    let model_choice = (!found.models.is_empty()
        && found.footprint_file.is_some()
        && copy_models.is_none())
    .then(|| {
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
    let same_mpn = move || {
        let parts = form.same_mpn.get();
        if parts.is_empty() {
            return ().into_any();
        }
        let mpn = form.mpn.get_untracked().trim().to_owned();
        let manufacturer = form.manufacturer.get_untracked().trim().to_owned();
        view! {
            <li class="check check-warn" id="add-same-mpn">
                <p>{format!("The MPN {mpn} is already in the Catalog:")}</p>
                <ul>
                    {parts.iter().map(|part| {
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
        .into_any()
    };
    let known = found.known_footprints.clone();
    let footprint_check = (!known.is_empty() && found.footprint_file.is_some()).then_some({
        move || match form.known_footprint.get() {
            Some(reference) => view! {
                <li class="check" id="add-known-footprint">
                    <p>{format!("Uses {reference} instead of the chosen footprint.")}</p>
                    <button
                        type="button"
                        on:click=move |_| {
                            form.known_footprint.set(None);
                            draw.run(());
                        }
                    >
                        "Use the chosen footprint"
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
    // What stops the save for the chosen items. A copied footprint's
    // unreadable 3D models do not stop it once No 3D model is chosen.
    let chosen_problems = {
        let picks = found.picks.clone();
        move || {
            let no_models = form.no_models.get();
            picks
                .iter()
                .flat_map(|pick| {
                    pick_problems(pick, pick.kind == PreviewKind::Footprint && no_models)
                })
                .collect::<Vec<_>>()
        }
    };
    // What else is known about them, after the line on how each is used.
    let findings: Vec<String> = found
        .picks
        .iter()
        .flat_map(|pick| pick.notes.iter().skip(1).cloned())
        .collect();
    let references: Vec<String> = found
        .picks
        .iter()
        .map(|pick| pick.reference.clone())
        .collect();
    let sends_files = found.symbol_file.is_some() || found.footprint_file.is_some();
    let copied = found.picks.iter().any(|pick| pick.use_as == PickUse::Copy);
    let ours = found.picks.iter().any(|pick| pick.use_as == PickUse::Ours);
    let license = if !sends_files && !ours {
        None
    } else if copied {
        Some(
            "License: Unknown, needs review. Check the installed library's terms before you use the part.",
        )
    } else if any_pick && downloads.is_empty() {
        Some("License: Unknown, needs review.")
    } else {
        Some("License: Unknown, needs review. Check the site's terms before you use the part.")
    };

    // The category: the best guess with its reason, the next two as quick
    // choices, and every category.
    let guesses = found.guesses.clone();
    let guess_line = match guesses.first() {
        Some(best) => view! { <p class="hint" id="add-guess">{guess_words(&best.name, &best.reasons)}</p> }.into_any(),
        None => view! {
            <p class="hint" id="add-guess">
                {if symbol_chosen { "Nothing in the symbol or footprint says which category this part is in. Choose one." } else { "Nothing in the download says which category this part is in. Choose one." }}
            </p>
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
            <p class="hint" id="add-unused-heading">{if symbol_chosen { "Also in the symbol (no field of this category holds it):" } else { "Also in the download (no field of this category holds it):" }}</p>
            <table class="add-unused" aria-labelledby="add-unused-heading">
                <tbody>
                    {unused.into_iter().map(|field| view! {
                        <tr><th scope="row">{field.name}</th><td>{field.value}</td></tr>
                    }).collect_view()}
                </tbody>
            </table>
        })
    };
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

    // Symbol and Footprint: the chosen references, read only, with
    // Choose..., and the library a file goes into.
    let symbol_value = {
        let reading = found.clone();
        Signal::derive(move || symbol_reference(&reading, &form.symbol_library.get()))
    };
    let footprint_value = {
        let reading = found.clone();
        Signal::derive(move || {
            footprint_reference(
                &reading,
                &form.footprint_library.get(),
                form.known_footprint.get().as_deref(),
            )
        })
    };
    // KiCad's two filters need a symbol a library has.
    let symbol_current = {
        let chosen = symbol_pick.clone();
        Signal::derive(move || chosen.as_ref().map(|pick| pick.reference.clone()))
    };
    let for_symbol = symbol_current;
    let footprint_current = {
        let chosen = footprint_pick.clone();
        Signal::derive(move || {
            form.known_footprint
                .get()
                .or_else(|| chosen.as_ref().map(|pick| pick.reference.clone()))
        })
    };
    let symbol_library = (found.symbol_file.is_some()
        && referenced(&found, PreviewKind::Symbol).is_none())
    .then(|| {
        view! {
            <LibrarySelect
                id="add-symbol-library".to_owned()
                kind=PreviewKind::Symbol
                chosen=Signal::derive(move || form.symbol_library.get())
                changed=Callback::new(move |library: String| {
                    form.symbol_library.set(library);
                    draw.run(());
                })
            />
        }
    });
    let footprint_library = (found.footprint_file.is_some()
        && referenced(&found, PreviewKind::Footprint).is_none())
    .then_some({
        move || {
            form.known_footprint.with(Option::is_none).then(|| {
                view! {
                    <LibrarySelect
                        id="add-footprint-library".to_owned()
                        kind=PreviewKind::Footprint
                        chosen=Signal::derive(move || form.footprint_library.get())
                        changed=Callback::new(move |library: String| {
                            form.footprint_library.set(library);
                            draw.run(());
                        })
                    />
                }
            })
        }
    });
    let symbol_words = symbol_pick
        .as_ref()
        .map(|pick| view! { <p class="file-name">{use_words(pick)}</p> });
    let footprint_words = footprint_pick
        .as_ref()
        .map(|pick| view! { <p class="file-name">{use_words(pick)}</p> });
    let choose = move |kind: PreviewKind| {
        Callback::new(move |pick: TeamPickRequest| {
            let (symbol, footprint) = form
                .reading
                .with_untracked(|reading| {
                    reading
                        .as_ref()
                        .map(|reading| (reading.symbol, reading.footprint))
                })
                .unwrap_or_default();
            form.choose(pick);
            match kind {
                PreviewKind::Symbol => read.run((None, footprint)),
                PreviewKind::Footprint => read.run((symbol, None)),
            }
        })
    };
    // Removing the first footprint makes the next choice the first: it is
    // chosen for the footprint and the files are read again.
    let remove_first = Callback::new(move |_: ()| {
        let Some(next) = form.extras.with_untracked(|extras| extras.first().cloned()) else {
            return;
        };
        form.extras.update(|extras| {
            extras.remove(0);
        });
        let symbol = form
            .reading
            .with_untracked(|reading| reading.as_ref().and_then(|reading| reading.symbol));
        if next.pick.use_as == PickUse::Copy {
            form.footprint_library.set(next.library.clone());
            form.offered_footprint_library
                .set_value(next.library.clone());
        }
        form.choose(TeamPickRequest {
            kind: PreviewKind::Footprint,
            library: next.pick.library.clone(),
            name: next.pick.name.clone(),
        });
        form.no_models.set(next.no_models);
        read.run((symbol, None));
    });
    let add_extra = Callback::new(move |pick: TeamPickRequest| {
        spawn_local(async move {
            match backend::team_pick(pick).await {
                Ok(pick) => {
                    form.extra_keys.update_value(|key| *key += 1);
                    let key = form.extra_keys.get_value();
                    let library = form.footprint_library.get_untracked();
                    form.extras.update(|extras| {
                        extras.push(Extra {
                            key,
                            pick,
                            library,
                            no_models: false,
                        })
                    });
                }
                Err(message) => form.problem.set(Some(message)),
            }
        });
    });

    view! {
        <p class="file-name" id="add-download">{names}</p>
        <section class="add-checks" aria-labelledby="add-checks-heading">
            <h4 id="add-checks-heading">"Before you save"</h4>
            <ul>
                {move || chosen_problems().into_iter().map(|problem| view! { <li class="check check-error" role="alert">{problem}</li> }).collect_view()}
                {findings.into_iter().map(|finding| view! { <li class="check">{note_with_reference(&finding, &references)}</li> }).collect_view()}
                {same_mpn}
                {footprint_check}
                {pin_check}
                {license.map(|license| view! { <li class="check" id="add-license">{license}</li> })}
                {notes.into_iter().map(|note| view! { <li class="check">{note_with_reference(&note, &references)}</li> }).collect_view()}
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
            <div class="edit-row">
                <label for="add-manufacturer">"Manufacturer"</label>
                <input
                    id="add-manufacturer"
                    type="text"
                    spellcheck="false"
                    prop:value=move || form.manufacturer.get()
                    on:input:target=move |event| {
                        form.manufacturer.set(event.target().value());
                        form.check_mpn_soon();
                    }
                />
            </div>
            <div class="edit-row">
                <label for="add-mpn">"MPN"</label>
                <input
                    id="add-mpn"
                    type="text"
                    spellcheck="false"
                    prop:value=move || form.mpn.get()
                    on:input:target=move |event| {
                        form.mpn.set(event.target().value());
                        form.check_mpn_soon();
                    }
                />
            </div>
        </fieldset>
        <fieldset>
            <legend>"Symbol and footprint"</legend>
            {symbol_choice}
            <div class="edit-row cad-row">
                <label for="add-symbol">"Symbol"</label>
                <input id="add-symbol" type="text" readonly spellcheck="false" class="cad-reference" title=move || symbol_value.get() prop:value=move || symbol_value.get() />
            </div>
            <div class="edit-file cad-actions">
                <div class="cad-buttons">
                    <ChooseButton
                        id="add-symbol-choose"
                        kind=PreviewKind::Symbol
                        what="the symbol".to_owned()
                        current=symbol_current
                        for_symbol=Signal::derive(|| None::<String>)
                        chose=choose(PreviewKind::Symbol)
                    />
                </div>
                {symbol_words}
                {symbol_library}
            </div>
            {footprint_choice}
            <div class="edit-row cad-row">
                <label for="add-footprint">"Footprint"</label>
                <input id="add-footprint" type="text" readonly spellcheck="false" class="cad-reference" title=move || footprint_value.get() prop:value=move || footprint_value.get() />
            </div>
            <div class="edit-file cad-actions">
                <div class="cad-buttons">
                    <ChooseButton
                        id="add-footprint-choose"
                        kind=PreviewKind::Footprint
                        what="the footprint".to_owned()
                        current=footprint_current
                        for_symbol=for_symbol
                        chose=choose(PreviewKind::Footprint)
                    />
                    {move || form.extras.with(|extras| !extras.is_empty()).then(|| view! {
                        <button type="button" class="file-remove" id="add-footprint-remove" on:click=move |_| remove_first.run(())>
                            "Remove"
                            <span class="visually-hidden">" the footprint"</span>
                        </button>
                    })}
                </div>
                {footprint_words}
                {footprint_library}
                {copy_models}
            </div>
            {model_choice}
            <ExtraFootprints form=form />
            <div class="edit-file cad-actions">
                <ChooseButton
                    id="add-footprint-add"
                    kind=PreviewKind::Footprint
                    what="another footprint choice".to_owned()
                    current=Signal::derive(|| None::<String>)
                    for_symbol=for_symbol
                    chose=add_extra
                    label="Add another…"
                />
            </div>
        </fieldset>
        {(!any_pick).then(|| text_row("add-from", "Where from", form.from))}
        <label for="add-reason" class="reason-label">"Why are you adding it?"</label>
        <textarea
            id="add-reason"
            rows="2"
            prop:value=move || form.reason.get()
            on:input:target=move |event| form.reason.set(event.target().value())
        ></textarea>
    }
}

/// The footprint choices after the first, each with Remove.
#[component]
fn ExtraFootprints(form: Form) -> impl IntoView {
    let keys = Memo::new(move |_| {
        form.extras
            .with(|extras| extras.iter().map(|extra| extra.key).collect::<Vec<_>>())
    });
    view! {
        <For
            each=move || keys.get()
            key=|key| *key
            children=move |key| {
                let extra = Memo::new(move |_| form.extras.with(|extras| extras.iter().find(|extra| extra.key == key).cloned()));
                let position = Memo::new(move |_| form.extras.with(|extras| extras.iter().position(|extra| extra.key == key).unwrap_or(0)));
                let id = move || format!("add-footprint-{}", position.get() + 2);
                let set = move |update: Box<dyn FnOnce(&mut Extra)>| {
                    form.extras.update(|extras| {
                        if let Some(extra) = extras.iter_mut().find(|extra| extra.key == key) {
                            update(extra);
                        }
                    });
                };
                let set = StoredValue::new(set);
                view! {
                    <div class="edit-row cad-row">
                        <label for=id>{move || format!("Footprint {}", position.get() + 2)}</label>
                        <input
                            id=id
                            type="text"
                            readonly
                            spellcheck="false"
                            class="cad-reference"
                            prop:value=move || extra.get().map(|extra| extra.reference()).unwrap_or_default()
                        />
                    </div>
                    <div class="edit-file cad-actions">
                        <div class="cad-buttons">
                            <button
                                type="button"
                                class="file-remove"
                                id=move || format!("{}-remove", id())
                                on:click=move |_| {
                                    form.extras.update(|extras| extras.retain(|extra| extra.key != key));
                                    focus_later("add-footprint-add");
                                }
                            >
                                "Remove"
                                <span class="visually-hidden">{move || format!(" footprint {}", position.get() + 2)}</span>
                            </button>
                        </div>
                        {move || extra.get().map(|now| {
                            let copy = now.pick.use_as == PickUse::Copy;
                            let library = now.library.clone();
                            let no_models = now.no_models;
                            let problems = pick_problems(&now.pick, no_models);
                            let at = position.get_untracked() + 2;
                            view! {
                                <p class="file-name">{use_words(&now.pick)}</p>
                                {copy.then(|| view! {
                                    <LibrarySelect
                                        id=format!("add-footprint-library-{at}")
                                        kind=PreviewKind::Footprint
                                        chosen=Signal::derive(move || library.clone())
                                        changed=Callback::new(move |library: String| set.with_value(|set| set(Box::new(move |extra| extra.library = library))))
                                    />
                                    <CopyModels
                                        id=format!("add-footprint-models-{at}")
                                        pick=now.pick.clone()
                                        no_models=Signal::derive(move || no_models)
                                        changed=Callback::new(move |none: bool| set.with_value(|set| set(Box::new(move |extra| extra.no_models = none))))
                                    />
                                })}
                                {problems.into_iter().map(|problem| view! { <p class="problem file-problem" role="alert">{problem}</p> }).collect_view()}
                            }
                        })}
                    </div>
                }
            }
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(kind: PreviewKind, group: LibraryGroup, nickname: &str, name: &str) -> TeamPick {
        TeamPick {
            kind,
            library: LibraryKey {
                group,
                nickname: nickname.into(),
            },
            name: name.into(),
            reference: format!("{nickname}:{name}"),
            use_as: match group {
                LibraryGroup::Ours => PickUse::Ours,
                LibraryGroup::Kicad => PickUse::Kicad,
                LibraryGroup::Installed => PickUse::Copy,
            },
            file: None,
            models: Vec::new(),
            model_problems: Vec::new(),
            problems: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn file(kind: FileKind) -> TeamFile {
        TeamFile {
            kind,
            file_name: "x".into(),
            base64: String::new(),
            symbol: None,
            model_key: None,
            replaces: None,
            label: None,
            installed: None,
            no_models: false,
            choice: None,
        }
    }

    #[test]
    fn a_file_goes_into_the_library_chosen_and_a_chosen_item_keeps_its_reference() {
        // A download: the names from the files, the libraries from the list.
        let download = TeamDownload {
            symbol_file: Some(file(FileKind::Symbol)),
            footprint_file: Some(file(FileKind::Footprint)),
            symbol_name: "C_New".into(),
            footprint_name: "C_0603_New".into(),
            ..TeamDownload::default()
        };
        assert_eq!(symbol_reference(&download, "g-cap"), "g-cap:C_New");
        assert_eq!(symbol_reference(&download, ""), "C_New");
        assert_eq!(
            footprint_reference(&download, "g-Capacitor_SMD", None),
            "g-Capacitor_SMD:C_0603_New"
        );
        // A footprint the Catalog already has, used instead.
        assert_eq!(
            footprint_reference(&download, "g-Capacitor_SMD", Some("g-Capacitor_SMD:C_0603")),
            "g-Capacitor_SMD:C_0603"
        );
        // KiCad's own items keep their references; a copy goes into the
        // library chosen.
        let kicad = TeamDownload {
            picks: vec![
                pick(PreviewKind::Symbol, LibraryGroup::Kicad, "Device", "C"),
                pick(
                    PreviewKind::Footprint,
                    LibraryGroup::Installed,
                    "Mine",
                    "C_Mine",
                ),
            ],
            footprint_file: Some(file(FileKind::Footprint)),
            footprint_name: "C_Mine".into(),
            ..TeamDownload::default()
        };
        assert_eq!(symbol_reference(&kicad, "g-cap"), "Device:C");
        assert_eq!(
            footprint_reference(&kicad, "g-Capacitor_SMD", None),
            "g-Capacitor_SMD:C_Mine"
        );
        let extra = Extra {
            key: 1,
            pick: pick(
                PreviewKind::Footprint,
                LibraryGroup::Installed,
                "Mine",
                "C_Mine",
            ),
            library: "g-Capacitor_SMD".into(),
            no_models: false,
        };
        assert_eq!(extra.reference(), "g-Capacitor_SMD:C_Mine");
        assert_eq!(joined("", "C_Mine"), "C_Mine");
    }
}

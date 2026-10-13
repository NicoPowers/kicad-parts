//! Add part (kicad-parts-9693): an engineer chooses the CAD files they
//! downloaded (a ZIP file, or the .kicad_sym, .kicad_mod and 3D model
//! files), sees the symbol and footprint drawn, confirms the category the
//! app guesses and the IPN it offers, looks at the checks, and saves the
//! new part with a reason.
//!
//! The form takes the part panel: the drawings on the left, the form on
//! the right, like Edit part.
//!
//! Instead of a download, the part can start from a symbol and footprint
//! already in our libraries (kicad-parts-37a7): From our libraries lists
//! the library check's symbols and footprints that the Catalog does not
//! have yet, and the Library check view opens the form with one chosen.

use crate::backend;
use crate::team::{FilePreview, MAX_FILE_BYTES, RetrySlot, base64, focus_later, shown};
use desktop_contract::{
    DownloadedFile, Field, FileKind, FilePreviewRequest, LibraryCheckView, ManufacturerPart,
    PreviewKind, PreviewOutcome, PreviewRequest, PublishResult, TeamDownload, TeamDownloadRequest,
    TeamFile, TeamLibraryChoice, TeamNewCategory, TeamNewPartRequest, TeamSameMpn,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The file types the file input offers.
const ACCEPTED: &str = ".zip,.kicad_sym,.kicad_mod,.step,.stp,.wrl";

/// How many items of our libraries the search shows at once.
const LIBRARY_RESULTS: usize = 12;

type Drawing = RwSignal<Option<Result<PreviewOutcome, String>>>;

/// What is chosen and typed in the form. Values the app changes by itself
/// (the IPN offered, the references) are signals; the fields are kept here
/// so typing never redraws them. The part panel keeps one, so selecting a
/// part while adding one keeps the form as it was; Add part shows it again.
///
/// For each value the app fills in, it remembers what it offered: reading
/// the download again (another symbol chosen) replaces only a value that
/// is still the app's own, never one the engineer typed or chose.
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
    draws: StoredValue<u64>,
    /// Parts that already have the MPN, and the MPN they were shown for.
    same_mpn: RwSignal<Vec<TeamSameMpn>>,
    mpn_shown: StoredValue<String>,
    mpn_checks: StoredValue<u64>,
    offered_manufacturer: StoredValue<String>,
    offered_mpn: StoredValue<String>,
    offered_from: StoredValue<String>,
    offered_footprint: StoredValue<String>,
    /// The fields of the chosen category as the app filled them.
    offered_fields: StoredValue<Vec<Field>>,
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
    /// From our libraries is open instead of a download.
    from_library: RwSignal<bool>,
    /// The symbol and footprint chosen in our libraries.
    library: RwSignal<Option<TeamLibraryChoice>>,
    /// What is typed in the search of our libraries.
    library_query: RwSignal<String>,
    /// Bumped for each new choice in our libraries, and the last one read,
    /// so the form reads each choice once, also when the Library check view
    /// opens it.
    library_asked: RwSignal<u32>,
    library_read: StoredValue<u32>,
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
            draws: StoredValue::new(0),
            same_mpn: RwSignal::new(Vec::new()),
            mpn_shown: StoredValue::new(String::new()),
            mpn_checks: StoredValue::new(0),
            offered_manufacturer: StoredValue::new(String::new()),
            offered_mpn: StoredValue::new(String::new()),
            offered_from: StoredValue::new(String::new()),
            offered_footprint: StoredValue::new(String::new()),
            offered_fields: StoredValue::new(Vec::new()),
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
            from_library: RwSignal::new(false),
            library: RwSignal::new(None),
            library_query: RwSignal::new(String::new()),
            library_asked: RwSignal::new(0),
            library_read: StoredValue::new(0),
        }
    }

    /// Starts again from `choice` in our libraries, as the Library check
    /// view's Add part from it does.
    pub fn start_from_library(&self, choice: TeamLibraryChoice) {
        self.reset();
        self.from_library.set(true);
        self.choose_in_library(choice);
    }

    /// Reads `choice` in our libraries (once the form is shown).
    fn choose_in_library(&self, choice: TeamLibraryChoice) {
        self.library.set(Some(choice));
        self.library_asked.update(|asked| *asked += 1);
    }

    /// What our libraries' items say about the part, when it starts from
    /// them.
    fn library_reading(&self) -> Option<desktop_contract::TeamLibraryReading> {
        self.reading
            .with_untracked(|reading| reading.as_ref().and_then(|reading| reading.library.clone()))
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
            self.symbol,
            self.footprint,
            self.from,
            self.reason,
        ] {
            value.set(String::new());
        }
        for offered in [
            self.offered_ipn,
            self.offered_symbol,
            self.offered_manufacturer,
            self.offered_mpn,
            self.offered_from,
            self.offered_footprint,
            self.mpn_shown,
        ] {
            offered.set_value(String::new());
        }
        self.fields.set_value(Vec::new());
        self.offered_fields.set_value(Vec::new());
        self.known_footprint.set(None);
        self.model.set(None);
        self.from_library.set(false);
        self.library.set(None);
        self.library_query.set(String::new());
        self.attempt.open();
    }

    /// Starts again for newly chosen files: nothing typed for the earlier
    /// files is kept, but the files themselves are, or the choice in our
    /// libraries.
    fn reset_for_new_files(&self) {
        let downloads = self.downloads.get_value();
        let from_library = self.from_library.get_untracked();
        let library = self.library.get_untracked();
        let query = self.library_query.get_untracked();
        self.reset();
        self.downloads.set_value(downloads);
        self.from_library.set(from_library);
        self.library.set(library);
        self.library_query.set(query);
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

    /// Moves the form to the category `code`: its fields as the download
    /// fills them (keeping a value typed for a field of the same name), its
    /// next free IPN, the symbol's library and the footprint's group,
    /// unless the person typed their own.
    fn choose_category(&self, code: String) {
        let category = self.category(&code);
        let (symbol_name, footprint_name) = self.reading.with_untracked(|reading| {
            reading
                .as_ref()
                .map(|reading| (reading.symbol_name.clone(), reading.footprint_name.clone()))
                .unwrap_or_default()
        });
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
        // Our libraries' items keep their references exactly.
        let library = self.library_reading();
        let symbol = match &library {
            Some(library) => library.symbol.clone(),
            None if code.is_empty() || symbol_name.is_empty() => symbol_name,
            None => format!("g-{code}:{symbol_name}"),
        };
        if self.symbol.get_untracked() == self.offered_symbol.get_value() {
            self.symbol.set(symbol.clone());
        }
        self.offered_symbol.set_value(symbol);
        // The download's footprint group, else the one the category
        // suggests.
        let library = category
            .as_ref()
            .map(|category| category.footprint_library.clone())
            .unwrap_or_default();
        let footprint = match &self.library_reading() {
            Some(reading) => reading.footprint.clone(),
            None if library.is_empty() || footprint_name.is_empty() => footprint_name,
            None => format!("{library}:{footprint_name}"),
        };
        if self.footprint.get_untracked() == self.offered_footprint.get_value() {
            self.footprint.set(footprint.clone());
        }
        self.offered_footprint.set_value(footprint);
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
    form: Form,
    close: Callback<()>,
    done: Callback<String>,
    open_part: Callback<(String, Option<ManufacturerPart>)>,
    /// The library check, whose symbols and footprints not in the Catalog
    /// yet From our libraries offers.
    library_check: RwSignal<Option<LibraryCheckView>>,
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
        draws,
        ..
    } = form;
    // Keyboard and screen reader users land in the form.
    focus_later("add-files");

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
        let library = form
            .from_library
            .get_untracked()
            .then(|| form.library.get_untracked())
            .flatten();
        let request = TeamDownloadRequest {
            files: if library.is_some() {
                Vec::new()
            } else {
                downloads.get_value()
            },
            symbol,
            footprint,
            // The 3D models do not change with the symbol or footprint.
            without_models: !first,
            library,
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
            let mut found = found;
            if first {
                form.reset_for_new_files();
            } else if let Some(before) = form.reading.get_untracked() {
                found.model_files = before.model_files;
            }
            // Only values that are still the app's own are replaced.
            Form::offer(
                form.manufacturer,
                form.offered_manufacturer,
                &found.manufacturer,
            );
            Form::offer(form.mpn, form.offered_mpn, &found.mpn);
            Form::offer(form.from, form.offered_from, &found.from);
            // Our libraries' items keep their references exactly.
            let (symbol, footprint) = match &found.library {
                Some(library) => (library.symbol.clone(), library.footprint.clone()),
                None if found.footprint_library.is_empty() => {
                    (found.symbol_name.clone(), found.footprint_name.clone())
                }
                None => (
                    found.symbol_name.clone(),
                    format!("{}:{}", found.footprint_library, found.footprint_name),
                ),
            };
            Form::offer(form.symbol, form.offered_symbol, &symbol);
            Form::offer(form.footprint, form.offered_footprint, &footprint);
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
                || form
                    .model
                    .get_untracked()
                    .is_none_or(|at| at >= found.models.len())
            {
                form.model.set(found.model);
            }
            form.mpn_shown.set_value(form.mpn.get_untracked());
            if form.mpn.get_untracked() == found.mpn {
                form.same_mpn.set(found.same_mpn.clone());
            } else {
                form.check_mpn_soon();
            }
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
                        // A download replaces a choice in our libraries.
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
        // Our libraries' items are read again from the folder when saving.
        let library = found.library.as_ref().map(|library| TeamLibraryChoice {
            symbol: (!library.symbol.is_empty()).then(|| library.symbol.clone()),
            footprint: (!library.footprint.is_empty()).then(|| library.footprint.clone()),
        });
        let mut files: Vec<TeamFile> = if library.is_some() {
            Vec::new()
        } else {
            found.symbol_file.iter().cloned().collect()
        };
        if known.is_none() && library.is_none() {
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
            downloads: if library.is_some() {
                Vec::new()
            } else {
                downloads
                    .with_value(|files| files.iter().map(|file| file.file_name.clone()).collect())
            },
            library,
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

    // Each new choice in our libraries is read once: a symbol chosen is a
    // new part, so it starts the form again; a footprint keeps what is
    // typed.
    Effect::new(move |_| {
        let asked = form.library_asked.get();
        if asked == form.library_read.get_value() || form.library.get_untracked().is_none() {
            return;
        }
        form.library_read.set_value(asked);
        let chosen = form
            .library
            .get_untracked()
            .and_then(|choice| choice.symbol);
        let first = match form.library_reading() {
            None => true,
            Some(reading) => chosen.is_some_and(|symbol| symbol != reading.symbol),
        };
        read(None, None, first);
    });

    let previews = move || {
        let chosen = form.reading.with(Option::is_some);
        let ours = form.from_library.get();
        view! {
            <section class="preview" aria-labelledby="symbol-heading">
                <h3 id="symbol-heading">"Symbol"</h3>
                {(!chosen).then(|| view! { <p class="hint">{if ours { "Choose a symbol in our libraries to see it here." } else { "Choose the download to see its symbol here." }}</p> })}
                <FilePreview kind=FileKind::Symbol preview=symbol_drawing previewing=symbol_busy />
            </section>
            <section class="preview" aria-labelledby="footprint-heading">
                <h3 id="footprint-heading">"Footprint"</h3>
                {(!chosen).then(|| view! { <p class="hint">{if ours { "Choose a footprint in our libraries to see it here." } else { "Choose the download to see its footprint here." }}</p> })}
                <FilePreview kind=FileKind::Footprint preview=footprint_drawing previewing=footprint_busy />
            </section>
        }
    };

    view! {
        <div class="part-view add-part">
            <header class="part-header">
                <h2 id="part-title">"Add part"</h2>
                <p class="part-description">
                    {move || if form.from_library.get() {
                        "Add a part from a symbol and footprint already in our libraries."
                    } else {
                        "Add a part from the CAD files you downloaded from SnapEDA, Ultra Librarian, SamacSys or a manufacturer's site."
                    }}
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
                                    {move || if form.reading.with(|reading| reading.as_ref().is_some_and(|reading| reading.library.is_none())) { "Choose other files…" } else { "Choose the download…" }}
                                </label>
                                <button
                                    type="button"
                                    id="add-from-library"
                                    aria-expanded=move || if form.from_library.get() { "true" } else { "false" }
                                    on:click=move |_| {
                                        if !form.from_library.get_untracked() {
                                            form.reset();
                                            form.from_library.set(true);
                                            focus_later("add-library-search");
                                        }
                                    }
                                >
                                    "From our libraries"
                                </button>
                            </div>
                            <p class="hint">"A ZIP file, or its .kicad_sym, .kicad_mod and 3D model files; or a symbol and footprint already in our libraries."</p>
                            {move || form.from_library.get().then(|| view! { <LibraryPicker form=form library_check=library_check /> })}
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
                        <div class="form-actions">
                            <button
                                type="submit"
                                class="primary"
                                disabled=move || saving.get() || form.reading.with(Option::is_none)
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

/// From our libraries: a search over the symbols and footprints of our
/// libraries that the Catalog does not have yet (the library check's Not in
/// the Catalog yet), each chosen with one click.
#[component]
fn LibraryPicker(form: Form, library_check: RwSignal<Option<LibraryCheckView>>) -> impl IntoView {
    // Each item: kind, `LIBRARY:NAME`.
    let items = move || {
        library_check
            .with(|check| {
                check.as_ref().map(|check| {
                    check
                        .groups
                        .iter()
                        .filter(|group| group.title == "Not in the Catalog yet")
                        .flat_map(|group| group.items.iter())
                        .filter_map(|item| {
                            let kind = if item.file.starts_with("symbols/") {
                                "Symbol"
                            } else if item.file.starts_with("footprints/") {
                                "Footprint"
                            } else {
                                return None;
                            };
                            Some((kind, format!("{}:{}", item.library, item.name)))
                        })
                        .collect::<Vec<_>>()
                })
            })
            // Symbols first: a part usually starts from its symbol, which
            // brings the footprint it names.
            .map(|mut items| {
                items.sort_by_key(|(kind, _)| *kind != "Symbol");
                items
            })
    };
    // The items every typed word matches.
    let found = move || {
        let query = form.library_query.get().to_lowercase();
        items().map(|items| {
            let words: Vec<&str> = query.split_whitespace().collect();
            items
                .into_iter()
                .filter(|(kind, reference)| {
                    let text = format!("{} {}", kind.to_lowercase(), reference.to_lowercase());
                    words.iter().all(|word| text.contains(word))
                })
                .collect::<Vec<_>>()
        })
    };
    // Said to screen readers as the list changes.
    let count = move || match found().map(|found| found.len()) {
        None => String::new(),
        Some(0) => "No symbol or footprint matches.".to_owned(),
        Some(1) => "1 symbol or footprint matches.".to_owned(),
        Some(many) => format!("{many} symbols and footprints match."),
    };
    let results = move || {
        let Some(all) = items() else {
            return view! { <p class="hint">"The library check has not finished yet. Try again in a moment."</p> }.into_any();
        };
        if all.is_empty() {
            return view! { <p class="hint">"Our libraries have no symbol or footprint that the Catalog does not have."</p> }.into_any();
        }
        let found = found().unwrap_or_default();
        if found.is_empty() {
            return view! { <p class="hint">"Nothing in our libraries matches."</p> }.into_any();
        }
        let more = found.len().saturating_sub(LIBRARY_RESULTS);
        // What the part uses now: the choice as read, which includes the
        // footprint a symbol brought, else the choice being read.
        let chosen = form
            .reading
            .with(|reading| {
                reading
                    .as_ref()
                    .and_then(|reading| reading.library.as_ref())
                    .map(|library| TeamLibraryChoice {
                        symbol: Some(library.symbol.clone()).filter(|symbol| !symbol.is_empty()),
                        footprint: Some(library.footprint.clone())
                            .filter(|footprint| !footprint.is_empty()),
                    })
            })
            .or_else(|| form.library.get())
            .unwrap_or_default();
        view! {
            <ul class="library-results" aria-label="Symbols and footprints not in the Catalog yet">
                {found.into_iter().take(LIBRARY_RESULTS).map(|(kind, reference)| {
                    let is_symbol = kind == "Symbol";
                    let pressed = if is_symbol {
                        chosen.symbol.as_deref() == Some(reference.as_str())
                    } else {
                        chosen.footprint.as_deref() == Some(reference.as_str())
                    };
                    let picked = reference.clone();
                    view! {
                        <li>
                            <button
                                type="button"
                                class="library-result"
                                aria-pressed=if pressed { "true" } else { "false" }
                                on:click=move |_| {
                                    let mut choice = form.library.get_untracked().unwrap_or_default();
                                    if is_symbol {
                                        choice.symbol = Some(picked.clone());
                                    } else {
                                        choice.footprint = Some(picked.clone());
                                    }
                                    form.choose_in_library(choice);
                                }
                            >
                                <span class="library-kind">{kind}</span>
                                <code>{crate::cad_reference(&reference)}</code>
                            </button>
                        </li>
                    }
                }).collect_view()}
            </ul>
            {(more > 0).then(|| view! { <p class="hint">{format!("And {more} more. Type to narrow the list.")}</p> })}
        }
        .into_any()
    };
    view! {
        <div class="library-picker">
            <label for="add-library-search">"Search our libraries"</label>
            <input
                id="add-library-search"
                type="search"
                autocomplete="off"
                spellcheck="false"
                placeholder="Library or name"
                aria-describedby="add-library-hint"
                prop:value=move || form.library_query.get()
                on:input:target=move |event| form.library_query.set(event.target().value())
            />
            <p class="hint" id="add-library-hint">
                "Symbols and footprints in our libraries that the Catalog does not have yet. To use one the Catalog has, type it in Symbol or Footprint below."
            </p>
            <p class="visually-hidden" role="status" aria-live="polite">{count}</p>
            {results}
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
    let library = found.library.clone();
    let ours = library.is_some();
    let names = if let Some(library) = &library {
        // The references break after their colon, never at a hyphen.
        let items: Vec<String> = [&library.symbol, &library.footprint]
            .into_iter()
            .filter(|item| !item.is_empty())
            .cloned()
            .collect();
        let read = match library.commit.get(..12) {
            Some(short) => format!(", read from commit {short}."),
            None => ".".to_owned(),
        };
        let count = items.len();
        view! {
            "From our libraries: "
            {items.into_iter().enumerate().map(|(at, item)| view! {
                {(at > 0).then_some(if at + 1 == count { " and " } else { ", " })}
                <span class="nowrap-reference">{crate::cad_reference(&item)}</span>
            }).collect_view()}
            {read}
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
    let symbol_choice = (found.symbols.len() > 1 && !ours).then(|| {
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
    let footprint_choice = (found.footprints.len() > 1 && !ours).then(|| {
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
    let model_choice = (!found.models.is_empty() && found.footprint_file.is_some() && !ours).then(|| {
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
                    <p>{if ours { format!("Uses {reference} instead of the footprint chosen in our libraries.") } else { format!("Uses {reference} instead of the footprint in the download.") }}</p>
                    <button
                        type="button"
                        on:click=move |_| {
                            form.known_footprint.set(None);
                            draw.run(());
                        }
                    >
                        {if ours { "Use the chosen footprint" } else { "Use the download's footprint" }}
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
    // What stops the save, and what else the library check found.
    let (problems, findings) = library
        .as_ref()
        .map(|library| (library.problems.clone(), library.findings.clone()))
        .unwrap_or_default();
    // Our libraries' references in the notes break only after their colon.
    let references: Vec<String> = library
        .as_ref()
        .map(|library| vec![library.footprint.clone(), library.symbol.clone()])
        .unwrap_or_default()
        .into_iter()
        .filter(|reference| !reference.is_empty())
        .collect();
    let license = if ours {
        "License: Unknown, needs review."
    } else {
        "License: Unknown, needs review. Check the site's terms before you use the part."
    };

    // The category: the best guess with its reason, the next two as quick
    // choices, and every category.
    let guesses = found.guesses.clone();
    let guess_line = match guesses.first() {
        Some(best) => view! { <p class="hint" id="add-guess">{guess_words(&best.name, &best.reasons)}</p> }.into_any(),
        None => view! {
            <p class="hint" id="add-guess">
                {if ours { "Nothing in the symbol or footprint says which category this part is in. Choose one." } else { "Nothing in the download says which category this part is in. Choose one." }}
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
            <p class="hint" id="add-unused-heading">{if ours { "Also in the symbol (no field of this category holds it):" } else { "Also in the download (no field of this category holds it):" }}</p>
            <table class="add-unused" aria-labelledby="add-unused-heading">
                <tbody>
                    {unused.into_iter().map(|field| view! {
                        <tr><th scope="row">{field.name}</th><td>{field.value}</td></tr>
                    }).collect_view()}
                </tbody>
            </table>
        })
    };
    let has_footprint = found.footprint_file.is_some();
    let library_hint = move || {
        (has_footprint
            && form.known_footprint.with(Option::is_none)
            && !form.footprint.with(|footprint| footprint.contains(':')))
        .then(|| view! {
            <p class="hint">"Type the footprint's library (its package group) before its name, such as g-Package_SO:NAME."</p>
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
    view! {
        <p class="file-name" id="add-download">{names}</p>
        <section class="add-checks" aria-labelledby="add-checks-heading">
            <h4 id="add-checks-heading">"Before you save"</h4>
            <ul>
                {problems.into_iter().map(|problem| view! { <li class="check check-error" role="alert">{problem}</li> }).collect_view()}
                {findings.into_iter().map(|finding| view! { <li class="check check-warn">{finding}</li> }).collect_view()}
                {same_mpn}
                {footprint_check}
                {pin_check}
                <li class="check" id="add-license">{license}</li>
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
        {(!ours).then(|| text_row("add-from", "Where from", form.from))}
        <label for="add-reason" class="reason-label">"Why are you adding it?"</label>
        <textarea
            id="add-reason"
            rows="2"
            prop:value=move || form.reason.get()
            on:input:target=move |event| form.reason.set(event.target().value())
        ></textarea>
    }
}

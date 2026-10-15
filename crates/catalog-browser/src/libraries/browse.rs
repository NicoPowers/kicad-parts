//! The Libraries browser's use cases on [`CatalogBrowser`]: list the
//! libraries, list and filter items, show one item's details and draw it.

use super::query::{self, SymbolOrFootprint, Uses};
use super::read::{Links, MAX_SYMBOL_FILE_BYTES, read_limited};
use super::sources::{Format, Places, is_link};
use crate::{CatalogBrowser, Drawn, PartValues, export_problem, problem, ready};
use desktop_contract::{
    LibrariesView, LibraryGroup, LibraryItemDetail, LibraryItemRequest, LibraryItems,
    LibraryItemsRequest, PartUse, PreviewKind, PreviewOutcome, PreviewProblemReason,
};
use kicad_integration::library_summary::read_symbol_file;
use kicad_integration::{
    Discovery, LibRef, LibraryOrigin, ResolvedFootprint, ResolvedSymbol, StoredItem,
    footprint_choices, installed_share_dir, sha256_hex,
};
use std::collections::BTreeMap;

impl CatalogBrowser {
    /// The libraries and how far the background reading is. With
    /// `refresh`, the libraries are found again and each is checked for
    /// changes in the background (the browser asks once when it opens).
    pub fn libraries(&self, refresh: bool) -> LibrariesView {
        if refresh {
            self.libraries.refresh(&self.library_places());
        }
        self.libraries.view()
    }

    /// Where the libraries are: our CSV catalog folder, KiCad's data
    /// folder (found even when kicad-cli cannot run) and the person's
    /// KiCad settings folder.
    fn library_places(&self) -> Places {
        let share = self
            .kicad_share()
            .or_else(|| installed_share_dir(&self.config.kicad_locations));
        Places {
            catalog_root: self.config.catalog_root.clone(),
            share,
            kicad_config: self.config.kicad_config_dir.clone(),
            environment: std::env::vars().collect(),
            documents: self.config.kicad_documents_dir.clone(),
        }
    }

    /// Waits until every library is read, at most `timeout`; for tests.
    pub fn wait_for_libraries(&self, timeout: std::time::Duration) -> bool {
        self.libraries.wait_until_ready(timeout)
    }

    /// The items `request` asks for. `current` holds the team catalog's
    /// parts, which mark items In the Catalog with the CSV catalog's.
    pub fn library_items(
        &self,
        request: &LibraryItemsRequest,
        current: &BTreeMap<String, PartValues>,
    ) -> LibraryItems {
        if let Some(key) = &request.library {
            self.libraries.want(request.kind, key);
        }
        let reading = self.libraries.reading();
        let libraries = self.libraries.snapshot(request.kind);
        let symbols = match request.kind {
            PreviewKind::Symbol => Vec::new(),
            PreviewKind::Footprint => self.libraries.snapshot(PreviewKind::Symbol),
        };
        query::items(&libraries, &symbols, request, &self.uses(current), reading)
    }

    /// One item's details, with the Catalog parts that use it.
    pub fn library_item(
        &self,
        request: &LibraryItemRequest,
        current: &BTreeMap<String, PartValues>,
    ) -> Option<LibraryItemDetail> {
        let libraries = self.libraries.snapshot(request.kind);
        query::detail(
            &libraries,
            &request.library,
            &request.name,
            &self.uses(current),
        )
    }

    /// Which Catalog parts use each reference: the CSV catalog's parts with
    /// the team catalog's current values, and parts only the team catalog
    /// has.
    fn uses(&self, current: &BTreeMap<String, PartValues>) -> Uses {
        let mut uses = Uses::new();
        let mut add =
            |ipn: &str, description: &str, symbol: Option<&str>, footprint: Option<&str>| {
                let references = symbol
                    .into_iter()
                    .chain(footprint.map(footprint_choices).unwrap_or_default());
                for reference in references {
                    let reference = reference.trim();
                    if reference.is_empty() {
                        continue;
                    }
                    let list = uses.entry(reference.to_owned()).or_default();
                    if !list.iter().any(|known| known.ipn == ipn) {
                        list.push(PartUse {
                            ipn: ipn.to_owned(),
                            description: description.to_owned(),
                        });
                    }
                }
            };
        if let Ok(catalog) = &self.catalog {
            for part in catalog.parts() {
                let part = current
                    .get(&part.ipn)
                    .map(|values| values.apply(part))
                    .unwrap_or_else(|| part.clone());
                add(
                    &part.ipn,
                    &part.description,
                    part.symbol.as_deref(),
                    part.footprint.as_deref(),
                );
            }
        }
        let csv = self.csv_ipns();
        for (ipn, values) in current.iter().filter(|(ipn, _)| !csv.contains(*ipn)) {
            add(
                ipn,
                values.description.as_deref().unwrap_or_default(),
                values.symbol.as_deref(),
                values.footprint.as_deref(),
            );
        }
        uses
    }

    /// Draws one library item with kicad-cli, through the preview cache.
    pub fn library_preview(&self, request: &LibraryItemRequest) -> PreviewOutcome {
        let kind = request.kind;
        let install = {
            let kicad = self.kicad.read().unwrap_or_else(|e| e.into_inner());
            match &kicad.discovery {
                Discovery::Ready(install) => install.clone(),
                _ => {
                    return problem(
                        kind,
                        PreviewProblemReason::KicadUnavailable,
                        None,
                        "Previews are drawn by KiCad 10's kicad-cli, which is not available."
                            .into(),
                    );
                }
            }
        };
        let reference = format!("{}:{}", request.library.nickname, request.name);
        let libraries = self.libraries.snapshot(kind);
        let Some(library) = libraries
            .iter()
            .find(|library| library.source.key == request.library)
        else {
            return problem(
                kind,
                PreviewProblemReason::Unresolved,
                Some(&reference),
                "This library is no longer listed.".into(),
            );
        };
        let Some(item) = query::find_item(&libraries, &request.library, &request.name) else {
            return problem(
                kind,
                PreviewProblemReason::Unresolved,
                Some(&reference),
                format!("{reference} is not in its library any more."),
            );
        };
        let Ok(lib_ref) = LibRef::parse(&reference) else {
            return problem(
                kind,
                PreviewProblemReason::Unexportable,
                Some(&reference),
                "Its name holds a character kicad-cli cannot be given, so it cannot be drawn."
                    .into(),
            );
        };
        let source = &library.source;
        let origin = match source.key.group {
            LibraryGroup::Kicad => LibraryOrigin::KicadBundled,
            _ => LibraryOrigin::Project,
        };
        let drawn = Drawn {
            library_path: source.path.display().to_string(),
            from_kicad_bundle: source.key.group == LibraryGroup::Kicad,
            installed_library: source.key.group == LibraryGroup::Installed,
            cad_item_revision: None,
            note: None,
        };
        let exported = match (item, source.format) {
            (SymbolOrFootprint::Symbol(_), Format::SymbolFile) => self.previews.symbol_for(
                &install,
                &ResolvedSymbol {
                    lib_ref,
                    library: source.path.clone(),
                    origin,
                },
                request.selection,
            ),
            (SymbolOrFootprint::Symbol(symbol), Format::SymbolFolder) => {
                // kicad-cli reads only symbol files, so the symbol (and the
                // parent it extends) is drawn as stored bytes.
                // The parent's file comes from the index, not its name.
                let file_of =
                    |name: &str| match query::find_item(&libraries, &request.library, name) {
                        Some(SymbolOrFootprint::Symbol(parent)) => parent.file.clone(),
                        _ => None,
                    };
                match symdir_bytes(source, symbol.file.as_deref(), &request.name, &file_of) {
                    Ok((bytes, version)) => {
                        let stored = StoredItem {
                            lib_ref,
                            blob_sha256: sha256_hex(&bytes),
                            format_version: version,
                        };
                        self.previews.stored_symbol(
                            &install,
                            &stored,
                            || Ok(bytes.clone()),
                            request.selection,
                        )
                    }
                    Err(why) => {
                        return problem(
                            kind,
                            PreviewProblemReason::Unresolved,
                            Some(&reference),
                            why,
                        );
                    }
                }
            }
            (SymbolOrFootprint::Footprint(_), Format::Footprints) => {
                let file = source.path.join(format!("{}.kicad_mod", request.name));
                if !file.is_file() || is_link(&file, source.strict_links) {
                    return problem(
                        kind,
                        PreviewProblemReason::Unresolved,
                        Some(&reference),
                        format!("{} is not a file (links are not followed).", file.display()),
                    );
                }
                self.previews.footprint_for(
                    &install,
                    &ResolvedFootprint {
                        lib_ref,
                        library: source.path.clone(),
                        file,
                        origin,
                    },
                    request.selection,
                )
            }
            _ => {
                return problem(
                    kind,
                    PreviewProblemReason::Internal,
                    Some(&reference),
                    "This item is not a symbol or footprint as asked.".into(),
                );
            }
        };
        match exported {
            Ok(preview) => ready(kind, &reference, &[], drawn, &install, preview),
            Err(error) => export_problem(kind, &reference, error),
        }
    }
}

/// The bytes of `name` in a `.kicad_symdir` (its file's entry, with the
/// parents it extends) and the file's version.
fn symdir_bytes(
    source: &super::sources::Source,
    file: Option<&str>,
    name: &str,
    file_of: &dyn Fn(&str) -> Option<String>,
) -> Result<(Vec<u8>, Option<String>), String> {
    let folder = &source.path;
    let links = Links::Refuse {
        strict: source.strict_links,
    };
    let read = |file: &str| -> Result<
        (
            Vec<u8>,
            kicad_integration::library_summary::SymbolFileSummary,
        ),
        String,
    > {
        if file.contains(['/', '\\']) || file.starts_with('.') {
            return Err(format!("{file} is not a plain file name."));
        }
        let bytes = read_limited(&folder.join(file), MAX_SYMBOL_FILE_BYTES, links)?;
        let summary = read_symbol_file(&bytes)
            .map_err(|error| format!("{file} could not be read: {error}."))?;
        Ok((bytes, summary))
    };
    let file = file.ok_or_else(|| format!("{name} has no file in its library folder."))?;
    let (bytes, summary) = read(file)?;
    let symbol = summary
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .ok_or_else(|| format!("{name} is not in {file} any more."))?;
    let mut entries = vec![bytes[symbol.span.clone()].to_vec()];
    let mut parent = symbol.extends.clone();
    // Each parent is in the file the index recorded for it.
    for _ in 0..8 {
        let Some(name) = parent.take() else { break };
        let Some(parent_file) = file_of(&name) else {
            break;
        };
        let Ok((bytes, summary)) = read(&parent_file) else {
            break;
        };
        if let Some(found) = summary.symbols.iter().find(|symbol| symbol.name == name) {
            entries.insert(0, bytes[found.span.clone()].to_vec());
            parent = found.extends.clone();
        }
    }
    Ok((entries.join(&b"\n"[..]), summary.version))
}

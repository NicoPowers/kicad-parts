//! The Libraries browser's use cases on [`CatalogBrowser`]: list the
//! libraries, list and filter items, show one item's details and draw it.

use super::query::{self, SymbolOrFootprint, Uses};
use super::read::{Links, MAX_SYMBOL_FILE_BYTES, read_limited};
use super::sources::{Format, Places, is_link};
use crate::library_repos::TeamLibraries;
use crate::team::KnownParts;
use crate::{CatalogBrowser, Drawn, TeamState, export_problem, problem, ready};
use desktop_contract::{
    LibrariesView, LibraryGroup, LibraryItemDetail, LibraryItemRequest, LibraryItems,
    LibraryItemsRequest, PartUse, PreviewKind, PreviewOutcome, PreviewProblemReason,
};
use kicad_integration::library_summary::read_symbol_file;
use kicad_integration::{
    Discovery, LibRef, LibraryOrigin, ResolvedFootprint, ResolvedSymbol, StoredItem,
    footprint_choices, installed_share_dir, sha256_hex,
};
use std::sync::Arc;

impl CatalogBrowser {
    /// The libraries and how far the background reading is. With
    /// `refresh`, the libraries are found again and each is checked for
    /// changes in the background (the browser asks once when it opens).
    pub fn libraries(&self, refresh: bool) -> LibrariesView {
        if refresh {
            let _ = self.libraries.refresh(&self.library_places());
        }
        self.libraries.view()
    }

    /// [`Self::libraries`], with the linked teams' reviewed library
    /// layouts (`teams`). They are found again with `refresh`, and at each
    /// later call while some team's files are still being written. When
    /// what is found changes, the libraries are found again too. Until
    /// then the view counts one more library to read, so the browser keeps
    /// asking.
    pub fn libraries_with_teams(
        &self,
        refresh: bool,
        teams: impl FnOnce() -> Vec<TeamLibraries>,
    ) -> LibrariesView {
        let waiting = |state: &TeamState| {
            !state.applied || state.found.iter().any(TeamLibraries::is_being_written)
        };
        let must_refresh = {
            let mut state = self
                .team_libraries
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if refresh || waiting(&state) {
                let found = teams();
                if found != state.found {
                    state.found = found;
                    state.applied = false;
                }
            }
            refresh || !state.applied
        };
        if must_refresh && self.libraries.refresh(&self.library_places()) {
            self.team_libraries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .applied = true;
        }
        let mut view = self.libraries.view();
        if waiting(
            &self
                .team_libraries
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        ) {
            view.total += 1;
        }
        view
    }

    /// Where the libraries are: the local library folder, KiCad's data
    /// folder (found even when kicad-cli cannot run) and the person's
    /// KiCad settings folder.
    pub(crate) fn library_places(&self) -> Places {
        let share = self
            .kicad_share()
            .or_else(|| installed_share_dir(&self.config.kicad_locations));
        Places {
            catalog_root: self.config.catalog_root.clone(),
            share,
            kicad_config: self.config.kicad_config_dir.clone(),
            environment: std::env::vars().collect(),
            documents: self.config.kicad_documents_dir.clone(),
            teams: self
                .team_libraries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .found
                .clone(),
        }
    }

    /// Waits until every library is read, at most `timeout`; for tests.
    pub fn wait_for_libraries(&self, timeout: std::time::Duration) -> bool {
        self.libraries.wait_until_ready(timeout)
    }

    /// The items `request` asks for. The parts the part list shows for
    /// `only` (a team's server address, or every team in `teams` when
    /// `None`) mark the items they use as In the Catalog.
    pub fn library_items(
        &self,
        request: &LibraryItemsRequest,
        teams: &[Arc<KnownParts>],
        only: Option<&str>,
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
        let uses = self.uses(teams, only);
        query::items(&libraries, &symbols, request, &uses, reading)
    }

    /// One item's details, with the parts that use it among those the part
    /// list shows for `only` (see [`Self::library_items`]).
    pub fn library_item(
        &self,
        request: &LibraryItemRequest,
        teams: &[Arc<KnownParts>],
        only: Option<&str>,
    ) -> Option<LibraryItemDetail> {
        let libraries = self.libraries.snapshot(request.kind);
        query::detail(
            &libraries,
            &request.library,
            &request.name,
            &self.uses(teams, only),
        )
    }

    /// Which parts use each reference: the parts the part list shows, each
    /// with its team, so a part two teams have is listed for each.
    fn uses(&self, teams: &[Arc<KnownParts>], only: Option<&str>) -> Uses {
        let mut uses = Uses::new();
        for (team, part) in self.listed_parts(teams, only) {
            let references = part.symbol.as_deref().into_iter().chain(
                part.footprint
                    .as_deref()
                    .map(footprint_choices)
                    .unwrap_or_default(),
            );
            for reference in references {
                let reference = reference.trim();
                if reference.is_empty() {
                    continue;
                }
                let list = uses.entry(reference.to_owned()).or_default();
                if !list
                    .iter()
                    .any(|known| known.ipn == part.ipn && known.team == team)
                {
                    list.push(PartUse {
                        team: team.clone(),
                        ipn: part.ipn.clone(),
                        description: part.description.clone(),
                    });
                }
            }
        }
        uses
    }

    /// Draws one of our library items as the Catalog has it (its newest
    /// revision), for a choice bound to the Catalog; an item the Catalog
    /// does not have, or cannot give, is drawn from its library.
    pub fn catalog_item_preview(
        &self,
        request: &LibraryItemRequest,
        team: &crate::team::TeamCatalog,
    ) -> PreviewOutcome {
        let kind = request.kind;
        let reference = format!("{}:{}", request.library.nickname, request.name);
        let stored = match team.current_cad(kind, &reference) {
            Ok(Some(stored)) if request.library.group == LibraryGroup::Ours => stored,
            _ => return self.library_preview(request),
        };
        let install = {
            let kicad = self.kicad.read().unwrap_or_else(|e| e.into_inner());
            match &kicad.discovery {
                Discovery::Ready(install) => install.clone(),
                _ => return self.library_preview(request),
            }
        };
        let load = || team.stored_bytes(&stored);
        let exported = match kind {
            PreviewKind::Symbol => {
                self.previews
                    .stored_symbol(&install, &stored.item, load, request.selection)
            }
            PreviewKind::Footprint => {
                self.previews
                    .stored_footprint(&install, &stored.item, load, request.selection)
            }
        };
        match exported {
            Ok(preview) => ready(
                kind,
                &reference,
                &[],
                Drawn {
                    library_path: String::new(),
                    from_kicad_bundle: false,
                    installed_library: false,
                    cad_item_revision: Some(stored.number),
                    stored_footprint: Some(stored.footprint()),
                    note: None,
                },
                &install,
                preview,
            ),
            Err(_) => self.library_preview(request),
        }
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
            stored_footprint: None,
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
pub(crate) fn symdir_bytes(
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

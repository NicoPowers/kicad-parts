//! The "browse the catalog with exact KiCad previews" use cases, and the
//! team catalog ([`team`]): sign in, history, edits and Add part.
//!
//! The desktop shell wraps each method in a Tauri command and does nothing
//! else, so everything here is testable without a window.

mod add_part;
pub mod drawing;
mod git;
mod libraries;
mod library;
mod library_part;
mod pick;
pub mod team;

use base64::Engine;
use csv_catalog::{CatalogPart, CsvCatalog};
use desktop_contract::{
    AppStatus, CatalogStatus, Field, FileKind, FilePreviewRequest, KicadStatus, LibraryCheckView,
    PartDetail, PartSummary, PreviewImage, PreviewKind, PreviewOutcome, PreviewProblem,
    PreviewProblemReason, PreviewRequest, PreviewSet, PublishResult, SearchRequest, SearchResponse,
    SearchSection, TeamDownload, TeamDownloadRequest, TeamIpnCheck, TeamNewPartRequest,
};
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup};
use kicad_integration::{
    CacheOutcome, CliSettingsFolder, Discovery, DiscoveryOptions, InstallSource, KicadInstall,
    LibRef, LibraryOrigin, LibraryResolver, Preview, PreviewCache, PreviewError, ResolveError,
    SUPPORTED_MAJOR, SearchLocations, StoredItem, discover, footprint_choices, sha256_hex,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::Instant;
use team::{StoredCad, TeamCad, TeamCadError, TeamCatalog};

/// Largest page of search results the UI may request.
pub const MAX_SEARCH_LIMIT: usize = 500;

#[derive(Clone, Debug)]
pub struct BrowserConfig {
    /// Directory holding `database/`, `symbols/` and `footprints/`.
    pub catalog_root: PathBuf,
    /// The app's cache folder, kept apart from WebView2's data; previews go
    /// under `previews/`, and kicad-cli keeps its own settings under
    /// `kicad-cli-settings/`.
    pub cache_dir: PathBuf,
    /// JSON file holding user settings such as the kicad-cli override.
    pub settings_file: PathBuf,
    pub kicad_locations: SearchLocations,
    /// KiCad's settings folder holding the person's library tables (such
    /// as `%APPDATA%\kicad\10.0`), read only; `None` lists no installed
    /// libraries. Tests pass a folder of fixture tables or `None`.
    pub kicad_config_dir: Option<PathBuf>,
    /// The person's Documents folder, for KiCad's `KICAD10_3RD_PARTY`.
    pub kicad_documents_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kicad_cli_override: Option<PathBuf>,
}

struct KicadState {
    settings: Settings,
    /// Why `settings.json` could not be used, if it could not.
    settings_error: Option<String>,
    /// Why the last path the user entered was not saved.
    override_rejected: Option<String>,
    discovery: Discovery,
}

pub struct CatalogBrowser {
    config: BrowserConfig,
    catalog: Result<CsvCatalog, String>,
    previews: PreviewCache,
    kicad: RwLock<KicadState>,
    /// The CSV catalog's footprints with their pads, read the first time
    /// Add part needs them.
    footprints: OnceLock<Vec<add_part::LocalFootprint>>,
    /// The CSV catalog's IPNs, read the first time they are needed.
    csv_ipns: OnceLock<BTreeSet<String>>,
    /// The last library check and what it was worked out for.
    library_check: library::Cached,
    /// The Libraries browser's index, read in the background.
    libraries: libraries::LibraryIndex,
}

impl CatalogBrowser {
    /// Loads the catalog and settings and looks for KiCad. Problems become
    /// part of [`AppStatus`] rather than errors, so the UI can explain them.
    pub fn open(config: BrowserConfig) -> Self {
        let catalog = CsvCatalog::load(&config.catalog_root).map_err(|e| e.to_string());
        let (settings, settings_error) = match load_settings(&config.settings_file) {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(error)),
        };
        let discovery = discover(&discovery_options(&config, &settings));
        // Opening the cache sweeps staging folders that crashed apps left.
        let previews = PreviewCache::new(config.cache_dir.join("previews"));
        if let Discovery::Ready(install) = &discovery {
            previews.evict_unused(&install.version);
        }
        Self {
            footprints: OnceLock::new(),
            csv_ipns: OnceLock::new(),
            library_check: library::Cached::default(),
            libraries: libraries::LibraryIndex::new(config.cache_dir.clone()),
            previews,
            catalog,
            kicad: RwLock::new(KicadState {
                settings,
                settings_error,
                override_rejected: None,
                discovery,
            }),
            config,
        }
    }

    pub fn status(&self) -> AppStatus {
        let kicad = self.kicad.read().unwrap_or_else(|e| e.into_inner());
        let catalog = match &self.catalog {
            Ok(catalog) => CatalogStatus {
                root: display(&self.config.catalog_root),
                parts: catalog.parts().len(),
                csv_files: catalog.categories().len(),
                error: None,
            },
            Err(error) => CatalogStatus {
                root: display(&self.config.catalog_root),
                parts: 0,
                csv_files: 0,
                error: Some(error.clone()),
            },
        };
        AppStatus {
            kicad: kicad_status(&kicad.discovery),
            kicad_override: kicad.settings.kicad_cli_override.as_deref().map(display),
            override_rejected: kicad.override_rejected.clone(),
            settings_error: kicad.settings_error.clone(),
            catalog,
        }
    }

    /// Searches for KiCad again, for example after the user installed it.
    pub fn rediscover_kicad(&self) -> AppStatus {
        {
            let mut kicad = self.kicad.write().unwrap_or_else(|e| e.into_inner());
            kicad.discovery = discover(&discovery_options(&self.config, &kicad.settings));
            kicad.override_rejected = None;
        }
        self.status()
    }

    /// Uses and saves the user's kicad-cli location, or clears it with
    /// `None` or a blank path. A path is saved only if it is a working
    /// KiCad 10 kicad-cli; otherwise nothing changes and the status says why
    /// in `override_rejected`.
    pub fn set_kicad_override(&self, path: Option<String>) -> Result<AppStatus, String> {
        let path = path
            .map(|path| path.trim().trim_matches('"').to_owned())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        {
            let mut kicad = self.kicad.write().unwrap_or_else(|e| e.into_inner());
            let settings = Settings {
                kicad_cli_override: path,
            };
            let discovery = discover(&discovery_options(&self.config, &settings));
            let rejected = settings
                .kicad_cli_override
                .as_ref()
                .and_then(|_| rejection(&discovery));
            if let Some(problem) = rejected {
                kicad.override_rejected = Some(problem);
            } else {
                save_settings(&self.config.settings_file, &settings)?;
                kicad.discovery = discovery;
                kicad.settings = settings;
                kicad.settings_error = None;
                kicad.override_rejected = None;
            }
        }
        Ok(self.status())
    }

    pub fn search(&self, query: &str, limit: usize) -> SearchResponse {
        self.search_with(query, limit, &BTreeMap::new(), &BTreeMap::new())
    }

    /// Like [`Self::search`], but the parts in `current` (by IPN, such as
    /// the team catalog's current details) are matched on and shown with
    /// those values, so a search finds what the app shows. `names` are the
    /// team catalog's category names by code; a category it does not name
    /// gets its starting name.
    pub fn search_with(
        &self,
        query: &str,
        limit: usize,
        current: &BTreeMap<String, PartValues>,
        names: &BTreeMap<String, String>,
    ) -> SearchResponse {
        let request = SearchRequest {
            query: query.to_owned(),
            limit,
            selected: None,
            category: None,
            uses: None,
        };
        self.search_request(&request, current, names)
    }

    /// The search behind the part list. The matches are grouped by
    /// category, one section each, with at most `request.limit` parts per
    /// section (0 for counts only) plus the selected part. Without a
    /// search the sections are in name order; while searching, the section
    /// with the best match comes first. With `request.category`, only that
    /// section is answered.
    pub fn search_request(
        &self,
        request: &SearchRequest,
        current: &BTreeMap<String, PartValues>,
        names: &BTreeMap<String, String>,
    ) -> SearchResponse {
        let started = Instant::now();
        let query = request.query.trim();
        let replace = |part: &CatalogPart| current.get(&part.ipn).map(|values| values.apply(part));
        let (total, sections, selected) = match &self.catalog {
            Ok(catalog) => {
                // Parts only the team catalog has, such as ones added in the
                // app, are found too, after the CSV catalog's.
                let extra = self.team_only_parts(current);
                // Every match is ranked (positions only, which is cheap), so
                // each section knows its own count.
                let results = catalog.search_with_extra(query, usize::MAX, replace, &extra.parts);
                let count = catalog.parts().len();
                let ipn_at = |position: usize| match position.checked_sub(count) {
                    Some(at) => &extra.parts[at].ipn,
                    None => &catalog.parts()[position].ipn,
                };
                let category_of = |position: usize| match position.checked_sub(count) {
                    Some(at) => extra.categories[at].clone(),
                    None => {
                        let part = &catalog.parts()[position];
                        current
                            .get(&part.ipn)
                            .and_then(|values| values.category.clone())
                            .unwrap_or_else(|| {
                                catalog::category::category_code(&part.category).to_owned()
                            })
                    }
                };
                // Only the parts using the reference, compared exactly.
                let uses = request
                    .uses
                    .as_deref()
                    .map(str::trim)
                    .filter(|reference| !reference.is_empty());
                let part_at = |position: usize| match position.checked_sub(count) {
                    Some(at) => extra.parts[at].clone(),
                    None => {
                        let part = &catalog.parts()[position];
                        replace(part).unwrap_or_else(|| part.clone())
                    }
                };
                let wanted = |position: usize| {
                    uses.is_none_or(|reference| part_uses(&part_at(position), reference))
                };
                let total = results
                    .hits
                    .iter()
                    .filter(|&&position| wanted(position))
                    .count();
                let selected_position = request.selected.as_deref().and_then(|selected| {
                    results
                        .hits
                        .iter()
                        .copied()
                        .find(|&position| ipn_at(position) == selected && wanted(position))
                });
                let selected = selected_position.map(category_of);
                let ranked = results
                    .hits
                    .iter()
                    .zip(&results.ranks)
                    .map(|(&position, &rank)| Ranked {
                        category: category_of(position),
                        position,
                        rank,
                    })
                    .filter(|ranked| {
                        request
                            .category
                            .as_ref()
                            .is_none_or(|wanted| *wanted == ranked.category)
                            && wanted(ranked.position)
                    });
                let groups = group_by_category(
                    ranked,
                    &Grouping {
                        limit: request.limit.min(MAX_SEARCH_LIMIT),
                        names,
                        searching: !query.is_empty() || uses.is_some(),
                        selected: selected_position,
                    },
                );
                let sections = groups
                    .into_iter()
                    .map(|group| SearchSection {
                        category: group.code,
                        name: group.name,
                        total: group.total,
                        hits: group
                            .positions
                            .into_iter()
                            .map(|position| match position.checked_sub(count) {
                                Some(at) => summary(&extra.parts[at]),
                                None => {
                                    let part = &catalog.parts()[position];
                                    summary(&replace(part).unwrap_or_else(|| part.clone()))
                                }
                            })
                            .collect(),
                    })
                    .collect();
                (total, sections, selected)
            }
            Err(_) => (0, Vec::new(), None),
        };
        SearchResponse {
            query: request.query.clone(),
            uses: request.uses.clone(),
            total,
            sections,
            selected,
            elapsed_micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        }
    }

    /// The parts in `current` (the team catalog's) that the CSV catalog
    /// does not have, in IPN order.
    fn team_only_parts(&self, current: &BTreeMap<String, PartValues>) -> TeamOnlyParts {
        let csv = self.csv_ipns();
        let (parts, categories) = current
            .iter()
            .filter(|(ipn, _)| !csv.contains(*ipn))
            .map(|(ipn, values)| {
                (
                    values.part(ipn),
                    values.category.clone().unwrap_or_default(),
                )
            })
            .unzip();
        TeamOnlyParts { parts, categories }
    }

    /// Every IPN the CSV catalog has, read once.
    pub fn csv_ipns(&self) -> &BTreeSet<String> {
        self.csv_ipns.get_or_init(|| {
            self.catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .parts()
                        .iter()
                        .map(|part| part.ipn.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Like [`Self::part`], but a part only the team catalog has (in
    /// `current`, such as one added in the app) is found too.
    pub fn part_with(
        &self,
        ipn: &str,
        current: &BTreeMap<String, PartValues>,
    ) -> Option<PartDetail> {
        self.part(ipn).or_else(|| {
            let values = current.get(ipn)?;
            Some(detail(&values.part(ipn)))
        })
    }

    /// Reads the files chosen for Add part (kicad-parts-9693): what they
    /// hold, what the symbol says, the category guess, the next free IPN
    /// for each category, and the checks before saving.
    ///
    /// With [`TeamDownloadRequest::library`], the symbol and footprint are
    /// read from our libraries in the CSV catalog folder instead
    /// (kicad-parts-37a7), with the library check's findings for them.
    pub fn read_download(
        &self,
        request: &TeamDownloadRequest,
        team: &TeamCatalog,
    ) -> Result<TeamDownload, String> {
        let context = self.add_part_context(team)?;
        if request.picks.is_empty() {
            add_part::read(request, &context)
        } else {
            self.read_picks(request, team, &context)
        }
    }

    /// Add part's reading with a symbol or footprint chosen in the Libraries
    /// browser (kicad-parts-bc6f), each used instead of the download's for
    /// its role. A chosen symbol brings the footprint it names when no
    /// footprint is chosen or downloaded.
    fn read_picks(
        &self,
        request: &TeamDownloadRequest,
        team: &TeamCatalog,
        context: &add_part::Context<'_>,
    ) -> Result<TeamDownload, String> {
        use catalog::download::{Download, DownloadFile, DownloadKind};
        let mut inputs = Vec::new();
        for file in &request.files {
            inputs.push((
                file.file_name.clone(),
                team::decoded(&file.base64, &file.file_name)?,
            ));
        }
        let mut download = if inputs.is_empty() {
            Download::default()
        } else {
            catalog::download::read_download(&inputs)?
        };
        let find = |kind: PreviewKind| request.picks.iter().find(|pick| pick.kind == kind);
        let mut notes = Vec::new();
        let mut symbol_at = request.symbol;
        let mut chosen = Vec::new();
        let mut footprint_pick = find(PreviewKind::Footprint).cloned();
        if let Some(wanted) = find(PreviewKind::Symbol) {
            let item = self.item(wanted)?;
            download
                .files
                .retain(|file| file.kind != DownloadKind::Symbol);
            symbol_at = catalog::cad_file::read_symbol_file(&item.bytes)
                .ok()
                .and_then(|file| file.names.iter().position(|name| *name == item.name));
            let has_footprint = download.of_kind(DownloadKind::Footprint).next().is_some();
            if footprint_pick.is_none() && !has_footprint {
                if let Some(named) = self.named_footprint(&item) {
                    notes.push(format!(
                        "The footprint is the one the symbol names, {}:{}.",
                        named.library.nickname, named.name
                    ));
                    footprint_pick = Some(named);
                }
            }
            download.files.push(DownloadFile {
                label: item.reference.clone(),
                file_name: format!("{}.kicad_sym", item.name),
                kind: DownloadKind::Symbol,
                bytes: item.bytes,
            });
            chosen.push(self.pick(wanted, team)?);
        }
        let mut footprint_reference = None;
        if let Some(wanted) = &footprint_pick {
            let item = self.item(wanted)?;
            // A 3D model of the download goes with the download's
            // footprint only.
            download.files.retain(|file| {
                file.kind != DownloadKind::Footprint && file.kind != DownloadKind::Model3d
            });
            footprint_reference = Some(item.reference.clone());
            download.files.push(DownloadFile {
                label: item.reference.clone(),
                file_name: format!("{}.kicad_mod", item.name),
                kind: DownloadKind::Footprint,
                bytes: item.bytes,
            });
            chosen.push(self.pick(wanted, team)?);
        }
        let mut reading = add_part::read_files(
            &download,
            symbol_at,
            if footprint_pick.is_some() {
                Some(0)
            } else {
                request.footprint
            },
            request.without_models,
            context,
            add_part::Origin::Picked {
                symbol: find(PreviewKind::Symbol).is_some(),
                footprint: footprint_reference.as_deref(),
            },
        );
        for pick in &chosen {
            let copy = pick.use_as == desktop_contract::PickUse::Copy;
            match pick.kind {
                PreviewKind::Symbol => {
                    reading.symbol_file = if copy { pick.file.clone() } else { None };
                    // One symbol is chosen, never a choice of several.
                    reading.symbols = vec![pick.reference.clone()];
                    reading.symbol = Some(0);
                }
                PreviewKind::Footprint => {
                    reading.footprint_file = if copy { pick.file.clone() } else { None };
                    reading.footprints = vec![pick.reference.clone()];
                    reading.footprint = Some(0);
                    reading.model_files = if copy {
                        pick.models.clone()
                    } else {
                        Vec::new()
                    };
                    reading.models = reading
                        .model_files
                        .iter()
                        .map(|file| file.label.clone().unwrap_or_else(|| file.file_name.clone()))
                        .collect();
                    reading.model = (!reading.models.is_empty()).then_some(0);
                    if !copy {
                        // The Catalog's, ours and KiCad's are used as they
                        // are: they keep their own library and links.
                        if pick.use_as != desktop_contract::PickUse::Ours {
                            reading.known_footprints.clear();
                        }
                        reading.footprint_library = pick.library.nickname.clone();
                        for category in &mut reading.categories {
                            category.footprint_library = pick.library.nickname.clone();
                        }
                    }
                }
            }
        }
        // Where the part came from, as its history says it: the symbol's
        // library, else the footprint's; a download's own site otherwise.
        let origin = chosen
            .iter()
            .find(|pick| pick.kind == PreviewKind::Symbol)
            .or_else(|| chosen.first().filter(|_| request.files.is_empty()));
        if let Some(pick) = origin {
            reading.from = match pick.library.group {
                desktop_contract::LibraryGroup::Ours => library_part::FROM_OUR_LIBRARIES.to_owned(),
                desktop_contract::LibraryGroup::Kicad => pick::FROM_KICAD.to_owned(),
                desktop_contract::LibraryGroup::Installed => {
                    pick::from_installed(&pick.library.nickname)
                }
            };
        }
        notes.append(&mut reading.notes);
        reading.notes = notes;
        reading.picks = chosen;
        Ok(reading)
    }

    /// What Add part reads besides the files: the team catalog's
    /// categories, remembered matches and parts, every IPN, the CSV
    /// catalog's footprints and KiCad's data folder.
    fn add_part_context(&self, team: &TeamCatalog) -> Result<add_part::Context<'_>, String> {
        let (categories, remembered) = team.categories_and_matches()?;
        let parts = team.current_parts();
        let mut ipns = self.csv_ipns().clone();
        ipns.extend(parts.iter().map(|(details, _)| details.ipn.clone()));
        let footprints = self
            .footprints
            .get_or_init(|| add_part::local_footprints(&self.config.catalog_root));
        Ok(add_part::Context {
            categories,
            remembered,
            parts,
            ipns,
            footprints,
            kicad_share: self.kicad_share(),
        })
    }

    /// KiCad's data folder, when KiCad was found.
    fn kicad_share(&self) -> Option<PathBuf> {
        let kicad = self.kicad.read().unwrap_or_else(|e| e.into_inner());
        match &kicad.discovery {
            Discovery::Ready(install) => install.share_dir.clone(),
            _ => None,
        }
    }

    /// The symbol and footprint `choice` names in our libraries, read from
    /// the CSV catalog folder, with what saving them needs.
    fn library_plan(
        &self,
        choice: &desktop_contract::TeamLibraryChoice,
        team: &TeamCatalog,
    ) -> Result<library_part::Plan, String> {
        let share = self.kicad_share();
        let check = self
            .library_check
            .check(team, &self.config.catalog_root, share.as_deref())?
            .ok_or_else(|| {
                "Sign in as someone who can edit to add a part from our libraries.".to_owned()
            })?;
        library_part::plan(&self.config.catalog_root, choice, &check, &|kind, key| {
            team.has_cad_item(kind, key)
        })
    }

    /// The library check (kicad-parts-da65) of this app's CSV catalog folder
    /// against the team catalog, for someone who can edit; `None` while
    /// signed out or for someone who can only view. It is worked out once
    /// per change of the team catalog (a new Change, a finished import or
    /// another person signing in) and kept until then.
    pub fn library_check(&self, team: &TeamCatalog) -> Result<Option<LibraryCheckView>, String> {
        let kicad_share = self.kicad_share();
        self.library_check
            .get(team, &self.config.catalog_root, kicad_share.as_deref())
    }

    /// Whether `ipn` can be a new part's IPN.
    pub fn check_ipn(&self, ipn: &str, prefix: &str, team: &TeamCatalog) -> TeamIpnCheck {
        team.check_ipn(ipn, prefix, self.csv_ipns())
    }

    /// Saves a new part from Add part. A part from our libraries
    /// ([`TeamNewPartRequest::library`]) is read again from the CSV catalog
    /// folder, so what is saved is what the folder and its commit hold now;
    /// only the chosen items the Symbol and Footprint still name are used.
    /// Saves an edit from Edit part. Our libraries' items the edit newly
    /// names and the Catalog lacks (chosen in the Libraries browser,
    /// kicad-parts-bc6f) are added from the CSV catalog folder as
    /// committed, as Add part does (kicad-parts-37a7).
    pub fn edit(
        &self,
        request: desktop_contract::TeamEditRequest,
        team: &TeamCatalog,
    ) -> Result<PublishResult, String> {
        team.edit_with(request, |named| {
            self.ours_uploads(named, team).map(|(uploads, _)| uploads)
        })
    }

    pub fn add_part(
        &self,
        request: TeamNewPartRequest,
        team: &TeamCatalog,
    ) -> Result<PublishResult, String> {
        // Our libraries' items the Catalog lacks, chosen in the
        // Libraries browser, are added from the folder as committed
        // (kicad-parts-bc6f).
        let files = || {
            let mut uploads = add_part::uploads(&request)?;
            let mut wanted = Vec::new();
            for (kind, role, field) in [
                (
                    catalog::CadItemKind::Symbol,
                    catalog::pocketbase::UploadRole::Symbol,
                    &request.symbol,
                ),
                (
                    catalog::CadItemKind::Footprint,
                    catalog::pocketbase::UploadRole::Footprint,
                    &request.footprint,
                ),
            ] {
                for reference in footprint_choices(field) {
                    let sent = uploads
                        .iter()
                        .any(|upload| upload.role == role && upload.reference == reference);
                    if !sent {
                        wanted.push((kind, reference.to_owned()));
                    }
                }
            }
            let (more, sources) = self.ours_uploads(&wanted, team)?;
            uploads.extend(more);
            let mut files = request.downloads.clone();
            files.extend(sources);
            Ok((uploads, files))
        };
        team.add_part_with(request.clone(), self.csv_ipns(), files)
    }

    pub fn part(&self, ipn: &str) -> Option<PartDetail> {
        let part = self.catalog.as_ref().ok()?.find(ipn)?;
        Some(detail(part))
    }

    /// The exact kicad-cli preview of a part's symbol (every unit) or
    /// footprint. Exports on the first view; later views use the cache.
    pub fn preview(&self, ipn: &str, kind: PreviewKind) -> PreviewOutcome {
        self.preview_field(ipn, kind, None)
    }

    /// Like [`Self::preview`], but previews `field` (a Symbol or Footprint
    /// field value, such as the team catalog's) instead of the CSV
    /// catalog's when it is given.
    pub fn preview_field(
        &self,
        ipn: &str,
        kind: PreviewKind,
        field: Option<&str>,
    ) -> PreviewOutcome {
        self.preview_request(
            &PreviewRequest {
                ipn: ipn.to_owned(),
                kind,
                field: field.map(str::to_owned),
                team_revision: None,
                selection: None,
            },
            None,
        )
    }

    /// The preview the UI asked for. With `team_revision`, it is drawn from
    /// the stored bytes of the exact CAD item revision that revision of the
    /// part uses, read through `team`; otherwise from the library on this
    /// computer.
    pub fn preview_request(
        &self,
        request: &PreviewRequest,
        team: Option<&TeamCatalog>,
    ) -> PreviewOutcome {
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
        match (request.team_revision, team) {
            (Some(revision), Some(team)) => self.team_preview(request, revision, team, &install),
            _ => self.local_preview(request, &install, None),
        }
    }

    /// Draws a symbol or footprint file chosen in Edit part, before it is
    /// saved, exactly as a stored CAD item revision with those bytes would
    /// be drawn (the same cache entries).
    pub fn preview_file(&self, request: &FilePreviewRequest) -> PreviewOutcome {
        let file = &request.file;
        let kind = match file.kind {
            FileKind::Symbol => PreviewKind::Symbol,
            FileKind::Footprint => PreviewKind::Footprint,
            FileKind::Model3d => {
                return problem(
                    PreviewKind::Footprint,
                    PreviewProblemReason::Unexportable,
                    None,
                    "A 3D model is not drawn here.".into(),
                );
            }
        };
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
        let bytes = match team::decoded(&file.base64, &file.file_name) {
            Ok(bytes) => bytes,
            Err(message) => return problem(kind, PreviewProblemReason::Internal, None, message),
        };
        // The bytes the Catalog would store, and the name they have in the
        // file. The library part of the reference only names the drawing,
        // so one still being typed is fine.
        let read = match kind {
            PreviewKind::Symbol => {
                let name = file.symbol.clone().unwrap_or_default();
                catalog::cad_file::symbol_upload(&bytes, &name, &format!("g-file:{name}"), "")
                    .map(|upload| (name, upload.bytes, upload.format_version))
            }
            PreviewKind::Footprint => catalog::cad_file::read_footprint_file(&bytes)
                .map(|footprint| (footprint.name, bytes.clone(), None)),
        };
        let (name, stored, format_version) = match read {
            Ok(read) => read,
            Err(message) => return problem(kind, PreviewProblemReason::Unresolved, None, message),
        };
        // The file names itself, and the name becomes a file name in the
        // staging folder, so it must be a plain name; a reference still
        // being typed falls back to a placeholder library.
        if !catalog::cad_file::is_plain_name(&name) {
            return problem(
                kind,
                PreviewProblemReason::Unexportable,
                None,
                format!(
                    "The {} in the file is named {name:?}, which cannot be used: a name cannot be . or .., start with -, or hold / \\ : or control characters.",
                    noun(kind)
                ),
            );
        }
        let typed = LibRef::parse(&request.reference).ok().filter(|reference| {
            reference.item == name && catalog::cad_file::is_plain_name(&reference.library)
        });
        let Some(lib_ref) = typed.or_else(|| LibRef::parse(&format!("g-file:{name}")).ok()) else {
            return problem(
                kind,
                PreviewProblemReason::Unexportable,
                None,
                format!("The {} in the file cannot be drawn.", noun(kind)),
            );
        };
        let item = StoredItem {
            lib_ref,
            blob_sha256: sha256_hex(&stored),
            format_version,
        };
        let load = || Ok(stored.clone());
        let exported = match kind {
            PreviewKind::Symbol => {
                self.previews
                    .stored_symbol(&install, &item, load, request.selection)
            }
            PreviewKind::Footprint => {
                self.previews
                    .stored_footprint(&install, &item, load, request.selection)
            }
        };
        let reference = item.lib_ref.to_string();
        match exported {
            Ok(preview) => ready(
                kind,
                &reference,
                &[],
                Drawn {
                    library_path: String::new(),
                    from_kicad_bundle: false,
                    installed_library: false,
                    cad_item_revision: None,
                    note: None,
                },
                &install,
                preview,
            ),
            Err(error) => export_problem(kind, &reference, error),
        }
    }

    /// How many times kicad-cli has exported a preview in this session.
    pub fn export_count(&self) -> usize {
        self.previews.export_count()
    }

    /// Draws what revision `revision` of the part uses, from the team
    /// catalog's stored bytes. If they cannot be fetched, draws the local
    /// library instead and says so.
    fn team_preview(
        &self,
        request: &PreviewRequest,
        revision: u32,
        team: &TeamCatalog,
        install: &KicadInstall,
    ) -> PreviewOutcome {
        let kind = request.kind;
        let cad = match team.cad_for_preview(&request.ipn, revision, kind) {
            Ok(cad) => cad,
            Err(TeamCadError::SignedOut) => {
                return problem(
                    kind,
                    PreviewProblemReason::TeamSignInNeeded,
                    None,
                    format!(
                        "Sign in to the team catalog to see the {} this part uses.",
                        noun(kind)
                    ),
                );
            }
            Err(TeamCadError::NotFound(message)) => {
                return problem(
                    kind,
                    PreviewProblemReason::Unresolved,
                    None,
                    capitalize(&message),
                );
            }
            Err(TeamCadError::Mismatch { reference, why }) => {
                let note = format!(
                    "The team catalog's revision of {reference} could not be used because \
                     {why}. This drawing is from your local library, which may differ from the \
                     revision this part uses."
                );
                let request = PreviewRequest {
                    field: Some(reference),
                    ..request.clone()
                };
                return self.local_preview(&request, install, Some(note));
            }
            Err(TeamCadError::Unreachable(why)) => {
                let note = format!(
                    "The team catalog could not be checked because {why}. This drawing is \
                     from your local library, which may differ from the revision this part uses."
                );
                return self.local_preview(request, install, Some(note));
            }
        };
        let stored = match cad {
            TeamCad::Unassigned => {
                return problem(
                    kind,
                    PreviewProblemReason::Unassigned,
                    None,
                    format!(
                        "{} has no {} assigned in the team catalog.",
                        request.ipn,
                        noun(kind)
                    ),
                );
            }
            TeamCad::Library { reference, .. } => {
                let request = PreviewRequest {
                    field: Some(reference),
                    ..request.clone()
                };
                return self.local_preview(&request, install, None);
            }
            TeamCad::Stored(stored) => stored,
        };
        let load = || team.stored_bytes(&stored);
        let exported = match kind {
            PreviewKind::Symbol => {
                self.previews
                    .stored_symbol(install, &stored.item, load, request.selection)
            }
            PreviewKind::Footprint => {
                self.previews
                    .stored_footprint(install, &stored.item, load, request.selection)
            }
        };
        let reference = stored.item.lib_ref.to_string();
        match exported {
            Ok(preview) => ready(
                kind,
                &reference,
                &stored.alternates,
                Drawn {
                    library_path: String::new(),
                    from_kicad_bundle: false,
                    installed_library: false,
                    cad_item_revision: Some(stored.number),
                    note: None,
                },
                install,
                preview,
            ),
            Err(PreviewError::StoredBytes(message)) => {
                let request = PreviewRequest {
                    field: Some(reference.clone()),
                    ..request.clone()
                };
                let note = self.fallback_note(kind, &stored, install, &message);
                self.local_preview(&request, install, Some(note))
            }
            Err(error) => export_problem(kind, &reference, error),
        }
    }

    /// Why a team part is drawn from the local library: its stored bytes
    /// could not be fetched. Says whether the local copy has the same bytes.
    fn fallback_note(
        &self,
        kind: PreviewKind,
        stored: &StoredCad,
        install: &KicadInstall,
        why: &str,
    ) -> String {
        let reference = &stored.item.lib_ref;
        let local = local_bytes(&self.resolver(install), reference, kind);
        let comparison = match local {
            Some(bytes) if sha256_hex(&bytes) == stored.item.blob_sha256 => {
                "which has the same bytes"
            }
            Some(_) => "which differs from that revision",
            None => "which may differ from that revision",
        };
        let what = match kind {
            PreviewKind::Symbol => "Symbol",
            PreviewKind::Footprint => "Footprint",
        };
        format!(
            "{what} revision {} of {reference} could not be fetched because {why}. This \
             drawing is from your local library, {comparison}.",
            stored.number
        )
    }

    /// Draws the part's symbol or footprint from the library on this
    /// computer: `request.field` when given, otherwise the CSV catalog's.
    fn local_preview(
        &self,
        request: &PreviewRequest,
        install: &KicadInstall,
        note: Option<String>,
    ) -> PreviewOutcome {
        let (ipn, kind) = (request.ipn.as_str(), request.kind);
        let csv_field = self
            .catalog
            .as_ref()
            .ok()
            .and_then(|catalog| catalog.find(ipn))
            .map(|part| match kind {
                PreviewKind::Symbol => part.symbol.clone(),
                PreviewKind::Footprint => part.footprint.clone(),
            });
        let field = match (&request.field, csv_field) {
            (Some(field), _) => Some(field.trim().to_owned()).filter(|field| !field.is_empty()),
            (None, Some(field)) => field,
            (None, None) => {
                return problem(
                    kind,
                    PreviewProblemReason::UnknownPart,
                    None,
                    format!("Part {ipn} is not in the catalog."),
                );
            }
        };
        // Footprint fields may list `;`-separated alternatives; symbol
        // fields hold exactly one reference.
        let choices = match (kind, field.as_deref()) {
            (_, None) => Vec::new(),
            (PreviewKind::Symbol, Some(field)) => vec![field],
            (PreviewKind::Footprint, Some(field)) => footprint_choices(field),
        };
        let Some((&reference, alternates)) = choices.split_first() else {
            return problem(
                kind,
                PreviewProblemReason::Unassigned,
                None,
                format!("{ipn} has no {} assigned in the catalog.", noun(kind)),
            );
        };
        let alternates: Vec<String> = alternates
            .iter()
            .map(|choice| (*choice).to_owned())
            .collect();
        let resolver = self.resolver(install);
        let selection = request.selection;
        let resolved = match kind {
            PreviewKind::Symbol => resolver.symbol(reference).map(|symbol| {
                let exported = self.previews.symbol_for(install, &symbol, selection);
                (exported, symbol.library, symbol.origin)
            }),
            PreviewKind::Footprint => resolver.footprint(reference).map(|footprint| {
                let exported = self.previews.footprint_for(install, &footprint, selection);
                (exported, footprint.library, footprint.origin)
            }),
        };
        let (exported, library, origin) = match resolved {
            Ok(resolved) => resolved,
            Err(error) => {
                let reason = match error {
                    ResolveError::Unassigned => PreviewProblemReason::Unassigned,
                    _ => PreviewProblemReason::Unresolved,
                };
                return problem(
                    kind,
                    reason,
                    Some(reference),
                    capitalize(&error.to_string()),
                );
            }
        };
        match exported {
            Ok(preview) => ready(
                kind,
                reference,
                &alternates,
                Drawn {
                    library_path: display(&library),
                    from_kicad_bundle: origin == LibraryOrigin::KicadBundled,
                    installed_library: false,
                    cad_item_revision: None,
                    note,
                },
                install,
                preview,
            ),
            Err(error) => export_problem(kind, reference, error),
        }
    }

    fn resolver(&self, install: &KicadInstall) -> LibraryResolver {
        LibraryResolver::new(
            Some(self.config.catalog_root.clone()),
            install.share_dir.clone(),
        )
    }
}

/// Whether `part` uses `reference` exactly: as its Symbol, or as one of
/// its Footprint choices.
fn part_uses(part: &CatalogPart, reference: &str) -> bool {
    part.symbol.as_deref().map(str::trim) == Some(reference)
        || part
            .footprint
            .as_deref()
            .is_some_and(|field| footprint_choices(field).contains(&reference))
}

/// Where a preview was drawn from, for [`PreviewSet`].
pub(crate) struct Drawn {
    library_path: String,
    from_kicad_bundle: bool,
    installed_library: bool,
    cad_item_revision: Option<u32>,
    note: Option<String>,
}

pub(crate) fn ready(
    kind: PreviewKind,
    reference: &str,
    alternates: &[String],
    drawn: Drawn,
    install: &KicadInstall,
    preview: Preview,
) -> PreviewOutcome {
    PreviewOutcome::Ready(PreviewSet {
        kind,
        reference: reference.to_owned(),
        alternates: alternates.to_vec(),
        library_path: drawn.library_path,
        from_kicad_bundle: drawn.from_kicad_bundle,
        installed_library: drawn.installed_library,
        cad_item_revision: drawn.cad_item_revision,
        note: drawn.note,
        kicad_version: install.version.to_string(),
        library_sha256: preview.library_sha256,
        from_cache: preview.cache == CacheOutcome::Hit,
        images: preview
            .images
            .iter()
            .map(|image| PreviewImage {
                unit: image.unit,
                demorgan: image.demorgan,
                data_uri: svg_data_uri(&image.svg),
                view_box: svg_view_box(&image.svg),
                drawing: match kind {
                    PreviewKind::Footprint => preview
                        .footprint
                        .as_deref()
                        .and_then(drawing::footprint_drawing),
                    PreviewKind::Symbol => None,
                },
            })
            .collect(),
    })
}

pub(crate) fn problem(
    kind: PreviewKind,
    reason: PreviewProblemReason,
    reference: Option<&str>,
    message: String,
) -> PreviewOutcome {
    PreviewOutcome::Problem(PreviewProblem {
        kind,
        reason,
        reference: reference.map(str::to_owned),
        message,
        command: None,
        cli_message: None,
    })
}

pub(crate) fn export_problem(
    kind: PreviewKind,
    reference: &str,
    error: PreviewError,
) -> PreviewOutcome {
    match error {
        PreviewError::ExportFailed {
            command,
            cli_message,
        } => PreviewOutcome::Problem(PreviewProblem {
            kind,
            reason: PreviewProblemReason::ExportFailed,
            reference: Some(reference.to_owned()),
            message: format!("kicad-cli could not export the {} {reference}.", noun(kind)),
            command: Some(command),
            cli_message: Some(cli_message),
        }),
        PreviewError::Unexportable(message) => problem(
            kind,
            PreviewProblemReason::Unexportable,
            Some(reference),
            message,
        ),
        PreviewError::Replaced => problem(
            kind,
            PreviewProblemReason::Replaced,
            Some(reference),
            "A newer selection replaced this preview.".into(),
        ),
        PreviewError::StoredBytes(message) | PreviewError::Io(message) => problem(
            kind,
            PreviewProblemReason::Internal,
            Some(reference),
            message,
        ),
    }
}

/// The local copy of what `reference` names, as the team catalog would
/// store it: one symbol's entry, or a whole footprint file.
fn local_bytes(
    resolver: &LibraryResolver,
    reference: &LibRef,
    kind: PreviewKind,
) -> Option<Vec<u8>> {
    let text = reference.to_string();
    if kind == PreviewKind::Symbol {
        let resolved = resolver.symbol(&text).ok()?;
        let bytes = fs::read(&resolved.library).ok()?;
        let library = SymbolLibrary::parse(&bytes).ok()?;
        match library.find(&reference.item) {
            SymbolLookup::Found(entry) => Some(bytes[entry.span.clone()].to_vec()),
            _ => None,
        }
    } else {
        fs::read(resolver.footprint(&text).ok()?.file).ok()
    }
}

fn discovery_options(config: &BrowserConfig, settings: &Settings) -> DiscoveryOptions {
    // Never the person's KiCad settings, which kicad-cli can overwrite.
    let mut cli_settings = CliSettingsFolder::new(config.cache_dir.join("kicad-cli-settings"));
    if let Some(folder) = &config.kicad_config_dir {
        cli_settings = cli_settings.keep_away_from(folder.clone());
    }
    DiscoveryOptions {
        user_override: settings.kicad_cli_override.clone(),
        locations: config.kicad_locations.clone(),
        settings: cli_settings,
    }
}

fn kicad_status(discovery: &Discovery) -> KicadStatus {
    match discovery {
        Discovery::Ready(install) => KicadStatus::Ready {
            version: install.version.to_string(),
            cli_path: display(&install.cli),
            from_override: install.source == InstallSource::UserOverride,
        },
        Discovery::Missing { searched } => KicadStatus::Missing {
            searched: searched.iter().map(|path| display(path)).collect(),
        },
        Discovery::Unsupported { cli, version, .. } => KicadStatus::Unsupported {
            version: version.to_string(),
            cli_path: display(cli),
            supported_major: SUPPORTED_MAJOR,
        },
        Discovery::OverrideInvalid { path, problem } => KicadStatus::OverrideInvalid {
            path: display(path),
            problem: capitalize(problem),
        },
        Discovery::Broken { cli, message, .. } => KicadStatus::Broken {
            cli_path: display(cli),
            message: message.clone(),
        },
        Discovery::SettingsFolder { cli, message } => KicadStatus::SettingsFolder {
            cli_path: display(cli),
            message: message.clone(),
        },
    }
}

fn detail(part: &CatalogPart) -> PartDetail {
    PartDetail {
        summary: summary(part),
        symbol: part.symbol.clone(),
        footprint: part.footprint.clone(),
        datasheet: part.datasheet.clone(),
        fields: part
            .fields
            .iter()
            .map(|(name, value)| Field {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

/// The parts only the team catalog has, with the category code of each.
struct TeamOnlyParts {
    parts: Vec<CatalogPart>,
    categories: Vec<String>,
}

/// One match of a search, with its category code and rank (see
/// [`csv_catalog::SearchResults::ranks`]).
#[derive(Clone, Debug)]
struct Ranked {
    category: String,
    position: usize,
    rank: u8,
}

/// How [`group_by_category`] groups.
struct Grouping<'a> {
    /// Most positions kept per section; 0 keeps none (counts only).
    limit: usize,
    /// The team catalog's category names, by code.
    names: &'a BTreeMap<String, String>,
    /// Whether this is a search, which orders sections by best match.
    searching: bool,
    /// The selected part's position, always kept in its section.
    selected: Option<usize>,
}

/// The matches of one category, for one section of the part list.
#[derive(Debug, Eq, PartialEq)]
struct CategoryGroup {
    code: String,
    name: String,
    /// Every match in this category.
    total: usize,
    /// The rank of its best match.
    best_rank: u8,
    /// The first `limit` of them, best first, and the selected part.
    positions: Vec<usize>,
}

/// Groups ranked matches, given best first, into one group per category.
/// Each group keeps the rank order and at most `limit` positions (plus the
/// selected part), but counts every match. Groups are in name order; while
/// searching, the group with the best match comes first, ties by name.
/// "Other parts" (no category) is always last. A category without matches
/// makes no section, so an empty CSV file (g-mec.csv) has none: 21 CSV
/// files, 20 sections.
fn group_by_category(
    ranked: impl IntoIterator<Item = Ranked>,
    how: &Grouping<'_>,
) -> Vec<CategoryGroup> {
    let mut groups: BTreeMap<String, CategoryGroup> = BTreeMap::new();
    for Ranked {
        category,
        position,
        rank,
    } in ranked
    {
        let group = groups
            .entry(category)
            .or_insert_with_key(|code| CategoryGroup {
                name: category_name(code, how.names),
                code: code.clone(),
                total: 0,
                best_rank: rank,
                positions: Vec::new(),
            });
        group.total += 1;
        group.best_rank = group.best_rank.min(rank);
        if group.positions.len() < how.limit || how.selected == Some(position) {
            group.positions.push(position);
        }
    }
    let mut groups: Vec<CategoryGroup> = groups.into_values().collect();
    groups.sort_by(|a, b| {
        let rank = |g: &CategoryGroup| if how.searching { g.best_rank } else { 0 };
        (a.code.is_empty(), rank(a))
            .cmp(&(b.code.is_empty(), rank(b)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.code.cmp(&b.code))
    });
    groups
}

/// A category's plain name: the team catalog's, else the one it starts
/// with, else its code. A part without a category is in "Other parts".
fn category_name(code: &str, names: &BTreeMap<String, String>) -> String {
    if code.is_empty() {
        return "Other parts".to_owned();
    }
    names
        .get(code)
        .cloned()
        .or_else(|| catalog::category::starting_category(code).map(|start| start.name.to_owned()))
        .unwrap_or_else(|| code.to_owned())
}

fn summary(part: &CatalogPart) -> PartSummary {
    PartSummary {
        ipn: part.ipn.clone(),
        // A part only the team catalog has comes from no CSV file.
        csv_file: if part.category.is_empty() {
            "Team catalog".to_owned()
        } else {
            format!("{}.csv", part.category)
        },
        mpn: part.mpn.clone(),
        manufacturer: part.manufacturer.clone(),
        description: part.description.clone(),
        value: part.value.clone(),
    }
}

/// Why a user-entered path is not saved, or `None` if it is usable.
fn rejection(discovery: &Discovery) -> Option<String> {
    match discovery {
        Discovery::Ready(_) => None,
        Discovery::OverrideInvalid { problem, .. } => Some(capitalize(problem)),
        Discovery::Unsupported { cli, version, .. } => Some(format!(
            "{} is KiCad {version}; this app needs KiCad {SUPPORTED_MAJOR}.",
            cli.display()
        )),
        Discovery::Broken { message, .. } | Discovery::SettingsFolder { message, .. } => {
            Some(capitalize(message))
        }
        Discovery::Missing { .. } => Some("No kicad-cli was found there.".into()),
    }
}

/// Settings from `path`. A missing file means defaults; an unreadable or
/// invalid one is an error the UI reports.
fn load_settings(path: &Path) -> Result<Settings, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not valid settings JSON: {error}", path.display()))
}

/// Writes the settings to a temporary file and renames it into place, so a
/// crash never leaves a half-written `settings.json`.
fn save_settings(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, json).map_err(|e| format!("cannot save {}: {e}", temp.display()))?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("cannot save {}: {e}", path.display())
    })
}

/// The four numbers of the root `<svg>`'s `viewBox`, all finite and the
/// size positive.
fn svg_view_box(svg: &str) -> Option<[f64; 4]> {
    let tag = &svg[svg.find("<svg")?..];
    let tag = &tag[..tag.find('>')?];
    let value = tag.split("viewBox=\"").nth(1)?.split('"').next()?;
    let numbers: Vec<f64> = value
        .split([' ', ','])
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [x, y, width, height] = numbers.as_slice() else {
        return None;
    };
    let view_box = [*x, *y, *width, *height];
    (view_box.iter().all(|value| value.is_finite()) && *width > 0.0 && *height > 0.0)
        .then_some(view_box)
}

fn svg_data_uri(svg: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    format!("data:image/svg+xml;base64,{encoded}")
}

fn noun(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::Symbol => "symbol",
        PreviewKind::Footprint => "footprint",
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// A part's current values from somewhere other than the CSV catalog, such
/// as the team catalog. `None` keeps the CSV catalog's value.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartValues {
    /// The part's category code.
    pub category: Option<String>,
    /// Every details field, in order.
    pub fields: Option<Vec<(String, String)>>,
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub symbol: Option<String>,
    pub footprint: Option<String>,
}

impl PartValues {
    /// The part `ipn` as these values describe it, for a part the CSV
    /// catalog does not have.
    fn part(&self, ipn: &str) -> CatalogPart {
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        let reference =
            |value: &Option<String>| value.clone().filter(|value| !value.trim().is_empty());
        let fields: Vec<(String, String)> = self
            .fields
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, value)| !value.trim().is_empty())
            .collect();
        CatalogPart {
            ipn: ipn.to_owned(),
            category: String::new(),
            mpn: text(&self.mpn),
            manufacturer: text(&self.manufacturer),
            description: text(&self.description),
            value: text(&self.value),
            symbol: reference(&self.symbol),
            footprint: reference(&self.footprint),
            datasheet: fields
                .iter()
                .find(|(name, _)| name == "Datasheet")
                .map(|(_, value)| value.clone())
                .unwrap_or_default(),
            fields,
        }
    }

    pub(crate) fn apply(&self, part: &CatalogPart) -> CatalogPart {
        let pick = |new: &Option<String>, old: &String| new.clone().unwrap_or_else(|| old.clone());
        let pick_reference = |new: &Option<String>, old: &Option<String>| match new {
            Some(reference) if reference.trim().is_empty() => None,
            Some(reference) => Some(reference.clone()),
            None => old.clone(),
        };
        CatalogPart {
            mpn: pick(&self.mpn, &part.mpn),
            manufacturer: pick(&self.manufacturer, &part.manufacturer),
            description: pick(&self.description, &part.description),
            value: pick(&self.value, &part.value),
            symbol: pick_reference(&self.symbol, &part.symbol),
            footprint: pick_reference(&self.footprint, &part.footprint),
            ..part.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_svg_view_box() {
        let svg = "<?xml version=\"1.0\"?>
<svg
  width=\"3.0mm\" viewBox=\"0.000000 0.000000 3.048000 1.549400\">
<rect viewBox=\"1 2 3 4\"/>";
        assert_eq!(svg_view_box(svg), Some([0.0, 0.0, 3.048, 1.5494]));
        assert_eq!(
            svg_view_box("<svg viewBox=\"0,0,2,1\">"),
            Some([0.0, 0.0, 2.0, 1.0])
        );
        for bad in [
            "<svg>",
            "<svg viewBox=\"0 0 0 1\">",
            "<svg viewBox=\"0 0 NaN 1\">",
            "<svg viewBox=\"0 0 1\">",
            "<svg><g viewBox=\"0 0 1 1\">",
            "no svg",
        ] {
            assert_eq!(svg_view_box(bad), None, "{bad}");
        }
    }

    /// Ranked matches as (category, position, rank), best first.
    fn ranked(items: &[(&str, usize, u8)]) -> Vec<Ranked> {
        items
            .iter()
            .map(|(code, position, rank)| Ranked {
                category: (*code).to_owned(),
                position: *position,
                rank: *rank,
            })
            .collect()
    }

    fn grouping(limit: usize, searching: bool) -> Grouping<'static> {
        static NO_NAMES: BTreeMap<String, String> = BTreeMap::new();
        Grouping {
            limit,
            names: &NO_NAMES,
            searching,
            selected: None,
        }
    }

    fn summary_of(groups: &[CategoryGroup]) -> Vec<(&str, &str, usize, &[usize])> {
        groups
            .iter()
            .map(|g| {
                (
                    g.code.as_str(),
                    g.name.as_str(),
                    g.total,
                    g.positions.as_slice(),
                )
            })
            .collect()
    }

    #[test]
    fn without_a_search_sections_are_in_name_order_keeping_rank_order() {
        let groups = group_by_category(
            ranked(&[
                ("res", 7, 2),
                ("cap", 3, 2),
                ("res", 1, 2),
                ("ana", 9, 2),
                ("cap", 0, 2),
            ]),
            &grouping(10, false),
        );
        assert_eq!(
            summary_of(&groups),
            [
                ("ana", "Analog ICs", 1, &[9][..]),
                ("cap", "Capacitors", 2, &[3, 0][..]),
                ("res", "Resistors", 2, &[7, 1][..]),
            ]
        );
    }

    #[test]
    fn while_searching_the_section_with_the_best_match_comes_first() {
        // res holds the exact match (rank 0); cap and ana tie on rank 1,
        // so they go by name; dio has only rank-2 matches.
        let groups = group_by_category(
            ranked(&[
                ("res", 4, 0),
                ("cap", 3, 1),
                ("ana", 9, 1),
                ("dio", 2, 2),
                ("res", 1, 2),
            ]),
            &grouping(10, true),
        );
        let codes: Vec<&str> = groups.iter().map(|g| g.code.as_str()).collect();
        assert_eq!(codes, ["res", "ana", "cap", "dio"]);
    }

    #[test]
    fn other_parts_always_come_last() {
        for searching in [false, true] {
            let groups = group_by_category(
                ranked(&[("", 0, 0), ("res", 1, 2), ("zzz", 2, 2)]),
                &grouping(10, searching),
            );
            let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
            assert_eq!(names.last(), Some(&"Other parts"), "searching: {searching}");
        }
    }

    #[test]
    fn each_section_shows_at_most_the_limit_but_counts_every_match() {
        let many: Vec<Ranked> = (0..5)
            .map(|n| Ranked {
                category: "res".to_owned(),
                position: n,
                rank: 2,
            })
            .collect();
        let groups = group_by_category(many.clone(), &grouping(2, false));
        assert_eq!(
            (groups[0].total, groups[0].positions.as_slice()),
            (5, &[0, 1][..])
        );
        // Counts only: no parts at all.
        let groups = group_by_category(many.clone(), &grouping(0, false));
        assert_eq!((groups[0].total, groups[0].positions.len()), (5, 0));
        // The selected part is always among its section's parts.
        let groups = group_by_category(
            many,
            &Grouping {
                selected: Some(4),
                ..grouping(2, false)
            },
        );
        assert_eq!(groups[0].positions, [0, 1, 4]);
    }

    #[test]
    fn team_names_win_then_starting_names_then_the_code() {
        let names: BTreeMap<String, String> =
            [("res".to_owned(), "Fixed resistors".to_owned())].into();
        let groups = group_by_category(
            ranked(&[("res", 0, 2), ("cap", 1, 2), ("zzz", 2, 2), ("", 3, 2)]),
            &Grouping {
                names: &names,
                ..grouping(10, false)
            },
        );
        let shown: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            shown,
            ["Capacitors", "Fixed resistors", "zzz", "Other parts"]
        );
    }

    #[test]
    fn nothing_matching_makes_no_sections() {
        assert!(group_by_category(Vec::new(), &grouping(10, true)).is_empty());
    }
}

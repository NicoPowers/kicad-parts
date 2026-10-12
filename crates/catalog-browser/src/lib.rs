//! The "browse the catalog with exact KiCad previews" use cases, and the
//! team catalog ([`team`]): sign in, history and edits.
//!
//! The desktop shell wraps each method in a Tauri command and does nothing
//! else, so everything here is testable without a window.

pub mod team;

use base64::Engine;
use csv_catalog::{CatalogPart, CsvCatalog};
use desktop_contract::{
    AppStatus, CatalogStatus, Field, FileKind, FilePreviewRequest, KicadStatus, PartDetail,
    PartSummary, PreviewImage, PreviewKind, PreviewOutcome, PreviewProblem, PreviewProblemReason,
    PreviewRequest, PreviewSet, SearchResponse,
};
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup};
use kicad_integration::{
    CacheOutcome, Discovery, DiscoveryOptions, InstallSource, KicadInstall, LibRef, LibraryOrigin,
    LibraryResolver, Preview, PreviewCache, PreviewError, ResolveError, SUPPORTED_MAJOR,
    SearchLocations, StoredItem, discover, footprint_choices, sha256_hex,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Instant;
use team::{StoredCad, TeamCad, TeamCadError, TeamCatalog};

/// Largest page of search results the UI may request.
pub const MAX_SEARCH_LIMIT: usize = 500;

#[derive(Clone, Debug)]
pub struct BrowserConfig {
    /// Directory holding `database/`, `symbols/` and `footprints/`.
    pub catalog_root: PathBuf,
    /// The app's cache folder, kept apart from WebView2's data; previews go
    /// under `previews/`.
    pub cache_dir: PathBuf,
    /// JSON file holding user settings such as the kicad-cli override.
    pub settings_file: PathBuf,
    pub kicad_locations: SearchLocations,
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
        self.search_with(query, limit, &BTreeMap::new())
    }

    /// Like [`Self::search`], but the parts in `current` (by IPN, such as
    /// the team catalog's current details) are matched on and shown with
    /// those values, so a search finds what the app shows.
    pub fn search_with(
        &self,
        query: &str,
        limit: usize,
        current: &BTreeMap<String, PartValues>,
    ) -> SearchResponse {
        let started = Instant::now();
        let replace = |part: &CatalogPart| current.get(&part.ipn).map(|values| values.apply(part));
        let (total, hits) = match &self.catalog {
            Ok(catalog) => {
                let results = if current.is_empty() {
                    catalog.search(query, limit.min(MAX_SEARCH_LIMIT))
                } else {
                    catalog.search_replacing(query, limit.min(MAX_SEARCH_LIMIT), replace)
                };
                let hits = results
                    .hits
                    .iter()
                    .map(|&position| {
                        let part = &catalog.parts()[position];
                        summary(&replace(part).unwrap_or_else(|| part.clone()))
                    })
                    .collect();
                (results.total, hits)
            }
            Err(_) => (0, Vec::new()),
        };
        SearchResponse {
            query: query.to_owned(),
            total,
            hits,
            elapsed_micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        }
    }

    pub fn part(&self, ipn: &str) -> Option<PartDetail> {
        let part = self.catalog.as_ref().ok()?.find(ipn)?;
        Some(PartDetail {
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
        })
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
        let lib_ref = LibRef::parse(&request.reference)
            .ok()
            .filter(|reference| reference.item == name)
            .unwrap_or(LibRef {
                library: "g-file".into(),
                item: name,
            });
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

/// Where a preview was drawn from, for [`PreviewSet`].
struct Drawn {
    library_path: String,
    from_kicad_bundle: bool,
    cad_item_revision: Option<u32>,
    note: Option<String>,
}

fn ready(
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
            })
            .collect(),
    })
}

fn problem(
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

fn export_problem(kind: PreviewKind, reference: &str, error: PreviewError) -> PreviewOutcome {
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
    DiscoveryOptions {
        user_override: settings.kicad_cli_override.clone(),
        locations: config.kicad_locations.clone(),
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
    }
}

fn summary(part: &CatalogPart) -> PartSummary {
    PartSummary {
        ipn: part.ipn.clone(),
        csv_file: format!("{}.csv", part.category),
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
        Discovery::Broken { message, .. } => Some(capitalize(message)),
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
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub symbol: Option<String>,
    pub footprint: Option<String>,
}

impl PartValues {
    fn apply(&self, part: &CatalogPart) -> CatalogPart {
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

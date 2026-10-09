//! The "browse the catalog with exact KiCad previews" use cases.
//!
//! The desktop shell wraps each method in a Tauri command and does nothing
//! else, so everything here is testable without a window.

use base64::Engine;
use desktop_contract::{
    AppStatus, CatalogStatus, Field, KicadStatus, PartDetail, PartSummary, PreviewImage,
    PreviewKind, PreviewOutcome, PreviewProblem, PreviewProblemReason, PreviewSet, SearchResponse,
};
use kicad_integration::{
    CacheOutcome, Discovery, DiscoveryOptions, InstallSource, LibraryOrigin, LibraryResolver,
    PreviewCache, PreviewError, ResolveError, SUPPORTED_MAJOR, SearchLocations, discover,
    footprint_choices,
};
use legacy_catalog::{CatalogPart, LegacyCatalog};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Instant;

/// Largest page of search results the UI may request.
pub const MAX_SEARCH_LIMIT: usize = 500;

#[derive(Clone, Debug)]
pub struct BrowserConfig {
    /// Directory holding `database/`, `symbols/` and `footprints/`.
    pub catalog_root: PathBuf,
    /// Application cache directory; previews go under `previews/`.
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
    catalog: Result<LegacyCatalog, String>,
    previews: PreviewCache,
    kicad: RwLock<KicadState>,
}

impl CatalogBrowser {
    /// Loads the catalog and settings and looks for KiCad. Problems become
    /// part of [`AppStatus`] rather than errors, so the UI can explain them.
    pub fn open(config: BrowserConfig) -> Self {
        let catalog = LegacyCatalog::load(&config.catalog_root).map_err(|e| e.to_string());
        let (settings, settings_error) = match load_settings(&config.settings_file) {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(error)),
        };
        let discovery = discover(&discovery_options(&config, &settings));
        Self {
            previews: PreviewCache::new(config.cache_dir.join("previews")),
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
        let started = Instant::now();
        let (total, hits) = match &self.catalog {
            Ok(catalog) => {
                let results = catalog.search(query, limit.min(MAX_SEARCH_LIMIT));
                let hits = results
                    .hits
                    .iter()
                    .map(|&position| summary(&catalog.parts()[position]))
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
        let problem = |reason, reference: Option<&str>, message: String| {
            PreviewOutcome::Problem(PreviewProblem {
                kind,
                reason,
                reference: reference.map(str::to_owned),
                message,
                command: None,
                cli_message: None,
            })
        };
        let Some(part) = self.catalog.as_ref().ok().and_then(|c| c.find(ipn)) else {
            return problem(
                PreviewProblemReason::UnknownPart,
                None,
                format!("Part {ipn} is not in the catalog."),
            );
        };
        let field = match kind {
            PreviewKind::Symbol => part.symbol.as_deref(),
            PreviewKind::Footprint => part.footprint.as_deref(),
        };
        // Footprint fields may list `;`-separated alternatives; symbol
        // fields hold exactly one reference.
        let choices = match (kind, field) {
            (_, None) => Vec::new(),
            (PreviewKind::Symbol, Some(field)) => vec![field],
            (PreviewKind::Footprint, Some(field)) => footprint_choices(field),
        };
        let Some((&reference, alternates)) = choices.split_first() else {
            return problem(
                PreviewProblemReason::Unassigned,
                None,
                format!("{ipn} has no {} assigned in the catalog.", noun(kind)),
            );
        };

        let install = {
            let kicad = self.kicad.read().unwrap_or_else(|e| e.into_inner());
            match &kicad.discovery {
                Discovery::Ready(install) => install.clone(),
                _ => {
                    return problem(
                        PreviewProblemReason::KicadUnavailable,
                        Some(reference),
                        "Previews are drawn by KiCad 10's kicad-cli, which is not available."
                            .into(),
                    );
                }
            }
        };
        let resolver = LibraryResolver::new(
            Some(self.config.catalog_root.clone()),
            install.share_dir.clone(),
        );
        let resolved = match kind {
            PreviewKind::Symbol => resolver.symbol(reference).map(|symbol| {
                let library = symbol.library.clone();
                let origin = symbol.origin;
                (self.previews.symbol(&install, &symbol), library, origin)
            }),
            PreviewKind::Footprint => resolver.footprint(reference).map(|footprint| {
                let library = footprint.library.clone();
                let origin = footprint.origin;
                (
                    self.previews.footprint(&install, &footprint),
                    library,
                    origin,
                )
            }),
        };
        let (exported, library, origin) = match resolved {
            Ok(resolved) => resolved,
            Err(error) => {
                let reason = match error {
                    ResolveError::Unassigned => PreviewProblemReason::Unassigned,
                    _ => PreviewProblemReason::Unresolved,
                };
                return problem(reason, Some(reference), capitalize(&error.to_string()));
            }
        };
        match exported {
            Ok(preview) => PreviewOutcome::Ready(PreviewSet {
                kind,
                reference: reference.to_owned(),
                alternates: alternates
                    .iter()
                    .map(|choice| (*choice).to_owned())
                    .collect(),
                library_path: display(&library),
                from_kicad_bundle: origin == LibraryOrigin::KicadBundled,
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
            }),
            Err(PreviewError::ExportFailed {
                command,
                cli_message,
            }) => PreviewOutcome::Problem(PreviewProblem {
                kind,
                reason: PreviewProblemReason::ExportFailed,
                reference: Some(reference.to_owned()),
                message: format!("kicad-cli could not export the {} {reference}.", noun(kind)),
                command: Some(command),
                cli_message: Some(cli_message),
            }),
            Err(PreviewError::Unexportable(message)) => {
                problem(PreviewProblemReason::Unexportable, Some(reference), message)
            }
            Err(PreviewError::Io(message)) => {
                problem(PreviewProblemReason::Internal, Some(reference), message)
            }
        }
    }

    /// How many times kicad-cli has exported a preview in this session.
    pub fn export_count(&self) -> usize {
        self.previews.export_count()
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

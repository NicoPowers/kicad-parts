//! Desktop shell. It parses launch options, picks application directories,
//! and exposes `catalog-browser` use cases as Tauri commands. Behaviour lives
//! in the crates under `crates/`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod sign_ins;

use catalog_browser::kicad_libraries::KicadLibraries;
use catalog_browser::teams::Teams;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use desktop_contract::{
    AppStatus, FilePreviewRequest, Footprint3dRequest, Footprint3dView, FootprintDrawing,
    KicadLibrariesStatus, LibrariesView, LibraryCheckView, LibraryItemDetail, LibraryItemRequest,
    LibraryItems, LibraryItemsRequest, OurLibraries, PartDetail, PreviewOutcome, PreviewRequest,
    PublishResult, SearchRequest, SearchResponse, SignInRequest, TeamArchiveRequest, TeamCodeCheck,
    TeamCsvReviewRequest, TeamDownload, TeamDownloadRequest, TeamEditRequest, TeamFileReading,
    TeamFileRequest, TeamIpnCheck, TeamIpnRequest, TeamMpnRequest, TeamNewPartRequest, TeamPart,
    TeamPick, TeamPickRequest, TeamSameMpn, TeamsStatus, UseNewServerRequest,
};
use kicad_integration::{SearchLocations, remove_old_cache};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{Manager, PhysicalSize, State};

type Browser = Arc<CatalogBrowser>;
type Team = Arc<Teams>;
type Libraries = Arc<KicadLibraries>;

/// Launch options. Every option exists so tests and unusual installs can run
/// the app without touching the user's settings or KiCad search.
#[derive(Debug, Default, PartialEq)]
struct LaunchOptions {
    /// `--catalog-root DIR`: holds `database/`, `symbols/`, `footprints/`.
    catalog_root: Option<PathBuf>,
    /// `--data-dir DIR`: settings, preview cache and the KiCad libraries,
    /// instead of the per-user application directories.
    data_dir: Option<PathBuf>,
    /// `--kicad-search-root DIR` (repeatable): look for KiCad only here.
    kicad_search_roots: Vec<PathBuf>,
    /// `--kicad-config-dir DIR`: KiCad's settings folder whose library
    /// tables list the person's installed libraries, instead of their own
    /// (the journeys pass fixture tables). Only ever read.
    kicad_config_dir: Option<PathBuf>,
    /// `--team-server URL`: the team catalog server to suggest at sign-in.
    team_server: Option<String>,
    /// `--team-email EMAIL`: the email to suggest at sign-in.
    team_email: Option<String>,
    /// `--sign-ins=memory` or `--sign-ins=until-closed`: keep team sign-ins
    /// in memory instead of the system's credential store, so tests never
    /// touch the person's store. See `sign_ins::for_tests`.
    sign_ins: Option<String>,
    /// `--display-scale N` (Windows only, N from 0.5 to 4): draw N screen pixels per CSS
    /// pixel, whatever the Windows display scaling is. The journeys use 1, so
    /// their screenshots are 1280x800 on every machine.
    display_scale: Option<f64>,
    /// `--software-rendering` (Windows only, no value): WebView2 draws
    /// without the GPU and always redraws whole tiles. The journeys use it
    /// so screenshots are drawn the same way every run: a sleeping or reset
    /// GPU, or a partly redrawn tile, changes how text and curves are
    /// smoothed.
    software_rendering: bool,
}

impl LaunchOptions {
    /// Accepts `--name VALUE` and `--name=VALUE`. WebDriver launches the
    /// app as if it were a browser and rewrites standalone arguments, so the
    /// journeys use the `=` form.
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let text = arg.to_string_lossy().into_owned();
            let (name, inline) = match text.split_once('=') {
                Some((name, value)) => (name.to_owned(), Some(OsString::from(value))),
                None => (text.clone(), None),
            };
            if name == "--software-rendering" && inline.is_none() {
                options.software_rendering = true;
                continue;
            }
            let target = match name.as_str() {
                "--catalog-root" => Target::CatalogRoot,
                "--data-dir" => Target::DataDir,
                "--kicad-search-root" => Target::KicadSearchRoot,
                "--kicad-config-dir" => Target::KicadConfigDir,
                "--team-server" => Target::TeamServer,
                "--team-email" => Target::TeamEmail,
                "--sign-ins" => Target::SignIns,
                "--display-scale" => Target::DisplayScale,
                // WebView and test drivers may add their own arguments.
                _ => continue,
            };
            let value = match inline {
                Some(value) => value,
                None => args.next().ok_or_else(|| format!("{name} needs a value"))?,
            };
            let text = || value.to_string_lossy().into_owned();
            match target {
                Target::CatalogRoot => options.catalog_root = Some(PathBuf::from(&value)),
                Target::DataDir => options.data_dir = Some(PathBuf::from(&value)),
                Target::KicadSearchRoot => options.kicad_search_roots.push(PathBuf::from(&value)),
                Target::KicadConfigDir => options.kicad_config_dir = Some(PathBuf::from(&value)),
                Target::TeamServer => options.team_server = Some(text()),
                Target::TeamEmail => options.team_email = Some(text()),
                Target::SignIns => options.sign_ins = Some(text()),
                Target::DisplayScale => options.display_scale = Some(parse_scale(&text())?),
            }
        }
        Ok(options)
    }
}

enum Target {
    CatalogRoot,
    DataDir,
    KicadSearchRoot,
    KicadConfigDir,
    TeamServer,
    TeamEmail,
    SignIns,
    DisplayScale,
}

/// The display scales `--display-scale` accepts: 50% to 400%.
const DISPLAY_SCALES: std::ops::RangeInclusive<f64> = 0.5..=4.0;

fn parse_scale(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|scale| DISPLAY_SCALES.contains(scale))
        .ok_or_else(|| {
            format!(
                "--display-scale needs a number from {} to {}, not {text}",
                DISPLAY_SCALES.start(),
                DISPLAY_SCALES.end()
            )
        })
}

/// The browser switches the launch options need: a fixed scale, and
/// drawing without the GPU in whole tiles.
fn webview_switches(display_scale: Option<f64>, software_rendering: bool) -> Vec<String> {
    let mut switches = Vec::new();
    if let Some(scale) = display_scale {
        switches.push(format!("--force-device-scale-factor={scale}"));
    }
    if software_rendering {
        switches.push("--disable-gpu".to_owned());
        switches.push("--disable-partial-raster".to_owned());
    }
    switches
}

/// `existing` with `added` at the end. WebView2 reads extra switches from
/// `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, and the WebDriver that runs the
/// journeys fills that variable with its own switches, so the app adds to
/// them instead of replacing them.
fn with_switches(existing: &str, added: &[String]) -> String {
    let mut all = existing
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    all.extend(added.iter().cloned());
    all.join(" ")
}

const WEBVIEW2_SWITCHES: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

fn add_webview_switches(added: &[String]) {
    let existing = std::env::var(WEBVIEW2_SWITCHES).unwrap_or_default();
    // SAFETY: main calls this before Tauri starts, while it is the only thread.
    unsafe { std::env::set_var(WEBVIEW2_SWITCHES, with_switches(&existing, added)) };
}

/// With a fixed WebView scale, Windows display scaling would still enlarge
/// the window, so give the configured window size in screen pixels.
fn size_window_in_screen_pixels(app: &tauri::App, scale: f64) -> tauri::Result<()> {
    let Some(config) = app.config().app.windows.first() else {
        return Ok(());
    };
    let Some(window) = app.get_webview_window(&config.label) else {
        return Ok(());
    };
    let pixels = |css: f64| (css * scale).round() as u32;
    if let (Some(width), Some(height)) = (config.min_width, config.min_height) {
        window.set_min_size(Some(PhysicalSize::new(pixels(width), pixels(height))))?;
    }
    window.set_size(PhysicalSize::new(
        pixels(config.width),
        pixels(config.height),
    ))
}

fn default_catalog_root() -> PathBuf {
    if let Some(root) = std::env::var_os("KICAD_PARTS_CATALOG_ROOT") {
        return PathBuf::from(root);
    }
    // Until the canonical catalog exists, development builds browse the
    // CSV catalog in this repository.
    normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// Removes `.` and `..` lexically, so paths shown in the UI stay readable
/// (`canonicalize` would add a `\\?\` prefix on Windows).
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

/// The app's cache folder. On Windows, Tauri's cache folder is the folder
/// WebView2 keeps its `EBWebView` data in, so the cache goes to a sibling
/// folder named `<identifier>-cache` instead; clearing either then never
/// touches the other. Elsewhere the cache folder is already separate.
fn cache_dir_apart(app_cache_dir: &Path, webview_data_dir: &Path) -> PathBuf {
    if app_cache_dir != webview_data_dir {
        return app_cache_dir.to_path_buf();
    }
    let mut name = app_cache_dir
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("kicad-parts"));
    name.push("-cache");
    app_cache_dir.with_file_name(name)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("background task failed: {error}"))
}

#[tauri::command]
async fn app_status(browser: State<'_, Browser>) -> Result<AppStatus, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.status()).await
}

#[tauri::command]
async fn rediscover_kicad(browser: State<'_, Browser>) -> Result<AppStatus, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.rediscover_kicad()).await
}

#[tauri::command]
async fn set_kicad_override(
    browser: State<'_, Browser>,
    path: Option<String>,
) -> Result<AppStatus, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.set_kicad_override(path)).await?
}

#[tauri::command]
async fn search_parts(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: SearchRequest,
) -> Result<SearchResponse, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    // Parts the team catalog has are found by, and shown with, its current
    // details, as the part header shows them.
    // The list is grouped by the team catalog's category names.
    blocking(move || {
        browser.search_request(&request, &team.current_values(), &team.category_names())
    })
    .await
}

#[tauri::command]
async fn part_detail(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    ipn: String,
) -> Result<Option<PartDetail>, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    // A part only the team catalog has, such as one added in the app, is
    // found there.
    // The team catalog's values are read only when the CSV catalog lacks it.
    blocking(move || {
        browser
            .part(&ipn)
            .or_else(|| browser.part_with(&ipn, &team.current_values()))
    })
    .await
}

#[tauri::command]
async fn part_preview(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: PreviewRequest,
) -> Result<PreviewOutcome, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    // Team parts are drawn from the stored bytes of the exact CAD item
    // revision they use.
    blocking(move || browser.preview_request(&request, Some(&team))).await
}

#[tauri::command]
async fn footprint_drawing(
    browser: State<'_, Browser>,
    key: String,
) -> Result<Option<FootprintDrawing>, String> {
    let browser = browser.inner().clone();
    // Read only when a close-up or the Libraries browser shows it.
    blocking(move || browser.footprint_drawing(&key)).await
}

#[tauri::command]
async fn footprint_3d(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: Footprint3dRequest,
) -> Result<Footprint3dView, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    // Made only when the person turns 3D on, because it takes seconds.
    blocking(move || browser.footprint_3d(&request, &team)).await
}

/// The GLB of a 3D view, sent as raw bytes rather than JSON, so a large
/// model is not copied into text first.
#[tauri::command]
async fn footprint_glb(
    browser: State<'_, Browser>,
    key: String,
) -> Result<tauri::ipc::Response, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.footprint_glb(&key))
        .await?
        .map(tauri::ipc::Response::new)
        .ok_or_else(|| "The 3D view is no longer in the app's cache. Turn 3D on again.".to_owned())
}

/// Every team in the list, each with whether its KiCad libraries wait
/// for the person to confirm a new server address.
fn teams_status(teams: &Teams, libraries: &KicadLibraries) -> TeamsStatus {
    let mut status = teams.status();
    libraries.note_other_servers(&mut status);
    status
}

#[tauri::command]
async fn team_status(
    team: State<'_, Team>,
    libraries: State<'_, Libraries>,
) -> Result<TeamsStatus, String> {
    let (teams, libraries) = (team.inner().clone(), libraries.inner().clone());
    blocking(move || teams_status(&teams, &libraries)).await
}

/// Signs in to one team, adding it to the list; other teams stay signed
/// in. A failure is said in the status, where the sign-in form shows it.
#[tauri::command]
async fn team_sign_in(
    team: State<'_, Team>,
    libraries: State<'_, Libraries>,
    request: SignInRequest,
) -> Result<TeamsStatus, String> {
    let (teams, libraries) = (team.inner().clone(), libraries.inner().clone());
    blocking(move || {
        let _ = teams.sign_in(request);
        teams_status(&teams, &libraries)
    })
    .await
}

#[tauri::command]
async fn team_sign_out(
    team: State<'_, Team>,
    libraries: State<'_, Libraries>,
    server_url: String,
) -> Result<TeamsStatus, String> {
    let (teams, libraries) = (team.inner().clone(), libraries.inner().clone());
    blocking(move || {
        teams.sign_out(&server_url);
        teams_status(&teams, &libraries)
    })
    .await
}

#[tauri::command]
async fn team_remove(
    team: State<'_, Team>,
    libraries: State<'_, Libraries>,
    server_url: String,
) -> Result<TeamsStatus, String> {
    let (teams, libraries) = (team.inner().clone(), libraries.inner().clone());
    blocking(move || {
        teams.remove(&server_url);
        teams_status(&teams, &libraries)
    })
    .await
}

/// Shows another team in the catalog view; no sign-in is needed.
#[tauri::command]
async fn team_show(
    team: State<'_, Team>,
    libraries: State<'_, Libraries>,
    server_url: String,
) -> Result<TeamsStatus, String> {
    let (teams, libraries) = (team.inner().clone(), libraries.inner().clone());
    blocking(move || {
        teams.show(&server_url)?;
        Ok(teams_status(&teams, &libraries))
    })
    .await?
}

#[tauri::command]
async fn team_part(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    ipn: String,
) -> Result<Option<TeamPart>, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.team_part(&ipn, &team)).await?
}

#[tauri::command]
async fn team_edit(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: TeamEditRequest,
) -> Result<PublishResult, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.edit(request, &team)).await?
}

#[tauri::command]
async fn team_pick(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: TeamPickRequest,
) -> Result<TeamPick, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.pick(&request, &team)).await?
}

#[tauri::command]
async fn team_item_preview(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: LibraryItemRequest,
) -> Result<PreviewOutcome, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.catalog_item_preview(&request, &team)).await
}

#[tauri::command]
async fn our_libraries(browser: State<'_, Browser>) -> Result<OurLibraries, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.our_libraries()).await
}

#[tauri::command]
async fn team_archive(
    team: State<'_, Team>,
    request: TeamArchiveRequest,
) -> Result<PublishResult, String> {
    let team = team.shown();
    blocking(move || team.archive(request)).await?
}

#[tauri::command]
async fn team_csv_review(
    team: State<'_, Team>,
    request: TeamCsvReviewRequest,
) -> Result<PublishResult, String> {
    let team = team.shown();
    blocking(move || team.csv_review(request)).await?
}

#[tauri::command]
async fn team_read_file(
    team: State<'_, Team>,
    request: TeamFileRequest,
) -> Result<TeamFileReading, String> {
    let team = team.shown();
    blocking(move || team.read_file(request)).await?
}

#[tauri::command]
async fn team_preview_file(
    browser: State<'_, Browser>,
    request: FilePreviewRequest,
) -> Result<PreviewOutcome, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.preview_file(&request)).await
}

#[tauri::command]
async fn team_read_download(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: TeamDownloadRequest,
) -> Result<TeamDownload, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.read_download(&request, &team)).await?
}

#[tauri::command]
async fn team_check_ipn(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: TeamIpnRequest,
) -> Result<TeamIpnCheck, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.check_ipn(&request.ipn, &request.prefix, &team)).await
}

#[tauri::command]
async fn team_check_mpn(
    team: State<'_, Team>,
    request: TeamMpnRequest,
) -> Result<Vec<TeamSameMpn>, String> {
    let team = team.shown();
    blocking(move || team.same_mpn(&request.mpn, &request.manufacturer)).await
}

#[tauri::command]
async fn team_add_part(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: TeamNewPartRequest,
) -> Result<PublishResult, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.add_part(request, &team)).await?
}

#[tauri::command]
async fn team_library_check(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
) -> Result<Option<LibraryCheckView>, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    // Worked out again only when the team catalog changed.
    blocking(move || browser.library_check(&team)).await?
}

#[tauri::command]
async fn libraries(browser: State<'_, Browser>, refresh: bool) -> Result<LibrariesView, String> {
    let browser = browser.inner().clone();
    // With refresh, the index checks every library in the background.
    blocking(move || browser.libraries(refresh)).await
}

#[tauri::command]
async fn library_items(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: LibraryItemsRequest,
) -> Result<LibraryItems, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.library_items(&request, &team.current_values())).await
}

#[tauri::command]
async fn library_item(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: LibraryItemRequest,
) -> Result<Option<LibraryItemDetail>, String> {
    let (browser, team) = (browser.inner().clone(), team.shown());
    blocking(move || browser.library_item(&request, &team.current_values())).await
}

#[tauri::command]
async fn library_preview(
    browser: State<'_, Browser>,
    request: LibraryItemRequest,
) -> Result<PreviewOutcome, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.library_preview(&request)).await
}

#[tauri::command]
async fn kicad_libraries_status(
    libraries: State<'_, Libraries>,
) -> Result<KicadLibrariesStatus, String> {
    let libraries = libraries.inner().clone();
    blocking(move || libraries.status()).await
}

#[tauri::command]
async fn kicad_libraries_update(
    libraries: State<'_, Libraries>,
) -> Result<KicadLibrariesStatus, String> {
    let libraries = libraries.inner().clone();
    // Runs on the libraries' own thread; this only asks for it.
    blocking(move || libraries.update_now()).await
}

/// The person copied the setup text for `kind` (`symbols` or
/// `footprints`).
#[tauri::command]
async fn kicad_libraries_copied(
    libraries: State<'_, Libraries>,
    kind: String,
) -> Result<KicadLibrariesStatus, String> {
    let libraries = libraries.inner().clone();
    blocking(move || libraries.copied(&kind)).await
}

#[tauri::command]
async fn team_code_check(
    libraries: State<'_, Libraries>,
    code: String,
) -> Result<TeamCodeCheck, String> {
    let libraries = libraries.inner().clone();
    blocking(move || libraries.check_code(&code)).await
}

/// Chooses the team's code on the team server, once.
#[tauri::command]
async fn team_code_set(
    libraries: State<'_, Libraries>,
    code: String,
) -> Result<KicadLibrariesStatus, String> {
    let libraries = libraries.inner().clone();
    blocking(move || libraries.set_code(&code)).await?
}

/// The person said a team moved to its server, so its KiCad libraries,
/// made from its old address, take updates from it again.
#[tauri::command]
async fn kicad_libraries_use_new_server(
    team: State<'_, Team>,
    libraries: State<'_, Libraries>,
    request: UseNewServerRequest,
) -> Result<TeamsStatus, String> {
    let (teams, libraries) = (team.inner().clone(), libraries.inner().clone());
    blocking(move || {
        libraries.use_new_server(&request)?;
        Ok(teams_status(&teams, &libraries))
    })
    .await?
}

/// Shows the KiCad libraries folder in the system's file manager.
#[tauri::command]
async fn kicad_libraries_open_folder(libraries: State<'_, Libraries>) -> Result<(), String> {
    let folder = libraries.inner().folder();
    blocking(move || {
        std::fs::create_dir_all(&folder)
            .map_err(|error| format!("Could not make {}: {error}", folder.display()))?;
        let manager = if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(manager)
            .arg(&folder)
            .spawn()
            .map(drop)
            .map_err(|error| format!("Could not open {}: {error}", folder.display()))
    })
    .await?
}

fn main() {
    let options = match LaunchOptions::parse(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("kicad-parts-desktop: {message}");
            std::process::exit(2);
        }
    };
    let sign_ins = match options.sign_ins.as_deref() {
        None => sign_ins::system_or_memory(),
        Some(choice) => match sign_ins::for_tests(choice) {
            Ok(sign_ins) => sign_ins,
            Err(message) => {
                eprintln!("kicad-parts-desktop: {message}");
                std::process::exit(2);
            }
        },
    };
    // Both options need WebView2 switches, so they are Windows only.
    let display_scale = options.display_scale.filter(|_| cfg!(windows));
    let switches = webview_switches(display_scale, options.software_rendering && cfg!(windows));
    if !switches.is_empty() {
        add_webview_switches(&switches);
    }

    tauri::Builder::default()
        .setup(move |app| {
            if let Some(scale) = display_scale {
                size_window_in_screen_pixels(app, scale)?;
            }
            // The KiCad libraries belong to this computer and are made again
            // from the team catalog, but KiCad reads them, so they are kept
            // with the app's data, not in its cache that may be cleared.
            let libraries_dir = match &options.data_dir {
                Some(dir) => dir.join("kicad-libraries"),
                None => app.path().app_local_data_dir()?.join("kicad-libraries"),
            };
            let (config_dir, cache_dir) = match &options.data_dir {
                Some(dir) => (dir.join("config"), dir.join("cache")),
                None => {
                    let shared = app.path().app_cache_dir()?;
                    let cache_dir = cache_dir_apart(&shared, &app.path().app_local_data_dir()?);
                    if cache_dir != shared {
                        // Earlier versions kept previews next to WebView2's
                        // data; remove them in the background.
                        let old = shared.join("previews");
                        std::thread::spawn(move || remove_old_cache(&old));
                    }
                    (app.path().app_config_dir()?, cache_dir)
                }
            };
            let kicad_locations = if options.kicad_search_roots.is_empty() {
                SearchLocations::Standard
            } else {
                SearchLocations::Only(options.kicad_search_roots.clone())
            };
            let browser = CatalogBrowser::open(BrowserConfig {
                catalog_root: options
                    .catalog_root
                    .clone()
                    .unwrap_or_else(default_catalog_root),
                cache_dir,
                settings_file: config_dir.join("settings.json"),
                kicad_locations,
                // The person's own KiCad settings, read only.
                kicad_config_dir: options.kicad_config_dir.clone().or_else(|| {
                    let environment = std::env::vars().collect();
                    kicad_integration::lib_table::user_config_dir(
                        kicad_integration::SUPPORTED_MAJOR,
                        &environment,
                    )
                }),
                // The real Documents folder, which may be under OneDrive.
                kicad_documents_dir: app.path().document_dir().ok(),
            });
            app.manage(Arc::new(browser));
            // Each team's server address and email are saved in a file;
            // its renewable sign-in only in the credential store.
            let teams = Arc::new(Teams::open(
                config_dir.join("team.json"),
                sign_ins.clone(),
                options.team_server.clone(),
                options.team_email.clone(),
            ));
            app.manage(teams.clone());
            app.manage(Arc::new(KicadLibraries::start(libraries_dir, teams)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            rediscover_kicad,
            set_kicad_override,
            search_parts,
            part_detail,
            part_preview,
            footprint_drawing,
            footprint_3d,
            footprint_glb,
            team_status,
            team_sign_in,
            team_sign_out,
            team_remove,
            team_show,
            team_part,
            team_edit,
            team_pick,
            our_libraries,
            team_item_preview,
            team_archive,
            team_csv_review,
            team_read_file,
            team_preview_file,
            team_read_download,
            team_check_ipn,
            team_add_part,
            team_check_mpn,
            team_library_check,
            libraries,
            library_items,
            library_item,
            library_preview,
            kicad_libraries_status,
            kicad_libraries_update,
            kicad_libraries_open_folder,
            kicad_libraries_copied,
            team_code_check,
            team_code_set,
            kicad_libraries_use_new_server,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the KiCad Parts desktop app");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launch_options_and_ignores_unknown_arguments() {
        let args = [
            "--data-dir",
            "D",
            "--kicad-search-root",
            "A",
            "--webview-flag",
            "--kicad-search-root=B",
            "--kicad-config-dir=K",
            "--catalog-root",
            "C",
            "--team-server=http://127.0.0.1:8090",
            "--team-email",
            "a@example.test",
            "--sign-ins=memory",
            "--display-scale=1",
            "--software-rendering",
        ]
        .map(OsString::from);
        let options = LaunchOptions::parse(args).unwrap();
        assert_eq!(options.data_dir, Some(PathBuf::from("D")));
        assert_eq!(options.catalog_root, Some(PathBuf::from("C")));
        assert_eq!(options.kicad_config_dir, Some(PathBuf::from("K")));
        assert_eq!(
            options.kicad_search_roots,
            [PathBuf::from("A"), PathBuf::from("B")]
        );
        assert_eq!(
            options.team_server.as_deref(),
            Some("http://127.0.0.1:8090")
        );
        assert_eq!(options.team_email.as_deref(), Some("a@example.test"));
        assert_eq!(options.sign_ins.as_deref(), Some("memory"));
        assert_eq!(options.display_scale, Some(1.0));
        assert!(options.software_rendering);
        assert!(LaunchOptions::parse([OsString::from("--data-dir")]).is_err());
    }

    #[test]
    fn rejects_a_display_scale_outside_half_to_four() {
        for scale in ["0", "-1", "0.49", "4.01", "100", "big", "NaN", "inf"] {
            let arg = OsString::from(format!("--display-scale={scale}"));
            assert!(LaunchOptions::parse([arg]).is_err(), "{scale}");
        }
        assert_eq!(parse_scale("1.5"), Ok(1.5));
        assert_eq!(parse_scale("0.5"), Ok(0.5));
        assert_eq!(parse_scale("4"), Ok(4.0));
        assert_eq!(
            parse_scale("9"),
            Err("--display-scale needs a number from 0.5 to 4, not 9".to_owned())
        );
    }

    #[test]
    fn the_switches_are_added_after_the_drivers_switches() {
        let both = webview_switches(Some(1.0), true);
        assert_eq!(
            both,
            [
                "--force-device-scale-factor=1",
                "--disable-gpu",
                "--disable-partial-raster"
            ]
        );
        assert_eq!(
            with_switches("", &both[..1]),
            "--force-device-scale-factor=1"
        );
        assert_eq!(
            with_switches("--remote-debugging-port=0 ", &both),
            "--remote-debugging-port=0 --force-device-scale-factor=1 --disable-gpu --disable-partial-raster"
        );
        assert_eq!(
            webview_switches(Some(1.25), false),
            ["--force-device-scale-factor=1.25"]
        );
        assert!(webview_switches(None, false).is_empty());
    }

    #[test]
    fn command_names_match_the_ui_contract() {
        use desktop_contract::command;
        let source = include_str!("main.rs");
        for name in [
            command::APP_STATUS,
            command::REDISCOVER_KICAD,
            command::SET_KICAD_OVERRIDE,
            command::SEARCH_PARTS,
            command::PART_DETAIL,
            command::PART_PREVIEW,
            command::FOOTPRINT_DRAWING,
            command::FOOTPRINT_3D,
            command::FOOTPRINT_GLB,
            command::TEAM_STATUS,
            command::TEAM_SIGN_IN,
            command::TEAM_SIGN_OUT,
            command::TEAM_REMOVE,
            command::TEAM_SHOW,
            command::TEAM_PART,
            command::TEAM_EDIT,
            command::TEAM_ARCHIVE,
            command::TEAM_CSV_REVIEW,
            command::TEAM_READ_FILE,
            command::TEAM_PREVIEW_FILE,
            command::TEAM_READ_DOWNLOAD,
            command::TEAM_CHECK_IPN,
            command::TEAM_ADD_PART,
            command::TEAM_CHECK_MPN,
            command::TEAM_LIBRARY_CHECK,
            command::LIBRARIES,
            command::LIBRARY_ITEMS,
            command::LIBRARY_ITEM,
            command::LIBRARY_PREVIEW,
            command::TEAM_PICK,
            command::OUR_LIBRARIES,
            command::TEAM_ITEM_PREVIEW,
            command::KICAD_LIBRARIES_STATUS,
            command::KICAD_LIBRARIES_UPDATE,
            command::KICAD_LIBRARIES_OPEN_FOLDER,
            command::KICAD_LIBRARIES_COPIED,
            command::TEAM_CODE_CHECK,
            command::TEAM_CODE_SET,
            command::KICAD_LIBRARIES_USE_NEW_SERVER,
        ] {
            assert!(
                source.contains(&format!("async fn {name}(")),
                "no Tauri command named {name}"
            );
        }
    }

    #[test]
    fn the_cache_folder_is_kept_apart_from_webview_data() {
        let local = Path::new("C:/Users/a/AppData/Local/org.kicadparts.desktop");
        assert_eq!(
            cache_dir_apart(local, local),
            Path::new("C:/Users/a/AppData/Local/org.kicadparts.desktop-cache")
        );
        let cache = Path::new("/home/a/.cache/org.kicadparts.desktop");
        let data = Path::new("/home/a/.local/share/org.kicadparts.desktop");
        assert_eq!(cache_dir_apart(cache, data), cache);
    }

    #[test]
    fn default_catalog_root_is_the_repository() {
        let root = normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
        assert!(root.join("database").is_dir(), "{root:?}");
        assert!(!root.to_string_lossy().contains(".."));
    }
}

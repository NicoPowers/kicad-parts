//! Desktop shell. It parses launch options, picks application directories,
//! and exposes `catalog-browser` use cases as Tauri commands. Behaviour lives
//! in the crates under `crates/`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use desktop_contract::{
    AppStatus, PartDetail, PreviewOutcome, PreviewRequest, PublishResult, SearchRequest,
    SearchResponse, SignInRequest, TeamEditRequest, TeamPart, TeamRestoreRequest, TeamStatus,
};
use kicad_integration::{SearchLocations, remove_old_cache};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{Manager, PhysicalSize, State};

type Browser = Arc<CatalogBrowser>;
type Team = Arc<TeamCatalog>;

/// Launch options. Every option exists so tests and unusual installs can run
/// the app without touching the user's settings or KiCad search.
#[derive(Debug, Default, PartialEq)]
struct LaunchOptions {
    /// `--catalog-root DIR`: holds `database/`, `symbols/`, `footprints/`.
    catalog_root: Option<PathBuf>,
    /// `--data-dir DIR`: settings and preview cache, instead of the
    /// per-user application directories.
    data_dir: Option<PathBuf>,
    /// `--kicad-search-root DIR` (repeatable): look for KiCad only here.
    kicad_search_roots: Vec<PathBuf>,
    /// `--team-server URL`: the team catalog server to suggest at sign-in.
    team_server: Option<String>,
    /// `--team-email EMAIL`: the email to suggest at sign-in.
    team_email: Option<String>,
    /// `--display-scale N` (Windows only): draw N screen pixels per CSS
    /// pixel, whatever the Windows display scaling is. The journeys use 1, so
    /// their screenshots are 1280x800 on every machine.
    display_scale: Option<f64>,
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
            let target = match name.as_str() {
                "--catalog-root" => Target::CatalogRoot,
                "--data-dir" => Target::DataDir,
                "--kicad-search-root" => Target::KicadSearchRoot,
                "--team-server" => Target::TeamServer,
                "--team-email" => Target::TeamEmail,
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
                Target::TeamServer => options.team_server = Some(text()),
                Target::TeamEmail => options.team_email = Some(text()),
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
    TeamServer,
    TeamEmail,
    DisplayScale,
}

fn parse_scale(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .ok_or_else(|| format!("--display-scale needs a number above 0, not {text}"))
}

/// The browser switch that fixes WebView2's scale. WebView2 reads extra
/// switches from `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, and the WebDriver
/// that runs the journeys fills that variable with its own switches, so the
/// app adds this one to the end instead of replacing them.
fn with_scale_switch(switches: &str, scale: f64) -> String {
    let switch = format!("--force-device-scale-factor={scale}");
    if switches.trim().is_empty() {
        switch
    } else {
        format!("{} {switch}", switches.trim_end())
    }
}

const WEBVIEW2_SWITCHES: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

fn fix_webview_scale(scale: f64) {
    let switches = std::env::var(WEBVIEW2_SWITCHES).unwrap_or_default();
    // SAFETY: main calls this before Tauri starts, while it is the only thread.
    unsafe { std::env::set_var(WEBVIEW2_SWITCHES, with_scale_switch(&switches, scale)) };
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
    // legacy catalog in this repository.
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
    let (browser, team) = (browser.inner().clone(), team.inner().clone());
    // Parts the team catalog has are found by, and shown with, its current
    // details, as the part header shows them.
    blocking(move || browser.search_with(&request.query, request.limit, &team.current_values()))
        .await
}

#[tauri::command]
async fn part_detail(
    browser: State<'_, Browser>,
    ipn: String,
) -> Result<Option<PartDetail>, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.part(&ipn)).await
}

#[tauri::command]
async fn part_preview(
    browser: State<'_, Browser>,
    team: State<'_, Team>,
    request: PreviewRequest,
) -> Result<PreviewOutcome, String> {
    let (browser, team) = (browser.inner().clone(), team.inner().clone());
    // Team parts are drawn from the stored bytes of the exact CAD item
    // revision they use.
    blocking(move || browser.preview_request(&request, Some(&team))).await
}

#[tauri::command]
async fn team_status(team: State<'_, Team>) -> Result<TeamStatus, String> {
    let team = team.inner().clone();
    blocking(move || team.status()).await
}

#[tauri::command]
async fn team_sign_in(team: State<'_, Team>, request: SignInRequest) -> Result<TeamStatus, String> {
    let team = team.inner().clone();
    blocking(move || team.sign_in(request)).await
}

#[tauri::command]
async fn team_sign_out(team: State<'_, Team>) -> Result<TeamStatus, String> {
    let team = team.inner().clone();
    blocking(move || team.sign_out()).await
}

#[tauri::command]
async fn team_part(team: State<'_, Team>, ipn: String) -> Result<Option<TeamPart>, String> {
    let team = team.inner().clone();
    blocking(move || team.part(&ipn)).await?
}

#[tauri::command]
async fn team_edit(
    team: State<'_, Team>,
    request: TeamEditRequest,
) -> Result<PublishResult, String> {
    let team = team.inner().clone();
    blocking(move || team.edit(request)).await?
}

#[tauri::command]
async fn team_restore(
    team: State<'_, Team>,
    request: TeamRestoreRequest,
) -> Result<PublishResult, String> {
    let team = team.inner().clone();
    blocking(move || team.restore(request)).await?
}

fn main() {
    let options = match LaunchOptions::parse(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("kicad-parts-desktop: {message}");
            std::process::exit(2);
        }
    };
    // The fixed scale needs a WebView2 switch, so it is Windows only.
    let display_scale = options.display_scale.filter(|_| cfg!(windows));
    if let Some(scale) = display_scale {
        fix_webview_scale(scale);
    }

    tauri::Builder::default()
        .setup(move |app| {
            if let Some(scale) = display_scale {
                size_window_in_screen_pixels(app, scale)?;
            }
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
            });
            app.manage(Arc::new(browser));
            // Only the server address and email are saved; the session
            // stays in memory and ends with the app.
            let team = TeamCatalog::open(
                config_dir.join("team.json"),
                options.team_server.clone(),
                options.team_email.clone(),
            );
            app.manage(Arc::new(team));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            rediscover_kicad,
            set_kicad_override,
            search_parts,
            part_detail,
            part_preview,
            team_status,
            team_sign_in,
            team_sign_out,
            team_part,
            team_edit,
            team_restore,
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
            "--catalog-root",
            "C",
            "--team-server=http://127.0.0.1:8090",
            "--team-email",
            "a@example.test",
            "--display-scale=1",
        ]
        .map(OsString::from);
        let options = LaunchOptions::parse(args).unwrap();
        assert_eq!(options.data_dir, Some(PathBuf::from("D")));
        assert_eq!(options.catalog_root, Some(PathBuf::from("C")));
        assert_eq!(
            options.kicad_search_roots,
            [PathBuf::from("A"), PathBuf::from("B")]
        );
        assert_eq!(
            options.team_server.as_deref(),
            Some("http://127.0.0.1:8090")
        );
        assert_eq!(options.team_email.as_deref(), Some("a@example.test"));
        assert_eq!(options.display_scale, Some(1.0));
        assert!(LaunchOptions::parse([OsString::from("--data-dir")]).is_err());
    }

    #[test]
    fn rejects_a_display_scale_that_is_not_above_zero() {
        for scale in ["0", "-1", "big", "NaN", "inf"] {
            let arg = OsString::from(format!("--display-scale={scale}"));
            assert!(LaunchOptions::parse([arg]).is_err(), "{scale}");
        }
        assert_eq!(parse_scale("1.5"), Ok(1.5));
    }

    #[test]
    fn the_scale_switch_is_added_after_the_drivers_switches() {
        assert_eq!(with_scale_switch("", 1.0), "--force-device-scale-factor=1");
        assert_eq!(
            with_scale_switch("--remote-debugging-port=0 ", 1.0),
            "--remote-debugging-port=0 --force-device-scale-factor=1"
        );
        assert_eq!(
            with_scale_switch("--a", 1.25),
            "--a --force-device-scale-factor=1.25"
        );
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
            command::TEAM_STATUS,
            command::TEAM_SIGN_IN,
            command::TEAM_SIGN_OUT,
            command::TEAM_PART,
            command::TEAM_EDIT,
            command::TEAM_RESTORE,
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

//! Desktop shell. It parses launch options, picks application directories,
//! and exposes `catalog-browser` use cases as Tauri commands. Behaviour lives
//! in the crates under `crates/`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use catalog_browser::{BrowserConfig, CatalogBrowser};
use desktop_contract::{
    AppStatus, PartDetail, PreviewOutcome, PreviewRequest, SearchRequest, SearchResponse,
};
use kicad_integration::SearchLocations;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{Manager, State};

type Browser = Arc<CatalogBrowser>;

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
                Some((name, value)) => (name.to_owned(), Some(PathBuf::from(value))),
                None => (text.clone(), None),
            };
            let target = match name.as_str() {
                "--catalog-root" => Target::CatalogRoot,
                "--data-dir" => Target::DataDir,
                "--kicad-search-root" => Target::KicadSearchRoot,
                // WebView and test drivers may add their own arguments.
                _ => continue,
            };
            let value = match inline {
                Some(value) => value,
                None => args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{name} needs a directory"))?,
            };
            match target {
                Target::CatalogRoot => options.catalog_root = Some(value),
                Target::DataDir => options.data_dir = Some(value),
                Target::KicadSearchRoot => options.kicad_search_roots.push(value),
            }
        }
        Ok(options)
    }
}

enum Target {
    CatalogRoot,
    DataDir,
    KicadSearchRoot,
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
    request: SearchRequest,
) -> Result<SearchResponse, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.search(&request.query, request.limit)).await
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
    request: PreviewRequest,
) -> Result<PreviewOutcome, String> {
    let browser = browser.inner().clone();
    blocking(move || browser.preview(&request.ipn, request.kind)).await
}

fn main() {
    let options = match LaunchOptions::parse(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("kicad-parts-desktop: {message}");
            std::process::exit(2);
        }
    };

    tauri::Builder::default()
        .setup(move |app| {
            let (config_dir, cache_dir) = match &options.data_dir {
                Some(dir) => (dir.join("config"), dir.join("cache")),
                None => (app.path().app_config_dir()?, app.path().app_cache_dir()?),
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            rediscover_kicad,
            set_kicad_override,
            search_parts,
            part_detail,
            part_preview,
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
        ]
        .map(OsString::from);
        let options = LaunchOptions::parse(args).unwrap();
        assert_eq!(options.data_dir, Some(PathBuf::from("D")));
        assert_eq!(options.catalog_root, Some(PathBuf::from("C")));
        assert_eq!(
            options.kicad_search_roots,
            [PathBuf::from("A"), PathBuf::from("B")]
        );
        assert!(LaunchOptions::parse([OsString::from("--data-dir")]).is_err());
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
        ] {
            assert!(
                source.contains(&format!("async fn {name}(")),
                "no Tauri command named {name}"
            );
        }
    }

    #[test]
    fn default_catalog_root_is_the_repository() {
        let root = normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
        assert!(root.join("database").is_dir(), "{root:?}");
        assert!(!root.to_string_lossy().contains(".."));
    }
}

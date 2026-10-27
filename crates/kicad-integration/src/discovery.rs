//! Finds the installed kicad-cli and records its version.

use crate::process::{CliSettingsFolder, RunError, run_kicad_cli};
use std::cmp::Reverse;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The KiCad major version this application is built and tested against.
pub const SUPPORTED_MAJOR: u32 = 10;

const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(windows)]
const CLI_FILE: &str = "kicad-cli.exe";
#[cfg(not(windows))]
const CLI_FILE: &str = "kicad-cli";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct KicadVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl KicadVersion {
    /// Parses the first `MAJOR.MINOR[.PATCH]` in `text`, such as the
    /// `10.0.4` printed by `kicad-cli version`.
    pub fn parse(text: &str) -> Option<Self> {
        text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .find_map(|token| {
                let mut numbers = token.split('.').filter(|part| !part.is_empty());
                let major = numbers.next()?.parse().ok()?;
                let minor = numbers.next()?.parse().ok()?;
                let patch = numbers.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                Some(Self {
                    major,
                    minor,
                    patch,
                })
            })
    }

    /// KiCad 10 stable releases. Development builds toward the next major
    /// version are numbered `10.99.x`; their CLI output and file formats can
    /// change at any time, so they count as unsupported.
    pub fn is_supported(&self) -> bool {
        self.major == SUPPORTED_MAJOR && self.minor < 99
    }
}

impl fmt::Display for KicadVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallSource {
    /// The path the user chose in settings.
    UserOverride,
    /// A standard install location or a directory on `PATH`.
    Discovered,
}

/// A usable kicad-cli.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KicadInstall {
    pub cli: PathBuf,
    pub version: KicadVersion,
    pub source: InstallSource,
    /// KiCad's bundled data directory (contains `symbols/` and
    /// `footprints/`), when it can be located next to the CLI.
    pub share_dir: Option<PathBuf>,
    /// Where every run of this kicad-cli keeps its settings.
    pub settings: CliSettingsFolder,
}

/// Outcome of looking for kicad-cli. Every non-ready state carries enough
/// detail for the UI to tell the user how to fix it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Discovery {
    Ready(KicadInstall),
    /// No kicad-cli was found in any searched location.
    Missing {
        searched: Vec<PathBuf>,
    },
    /// A kicad-cli was found, but its version is not KiCad 10.
    Unsupported {
        cli: PathBuf,
        version: KicadVersion,
        source: InstallSource,
    },
    /// The user override does not point at a kicad-cli.
    OverrideInvalid {
        path: PathBuf,
        problem: String,
    },
    /// A kicad-cli was found but could not report its version.
    Broken {
        cli: PathBuf,
        source: InstallSource,
        message: String,
    },
    /// kicad-cli was not started because its own settings folder was
    /// refused or could not be made. KiCad itself may be fine.
    SettingsFolder {
        cli: PathBuf,
        /// Names the folder and what went wrong.
        message: String,
    },
}

/// Where to look for kicad-cli.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SearchLocations {
    /// The platform's standard install locations, then `PATH`.
    #[default]
    Standard,
    /// Only these install roots. Each root may hold `kicad-cli`, `bin/`, or
    /// `KiCad/<version>/bin/`, so a root works like `C:\Program Files`.
    Only(Vec<PathBuf>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryOptions {
    /// A user-chosen kicad-cli file, KiCad `bin` directory, or install root.
    /// When set, only this location is used.
    pub user_override: Option<PathBuf>,
    pub locations: SearchLocations,
    /// Where kicad-cli keeps its settings, for the version check and for
    /// every run of the install found.
    pub settings: CliSettingsFolder,
}

/// Looks for kicad-cli and probes its version.
pub fn discover(options: &DiscoveryOptions) -> Discovery {
    if let Some(path) = &options.user_override {
        return match override_candidate(path) {
            Some(cli) => probe(cli, InstallSource::UserOverride, &options.settings),
            None => Discovery::OverrideInvalid {
                path: path.clone(),
                problem: if path.exists() {
                    format!(
                        "{} is not kicad-cli and contains no {CLI_FILE}",
                        path.display()
                    )
                } else {
                    format!("{} does not exist", path.display())
                },
            },
        };
    }

    let roots = match &options.locations {
        SearchLocations::Standard => standard_roots(),
        SearchLocations::Only(roots) => roots.clone(),
    };
    let mut candidates = Vec::new();
    for root in &roots {
        candidates.extend(candidates_under(root));
    }
    if options.locations == SearchLocations::Standard {
        if let Some(path) = env::var_os("PATH") {
            candidates.extend(
                env::split_paths(&path)
                    .map(|dir| dir.join(CLI_FILE))
                    .filter(|cli| cli.is_file()),
            );
        }
    }

    // Prefer the first supported install; otherwise report the most useful
    // problem so the user knows what was found.
    let mut first_problem = None;
    let mut seen = Vec::new();
    for cli in candidates {
        if seen.contains(&cli) {
            continue;
        }
        seen.push(cli.clone());
        match probe(cli, InstallSource::Discovered, &options.settings) {
            ready @ Discovery::Ready(_) => return ready,
            problem => {
                first_problem.get_or_insert(problem);
            }
        }
    }
    first_problem.unwrap_or(Discovery::Missing { searched: roots })
}

/// KiCad's bundled data folder (holding `symbols/` and `footprints/`) of
/// the newest KiCad install found in `locations`, without running
/// kicad-cli. For tools that only read KiCad's libraries, such as the
/// Libraries browser.
pub fn installed_share_dir(locations: &SearchLocations) -> Option<PathBuf> {
    let roots = match locations {
        SearchLocations::Standard => standard_roots(),
        SearchLocations::Only(roots) => roots.clone(),
    };
    roots
        .iter()
        .flat_map(|root| candidates_under(root))
        .find_map(|cli| share_dir_for(&cli))
}

fn probe(cli: PathBuf, source: InstallSource, settings: &CliSettingsFolder) -> Discovery {
    let output = match run_kicad_cli(&cli, settings, ["version"], VERSION_TIMEOUT) {
        Ok(output) => output,
        Err(RunError::SettingsFolder(message)) => {
            return Discovery::SettingsFolder { cli, message };
        }
        Err(RunError::Start(error)) => {
            return Discovery::Broken {
                cli,
                source,
                message: format!("could not start kicad-cli: {error}"),
            };
        }
    };
    let version = output
        .success()
        .then(|| KicadVersion::parse(&output.stdout))
        .flatten();
    match version {
        Some(version) if version.is_supported() => Discovery::Ready(KicadInstall {
            share_dir: share_dir_for(&cli),
            cli,
            version,
            source,
            settings: settings.clone(),
        }),
        Some(version) => Discovery::Unsupported {
            cli,
            version,
            source,
        },
        None => Discovery::Broken {
            cli,
            source,
            message: format!(
                "`kicad-cli version` did not report a version: {}",
                output.message()
            ),
        },
    }
}

fn override_candidate(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        // Only ever run kicad-cli itself, so a path to kicad.exe or another
        // program cannot open a GUI or run something unexpected.
        return is_kicad_cli(path).then(|| path.to_path_buf());
    }
    candidates_under(path).into_iter().next()
}

fn is_kicad_cli(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        if cfg!(windows) {
            name.eq_ignore_ascii_case("kicad-cli.exe")
        } else {
            name == "kicad-cli"
        }
    })
}

/// kicad-cli locations under one install root, newest KiCad version first.
fn candidates_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for direct in [root.join(CLI_FILE), root.join("bin").join(CLI_FILE)] {
        if direct.is_file() {
            found.push(direct);
        }
    }
    // Windows installs as `<ProgramFiles>\KiCad\<version>\bin\kicad-cli.exe`.
    let mut versioned: Vec<(Option<KicadVersion>, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("KiCad")) {
        for entry in entries.flatten() {
            let cli = entry.path().join("bin").join(CLI_FILE);
            if cli.is_file() {
                let version = KicadVersion::parse(&entry.file_name().to_string_lossy());
                versioned.push((version, cli));
            }
        }
    }
    versioned.sort_by_key(|(version, _)| Reverse(version.clone()));
    found.extend(versioned.into_iter().map(|(_, cli)| cli));
    found
}

fn standard_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        for variable in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(dir) = env::var_os(variable) {
                roots.push(PathBuf::from(dir));
            }
        }
        if let Some(dir) = env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(dir).join("Programs"));
        }
    } else if cfg!(target_os = "macos") {
        roots.push(PathBuf::from(
            "/Applications/KiCad/KiCad.app/Contents/MacOS",
        ));
    } else {
        roots.push(PathBuf::from("/usr"));
        roots.push(PathBuf::from("/usr/local"));
    }
    let mut unique = Vec::new();
    for root in roots {
        if !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
}

/// KiCad's bundled library directory relative to the CLI:
/// `<root>/bin` -> `<root>/share/kicad` on Windows and Linux, and
/// `Contents/MacOS` -> `Contents/SharedSupport` on macOS.
fn share_dir_for(cli: &Path) -> Option<PathBuf> {
    let bin = cli.parent()?;
    let root = bin.parent()?;
    [root.join("share").join("kicad"), root.join("SharedSupport")]
        .into_iter()
        .find(|dir| dir.join("symbols").is_dir() || dir.join("footprints").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kicad_version_output() {
        let version = KicadVersion::parse("10.0.4\r\n").unwrap();
        assert_eq!(version.to_string(), "10.0.4");
        assert!(version.is_supported());
        assert_eq!(KicadVersion::parse("9.0").unwrap().to_string(), "9.0.0");
        assert!(!KicadVersion::parse("9.0.7").unwrap().is_supported());
        let nightly = KicadVersion::parse("KiCad 10.99.0-123").unwrap();
        assert_eq!(nightly.minor, 99);
        assert!(
            !nightly.is_supported(),
            "10.99 nightlies are KiCad 11 development"
        );
        assert!(KicadVersion::parse("10.1.2").unwrap().is_supported());
        assert!(!KicadVersion::parse("11.0.0").unwrap().is_supported());
        assert_eq!(KicadVersion::parse("no version"), None);
    }

    #[test]
    fn newest_versioned_install_is_preferred() {
        let root = tempfile::tempdir().unwrap();
        for version in ["9.0", "10.0", "8.0"] {
            let bin = root.path().join("KiCad").join(version).join("bin");
            fs::create_dir_all(&bin).unwrap();
            fs::write(bin.join(CLI_FILE), b"").unwrap();
        }
        let found = candidates_under(root.path());
        let versions: Vec<_> = found
            .iter()
            .map(|cli| cli.parent().unwrap().parent().unwrap().file_name().unwrap())
            .collect();
        assert_eq!(versions, ["10.0", "9.0", "8.0"]);
    }

    #[test]
    fn empty_search_roots_report_missing_with_searched_locations() {
        let root = tempfile::tempdir().unwrap();
        let options = DiscoveryOptions {
            user_override: None,
            locations: SearchLocations::Only(vec![root.path().to_path_buf()]),
            settings: CliSettingsFolder::new(root.path().join("settings")),
        };
        assert_eq!(
            discover(&options),
            Discovery::Missing {
                searched: vec![root.path().to_path_buf()]
            }
        );
    }

    #[test]
    fn override_must_be_kicad_cli_itself() {
        let dir = tempfile::tempdir().unwrap();
        let gui = dir
            .path()
            .join(if cfg!(windows) { "kicad.exe" } else { "kicad" });
        fs::write(&gui, b"").unwrap();
        let options = DiscoveryOptions {
            user_override: Some(gui.clone()),
            locations: SearchLocations::Only(Vec::new()),
            settings: CliSettingsFolder::new(dir.path().join("settings")),
        };
        match discover(&options) {
            Discovery::OverrideInvalid { path, problem } => {
                assert_eq!(path, gui);
                assert!(problem.contains("is not kicad-cli"), "{problem}");
            }
            other => panic!("expected OverrideInvalid, got {other:?}"),
        }
    }

    #[test]
    fn missing_override_is_reported_instead_of_falling_back() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("nowhere").join(CLI_FILE);
        let options = DiscoveryOptions {
            user_override: Some(missing.clone()),
            locations: SearchLocations::Standard,
            settings: CliSettingsFolder::new(root.path().join("settings")),
        };
        match discover(&options) {
            Discovery::OverrideInvalid { path, problem } => {
                assert_eq!(path, missing);
                assert!(problem.contains("does not exist"), "{problem}");
            }
            other => panic!("expected OverrideInvalid, got {other:?}"),
        }
    }
}

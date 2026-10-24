//! The single place this crate starts a process. Only kicad-cli is launched.
//!
//! kicad-cli loads the person's KiCad settings at start and saves them
//! again when they changed. When it reads `kicad_common.json` while
//! another KiCad process is saving it, it saves defaults over the whole
//! file. That empties the person's path variables. So every run gets its
//! own settings folder, a [`CliSettingsFolder`], and never sees theirs.

use crate::discovery::{KicadVersion, SUPPORTED_MAJOR};
use crate::lib_table::is_inside;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Captured result of one kicad-cli invocation.
#[derive(Clone, Debug)]
pub struct CliOutput {
    /// Exit code, or `None` if the process was killed or ended by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CliOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    /// stdout and stderr joined, trimmed, for showing to a person. kicad-cli
    /// prints some errors on stdout and some on stderr.
    pub fn message(&self) -> String {
        let mut message = String::new();
        for part in [self.stdout.trim(), self.stderr.trim()] {
            if !part.is_empty() {
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(part);
            }
        }
        if self.timed_out {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str("kicad-cli did not finish in time and was stopped.");
        }
        message
    }
}

/// A folder our code owns, where kicad-cli keeps its settings and cache
/// instead of the person's KiCad folders. kicad-cli fills it with default
/// settings the first time. Several runs may share one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliSettingsFolder {
    path: PathBuf,
    /// More of the person's folders that kicad-cli must stay out of.
    keep_away_from: Vec<PathBuf>,
}

impl CliSettingsFolder {
    /// The folder `path`, created on the first run. Choose a folder of the
    /// app's own, such as one next to the preview cache, or a temporary
    /// folder in tests.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            keep_away_from: Vec::new(),
        }
    }

    /// Also refuses to run kicad-cli if its folders and `folder` overlap,
    /// such as the KiCad settings folder the app reads library tables from.
    pub fn keep_away_from(mut self, folder: PathBuf) -> Self {
        self.keep_away_from.push(folder);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// kicad-cli's settings folder: `KICAD_CONFIG_HOME`.
    fn config_home(&self) -> PathBuf {
        self.path.join("config")
    }

    /// The folder where kicad-cli `version` keeps its settings files, such
    /// as `kicad_advanced`: `KICAD_CONFIG_HOME/10.0`. KiCad names it by
    /// major and minor version.
    pub fn settings_for(&self, version: &KicadVersion) -> PathBuf {
        self.config_home()
            .join(format!("{}.{}", version.major, version.minor))
    }

    /// kicad-cli's cache folder: `KICAD_CACHE_HOME`.
    fn cache_home(&self) -> PathBuf {
        self.path.join("cache")
    }

    /// Checks the folder and creates the two folders kicad-cli gets. The
    /// error names the folder, so nobody is told to reinstall KiCad.
    fn prepare(&self) -> Result<(PathBuf, PathBuf), String> {
        self.check(|name| env::var_os(name))?;
        let (config_home, cache_home) = (self.config_home(), self.cache_home());
        for folder in [&config_home, &cache_home] {
            fs::create_dir_all(folder).map_err(|error| {
                format!(
                    "Could not make kicad-cli's settings folder {}: {error}",
                    folder.display()
                )
            })?;
        }
        Ok((config_home, cache_home))
    }

    /// Refuses the folder if kicad-cli's settings or cache folder overlaps
    /// one of the person's KiCad folders. `var` reads an environment
    /// variable.
    fn check(&self, var: impl Fn(&str) -> Option<OsString>) -> Result<(), String> {
        let theirs = self.persons_folders(var);
        for ours in [self.config_home(), self.cache_home()] {
            if let Some(their) = theirs.iter().find(|their| overlap(&ours, their)) {
                return Err(format!(
                    "kicad-cli was not started: its settings folder {} overlaps your KiCad folder {}.",
                    ours.display(),
                    their.display()
                ));
            }
        }
        Ok(())
    }

    /// The folders KiCad itself keeps settings and cache in. A parent's
    /// `KICAD_CONFIG_HOME` or `KICAD_CACHE_HOME` counts only through the
    /// version folder KiCad makes in it, so a broad one such as
    /// `%LOCALAPPDATA%` does not refuse every folder inside it.
    fn persons_folders(&self, var: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
        let get = |name: &str| {
            var(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let version = format!("{SUPPORTED_MAJOR}.0");
        let mut theirs = Vec::new();
        theirs.extend(get("KICAD_CONFIG_HOME").map(|home| home.join(&version)));
        theirs.extend(get("KICAD_CACHE_HOME").map(|home| home.join("KiCad").join(&version)));
        for base in ["APPDATA", "LOCALAPPDATA"] {
            theirs.extend(get(base).map(|base| base.join("kicad")));
        }
        theirs.extend(self.keep_away_from.iter().cloned());
        theirs
    }
}

/// Whether one folder is the other or inside it. Compares the real folders
/// where they exist, so other spellings of a path (short names, `\\?\`,
/// junctions) still match, and compares the names as written too.
fn overlap(a: &Path, b: &Path) -> bool {
    let inside = |a: &Path, b: &Path| is_inside(a, b) || is_inside(b, a);
    let (a, b) = (absolute(a), absolute(b));
    inside(&a, &b) || inside(&resolved(&a), &resolved(&b))
}

/// `path` made absolute; on Windows this also removes `..` and a trailing
/// `.` or space from names.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `path` with its nearest existing parent replaced by the real folder.
fn resolved(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut base = path;
    loop {
        if let Ok(real) = fs::canonicalize(base) {
            return rest.iter().rev().fold(real, |path, name| path.join(name));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                base = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Why kicad-cli did not run.
#[derive(Debug)]
pub enum RunError {
    /// The settings folder was refused or could not be made. The message
    /// names the folder.
    SettingsFolder(String),
    /// kicad-cli itself could not be started or waited for.
    Start(io::Error),
}

impl RunError {
    /// What went wrong, for a person, naming the settings folder or `cli`.
    pub fn message(&self, cli: &Path) -> String {
        match self {
            Self::SettingsFolder(message) => message.clone(),
            Self::Start(error) => format!("could not start {}: {error}", cli.display()),
        }
    }
}

/// Runs `cli` with `args`, capturing output, and kills it after `timeout`.
/// kicad-cli keeps its settings and cache in `settings`.
pub fn run_kicad_cli<I, S>(
    cli: &Path,
    settings: &CliSettingsFolder,
    args: I,
    timeout: Duration,
) -> Result<CliOutput, RunError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (config_home, cache_home) = settings.prepare().map_err(RunError::SettingsFolder)?;
    run(cli, &config_home, &cache_home, args, timeout).map_err(RunError::Start)
}

fn run<I, S>(
    cli: &Path,
    config_home: &Path,
    cache_home: &Path,
    args: I,
    timeout: Duration,
) -> io::Result<CliOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    #[allow(clippy::disallowed_methods)] // The one sanctioned spawn point.
    let mut command = Command::new(cli);
    command
        .args(args)
        .env("KICAD_CONFIG_HOME", config_home)
        .env("KICAD_CACHE_HOME", cache_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a GUI app must not flash a console per export.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break child.wait().ok();
        }
        thread::sleep(Duration::from_millis(10));
    };
    let collect = |handle: Option<thread::JoinHandle<Vec<u8>>>| {
        handle
            .and_then(|handle| handle.join().ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    };
    Ok(CliOutput {
        code: status.and_then(|status| status.code()),
        stdout: collect(stdout),
        stderr: collect(stderr),
        timed_out,
    })
}

fn drain<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment of only `pairs`, so tests never change the real one.
    fn vars(pairs: &[(&str, PathBuf)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone().into_os_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn refused(folder: &CliSettingsFolder, var: impl Fn(&str) -> Option<OsString>) -> String {
        folder.check(var).expect_err("the folder should be refused")
    }

    #[test]
    fn a_folder_in_kicads_settings_or_cache_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let (roaming, local) = (home.path().join("Roaming"), home.path().join("Local"));
        let var = || {
            vars(&[
                ("APPDATA", roaming.clone()),
                ("LOCALAPPDATA", local.clone()),
            ])
        };
        let message = refused(
            &CliSettingsFolder::new(roaming.join("kicad").join("x")),
            var(),
        );
        assert!(message.contains("overlaps your KiCad folder"), "{message}");
        refused(&CliSettingsFolder::new(local.join("kicad")), var());
        // `..` does not hide it.
        refused(
            &CliSettingsFolder::new(local.join("other").join("..").join("kicad")),
            var(),
        );
    }

    #[cfg(windows)]
    #[test]
    fn other_spellings_of_kicads_folder_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let roaming = home.path().join("Roaming");
        fs::create_dir_all(roaming.join("kicad")).unwrap();
        let shouted = PathBuf::from(roaming.to_string_lossy().to_uppercase().replace('\\', "/"));
        refused(
            &CliSettingsFolder::new(roaming.join("KICAD").join("x")),
            vars(&[("APPDATA", shouted)]),
        );
        let verbatim = PathBuf::from(format!(r"\\?\{}", roaming.join("kicad").display()));
        refused(
            &CliSettingsFolder::new(verbatim),
            vars(&[("APPDATA", roaming.clone())]),
        );
        // A trailing dot or space is dropped by Windows.
        refused(
            &CliSettingsFolder::new(roaming.join("kicad. ")),
            vars(&[("APPDATA", roaming)]),
        );
    }

    #[test]
    fn the_folders_kicad_cli_gets_are_checked_not_only_ours() {
        // KiCad would keep its settings in X/config/10.0, which is inside
        // the X/config that kicad-cli gets.
        let x = tempfile::tempdir().unwrap();
        refused(
            &CliSettingsFolder::new(x.path().to_path_buf()),
            vars(&[("KICAD_CONFIG_HOME", x.path().join("config"))]),
        );
    }

    #[test]
    fn a_broad_kicad_home_does_not_refuse_our_folder() {
        let home = tempfile::tempdir().unwrap();
        let (roaming, local) = (home.path().join("Roaming"), home.path().join("Local"));
        let folder = CliSettingsFolder::new(local.join("app-cache").join("kicad-cli-settings"));
        let var = vars(&[
            ("APPDATA", roaming.clone()),
            ("LOCALAPPDATA", local.clone()),
            ("KICAD_CONFIG_HOME", roaming),
            ("KICAD_CACHE_HOME", local),
        ]);
        assert_eq!(folder.check(var), Ok(()));
    }

    #[test]
    fn a_folder_we_were_told_to_keep_away_from_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let theirs = home.path().join("kicad-config");
        let folder = CliSettingsFolder::new(theirs.join("cli")).keep_away_from(theirs);
        refused(&folder, vars(&[]));
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_named() {
        let home = tempfile::tempdir().unwrap();
        let blocked = home.path().join("blocked");
        fs::write(&blocked, b"a file, not a folder").unwrap();
        let error = CliSettingsFolder::new(blocked.clone())
            .prepare()
            .unwrap_err();
        assert!(
            error.starts_with("Could not make kicad-cli's settings folder"),
            "{error}"
        );
        assert!(error.contains(&blocked.display().to_string()), "{error}");
    }
}

//! The single place this crate starts a process. Only kicad-cli is launched.
//!
//! kicad-cli loads the person's KiCad settings at start and saves them
//! again when they changed. When it reads `kicad_common.json` while
//! another KiCad process is saving it, it saves defaults over the whole
//! file. That empties the person's path variables. So every run gets its
//! own settings folder, a [`CliSettingsFolder`], and never sees theirs.

use crate::lib_table::is_inside;
use std::env;
use std::ffi::OsStr;
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
pub struct CliSettingsFolder(PathBuf);

impl CliSettingsFolder {
    /// The folder `path`, created on the first run. Choose a folder of the
    /// app's own, such as one next to the preview cache, or a temporary
    /// folder in tests.
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// kicad-cli's settings folder: `KICAD_CONFIG_HOME`.
    fn config_home(&self) -> PathBuf {
        self.0.join("config")
    }

    /// kicad-cli's cache folder: `KICAD_CACHE_HOME`.
    fn cache_home(&self) -> PathBuf {
        self.0.join("cache")
    }

    /// Refuses a folder inside the person's own KiCad settings or cache,
    /// which is exactly what this type keeps kicad-cli away from.
    fn check(&self) -> io::Result<()> {
        let get = |name: &str| env::var_os(name).filter(|value| !value.is_empty());
        let mut theirs: Vec<PathBuf> = ["KICAD_CONFIG_HOME", "KICAD_CACHE_HOME"]
            .into_iter()
            .filter_map(|name| get(name).map(PathBuf::from))
            .collect();
        for base in ["APPDATA", "LOCALAPPDATA"] {
            theirs.extend(get(base).map(|base| PathBuf::from(base).join("kicad")));
        }
        match theirs.iter().find(|folder| is_inside(&self.0, folder)) {
            Some(folder) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "kicad-cli must not use {} for its settings: it is inside your KiCad folder {}",
                    self.0.display(),
                    folder.display()
                ),
            )),
            None => Ok(()),
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
) -> io::Result<CliOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    settings.check()?;
    let (config_home, cache_home) = (settings.config_home(), settings.cache_home());
    fs::create_dir_all(&config_home)?;
    fs::create_dir_all(&cache_home)?;
    #[allow(clippy::disallowed_methods)] // The one sanctioned spawn point.
    let mut command = Command::new(cli);
    command
        .args(args)
        .env("KICAD_CONFIG_HOME", &config_home)
        .env("KICAD_CACHE_HOME", &cache_home)
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

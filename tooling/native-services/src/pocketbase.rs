//! The pinned PocketBase release as a disposable loopback service.

use crate::fetch::Cache;
use crate::pins::NativePins;
use crate::process::{ServiceProcess, free_loopback_port, run_hidden};
use crate::readiness::{answers_200, log_has_line_ending_with, wait_until_ready};
use crate::{ServiceResult, error, random_name, random_token};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

/// Returns the verified PocketBase executable for this host, downloading
/// and unpacking the pinned release on first use.
pub fn pocketbase_binary(repository: &Path) -> ServiceResult<PathBuf> {
    static BINARY: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut cached = BINARY.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(path) = cached.as_ref() {
        return Ok(path.clone());
    }
    let pins = NativePins::load(repository)?;
    let cache = Cache::for_repository(repository);
    let directory = cache.unpacked(
        &format!("pocketbase-{}", pins.pocketbase.version),
        &pins.pocketbase,
    )?;
    let executable = directory.join(pins.platform.executable("pocketbase"));
    make_executable(&executable)?;
    let output = run_hidden(
        "pocketbase --version",
        Command::new(&executable).arg("--version"),
    )?;
    let reported = String::from_utf8_lossy(&output.stdout);
    let expected = format!(" version {}", pins.pocketbase.version);
    if !output.status.success() || !reported.trim_end().ends_with(&expected) {
        return Err(error(format!(
            "`{}` reports `{}`, expected PocketBase {}",
            executable.display(),
            reported.trim(),
            pins.pocketbase.version
        )));
    }
    *cached = Some(executable.clone());
    Ok(executable)
}

#[cfg(unix)]
pub(crate) fn make_executable(path: &Path) -> ServiceResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|io| error(format!("cannot mark `{}` executable: {io}", path.display())))
}

#[cfg(not(unix))]
pub(crate) fn make_executable(path: &Path) -> ServiceResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(error(format!("`{}` is missing", path.display())))
    }
}

/// Where a PocketBase instance keeps its state and finds its schema.
#[derive(Clone, Debug)]
pub struct PocketBaseOptions {
    /// `pb_data`: the SQLite databases and stored files.
    pub data_dir: PathBuf,
    pub migrations_dir: PathBuf,
    pub hooks_dir: PathBuf,
    /// The service log; keep it outside `data_dir`.
    pub log_file: PathBuf,
}

impl PocketBaseOptions {
    fn common_args(&self, command: &mut Command) {
        command
            .arg("--dir")
            .arg(&self.data_dir)
            .arg("--migrationsDir")
            .arg(&self.migrations_dir)
            .arg("--hooksDir")
            .arg(&self.hooks_dir)
            // Never write generated migrations into the repository.
            .arg("--automigrate=false")
            .arg("--dev=false");
    }
}

/// Superuser credentials for one test run, kept in memory only.
#[derive(Clone)]
pub struct Superuser {
    pub email: String,
    pub password: String,
}

impl Superuser {
    /// New random credentials.
    pub fn generate() -> Self {
        Self {
            email: format!("su-{}@example.test", random_name(12)),
            password: random_token(32),
        }
    }
}

impl std::fmt::Debug for Superuser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Superuser")
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// What PocketBase prints when it has no superuser and opens its installer
/// page in the default browser. A test service must never reach that.
const INSTALLER_MARKERS: [&str; 3] = ["pbinstal", "Launch the URL", "create your first superuser"];

/// True if a PocketBase log shows the browser-opening installer banner.
pub fn shows_installer_banner(log: &str) -> bool {
    INSTALLER_MARKERS.iter().any(|marker| log.contains(marker))
}

/// Creates or updates a superuser. This also applies pending migrations.
/// [`PocketBaseServer::start`] always calls it first, because `serve` on a
/// data directory without a superuser opens the installer in the user's
/// browser.
pub fn upsert_superuser(
    binary: &Path,
    options: &PocketBaseOptions,
    superuser: &Superuser,
) -> ServiceResult<()> {
    let mut command = Command::new(binary);
    command.args([
        "superuser",
        "upsert",
        superuser.email.as_str(),
        superuser.password.as_str(),
    ]);
    options.common_args(&mut command);
    let output = run_hidden("pocketbase superuser upsert", &mut command)?;
    if !output.status.success() {
        return Err(error(format!(
            "pocketbase superuser upsert failed: {}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// How long PocketBase may take to apply migrations and start listening.
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether the PocketBase we started on `port`, whose log so far is `log`,
/// is ready: `Err` says why not.
///
/// PocketBase prints `Server started at http://127.0.0.1:<port>` only after
/// it has applied migrations, loaded the hooks and bound the port. We need
/// that line before we trust `/api/health`. Without it, when another
/// test's PocketBase took the port first, the other server answered our
/// health check, and our superuser then failed to sign in (kicad-parts-2091).
fn pocketbase_is_ready(port: u16, log: &str) -> Result<(), String> {
    log_has_line_ending_with(log, &format!("Server started at http://127.0.0.1:{port}"))?;
    answers_200(&format!("http://127.0.0.1:{port}/api/health"))
}

/// A running PocketBase server on a random 127.0.0.1 port.
pub struct PocketBaseServer {
    process: ServiceProcess,
    url: String,
    options: PocketBaseOptions,
}

impl PocketBaseServer {
    /// Upserts `superuser`, then starts `serve` (which applies pending
    /// migrations and loads the hooks before it listens) and waits until it
    /// is ready: its own log says it listens on our port, and
    /// `/api/health` answers 200. See [`pocketbase_is_ready`].
    ///
    /// Requiring the superuser here means no instance, blank, restored or
    /// upgraded, can start without one and open PocketBase's installer in
    /// the browser. The process also runs in a job that forbids child
    /// processes, so it could not launch a browser anyway, and a log that
    /// shows the installer banner fails the start.
    pub fn start(
        binary: &Path,
        options: &PocketBaseOptions,
        superuser: &Superuser,
    ) -> ServiceResult<Self> {
        upsert_superuser(binary, options, superuser)?;
        let mut last_failure = String::new();
        for _ in 0..3 {
            let port = free_loopback_port()?;
            let mut command = Command::new(binary);
            command.arg("serve").arg(format!("--http=127.0.0.1:{port}"));
            options.common_args(&mut command);
            command.arg("--hooksWatch=false").arg(format!(
                "--publicDir={}",
                options.data_dir.with_file_name("pb_public").display()
            ));
            let process = ServiceProcess::spawn("pocketbase", command, &options.log_file)?;
            let url = format!("http://127.0.0.1:{port}");
            let mut server = Self {
                process,
                url,
                options: options.clone(),
            };
            match wait_until_ready(&mut server.process, START_TIMEOUT, |log| {
                pocketbase_is_ready(port, log)
            }) {
                Ok(()) => {
                    server.check_no_installer()?;
                    return Ok(server);
                }
                Err(failure) => {
                    last_failure = failure.0;
                    // Something else took the port before we bound it: try
                    // another port. Any other failure is real.
                    if !server.process.log_tail().contains("bind") {
                        break;
                    }
                }
            }
        }
        Err(error(format!("PocketBase did not start: {last_failure}")))
    }

    /// Fails if the log shows the installer banner.
    pub fn check_no_installer(&self) -> ServiceResult<()> {
        let log = fs::read_to_string(self.process.log_path()).unwrap_or_default();
        if shows_installer_banner(&log) {
            return Err(error(format!(
                "PocketBase started its superuser installer, which opens a browser:
{log}"
            )));
        }
        Ok(())
    }

    /// The base URL, `http://127.0.0.1:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn options(&self) -> &PocketBaseOptions {
        &self.options
    }

    pub fn log_tail(&self) -> String {
        self.process.log_tail()
    }

    /// Kills the server and waits for it, releasing `pb_data`. SQLite's
    /// write-ahead log makes this as safe as a crash, which PocketBase
    /// survives.
    pub fn stop(mut self) {
        self.process.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readiness::fake_server;

    #[test]
    fn the_installer_banner_is_recognised() {
        // PocketBase 0.38's output on a data directory without a superuser.
        let banner = "(!) Launch the URL below in the browser if it hasn't been open already             to create your first superuser account:
            http://127.0.0.1:8090/_/#/pbinstal/eyJhbGciOi";
        assert!(shows_installer_banner(banner));
        assert!(!shows_installer_banner(
            "Server started at http://127.0.0.1:8090
├─ REST API:  http://127.0.0.1:8090/api/"
        ));
    }

    #[test]
    fn another_pocketbase_on_our_port_is_not_ours() {
        // kicad-parts-2091: our PocketBase could not bind its port, and
        // another test's PocketBase answered the health check.
        let (port, _answers) = fake_server::start(&[("/api/health", 200)]);
        let ours = "Error: listen tcp 127.0.0.1:52362: bind: Only one usage of each \
                    socket address (protocol/network address/port) is normally permitted.\n";
        let reason = pocketbase_is_ready(port, ours).unwrap_err();
        assert!(reason.contains("Server started at"), "{reason}");
    }

    #[test]
    fn our_pocketbase_is_ready_once_it_says_so_and_is_healthy() {
        let (port, answers) = fake_server::start(&[("/api/health", 503)]);
        let ours = format!("2026/10/17 21:04:05 Server started at http://127.0.0.1:{port}\n");
        let reason = pocketbase_is_ready(port, &ours).unwrap_err();
        assert!(reason.contains("answered 503"), "{reason}");
        answers.lock().unwrap().insert("/api/health".into(), 200);
        assert_eq!(pocketbase_is_ready(port, &ours), Ok(()));
    }

    #[test]
    fn superuser_credentials_are_random_and_not_printed() {
        let superuser = Superuser::generate();
        assert_ne!(superuser.password, Superuser::generate().password);
        assert!(!format!("{superuser:?}").contains(&superuser.password));
    }
}

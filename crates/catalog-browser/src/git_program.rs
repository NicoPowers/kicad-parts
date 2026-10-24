//! The one place the app runs the system's git program (ADR 0021).
//!
//! The app uses the person's own git, so their git settings, SSH setup,
//! agents and credential helpers all apply, and the app never sees a key or
//! a password. Every run is made safe for an app with no terminal:
//!
//! - no window: on Windows, git and the programs it starts get a hidden
//!   console;
//! - no prompt in a terminal: standard input is empty and
//!   `GIT_TERMINAL_PROMPT=0` makes git fail rather than ask;
//! - a time limit: a run that takes longer is stopped, together with every
//!   program it started, such as `ssh` waiting on a question nobody can
//!   see.
//!
//! Steps that reach another computer also get [`Git::run_network`]'s SSH
//! settings, so SSH fails at once rather than asking about a passphrase or
//! a new host key.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The SSH command used when the person has not chosen one of their own.
/// `BatchMode=yes` makes ssh fail at once instead of asking for a
/// passphrase or about a host it has not met. Keys in an SSH agent (such as
/// the macOS Keychain's, 1Password's or Pageant's) still work, because
/// using them asks nothing. `ConnectTimeout` gives a host that does not
/// answer a clear "timed out" well inside the app's own time limit.
const BATCH_SSH: &str = "ssh -o BatchMode=yes -o ConnectTimeout=20";

/// How long git may take to say what it said after it ended or was
/// stopped. Only a program git started that holds on to its output, and
/// was not stopped with it, could make this wait.
const OUTPUT_WAIT: Duration = Duration::from_secs(2);

/// How to run git: which program, and any settings of its environment.
#[derive(Clone, Debug)]
pub(crate) struct Git {
    program: OsString,
    /// Changes to the app's own environment: a value to set, or `None` to
    /// leave the name out. Tests use it to keep git away from the
    /// developer's settings (see [`Git::isolated`]).
    env: Vec<(OsString, Option<OsString>)>,
}

/// What one run of git did.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ran {
    /// git ended by itself and said it succeeded.
    pub(crate) succeeded: bool,
    /// git took longer than its time limit and was stopped.
    pub(crate) timed_out: bool,
    /// What git wrote to its standard output, as bytes: a file read from
    /// a repo may be a 3D model, not text.
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

impl Ran {
    /// git's standard output as text.
    pub(crate) fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// The first line git wrote about what went wrong, or "it gave no
    /// reason".
    pub(crate) fn first_problem_line(&self) -> &str {
        self.stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it gave no reason")
    }
}

/// git could not be started at all: it is not installed, or not where the
/// app looks for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NotInstalled;

impl Git {
    /// The git program on the system's search path, as the person runs it.
    pub(crate) fn system() -> Self {
        Self::named("git")
    }

    /// The git program `program`, such as a full path, or a name that
    /// does not exist in a test of "git is not installed".
    pub(crate) fn named(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            env: Vec::new(),
        }
    }

    /// The same git with `name` set to `value` in its environment.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_env(
        mut self,
        name: impl Into<OsString>,
        value: impl Into<OsString>,
    ) -> Self {
        self.env.push((name.into(), Some(value.into())));
        self
    }

    /// The same git with `name` left out of its environment.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn without_env(mut self, name: impl Into<OsString>) -> Self {
        self.env.push((name.into(), None));
        self
    }

    /// git for tests: it reads only `config` (made empty if missing) as
    /// its settings, never the developer's or the system's, uses no SSH
    /// command or proxy of theirs, and keeps 127.0.0.1 off any proxy.
    #[cfg(test)]
    pub(crate) fn isolated(config: &Path) -> Self {
        if !config.exists() {
            std::fs::write(config, "").unwrap();
        }
        Self::system()
            .with_env("GIT_CONFIG_GLOBAL", config)
            .with_env("GIT_CONFIG_NOSYSTEM", "1")
            .without_env("GIT_SSH_COMMAND")
            .without_env("GIT_SSH")
            .without_env("GIT_SSH_VARIANT")
            .without_env("HTTPS_PROXY")
            .without_env("HTTP_PROXY")
            .without_env("ALL_PROXY")
            .with_env("NO_PROXY", "127.0.0.1,localhost")
    }

    /// Runs git with `args`, in `dir` if one is given, and stops it after
    /// `limit`. See the module notes for what makes it safe.
    pub(crate) fn run<I, S>(
        &self,
        dir: Option<&Path>,
        args: I,
        limit: Duration,
    ) -> Result<Ran, NotInstalled>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(dir);
        command.args(args);
        run_with_limit(command, limit)
    }

    /// Like [`Git::run`], for a step that reaches another computer, such as
    /// `ls-remote`, `clone`, `fetch` or `push`.
    ///
    /// SSH must fail rather than ask. So when the person has not chosen an
    /// SSH command of their own, the app runs [`BATCH_SSH`]. A command of
    /// their own (`GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand`, for
    /// example Windows' own OpenSSH for 1Password or the Windows agent, or
    /// PuTTY's plink for Pageant) is left exactly as it is: changing it
    /// would break their agent. Such a command that asks something still
    /// cannot wait for ever, because the time limit stops it.
    pub(crate) fn run_network<I, S>(
        &self,
        dir: Option<&Path>,
        args: I,
        limit: Duration,
    ) -> Result<Ran, NotInstalled>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(dir);
        if !self.has_own_ssh_command(dir)? {
            command.env("GIT_SSH_COMMAND", BATCH_SSH);
        }
        command.args(args);
        run_with_limit(command, limit)
    }

    /// Whether the person chose an SSH command of their own, in the
    /// environment or in git's settings as seen from `dir`. Telling git
    /// which kind of SSH program to expect (`GIT_SSH_VARIANT` or
    /// `ssh.variant`, as for PuTTY) counts too: "ssh -o ..." might not suit
    /// it.
    pub(crate) fn has_own_ssh_command(&self, dir: Option<&Path>) -> Result<bool, NotInstalled> {
        if ["GIT_SSH_COMMAND", "GIT_SSH", "GIT_SSH_VARIANT"]
            .iter()
            .any(|name| self.env_var(name).is_some_and(|value| !value.is_empty()))
        {
            return Ok(true);
        }
        let ran = self.run(
            dir,
            [
                "config",
                "--get-regexp",
                r"^(core\.sshcommand|ssh\.variant)$",
            ],
            Duration::from_secs(30),
        )?;
        Ok(ran.succeeded && !ran.stdout_text().trim().is_empty())
    }

    /// `name` in git's environment: this git's own settings first, then
    /// the app's environment.
    fn env_var(&self, name: &str) -> Option<OsString> {
        match self.env.iter().rev().find(|(set, _)| set == name) {
            Some((_, value)) => value.clone(),
            None => std::env::var_os(name),
        }
    }

    fn command(&self, dir: Option<&Path>) -> Command {
        let mut command = Command::new(&self.program);
        if let Some(dir) = dir {
            command.arg("-C").arg(dir);
        }
        for (name, value) in &self.env {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        // git's messages are read in English, so the app can tell what
        // went wrong and say it in plain words.
        command
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_and_group(&mut command);
        command
    }
}

/// Starts `command`, and stops it, with every program it started, once it
/// has run for `limit`.
fn run_with_limit(mut command: Command, limit: Duration) -> Result<Ran, NotInstalled> {
    let mut child = command.spawn().map_err(|_| NotInstalled)?;
    let stdout = child.stdout.take().map(read_all);
    let stderr = child.stderr.take().map(read_all);
    let started = Instant::now();
    let mut timed_out = false;
    let succeeded = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() >= limit => {
                timed_out = true;
                stop_with_everything_it_started(&mut child);
                break false;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                stop_with_everything_it_started(&mut child);
                break false;
            }
        }
    };
    let collect = |output: Option<mpsc::Receiver<Vec<u8>>>| {
        output
            .and_then(|output| output.recv_timeout(OUTPUT_WAIT).ok())
            .unwrap_or_default()
    };
    Ok(Ran {
        succeeded: succeeded && !timed_out,
        timed_out,
        stdout: collect(stdout),
        stderr: String::from_utf8_lossy(&collect(stderr)).into_owned(),
    })
}

/// Reads all of `reader` on a thread of its own, and sends it when done.
/// A receiver that stops waiting leaves the thread to end by itself.
fn read_all<R: Read + Send + 'static>(mut reader: R) -> mpsc::Receiver<Vec<u8>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        let _ = send.send(bytes);
    });
    receive
}

/// No window on Windows. Elsewhere, a process group of its own, so the
/// whole group can be stopped, and so ssh never reads from a terminal the
/// app was started from.
fn hide_and_group(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a GUI app must not flash a console. Programs
        // git starts, such as ssh, share the hidden console.
        command.creation_flags(0x0800_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

/// Stops `child` and every program it started, such as ssh, then waits for
/// it. Stopping git alone would leave ssh waiting on a hidden prompt.
fn stop_with_everything_it_started(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    let mut stopper = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("taskkill");
        command
            .args(["/F", "/T", "/PID", &pid])
            .creation_flags(0x0800_0000);
        command
    };
    #[cfg(not(windows))]
    let mut stopper = {
        // The child leads its own process group (see `hide_and_group`).
        let mut command = Command::new("kill");
        command.args(["-s", "KILL", "--", &format!("-{pid}")]);
        command
    };
    let _ = stopper
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_git(dir: &Path) -> Git {
        Git::isolated(&dir.join("gitconfig"))
    }

    #[test]
    fn git_s_output_and_its_first_problem_line_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let git = test_git(dir.path());
        let ran = git
            .run(None, ["--version"], Duration::from_secs(30))
            .unwrap();
        assert!(
            ran.succeeded && ran.stdout_text().starts_with("git version"),
            "{ran:?}"
        );
        let ran = git
            .run(
                Some(dir.path()),
                ["rev-parse", "HEAD"],
                Duration::from_secs(30),
            )
            .unwrap();
        assert!(!ran.succeeded && !ran.timed_out);
        assert!(
            ran.first_problem_line().contains("not a git repository"),
            "{ran:?}"
        );
    }

    #[test]
    fn git_that_is_not_installed_is_said() {
        let ran =
            Git::named("no-such-git-program").run(None, ["--version"], Duration::from_secs(5));
        assert_eq!(ran.unwrap_err(), NotInstalled);
    }

    /// A server that takes the connection and never answers: git waits
    /// for its first words for ever, so only the time limit ends the run.
    #[test]
    fn a_run_that_takes_too_long_is_stopped_at_its_time_limit() {
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _silent = thread::spawn(move || {
            let held: Vec<_> = listener.incoming().take(4).collect();
            thread::sleep(Duration::from_secs(60));
            drop(held);
        });
        let started = Instant::now();
        let ran = test_git(dir.path())
            .run_network(
                None,
                ["ls-remote", &format!("git://127.0.0.1:{port}/library.git")],
                Duration::from_secs(2),
            )
            .unwrap();
        assert!(ran.timed_out && !ran.succeeded, "{ran:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_person_s_own_ssh_command_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let git = test_git(dir.path());
        assert_eq!(git.has_own_ssh_command(None), Ok(false));
        let own = git
            .clone()
            .with_env("GIT_SSH_COMMAND", "C:/Windows/System32/OpenSSH/ssh.exe");
        assert_eq!(own.has_own_ssh_command(None), Ok(true));
        let plink = git.clone().with_env("GIT_SSH", "plink.exe");
        assert_eq!(plink.has_own_ssh_command(None), Ok(true));
        let variant = git.clone().with_env("GIT_SSH_VARIANT", "putty");
        assert_eq!(variant.has_own_ssh_command(None), Ok(true));
        std::fs::write(dir.path().join("gitconfig"), "[ssh]\n\tvariant = plink\n").unwrap();
        assert_eq!(git.has_own_ssh_command(None), Ok(true));
        // Set in git's settings, as Windows' OpenSSH for 1Password often is.
        std::fs::write(
            dir.path().join("gitconfig"),
            "[core]\n\tsshCommand = C:/Windows/System32/OpenSSH/ssh.exe\n",
        )
        .unwrap();
        assert_eq!(git.has_own_ssh_command(None), Ok(true));
    }
}

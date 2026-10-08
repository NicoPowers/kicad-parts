//! The single place this crate starts a process. Only kicad-cli is launched.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
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

/// Runs `cli` with `args`, capturing output, and kills it after `timeout`.
pub fn run_kicad_cli<I, S>(cli: &Path, args: I, timeout: Duration) -> std::io::Result<CliOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    #[allow(clippy::disallowed_methods)] // The one sanctioned spawn point.
    let mut command = Command::new(cli);
    command
        .args(args)
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

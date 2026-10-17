//! Waiting until a service we started can do real work.
//!
//! Each service has its own readiness check, because "the process answers"
//! is not enough. Two things went wrong before:
//!
//! - We pick a free port and then start the service, so another test's
//!   service can take that port in between. Ours then fails to bind, but
//!   the other service answers our health check. So a check must prove the
//!   service is ours: by its own log, or by our own credentials.
//! - MinIO's `/minio/health/ready` answers 200 before MinIO can serve
//!   bucket calls. So a check must mean "ready for the calls a test makes".
//!
//! [`wait_until_ready`] runs a service's check until it passes, and fails
//! with a plain message if the service exits or the time runs out.

use crate::process::ServiceProcess;
use crate::{ServiceResult, error};
use std::time::{Duration, Instant};

/// How long one health check request may take before we try again.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// How long to wait between tries.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Runs `is_ready` until it passes. It gets the service's whole log so far,
/// and returns `Err` with the reason when the service is not ready yet.
///
/// Fails, with the reason and the end of the service log, if the process
/// exits first or `is_ready` still fails after `timeout`.
pub(crate) fn wait_until_ready(
    process: &mut ServiceProcess,
    timeout: Duration,
    mut is_ready: impl FnMut(&str) -> Result<(), String>,
) -> ServiceResult<()> {
    let name = process.name().to_owned();
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = process.exited() {
            return Err(error(format!(
                "{name} exited with {status} before it was ready:\n{}",
                process.log_tail()
            )));
        }
        let log = process.log();
        let reason = match is_ready(&log) {
            Ok(()) => return Ok(()),
            Err(reason) => reason,
        };
        if Instant::now() > deadline {
            return Err(error(format!(
                "{name} was not ready after {timeout:?}: {reason}\n{}",
                process.log_tail()
            )));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// `Ok` if a line of `log` ends with `text`. Matching the end of a line
/// keeps port 5236 from matching 52362. Trailing spaces and carriage
/// returns are ignored.
pub(crate) fn log_has_line_ending_with(log: &str, text: &str) -> Result<(), String> {
    if log.lines().any(|line| line.trim_end().ends_with(text)) {
        Ok(())
    } else {
        Err(format!("its log does not yet say `{text}`"))
    }
}

/// `Ok` if a GET of `url` answers 200.
pub(crate) fn answers_200(url: &str) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    match agent.get(url).call() {
        Ok(response) if response.status() == 200 => Ok(()),
        Ok(response) => Err(format!("{url} answered {}", response.status())),
        Err(ureq::Error::Status(status, _)) => Err(format!("{url} answered {status}")),
        Err(failure) => Err(format!("{url} failed: {failure}")),
    }
}

/// A tiny HTTP server for tests that answers each path with a status
/// that the test can change while it runs.
#[cfg(test)]
pub(crate) mod fake_server {
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// Status codes by path, without the query; other paths answer 404.
    pub type Answers = Arc<Mutex<BTreeMap<String, u16>>>;

    /// Answers with `statuses` (path, status) until the test process ends,
    /// and returns the server's port and the handle to change answers.
    pub fn start(statuses: &[(&str, u16)]) -> (u16, Answers) {
        let answers = Answers::default();
        for (path, status) in statuses {
            answers.lock().unwrap().insert((*path).to_owned(), *status);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = answers.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Ok(reading) = stream.try_clone() else {
                    continue;
                };
                let mut reader = BufReader::new(reading);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                // Read the headers, so the client sees a complete exchange.
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
                    header.clear();
                }
                let target = request_line.split(' ').nth(1).unwrap_or("");
                let path = target.split('?').next().unwrap_or("");
                let status = shared.lock().unwrap().get(path).copied().unwrap_or(404);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        });
        (port, answers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_line_must_end_with_the_text() {
        let log = "2026/10/17 21:04:05 Server started at http://127.0.0.1:52362\r\n\
                   ├─ REST API:  http://127.0.0.1:52362/api/\n\
                   API: http://127.0.0.1:59003 \n";
        let started = |port: u16| format!("Server started at http://127.0.0.1:{port}");
        assert!(log_has_line_ending_with(log, &started(52362)).is_ok());
        assert!(log_has_line_ending_with(log, &started(5236)).is_err());
        assert!(log_has_line_ending_with(log, "API: http://127.0.0.1:59003").is_ok());
        let reason = log_has_line_ending_with("", &started(52362)).unwrap_err();
        assert!(reason.contains("does not yet say"), "{reason}");
    }

    #[test]
    fn only_200_counts_as_an_answer() {
        let (port, answers) = fake_server::start(&[("/health", 503)]);
        let url = format!("http://127.0.0.1:{port}/health");
        assert_eq!(answers_200(&url), Err(format!("{url} answered 503")));
        answers.lock().unwrap().insert("/health".into(), 200);
        assert_eq!(answers_200(&url), Ok(()));
    }
}

#[cfg(all(test, windows))]
mod waiting_tests {
    use super::*;
    use std::process::Command;

    /// A stand-in service: one process that runs for 30 s, or until the
    /// test drops it.
    fn stand_in_service(log: &std::path::Path) -> ServiceProcess {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 30",
        ]);
        ServiceProcess::spawn("stand-in", command, log).unwrap()
    }

    #[test]
    fn waiting_retries_until_the_check_passes() {
        let temporary = tempfile::tempdir().unwrap();
        let mut service = stand_in_service(&temporary.path().join("service.log"));
        let mut tries = 0;
        wait_until_ready(&mut service, Duration::from_secs(10), |_| {
            tries += 1;
            if tries < 3 {
                Err("not yet".into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(tries, 3);
    }

    #[test]
    fn waiting_too_long_fails_with_the_last_reason() {
        let temporary = tempfile::tempdir().unwrap();
        let mut service = stand_in_service(&temporary.path().join("service.log"));
        let failure = wait_until_ready(&mut service, Duration::from_millis(300), |_| {
            Err("the health check answered 503".into())
        })
        .unwrap_err();
        assert!(
            failure.0.starts_with("stand-in was not ready after"),
            "{failure}"
        );
        assert!(failure.0.contains("answered 503"), "{failure}");
    }

    #[test]
    fn a_service_that_exits_fails_at_once_with_its_log() {
        let temporary = tempfile::tempdir().unwrap();
        let mut command = Command::new("cmd");
        command.args(["/C", "echo bind: address in use"]);
        let mut service =
            ServiceProcess::spawn("stand-in", command, &temporary.path().join("exits.log"))
                .unwrap();
        let started = Instant::now();
        let failure = wait_until_ready(&mut service, Duration::from_secs(30), |_| {
            Err("not yet".into())
        })
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(failure.0.contains("exited"), "{failure}");
        assert!(failure.0.contains("bind: address in use"), "{failure}");
    }
}

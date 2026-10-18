//! Why the app could not talk to a team server, in plain words.
//!
//! Every network failure of the REST and realtime clients becomes one
//! [`ConnectionProblem`] here, so the sign-in form, the live-updates line
//! and the error messages all say the same thing for the same cause.

use std::error::Error;
use std::fmt;
use std::io;

/// Why a connection to the team server failed or broke.
///
/// Each message says what went wrong and one next step, because the
/// people reading it are hardware engineers, not network administrators.
/// The `String`s hold the technical detail, shown after the plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionProblem {
    /// No server answered: a wrong address, the server is down, or no
    /// network.
    NotReachable(String),
    /// The server took too long to answer.
    TimedOut,
    /// The connection worked, then broke.
    Lost(String),
}

impl ConnectionProblem {
    /// The problem behind a failed `ureq` request. `ureq` reports failures
    /// as a chain of errors, so this walks the chain for the cause.
    pub(crate) fn from_ureq(error: &ureq::Error) -> Self {
        if let Some(problem) = problem_in_chain(error) {
            return problem;
        }
        // The text can hold a file token in the URL, so it goes.
        let detail = without_queries(&error.to_string());
        match error {
            ureq::Error::Transport(transport)
                if matches!(
                    transport.kind(),
                    ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed
                ) =>
            {
                Self::NotReachable(detail)
            }
            _ => Self::Lost(detail),
        }
    }

    /// The problem behind an I/O error while connecting.
    pub(crate) fn while_connecting(error: &io::Error) -> Self {
        problem_in_io(error).unwrap_or_else(|| Self::NotReachable(error.to_string()))
    }

    /// The problem behind an I/O error after the connection worked.
    pub(crate) fn after_connecting(error: &io::Error) -> Self {
        problem_in_io(error).unwrap_or_else(|| Self::Lost(error.to_string()))
    }
}

impl fmt::Display for ConnectionProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReachable(detail) => write!(
                formatter,
                "Cannot reach the team server. Check the address and your network connection. (Details: {detail})"
            ),
            Self::TimedOut => write!(
                formatter,
                "The team server did not answer in time. Try again in a minute."
            ),
            Self::Lost(detail) => write!(
                formatter,
                "The connection to the team server broke. (Details: {detail})"
            ),
        }
    }
}

impl Error for ConnectionProblem {}

/// The first cause in `error`'s chain that names a problem.
fn problem_in_chain(error: &(dyn Error + 'static)) -> Option<ConnectionProblem> {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if let Some(io) = error.downcast_ref::<io::Error>() {
            if let Some(problem) = problem_in_io(io) {
                return Some(problem);
            }
        }
        cause = error.source();
    }
    None
}

/// A problem an I/O error names: a timeout, or a problem it carries.
/// `io::Error::source` skips the error it wraps, so this looks inside
/// with `get_ref` instead.
fn problem_in_io(error: &io::Error) -> Option<ConnectionProblem> {
    if let Some(inner) = error.get_ref() {
        if let Some(problem) = inner.downcast_ref::<ConnectionProblem>() {
            return Some(problem.clone());
        }
    }
    // Windows reports a socket timeout as TimedOut, Linux and macOS as
    // WouldBlock.
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
    .then_some(ConnectionProblem::TimedOut)
}

/// `text` without URL query strings, which can carry file tokens
/// (`?token=...`): drops everything from each `?` to the next whitespace.
pub(crate) fn without_queries(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut in_query = false;
    for ch in text.chars() {
        if ch == '?' {
            in_query = true;
        } else if ch.is_whitespace() {
            in_query = false;
        }
        if !in_query {
            clean.push(ch);
        }
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_are_named_on_every_platform() {
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::WouldBlock] {
            assert_eq!(
                ConnectionProblem::after_connecting(&io::Error::from(kind)),
                ConnectionProblem::TimedOut
            );
        }
    }

    #[test]
    fn a_problem_carried_inside_an_io_error_is_kept() {
        let carried = io::Error::new(io::ErrorKind::Other, ConnectionProblem::TimedOut);
        assert_eq!(
            ConnectionProblem::while_connecting(&carried),
            ConnectionProblem::TimedOut
        );
    }

    #[test]
    fn other_io_errors_say_whether_the_connection_ever_worked() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert!(matches!(
            ConnectionProblem::while_connecting(&refused),
            ConnectionProblem::NotReachable(_)
        ));
        assert!(matches!(
            ConnectionProblem::after_connecting(&refused),
            ConnectionProblem::Lost(_)
        ));
    }

    #[test]
    fn messages_say_what_to_do_and_keep_the_detail() {
        let message = ConnectionProblem::NotReachable("connection refused".into()).to_string();
        assert!(message.starts_with("Cannot reach the team server. Check the address"));
        assert!(message.ends_with("(Details: connection refused)"));
    }

    #[test]
    fn queries_are_left_out() {
        assert_eq!(
            without_queries("http://h/api/files/c/r/f.pdf?token=secret: Connection refused"),
            "http://h/api/files/c/r/f.pdf Connection refused"
        );
    }
}

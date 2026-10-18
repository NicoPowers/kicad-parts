//! Why the app could not talk to a team server, in plain words.
//!
//! Every network failure of the REST and realtime clients becomes one
//! [`ConnectionProblem`] here, so the sign-in form, the live-updates line
//! and the error messages all say the same thing for the same cause.
//! Certificate problems come from rustls; the operating system finds them
//! (see `tls.rs`).

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
    /// The certificate does not lead to an authority this computer
    /// trusts: self-made, or from a company authority not installed here.
    CertificateNotTrusted,
    /// The certificate is for another server name: a typo in the address,
    /// or the server's IP address typed instead of its name.
    CertificateForAnotherName,
    /// The certificate has expired or starts in the future. One problem,
    /// because Windows reports both as expired, and either may be this
    /// computer's clock.
    CertificateOutOfDate,
    /// Any other certificate problem. macOS reports an expired
    /// certificate this way too, so the message stays general.
    CertificateNotAccepted(String),
    /// Any other TLS failure, such as an `https://` address for a server
    /// that only speaks plain HTTP.
    NotSecure(String),
    /// An `https://` server redirected the app to `http://`, which would
    /// send the password or token unencrypted. The app refuses.
    RedirectToUnencrypted,
    /// The server redirected the app again and again.
    TooManyRedirects,
    /// The server answered the live-updates request with an error status,
    /// such as a proxy's `502 Bad Gateway`. Holds the status line.
    /// The operating system's trusted certificates could not be read, so
    /// no server can be checked.
    NoCertificateStore(String),
    /// No server answered: a wrong address, the server is down, or no
    /// network.
    NotReachable(String),
    /// The server took too long to answer.
    TimedOut,
    LiveUpdatesRefused(String),
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
        let ureq::Error::Transport(transport) = error else {
            return Self::Lost(detail);
        };
        match transport.kind() {
            ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed => Self::NotReachable(detail),
            ureq::ErrorKind::InsecureRequestHttpsOnly => Self::RedirectToUnencrypted,
            ureq::ErrorKind::TooManyRedirects => Self::TooManyRedirects,
            _ => Self::Lost(detail),
        }
    }

    /// The problem behind a failed TLS handshake.
    pub(crate) fn from_tls(error: &rustls::Error) -> Self {
        use rustls::CertificateError as Certificate;
        let rustls::Error::InvalidCertificate(certificate) = error else {
            return Self::NotSecure(error.to_string());
        };
        match certificate {
            Certificate::UnknownIssuer => Self::CertificateNotTrusted,
            Certificate::NotValidForName | Certificate::NotValidForNameContext { .. } => {
                Self::CertificateForAnotherName
            }
            Certificate::Expired
            | Certificate::ExpiredContext { .. }
            | Certificate::NotValidYet
            | Certificate::NotValidYetContext { .. } => Self::CertificateOutOfDate,
            _ => Self::CertificateNotAccepted(error.to_string()),
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
            Self::CertificateNotTrusted => write!(
                formatter,
                "The team server's certificate is not from an authority this computer trusts. Ask whoever runs the server to fix its certificate."
            ),
            Self::CertificateForAnotherName => write!(
                formatter,
                "The team server's certificate is for a different address. Check that you typed the address exactly as you were given it."
            ),
            Self::CertificateOutOfDate => write!(
                formatter,
                "The team server's certificate is out of date, or this computer's date is wrong. Check this computer's date and time. If it is right, ask whoever runs the server to renew the certificate."
            ),
            Self::CertificateNotAccepted(detail) => write!(
                formatter,
                "The team server's certificate was not accepted. Ask whoever runs the server to check its certificate. (Details: {detail})"
            ),
            Self::NotSecure(detail) => write!(
                formatter,
                "Could not make a secure connection to the team server. Ask whoever runs the server to check its HTTPS setup. (Details: {detail})"
            ),
            Self::RedirectToUnencrypted => write!(
                formatter,
                "The team server tried to switch to an unencrypted connection. Ask whoever runs the server to check its HTTPS setup."
            ),
            Self::TooManyRedirects => write!(
                formatter,
                "The team server keeps sending the app to another address. Ask whoever runs the server to check its HTTPS setup."
            ),
            Self::LiveUpdatesRefused(status) => write!(
                formatter,
                "The team server could not start live updates (it answered {status}). The app tries again by itself. If this lasts, ask whoever runs the server to check it."
            ),
            Self::NoCertificateStore(detail) => write!(
                formatter,
                "The app could not read this computer's trusted certificates, so it cannot check the team server. Ask your IT support for help. (Details: {detail})"
            ),
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
                "The connection to the team server broke. Check your network connection, then try again. (Details: {detail})"
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

/// A problem an I/O error names: a timeout, a TLS error from rustls, or
/// a problem it carries. `io::Error::source` skips the error it wraps, so
/// this looks inside with `get_ref` instead.
fn problem_in_io(error: &io::Error) -> Option<ConnectionProblem> {
    if let Some(inner) = error.get_ref() {
        if let Some(problem) = inner.downcast_ref::<ConnectionProblem>() {
            return Some(problem.clone());
        }
        if let Some(tls) = inner.downcast_ref::<rustls::Error>() {
            return Some(ConnectionProblem::from_tls(tls));
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
    fn certificate_errors_get_their_own_plain_reason() {
        use rustls::CertificateError as Certificate;
        let cases = [
            (
                Certificate::UnknownIssuer,
                ConnectionProblem::CertificateNotTrusted,
            ),
            (
                Certificate::NotValidForName,
                ConnectionProblem::CertificateForAnotherName,
            ),
            (
                Certificate::Expired,
                ConnectionProblem::CertificateOutOfDate,
            ),
            (
                Certificate::NotValidYet,
                ConnectionProblem::CertificateOutOfDate,
            ),
        ];
        for (certificate, expected) in cases {
            // rustls hands TLS errors over inside an io::Error.
            let io = io::Error::new(
                io::ErrorKind::InvalidData,
                rustls::Error::InvalidCertificate(certificate),
            );
            assert_eq!(ConnectionProblem::while_connecting(&io), expected);
        }
        // Anything else about the certificate, such as macOS's way of
        // saying "expired", still reads as a certificate problem.
        for other in [
            Certificate::Revoked,
            Certificate::Other(rustls::OtherError(std::sync::Arc::new(io::Error::other(
                "expired",
            )))),
        ] {
            let problem = ConnectionProblem::from_tls(&rustls::Error::InvalidCertificate(other));
            assert!(matches!(
                problem,
                ConnectionProblem::CertificateNotAccepted(_)
            ));
            assert!(
                problem
                    .to_string()
                    .starts_with("The team server's certificate")
            );
        }
        assert!(matches!(
            ConnectionProblem::from_tls(&rustls::Error::DecryptError),
            ConnectionProblem::NotSecure(_)
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

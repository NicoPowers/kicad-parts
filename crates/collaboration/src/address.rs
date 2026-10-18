//! Team server addresses: the rules for what a person may type at sign-in,
//! and the parts a connection needs.
//!
//! The rules protect the password, which sign-in sends to the server:
//!
//! - `https://` works for any server.
//! - `http://` works only for a server on this computer (loopback), such
//!   as a local test server, because the password never leaves the
//!   computer. Any other `http://` address is refused.
//! - An address without `http://` or `https://` gets `https://`, or
//!   `http://` for a server on this computer, which is what local test
//!   servers speak.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// A team server's address, checked: a scheme, a host and maybe a port,
/// with nothing after them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    secure: bool,
    /// As typed, IPv6 addresses in brackets.
    host: String,
    port: Option<u16>,
}

/// Why a typed address was refused, in words for the sign-in form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressRefusal {
    Empty,
    NotAnAddress,
    /// A scheme other than `http://` or `https://`, such as `ftp://`.
    UnknownScheme,
    /// A path, query or fragment after the host, such as a dashboard link.
    HasPath,
    /// `http://` for a server on another computer; `host` is as typed.
    PlainHttpToAnotherComputer {
        host: String,
    },
}

impl fmt::Display for AddressRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(
                formatter,
                "Enter the team server address, such as https://team.example.com."
            ),
            Self::NotAnAddress => write!(
                formatter,
                "This is not a server address. Enter one such as https://team.example.com."
            ),
            Self::UnknownScheme => write!(
                formatter,
                "The address must start with https://, such as https://team.example.com."
            ),
            Self::HasPath => write!(
                formatter,
                "Enter only the server's name and port, with nothing after them, such as https://team.example.com."
            ),
            Self::PlainHttpToAnotherComputer { host } => write!(
                formatter,
                "Addresses starting with http:// send your password unencrypted, so they only work for a server on this computer. Use https://{host} instead."
            ),
        }
    }
}

impl std::error::Error for AddressRefusal {}

impl ServerAddress {
    /// The address a person typed at sign-in, checked against the rules
    /// in this module's documentation.
    pub fn for_sign_in(text: &str) -> Result<Self, AddressRefusal> {
        let text = text.trim().trim_end_matches('/');
        if text.is_empty() {
            return Err(AddressRefusal::Empty);
        }
        let (scheme, rest) = match text.split_once("://") {
            Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
            None => (None, text),
        };
        let (host, port) = host_and_port(rest)?;
        let on_this_computer = is_loopback(&host);
        let secure = match scheme.as_deref() {
            Some("https") => true,
            Some("http") if on_this_computer => false,
            Some("http") => return Err(AddressRefusal::PlainHttpToAnotherComputer { host }),
            Some(_) => return Err(AddressRefusal::UnknownScheme),
            None => !on_this_computer,
        };
        Ok(Self { secure, host, port })
    }

    /// A base URL the app already holds, such as `http://127.0.0.1:8090`.
    /// It must name its scheme, and only the sign-in rules decide which
    /// hosts may use plain HTTP.
    pub(crate) fn from_url(url: &str) -> Result<Self, AddressRefusal> {
        let url = url.trim_end_matches('/');
        let (scheme, rest) = url.split_once("://").ok_or(AddressRefusal::UnknownScheme)?;
        let secure = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(AddressRefusal::UnknownScheme),
        };
        let (host, port) = host_and_port(rest)?;
        Ok(Self { secure, host, port })
    }

    /// The address as a base URL, such as `https://team.example.com`.
    pub fn url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}", self.authority())
    }

    /// Whether the connection uses TLS (`https://`).
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// The host as typed, IPv6 addresses in brackets.
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    /// The port, or the scheme's usual one.
    pub(crate) fn port(&self) -> u16 {
        self.port.unwrap_or(if self.secure { 443 } else { 80 })
    }

    /// `host` or `host:port`, as in the URL and the `Host` header.
    pub(crate) fn authority(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }
}

/// Splits `host[:port]`, refusing anything after it, user names (`a@b`)
/// and characters no server name has.
fn host_and_port(authority: &str) -> Result<(String, Option<u16>), AddressRefusal> {
    if authority.contains(['/', '?', '#']) {
        return Err(AddressRefusal::HasPath);
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        // [IPv6]:port
        let (inside, after) = bracketed
            .split_once(']')
            .ok_or(AddressRefusal::NotAnAddress)?;
        inside
            .parse::<Ipv6Addr>()
            .map_err(|_| AddressRefusal::NotAnAddress)?;
        let port = match after {
            "" => None,
            _ => Some(
                after
                    .strip_prefix(':')
                    .ok_or(AddressRefusal::NotAnAddress)?,
            ),
        };
        (format!("[{inside}]"), port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (authority.to_owned(), None),
        }
    };
    let is_name_character = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
    if host.is_empty() || !(host.starts_with('[') || host.chars().all(is_name_character)) {
        return Err(AddressRefusal::NotAnAddress);
    }
    let port = port
        .map(|digits| match digits.parse::<u16>() {
            // parse alone takes "+80".
            Ok(port) if port > 0 && digits.bytes().all(|b| b.is_ascii_digit()) => Ok(port),
            _ => Err(AddressRefusal::NotAnAddress),
        })
        .transpose()?;
    Ok((host, port))
}

/// Whether `host` is this computer: `localhost`, `127.x.x.x` or `[::1]`.
fn is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Some(inside) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inside.parse::<Ipv6Addr>().is_ok_and(|ip| ip.is_loopback());
    }
    host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign_in_url(text: &str) -> Result<String, AddressRefusal> {
        ServerAddress::for_sign_in(text).map(|address| address.url())
    }

    #[test]
    fn https_works_for_any_server() {
        for (typed, url) in [
            ("https://team.example.com", "https://team.example.com"),
            (
                " HTTPS://team.example.com:8443/ ",
                "https://team.example.com:8443",
            ),
            ("https://127.0.0.1:8090", "https://127.0.0.1:8090"),
            ("https://[2001:db8::1]:443", "https://[2001:db8::1]:443"),
        ] {
            assert_eq!(sign_in_url(typed).as_deref(), Ok(url), "{typed}");
        }
    }

    #[test]
    fn plain_http_works_only_on_this_computer() {
        for typed in [
            "http://127.0.0.1:8090",
            "http://127.1.2.3:8090",
            "http://localhost:8090",
            "http://LocalHost",
            "http://[::1]:8090",
        ] {
            assert_eq!(sign_in_url(typed).as_deref(), Ok(typed), "{typed}");
        }
        assert_eq!(
            sign_in_url("http://team.example.com:8090"),
            Err(AddressRefusal::PlainHttpToAnotherComputer {
                host: "team.example.com".into()
            })
        );
        for refused in [
            "http://192.168.1.20:8090",
            "http://127.0.0.1.example.com",
            "http://[2001:db8::1]",
        ] {
            assert!(
                matches!(
                    sign_in_url(refused),
                    Err(AddressRefusal::PlainHttpToAnotherComputer { .. })
                ),
                "{refused}"
            );
        }
    }

    #[test]
    fn the_refusal_suggests_the_https_address() {
        let refusal = sign_in_url("http://team.example.com:8090").unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "Addresses starting with http:// send your password unencrypted, so they only work for a server on this computer. Use https://team.example.com instead."
        );
    }

    #[test]
    fn without_a_scheme_https_is_assumed_except_on_this_computer() {
        assert_eq!(
            sign_in_url("team.example.com").as_deref(),
            Ok("https://team.example.com")
        );
        assert_eq!(
            sign_in_url("127.0.0.1:8090").as_deref(),
            Ok("http://127.0.0.1:8090")
        );
        assert_eq!(
            sign_in_url("localhost:8090").as_deref(),
            Ok("http://localhost:8090")
        );
    }

    #[test]
    fn anything_but_a_server_address_is_refused_plainly() {
        for (typed, refusal) in [
            ("", AddressRefusal::Empty),
            ("  / ", AddressRefusal::Empty),
            ("ftp://team.example.com", AddressRefusal::UnknownScheme),
            ("https://team.example.com/_/", AddressRefusal::HasPath),
            ("http://127.0.0.1:8090/api", AddressRefusal::HasPath),
            ("https://team.example.com?x=1", AddressRefusal::HasPath),
            ("https://", AddressRefusal::NotAnAddress),
            ("https://me@team.example.com", AddressRefusal::NotAnAddress),
            (
                "https://team.example.com:http",
                AddressRefusal::NotAnAddress,
            ),
            ("https://team.example.com:0", AddressRefusal::NotAnAddress),
            ("https://team.example.com:+80", AddressRefusal::NotAnAddress),
            ("https://team example.com", AddressRefusal::NotAnAddress),
            ("https://[not-ipv6]", AddressRefusal::NotAnAddress),
            ("https://2001:db8::1", AddressRefusal::NotAnAddress),
        ] {
            assert_eq!(sign_in_url(typed), Err(refusal), "{typed:?}");
        }
    }

    #[test]
    fn base_urls_name_their_parts() {
        let address = ServerAddress::from_url("https://team.example.com/").unwrap();
        assert!(address.is_secure());
        assert_eq!(
            (address.host(), address.port(), address.authority().as_str()),
            ("team.example.com", 443, "team.example.com")
        );
        let local = ServerAddress::from_url("http://[::1]:8090").unwrap();
        assert!(!local.is_secure());
        assert_eq!((local.host(), local.port()), ("[::1]", 8090));
        assert!(ServerAddress::from_url("team.example.com").is_err());
    }
}

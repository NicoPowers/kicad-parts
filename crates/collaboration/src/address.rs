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
//!
//! An address may end in a path, such as `https://parts.example.com/acme`,
//! so several team servers can share one proxy. A pasted link to a page on
//! the server (`/_/` for the admin page, or `/api/...`) is refused: it is
//! a mistake, not an address.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// A team server's address, checked: a scheme, a host, maybe a port and
/// maybe a path, with no query or fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    secure: bool,
    /// As typed, IPv6 addresses in brackets.
    host: String,
    port: Option<u16>,
    /// Empty, or `/` and segments, such as `/acme`. Never ends in `/`.
    path: String,
}

/// Why a typed address was refused, in words for the sign-in form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressRefusal {
    Empty,
    NotAnAddress,
    /// A scheme other than `http://` or `https://`, such as `ftp://`.
    UnknownScheme,
    /// A `?` query or `#` fragment after the address.
    HasQuery,
    /// A link to a page on the server: the admin page (`/_/`) or the API
    /// (`/api/...`).
    PageLink,
    /// A path with characters other than letters, digits and `-._~`, or
    /// with an empty, `.` or `..` segment.
    BadPath,
    /// `http://` for a server on another computer. `suggestion` is the
    /// same address with `https://` and no port.
    PlainHttpToAnotherComputer {
        suggestion: String,
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
            Self::HasQuery => write!(
                formatter,
                "Enter the address without ? or # and what follows them, such as https://team.example.com."
            ),
            Self::PageLink => write!(
                formatter,
                "This is a link to a page on the team server, not its address. Leave out /_/ or /api and everything after it."
            ),
            Self::BadPath => write!(
                formatter,
                "After the server name, the address may hold only letters, digits, -, ., _ and ~, such as https://parts.example.com/acme."
            ),
            Self::PlainHttpToAnotherComputer { suggestion } => write!(
                formatter,
                "Addresses starting with http:// send your password unencrypted, so they only work for a server on this computer. Use an https:// address, such as {suggestion}."
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
        let (host, port, path) = host_port_and_path(rest)?;
        let secure = match scheme.as_deref() {
            Some("https") => true,
            Some("http") => false,
            Some(_) => return Err(AddressRefusal::UnknownScheme),
            None => !is_loopback(&host),
        };
        let address = Self {
            secure,
            host,
            port,
            path,
        };
        address.may_receive_password()?;
        Ok(address)
    }

    /// A base URL the app already holds, such as `http://127.0.0.1:8090`.
    /// It must name its scheme. Plain HTTP to another computer parses, but
    /// [`ServerAddress::may_receive_password`] refuses it.
    pub(crate) fn from_url(url: &str) -> Result<Self, AddressRefusal> {
        let url = url.trim_end_matches('/');
        let (scheme, rest) = url.split_once("://").ok_or(AddressRefusal::UnknownScheme)?;
        let secure = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(AddressRefusal::UnknownScheme),
        };
        let (host, port, path) = host_port_and_path(rest)?;
        Ok(Self {
            secure,
            host,
            port,
            path,
        })
    }

    /// Ok if a password may go to this address: it uses `https://`, or the
    /// server is on this computer. Sign-in and
    /// [`crate::PocketBaseClient::authenticate`] both check it.
    pub(crate) fn may_receive_password(&self) -> Result<(), AddressRefusal> {
        if self.secure || is_loopback(&self.host) {
            return Ok(());
        }
        Err(AddressRefusal::PlainHttpToAnotherComputer {
            suggestion: format!("https://{}{}", self.host, self.path),
        })
    }

    /// The address as a base URL, such as `https://team.example.com` or
    /// `https://parts.example.com/acme`.
    pub fn url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}{}", self.authority(), self.path)
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

    /// Empty, or the path the server is under, such as `/acme`. Requests
    /// go to this path followed by `/api/...`.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }
}

/// Splits `host[:port][/path]`, refusing queries, fragments, user names
/// (`a@b`), links to pages on the server and characters no address has.
fn host_port_and_path(rest: &str) -> Result<(String, Option<u16>, String), AddressRefusal> {
    if rest.contains(['?', '#']) {
        return Err(AddressRefusal::HasQuery);
    }
    let (authority, path) = match rest.find('/') {
        Some(slash) => rest.split_at(slash),
        None => (rest, ""),
    };
    let (host, port) = host_and_port(authority)?;
    check_path(path)?;
    Ok((host, port, path.to_owned()))
}

/// Checks a path such as `/acme` or `/clients/acme`. Each segment holds
/// only unreserved characters (RFC 3986), so it never needs encoding.
fn check_path(path: &str) -> Result<(), AddressRefusal> {
    let Some(segments) = path.strip_prefix('/') else {
        return Ok(()); // No path.
    };
    let segments: Vec<&str> = segments.split('/').collect();
    if segments
        .iter()
        .any(|segment| matches!(*segment, "_" | "api"))
    {
        return Err(AddressRefusal::PageLink);
    }
    let unreserved = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~');
    let good = |segment: &&str| {
        !segment.is_empty() && !matches!(*segment, "." | "..") && segment.chars().all(unreserved)
    };
    if segments.iter().all(good) {
        Ok(())
    } else {
        Err(AddressRefusal::BadPath)
    }
}

/// Splits `host[:port]`, refusing user names (`a@b`) and characters no
/// server name has.
fn host_and_port(authority: &str) -> Result<(String, Option<u16>), AddressRefusal> {
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
/// An IPv4-mapped address such as `[::ffff:127.0.0.1]` does not count:
/// nobody types one for a local server, so it gets the stricter rule.
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
    fn an_address_may_end_in_a_path_for_servers_behind_one_proxy() {
        for (typed, url) in [
            (
                "https://parts.example.com/acme",
                "https://parts.example.com/acme",
            ),
            (
                "https://parts.example.com/acme/",
                "https://parts.example.com/acme",
            ),
            (
                "https://parts.example.com:8443/clients/acme-2_x.y~z",
                "https://parts.example.com:8443/clients/acme-2_x.y~z",
            ),
            ("parts.example.com/acme", "https://parts.example.com/acme"),
            ("http://127.0.0.1:8090/acme", "http://127.0.0.1:8090/acme"),
        ] {
            assert_eq!(sign_in_url(typed).as_deref(), Ok(url), "{typed}");
        }
        let address = ServerAddress::for_sign_in("https://parts.example.com/acme/").unwrap();
        assert_eq!(address.path(), "/acme");
        let bare = ServerAddress::for_sign_in("https://team.example.com").unwrap();
        assert_eq!(bare.path(), "");
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
            sign_in_url("http://team.example.com:8090/acme"),
            Err(AddressRefusal::PlainHttpToAnotherComputer {
                suggestion: "https://team.example.com/acme".into()
            })
        );
        for refused in [
            "http://192.168.1.20:8090",
            "http://127.0.0.1.example.com",
            "http://127.0.0.1.evil.com",
            "http://localhost.evil.com",
            "http://[2001:db8::1]",
            "http://[::ffff:127.0.0.1]:8090",
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
    fn the_refusal_suggests_an_https_address() {
        let refusal = sign_in_url("http://team.example.com:8090").unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "Addresses starting with http:// send your password unencrypted, so they only work for a server on this computer. Use an https:// address, such as https://team.example.com."
        );
    }

    #[test]
    fn without_a_scheme_https_is_assumed_except_on_this_computer() {
        for (typed, url) in [
            ("team.example.com", "https://team.example.com"),
            ("127.0.0.1:8090", "http://127.0.0.1:8090"),
            ("localhost:8090", "http://localhost:8090"),
            ("localhost.evil.com", "https://localhost.evil.com"),
            ("127.0.0.1.evil.com", "https://127.0.0.1.evil.com"),
            ("[::ffff:127.0.0.1]:8090", "https://[::ffff:127.0.0.1]:8090"),
        ] {
            assert_eq!(sign_in_url(typed).as_deref(), Ok(url), "{typed}");
        }
    }

    #[test]
    fn anything_but_a_server_address_is_refused_plainly() {
        for (typed, refusal) in [
            ("", AddressRefusal::Empty),
            ("  / ", AddressRefusal::Empty),
            ("ftp://team.example.com", AddressRefusal::UnknownScheme),
            ("https://team.example.com/_/", AddressRefusal::PageLink),
            ("https://parts.example.com/acme/_", AddressRefusal::PageLink),
            ("http://127.0.0.1:8090/api", AddressRefusal::PageLink),
            (
                "https://team.example.com/api/health",
                AddressRefusal::PageLink,
            ),
            ("https://team.example.com?x=1", AddressRefusal::HasQuery),
            (
                "https://team.example.com/acme#top",
                AddressRefusal::HasQuery,
            ),
            ("https://parts.example.com//acme", AddressRefusal::BadPath),
            ("https://parts.example.com/a%20b", AddressRefusal::BadPath),
            (
                "https://parts.example.com/acme corp",
                AddressRefusal::BadPath,
            ),
            ("https://parts.example.com/../x", AddressRefusal::BadPath),
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
        let behind_proxy = ServerAddress::from_url("https://parts.example.com/acme").unwrap();
        assert_eq!(behind_proxy.path(), "/acme");
        assert!(ServerAddress::from_url("team.example.com").is_err());
        // It parses, but no password may go there.
        let plain = ServerAddress::from_url("http://team.example.com").unwrap();
        assert!(plain.may_receive_password().is_err());
    }
}

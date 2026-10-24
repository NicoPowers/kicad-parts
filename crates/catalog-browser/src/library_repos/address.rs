//! A library repo's address, and the one key the app knows the repo by.
//!
//! The same repo can be written several ways: `git@github.com:acme/lib.git`,
//! `ssh://git@github.com/acme/lib` and `https://github.com/acme/lib.git` are
//! one repo. The app keys everything by [`RepoAddress::key`], here
//! `github.com/acme/lib`, so two teams that link one repo share one copy.

use crate::file_safe::file_safe;
use std::path::PathBuf;

/// Why an address with a password or token in it is refused. It would be
/// kept in the list of teams, shown in the Teams menu and written in the
/// app's copy, so the app never takes one.
pub const HAS_PASSWORD: &str = "That address has a password or token in it. The app never keeps passwords: sign in with a git credential helper, or use the repo's ssh address.";

/// A git address the app accepts for a library repo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoAddress {
    /// The address as given, which is what git is given.
    url: String,
    place: Place,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Place {
    /// On another computer: the host in small letters, a port that is not
    /// the scheme's usual one, and the repo's path on the host.
    Host {
        host: String,
        port: Option<u16>,
        path: String,
    },
    /// A folder on this computer or a network share, such as a bare repo.
    Folder { path: String },
}

/// The schemes git reaches repos with, and the port each uses unless told
/// otherwise.
const SCHEMES: [(&str, u16); 7] = [
    ("ssh", 22),
    ("git+ssh", 22),
    ("ssh+git", 22),
    ("https", 443),
    ("http", 80),
    ("git", 9418),
    ("file", 0),
];

impl RepoAddress {
    /// The address in `text`, or why it is not one, in plain words.
    ///
    /// Accepts `[user@]host:path` (as GitHub, GitLab and Bitbucket show for
    /// SSH), `ssh://`, `https://` and git's other URLs, and a whole folder
    /// path such as a bare repo on a network share.
    pub fn parse(text: &str) -> Result<Self, String> {
        let url = text.trim();
        if url.is_empty() {
            return Err("Enter the repo's address, or a folder with a clone of it.".to_owned());
        }
        // git would read the first as an option, and the second as a
        // helper program to run (`ext::`, `fd::`), never as an address.
        let helper = url
            .split('/')
            .next()
            .is_some_and(|start| start.contains("::"));
        if url.starts_with('-') || helper {
            return Err(format!("{url} is not a repo address."));
        }
        let place = if let Some((scheme, rest)) = url.split_once("://") {
            if has_password(rest) {
                return Err(HAS_PASSWORD.to_owned());
            }
            from_url(&scheme.to_ascii_lowercase(), rest)?
        } else if let Some((authority, path)) = scp_like(url) {
            host_place(authority, None, path)?
        } else if is_whole_path(url) {
            Place::Folder {
                path: url.to_owned(),
            }
        } else {
            return Err(format!(
                "{url} is not a repo address or a whole folder path. Copy the repo's address from its page on the git host, such as git@gitlab.com:acme/kicad-library.git."
            ));
        };
        Ok(Self {
            url: url.to_owned(),
            place,
        })
    }

    /// The address as given: what git is given.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether the repo is in a folder on this computer or a network share,
    /// given as a path or a `file://` URL.
    pub fn is_folder(&self) -> bool {
        matches!(self.place, Place::Folder { .. })
    }

    /// The folder, for a repo in one.
    pub fn folder_path(&self) -> Option<PathBuf> {
        match &self.place {
            Place::Folder { path } => Some(PathBuf::from(path)),
            Place::Host { .. } => None,
        }
    }

    /// Whether git reaches the repo over https or http, where a credential
    /// helper may ask for a sign-in.
    pub fn is_web(&self) -> bool {
        let scheme = self
            .url
            .split_once("://")
            .map(|(scheme, _)| scheme.to_ascii_lowercase());
        matches!(scheme.as_deref(), Some("https" | "http"))
    }

    /// The key the app knows the repo by, whatever way its address is
    /// written: the host in small letters, any port that is not the usual
    /// one, and the path without `.git` at its end, such as
    /// `github.com/acme/lib`. A folder is keyed by its path with `/`, in
    /// small letters on Windows, where folder names ignore case.
    pub fn key(&self) -> String {
        match &self.place {
            Place::Host { host, port, path } => {
                // An IPv6 address keeps its brackets, so its port stays apart.
                let host = if host.contains(':') {
                    format!("[{host}]")
                } else {
                    host.clone()
                };
                match port {
                    Some(port) => format!("{host}:{port}/{path}"),
                    None => format!("{host}/{path}"),
                }
            }
            Place::Folder { path } => {
                let path = path.replace('\\', "/");
                let path = path.trim_end_matches('/');
                if cfg!(windows) {
                    path.to_lowercase()
                } else {
                    path.to_owned()
                }
            }
        }
    }

    /// The name of the folder the app keeps its copy of the repo in, made
    /// from [`RepoAddress::key`].
    pub fn folder_name(&self) -> String {
        file_safe(&self.key())
    }

    /// The repo's short name, such as `acme/kicad-lib`, or a folder's own
    /// name.
    pub fn name(&self) -> String {
        match &self.place {
            Place::Host { path, .. } => path.clone(),
            Place::Folder { path } => {
                let path = path.replace('\\', "/");
                let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                last.strip_suffix(".git").unwrap_or(last).to_owned()
            }
        }
    }

    /// Where the repo is, for messages: its host, such as `gitlab.com`, or
    /// "this computer" for a folder.
    pub fn host(&self) -> String {
        match &self.place {
            Place::Host { host, .. } => host.clone(),
            Place::Folder { .. } => "this computer".to_owned(),
        }
    }
}

/// The place in a URL with `scheme`, whose text after `://` is `rest`.
fn from_url(scheme: &str, rest: &str) -> Result<Place, String> {
    let Some((_, usual_port)) = SCHEMES.iter().find(|(known, _)| *known == scheme) else {
        return Err(format!(
            "The app reaches repos by ssh or https, not {scheme}://. Copy the repo's ssh or https address from its page on the git host."
        ));
    };
    if scheme == "file" {
        // `file:///C:/x` names the folder `C:/x`; `file:///srv/x` names `/srv/x`.
        let path = match rest.strip_prefix('/') {
            Some(after) if is_drive(after) => after,
            _ => rest,
        };
        return Ok(Place::Folder {
            path: path.to_owned(),
        });
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = split_port(authority)?;
    let port = port.filter(|port| port != usual_port);
    host_place(host, port, path)
}

/// Whether the text after `scheme://` names a user with a password, as in
/// `https://oauth2:glpat-…@gitlab.com/acme/lib.git`.
fn has_password(rest: &str) -> bool {
    let authority = rest.split('/').next().unwrap_or(rest);
    authority
        .rsplit_once('@')
        .is_some_and(|(user, _)| user.contains(':'))
}

/// `[user@]host[:port]` as the host and the port, if any.
fn split_port(authority: &str) -> Result<(&str, Option<u16>), String> {
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    // An IPv6 address is written in brackets: `[::1]:2222`.
    let (host, port) = if let Some(after) = host.strip_prefix('[') {
        match after.split_once(']') {
            Some((inside, rest)) => (inside, rest.strip_prefix(':')),
            None => return Err(format!("{authority} is not a host name.")),
        }
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host, None),
        }
    };
    let port = match port {
        None | Some("") => None,
        Some(port) => Some(
            port.parse::<u16>()
                .map_err(|_| format!("{port} is not a port number."))?,
        ),
    };
    Ok((host, port))
}

/// A place on host `authority` (any `user@` dropped), with `port`, at
/// `path`: the path without `/` at either end or `.git` at its end.
fn host_place(authority: &str, port: Option<u16>, path: &str) -> Result<Place, String> {
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err("The address needs the git host's name, such as gitlab.com.".to_owned());
    }
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    if path.is_empty() {
        return Err(format!(
            "The address needs the repo's path on {host}, such as {host}:acme/kicad-library.git."
        ));
    }
    Ok(Place::Host {
        host,
        port,
        path: path.to_owned(),
    })
}

/// `[user@]host:path`, as git reads it: a colon before any slash. A
/// Windows drive such as `C:` is a folder, not a host.
fn scp_like(text: &str) -> Option<(&str, &str)> {
    let (before, after) = text.split_once(':')?;
    let is_host = !before.is_empty() && !before.contains(['/', '\\']) && !is_drive(text);
    is_host.then_some((before, after))
}

/// Whether `text` starts with a Windows drive, such as `C:\` or `C:/`.
fn is_drive(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/' || bytes[2] == b'\\')
}

/// Whether `text` is a whole folder path: from a drive, a network share
/// (`\\server\share`) or the root (`/`).
fn is_whole_path(text: &str) -> bool {
    is_drive(text) || text.starts_with("\\\\") || text.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> String {
        RepoAddress::parse(text).unwrap().key()
    }

    #[test]
    fn one_repo_written_any_way_has_one_key() {
        for text in [
            "git@github.com:acme/kicad-lib.git",
            "git@GitHub.com:acme/kicad-lib",
            "ssh://git@github.com/acme/kicad-lib.git",
            "ssh://git@github.com:22/acme/kicad-lib/",
            "SSH://git@github.com/acme/kicad-lib",
            "https://github.com/acme/kicad-lib.git",
            "https://ada@github.com:443/acme/kicad-lib",
            "  https://github.com/acme/kicad-lib.git/  ",
        ] {
            assert_eq!(key(text), "github.com/acme/kicad-lib", "{text}");
        }
        // The path keeps its capitals: some git servers tell them apart.
        assert_eq!(key("git@gitlab.com:Acme/Lib.git"), "gitlab.com/Acme/Lib");
        // Another port may be another server.
        assert_eq!(
            key("ssh://git@git.example.com:2222/hw/lib.git"),
            "git.example.com:2222/hw/lib"
        );
        assert_eq!(key("ssh://git@[::1]:2222/lib"), "[::1]:2222/lib");
        // GitLab subgroups and Bitbucket keep their whole path.
        assert_eq!(
            key("git@gitlab.com:client/hardware/kicad-library.git"),
            "gitlab.com/client/hardware/kicad-library"
        );
        assert_eq!(
            key("https://ada@bitbucket.org/client/kicad.git"),
            "bitbucket.org/client/kicad"
        );
    }

    #[test]
    fn a_short_name_and_a_host_name_the_repo_in_messages() {
        let address = RepoAddress::parse("git@gitlab.com:client/hw/kicad-library.git").unwrap();
        assert_eq!(address.name(), "client/hw/kicad-library");
        assert_eq!(address.host(), "gitlab.com");
        // git is given the address as written.
        assert_eq!(address.url(), "git@gitlab.com:client/hw/kicad-library.git");
        let folder = RepoAddress::parse(r"\\nas\hw\client-library.git").unwrap();
        assert_eq!(folder.name(), "client-library");
        assert_eq!(folder.host(), "this computer");
        assert!(folder.is_folder() && !address.is_folder());
        assert!(
            RepoAddress::parse("file:///srv/lib.git")
                .unwrap()
                .is_folder()
        );
        assert!(
            RepoAddress::parse("https://gitlab.com/a/b")
                .unwrap()
                .is_web()
        );
        assert!(!address.is_web());
    }

    #[test]
    fn folders_are_repos_too() {
        assert_eq!(key("/srv/git/library.git/"), "/srv/git/library.git");
        let windows = key(r"C:\Repos\Library.git");
        if cfg!(windows) {
            assert_eq!(windows, "c:/repos/library.git");
            assert_eq!(key("file:///C:/Repos/Library.git"), "c:/repos/library.git");
        } else {
            assert_eq!(windows, "C:/Repos/Library.git");
        }
        assert_eq!(key("file:///srv/git/library.git"), "/srv/git/library.git");
    }

    #[test]
    fn folder_names_never_clash() {
        let name = |text: &str| RepoAddress::parse(text).unwrap().folder_name();
        assert_eq!(
            name("git@github.com:acme/lib.git"),
            "github.com_2f_acme_2f_lib"
        );
        // Where folder names ignore case, these must stay apart.
        assert_ne!(
            name("git@github.com:acme/Lib.git").to_lowercase(),
            name("git@github.com:acme/lib.git").to_lowercase()
        );
    }

    #[test]
    fn what_is_no_address_is_said_plainly() {
        let refused = |text: &str| RepoAddress::parse(text).unwrap_err();
        assert!(refused("").starts_with("Enter the repo's address"));
        assert!(refused("kicad-library").contains("is not a repo address or a whole folder path"));
        assert!(refused("--upload-pack=evil").contains("is not a repo address"));
        assert!(refused("ext::sh -c touch% /tmp/x").contains("is not a repo address"));
        assert!(refused("fd::3").contains("is not a repo address"));
        // A password or token is never kept.
        for text in [
            "https://oauth2:glpat-abc123@gitlab.com/acme/lib.git",
            "https://ada:secret@github.com/acme/lib",
            "ssh://git:pw@git.example.com:2222/hw/lib.git",
        ] {
            assert_eq!(refused(text), HAS_PASSWORD, "{text}");
        }
        // A user name alone is fine.
        assert!(RepoAddress::parse("https://ada@bitbucket.org/acme/lib.git").is_ok());
        assert!(refused("ftp://host/x").contains("by ssh or https, not ftp://"));
        assert!(refused("git@gitlab.com:").contains("needs the repo's path on gitlab.com"));
        assert!(refused("https://gitlab.com/").contains("needs the repo's path"));
        assert!(refused("ssh://git@host:port/x").contains("port is not a port number"));
        assert!(refused("https:///x").contains("needs the git host's name"));
    }
}

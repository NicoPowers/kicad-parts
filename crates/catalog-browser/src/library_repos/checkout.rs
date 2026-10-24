//! Finding a library repo's address from a folder the person chose: their
//! own clone of the repo, or a bare repo such as one on a network share.
//!
//! The app only reads the folder, and never writes in it (mx-70700f): git
//! runs with `GIT_OPTIONAL_LOCKS=0`, so not even git's index is refreshed.
//! The app then works in a copy of its own, made from the address.

use super::address::RepoAddress;
use crate::git_program::{Git, NotInstalled};
use desktop_contract::RepoRemote;
use std::path::Path;
use std::time::Duration;

/// How long one read of the person's folder may take.
const READ_LIMIT: Duration = Duration::from_secs(30);

/// What a folder says about the repo it holds.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum FromFolder {
    /// The repo's address: the clone's `origin` or only remote, or a bare
    /// repo's own folder.
    Address(String),
    /// A clone with several remotes and no `origin`: the person chooses.
    ChooseRemote(Vec<RepoRemote>),
}

/// Why a folder gave no address.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum NoAddress {
    GitNotInstalled,
    /// What to say, in plain words.
    Said(String),
}

impl From<NotInstalled> for NoAddress {
    fn from(_: NotInstalled) -> Self {
        Self::GitNotInstalled
    }
}

/// The address of the repo in `folder`, the person's own clone or a bare
/// repo, read without writing anything there. A remote whose address holds
/// a password is never used, or shown.
pub(crate) fn address_in(git: &Git, folder: &Path) -> Result<FromFolder, NoAddress> {
    let git = git.clone().with_env("GIT_OPTIONAL_LOCKS", "0");
    let read = |args: &[&str]| git.run(Some(folder), args, READ_LIMIT);
    let bare = read(&["rev-parse", "--is-bare-repository"])?;
    if !bare.succeeded {
        return Err(NoAddress::Said(cannot_read(folder, &bare.stderr)));
    }
    if bare.stdout_text().trim() == "true" {
        // A bare repo has no work of anyone's in it: it is the remote.
        return Ok(FromFolder::Address(folder.display().to_string()));
    }
    let names = read(&["remote"])?;
    let names: Vec<String> = names
        .stdout_text()
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    let address_of = |name: &str| -> Result<String, NoAddress> {
        // `get-url` applies the person's `url.<base>.insteadOf` settings.
        let url = read(&["remote", "get-url", name])?;
        Ok(whole_address(folder, url.stdout_text().trim()))
    };
    let chosen = match names.as_slice() {
        [] => {
            return Err(NoAddress::Said(
                "That clone has no remote, so the app can't fetch from it or push to it. Enter the repo's address instead.".to_owned(),
            ));
        }
        [only] => only.clone(),
        many if many.iter().any(|name| name == "origin") => "origin".to_owned(),
        many => {
            let mut remotes = Vec::new();
            for name in many {
                let address = address_of(name)?;
                let refused = RepoAddress::parse(&address).err();
                remotes.push(RepoRemote {
                    name: name.clone(),
                    // Never shown when refused: it may hold a token.
                    address: if refused.is_some() {
                        String::new()
                    } else {
                        address
                    },
                    refused,
                });
            }
            return Ok(FromFolder::ChooseRemote(remotes));
        }
    };
    let address = address_of(&chosen)?;
    match RepoAddress::parse(&address) {
        Ok(_) => Ok(FromFolder::Address(address)),
        Err(why) => Err(NoAddress::Said(format!(
            "The clone's remote {chosen}: {why}"
        ))),
    }
}

/// A remote's address as git uses it from `folder`: a relative folder path,
/// such as `../library.git`, made whole.
fn whole_address(folder: &Path, url: &str) -> String {
    if RepoAddress::parse(url).is_err() && folder.join(url).exists() {
        std::path::absolute(folder.join(url))
            .unwrap_or_else(|_| folder.join(url))
            .display()
            .to_string()
    } else {
        url.to_owned()
    }
}

/// Why git could not read `folder`, from what it said, in plain words.
fn cannot_read(folder: &Path, said: &str) -> String {
    let lower = said.to_lowercase();
    if lower.contains("not a git repository") {
        format!(
            "{} is not in a git repo. Choose the folder of your clone of the library, or enter the repo's address.",
            folder.display()
        )
    } else if lower.contains("dubious ownership") {
        format!(
            "git does not trust the owner of {}, so it won't read it. Run the git config --global --add safe.directory command git suggests, then try again.",
            folder.display()
        )
    } else {
        let first = said
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it gave no reason");
        format!(
            "git could not read {}: {}",
            folder.display(),
            first.trim_start_matches("fatal: ")
        )
    }
}

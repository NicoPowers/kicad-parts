//! Finding a library repo's address from a folder the person chose: their
//! own clone of the repo, or a bare repo such as one on a network share.
//!
//! The app only reads the folder, and never writes in it (mx-70700f): git
//! runs with `GIT_OPTIONAL_LOCKS=0`, so not even git's index is refreshed.
//! The app then works in a copy of its own, made from the address.

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
/// repo, read without writing anything there.
pub(crate) fn address_in(git: &Git, folder: &Path) -> Result<FromFolder, NoAddress> {
    let git = git.clone().with_env("GIT_OPTIONAL_LOCKS", "0");
    let read = |args: &[&str]| git.run(Some(folder), args, READ_LIMIT);
    let bare = read(&["rev-parse", "--is-bare-repository"])?;
    if !bare.succeeded {
        let why = bare.stderr.to_lowercase();
        return Err(NoAddress::Said(if why.contains("not a git repository") {
            format!(
                "{} is not in a git repo. Choose the folder of your clone of the library, or enter the repo's address.",
                folder.display()
            )
        } else {
            format!(
                "git could not read {}: {}",
                folder.display(),
                bare.first_problem_line().trim_start_matches("fatal: ")
            )
        }));
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
        Ok(url.stdout_text().trim().to_owned())
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
                remotes.push(RepoRemote {
                    name: name.clone(),
                    address: address_of(name)?,
                });
            }
            return Ok(FromFolder::ChooseRemote(remotes));
        }
    };
    Ok(FromFolder::Address(address_of(&chosen)?))
}

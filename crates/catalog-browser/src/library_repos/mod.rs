//! Each team's library repo: the git repo, on any git host, that holds the
//! team's own KiCad library (kicad-parts-ab30, ADR 0021).
//!
//! The person links a team to its repo from a folder with their own clone
//! or from the repo's address. The app reaches the repo with the person's
//! own git and SSH setup ([`crate::git_program`]), so it never stores or
//! sees a key. It keeps a copy of its own in the local data folder, one per
//! repo, shared by every team linked to it ([`RepoAddress::key`]). Reading
//! works offline from that copy; fetching waits until the host answers.
//!
//! Linking checks the repo can be read. In the background, the app then
//! makes or updates its copy and tries a push, and each team's row says
//! what it can do: "Can read and write.", or which step failed and what to
//! check. Every linked repo is checked again at start and on "Check
//! again".
//!
//! Features that read or write a team's library get its [`LibraryRepo`]
//! from [`LibraryRepos::repo_for`].

mod address;
mod checkout;
mod problems;
mod repo;

pub use address::RepoAddress;
pub use repo::{CanWrite, LibraryRepo, Limits};

use crate::git_program::Git;
use crate::teams::Teams;
use checkout::{FromFolder, NoAddress};
use desktop_contract::{LibraryRepoStatus, RepoAccess, RepoProblem, RepoRemote, TeamsStatus};
use problems::{Step, git_not_installed};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

/// What linking a team to a library repo did.
#[derive(Debug, Eq, PartialEq)]
pub enum Linked {
    /// Linked; the copy and the write check run in the background.
    Done,
    /// The folder's clone has several remotes and none is `origin`.
    ChooseRemote(Vec<RepoRemote>),
}

pub struct LibraryRepos {
    shared: Arc<Shared>,
}

struct Shared {
    /// `library-repos` in the app's local data folder: one copy per repo.
    folder: PathBuf,
    git: Git,
    limits: Limits,
    teams: Arc<Teams>,
    /// What the app knows about each repo in this run, by key.
    known: Mutex<HashMap<String, Known>>,
    /// One lock per repo, by key, held while its copy changes, so a check
    /// and a deletion of the same copy never overlap.
    disk: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

#[derive(Clone, Debug, Default)]
struct Known {
    /// A check is running: `Some(true)` while it makes the first copy.
    running: Option<bool>,
    /// What the last check found.
    access: Option<RepoAccess>,
}

impl LibraryRepos {
    /// Starts with the person's own git. Copies in `folder` that no team
    /// links any more, such as one left by a team removed in an earlier
    /// run, are deleted, and every linked repo is checked in the
    /// background.
    pub fn start(folder: PathBuf, teams: Arc<Teams>) -> Self {
        Self::start_with(folder, teams, Git::system(), Limits::default())
    }

    pub(crate) fn start_with(folder: PathBuf, teams: Arc<Teams>, git: Git, limits: Limits) -> Self {
        let repos = Self {
            shared: Arc::new(Shared {
                folder,
                git,
                limits,
                teams,
                known: Mutex::new(HashMap::new()),
                disk: Mutex::new(HashMap::new()),
            }),
        };
        repos.delete_copies_no_team_links();
        for address in repos.linked_addresses() {
            if let Ok(address) = RepoAddress::parse(&address) {
                repos.check_in_background(address, false);
            }
        }
        repos
    }

    /// Links the team at `server_url` to the repo at `place`: a folder with
    /// the person's clone of it (or a bare repo), or its address. The repo
    /// must be readable; then the app copies it and tries a push in the
    /// background.
    pub fn link(&self, server_url: &str, place: &str) -> Result<Linked, RepoProblem> {
        let refused = |summary: String| RepoProblem {
            summary,
            details: String::new(),
            offline: false,
        };
        self.shared.teams.team(server_url).map_err(refused)?;
        let address = match self.address_for(place)? {
            Ok(address) => address,
            Err(remotes) => return Ok(Linked::ChooseRemote(remotes)),
        };
        self.repo(&address).can_read()?;
        self.shared
            .teams
            .set_library_repo(server_url, Some(address.url()))
            .map_err(refused)?;
        self.check_in_background(address, true);
        Ok(Linked::Done)
    }

    /// Checks the repo of the team at `server_url` again: reads it, brings
    /// the app's copy up to date and tries a push, in the background.
    pub fn check_again(&self, server_url: &str) {
        if let Some(address) = self.address_of(server_url) {
            if let Ok(address) = RepoAddress::parse(&address) {
                self.check_in_background(address, false);
            }
        }
    }

    /// Unlinks the team at `server_url` from its repo. The app's copy is
    /// deleted unless another team links the same repo.
    pub fn unlink(&self, server_url: &str) -> Result<(), String> {
        self.shared.teams.set_library_repo(server_url, None)?;
        self.delete_copies_no_team_links();
        Ok(())
    }

    /// The library repo of the team at `server_url`, if it has one: what
    /// scanning and writing a team's library work with.
    pub fn repo_for(&self, server_url: &str) -> Option<LibraryRepo> {
        let address = RepoAddress::parse(&self.address_of(server_url)?).ok()?;
        Some(self.repo(&address))
    }

    /// Fills in each team's library repo in `status`. A copy no team links
    /// any more, such as a removed team's, is deleted.
    pub fn note_status(&self, status: &mut TeamsStatus) {
        let linked: HashMap<String, String> =
            self.shared.teams.library_repos().into_iter().collect();
        let known = self.known();
        for team in &mut status.teams {
            team.library_repo = linked
                .get(&team.server_url)
                .map(|address| self.status_of(address, &known));
        }
        drop(known);
        if self.has_copies_no_team_links() {
            self.delete_copies_no_team_links();
        }
    }

    /// Waits until no check is running, for tests.
    #[cfg(test)]
    pub(crate) fn wait_until_idle(&self) {
        let started = std::time::Instant::now();
        while self.known().values().any(|known| known.running.is_some()) {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(120),
                "a library repo check never ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The address `place` names: an address as it is, or the address of
    /// the repo in a folder. `Err` holds the remotes to choose from.
    fn address_for(
        &self,
        place: &str,
    ) -> Result<Result<RepoAddress, Vec<RepoRemote>>, RepoProblem> {
        let refused = |summary: String| RepoProblem {
            summary,
            details: String::new(),
            offline: false,
        };
        let place = place.trim();
        let parsed = RepoAddress::parse(place);
        let folder = Path::new(place);
        // A URL, such as `ssh://` or `file://`, is an address; so is
        // `host:path`. Anything else must be a folder.
        let is_address = parsed
            .as_ref()
            .is_ok_and(|address| address.host() != "this computer" || place.contains("://"));
        if is_address || !folder.is_dir() {
            return match parsed {
                Ok(address) if is_address => Ok(Ok(address)),
                Ok(_) => Err(refused(format!(
                    "There is no folder {place}. Check the path, or enter the repo's address."
                ))),
                Err(why) => Err(refused(why)),
            };
        }
        match checkout::address_in(&self.shared.git, folder) {
            Ok(FromFolder::Address(url)) => RepoAddress::parse(&url).map(Ok).map_err(refused),
            Ok(FromFolder::ChooseRemote(remotes)) => Ok(Err(remotes)),
            Err(NoAddress::GitNotInstalled) => Err(git_not_installed(Step::Read)),
            Err(NoAddress::Said(why)) => Err(refused(why)),
        }
    }

    fn repo(&self, address: &RepoAddress) -> LibraryRepo {
        LibraryRepo::new(
            self.shared.git.clone(),
            &self.shared.folder,
            address.clone(),
            self.shared.limits,
        )
    }

    fn address_of(&self, server_url: &str) -> Option<String> {
        self.shared
            .teams
            .library_repos()
            .into_iter()
            .find(|(server, _)| server == server_url)
            .map(|(_, address)| address)
    }

    fn linked_addresses(&self) -> Vec<String> {
        self.shared.linked_addresses()
    }

    /// The row's view of the repo at `address`.
    fn status_of(&self, address: &str, known: &HashMap<String, Known>) -> LibraryRepoStatus {
        let parsed = RepoAddress::parse(address);
        let Ok(parsed) = parsed else {
            // Only a hand-edited list of teams holds such an address.
            return LibraryRepoStatus {
                address: address.to_owned(),
                name: address.to_owned(),
                host: String::new(),
                access: RepoAccess::CannotRead {
                    problem: RepoProblem {
                        summary: format!("Can't read the repo: {}", parsed.unwrap_err()),
                        details: String::new(),
                        offline: false,
                    },
                },
                copied_at: None,
            };
        };
        let repo = self.repo(&parsed);
        let state = known.get(&parsed.key()).cloned().unwrap_or_default();
        let access = match (state.running, state.access) {
            (Some(copying), _) => RepoAccess::Checking { copying },
            (None, Some(access)) => access,
            // Checked at start; until then, as if checking.
            (None, None) => RepoAccess::Checking { copying: false },
        };
        LibraryRepoStatus {
            address: address.to_owned(),
            name: parsed.name(),
            host: parsed.host(),
            access,
            copied_at: repo
                .copied_at()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_millis() as u64),
        }
    }

    /// Checks the repo at `address` on a thread of its own, unless a check
    /// of it is already running: reads it (unless `already_read`), makes
    /// or updates the app's copy, then tries a push.
    fn check_in_background(&self, address: RepoAddress, already_read: bool) {
        let key = address.key();
        let repo = self.repo(&address);
        {
            let mut known = self.known();
            let entry = known.entry(key.clone()).or_default();
            if entry.running.is_some() {
                return;
            }
            entry.running = Some(!repo.is_copied());
        }
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let lock = shared.disk_lock(&address.folder_name());
            let access = {
                let _held = lock.lock().unwrap_or_else(|error| error.into_inner());
                check(&repo, already_read)
            };
            {
                let mut known = shared.known();
                let entry = known.entry(key).or_default();
                entry.running = None;
                entry.access = Some(access);
            }
            // Unlinked while it was checked: its copy goes now.
            shared.delete_copies_no_team_links();
        });
    }

    fn has_copies_no_team_links(&self) -> bool {
        self.shared.has_copies_no_team_links()
    }

    fn delete_copies_no_team_links(&self) {
        self.shared.delete_copies_no_team_links();
    }

    fn known(&self) -> MutexGuard<'_, HashMap<String, Known>> {
        self.shared.known()
    }
}

impl Shared {
    fn linked_addresses(&self) -> Vec<String> {
        self.teams
            .library_repos()
            .into_iter()
            .map(|(_, address)| address)
            .collect()
    }

    /// The linked repos' addresses, parsed. Only a hand-edited list of
    /// teams holds one that does not parse.
    fn linked(&self) -> Vec<RepoAddress> {
        self.linked_addresses()
            .iter()
            .filter_map(|address| RepoAddress::parse(address).ok())
            .collect()
    }

    /// The folder names of the copies teams link.
    fn wanted_folders(&self) -> BTreeSet<String> {
        self.linked().iter().map(RepoAddress::folder_name).collect()
    }

    /// Whether the folder holds anything but the copies teams link.
    fn has_copies_no_team_links(&self) -> bool {
        let wanted = self.wanted_folders();
        fs::read_dir(&self.folder).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| !wanted.contains(&entry.file_name().to_string_lossy().into_owned()))
        })
    }

    /// Deletes every copy in the folder that no team links, and anything
    /// else in it, such as a copy stopped halfway. A copy whose check is
    /// running is left: the check deletes it when it ends.
    fn delete_copies_no_team_links(&self) {
        if let Ok(entries) = fs::read_dir(&self.folder) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let copy = name.strip_suffix(".partial").unwrap_or(&name).to_owned();
                if self.wanted_folders().contains(&name) {
                    continue;
                }
                let lock = self.disk_lock(&copy);
                let Ok(_held) = lock.try_lock() else {
                    continue;
                };
                // A team may have linked the repo meanwhile.
                if self.wanted_folders().contains(&name) {
                    continue;
                }
                if let Err(error) = repo::remove_folder(&entry.path()) {
                    eprintln!(
                        "kicad-parts-desktop: could not delete the unused library repo copy {} ({error}).",
                        entry.path().display()
                    );
                }
            }
        }
        let linked: BTreeSet<String> = self.linked().iter().map(RepoAddress::key).collect();
        self.known()
            .retain(|key, known| linked.contains(key) || known.running.is_some());
    }

    /// The lock of the copy in the folder named `copy`.
    fn disk_lock(&self, copy: &str) -> Arc<Mutex<()>> {
        self.disk
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(copy.to_owned())
            .or_default()
            .clone()
    }

    fn known(&self) -> MutexGuard<'_, HashMap<String, Known>> {
        self.known.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// One whole check of `repo`: read it (unless `already_read`), make or
/// update the app's copy, then try a push.
fn check(repo: &LibraryRepo, already_read: bool) -> RepoAccess {
    let ready = if already_read {
        Ok(())
    } else {
        repo.can_read()
    }
    .and_then(|()| repo.copy_or_fetch());
    if let Err(problem) = ready {
        return RepoAccess::CannotRead { problem };
    }
    match repo.can_write() {
        Ok(CanWrite::Yes) => RepoAccess::CanReadAndWrite,
        Ok(CanWrite::EmptyRepo) => RepoAccess::CanReadEmpty,
        Err(problem) => RepoAccess::CanReadOnly { problem },
    }
}

#[cfg(test)]
mod tests;

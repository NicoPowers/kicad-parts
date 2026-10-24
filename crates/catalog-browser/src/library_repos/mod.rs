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
//! Linking and "Check again" run the whole check, each for the address
//! its team uses: read the repo, make or update the copy, try a push. Each
//! team's row then says what it can do: "Can read and write.", or which
//! step failed and what to check. The last results are kept in
//! `library-repos.json` beside the copies.
//!
//! At start, the app only fetches each linked repo, two at a time per
//! host, and asks the person nothing: no credential window, no askpass.
//! Rows show the last results meanwhile, and the fetch says whether the
//! repo can still be read.
//!
//! Features that read or write a team's library get its [`LibraryRepo`]
//! from [`LibraryRepos::repo_for`].

mod address;
mod checkout;
mod problems;
mod repo;

pub use address::{HAS_PASSWORD, RepoAddress};
pub use repo::{CanWrite, LibraryRepo, Limits, RepoFile};

use crate::git_program::Git;
use crate::teams::Teams;
use checkout::{FromFolder, NoAddress};
use desktop_contract::{LibraryRepoStatus, RepoAccess, RepoProblem, RepoRemote, TeamsStatus};
use problems::{Step, git_not_installed};
use repo::CopyLocks;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
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

/// How many clones the app follows, remote to remote, to find the repo.
const MAX_CLONE_STEPS: usize = 5;

/// How many repos on one host the app fetches at once at start. More could
/// trip a server's limit on new SSH connections (OpenSSH `MaxStartups`)
/// and wrongly show repos as offline.
const START_FETCHES_PER_HOST: usize = 2;

/// What a place the person entered leads to.
enum Found {
    Address(RepoAddress),
    /// A clone with several remotes and no `origin`.
    ChooseRemote(Vec<RepoRemote>),
}

/// A refusal in plain words, with no git message behind it.
fn refused(summary: String) -> RepoProblem {
    RepoProblem {
        summary,
        details: String::new(),
        offline: false,
    }
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
    /// What the app knows about each linked address, by the address as
    /// linked: two teams may reach one copy by different addresses.
    known: Mutex<HashMap<String, Known>>,
    locks: CopyLocks,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Known {
    /// A check is running. Never saved.
    #[serde(skip)]
    running: Option<Running>,
    /// What the last check found.
    access: Option<RepoAccess>,
    /// What the last push check found: one of the "can read" results. A
    /// fetch at start keeps it, since it tries no push.
    write: Option<RepoAccess>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Running {
    /// The whole check, which the row shows: `copying` while it makes the
    /// first copy.
    Check { copying: bool },
    /// The fetch at start, or waiting for its turn. The row keeps the last
    /// results meanwhile.
    StartFetch,
}

impl LibraryRepos {
    /// Starts with the person's own git. Copies in `folder` that no team
    /// links any more, such as one left by a team removed in an earlier
    /// run, are deleted, and every linked repo is fetched in the
    /// background.
    pub fn start(folder: PathBuf, teams: Arc<Teams>) -> Self {
        Self::start_with(folder, teams, Git::system(), Limits::default())
    }

    pub(crate) fn start_with(folder: PathBuf, teams: Arc<Teams>, git: Git, limits: Limits) -> Self {
        let known = read_results(&folder);
        let repos = Self {
            shared: Arc::new(Shared {
                folder,
                git,
                limits,
                teams,
                known: Mutex::new(known),
                locks: CopyLocks::default(),
            }),
        };
        repos.shared.delete_copies_no_team_links();
        repos.fetch_all_at_start();
        repos
    }

    /// Links the team at `server_url` to the repo at `place`: a folder with
    /// the person's clone of it (or a bare repo), or its address. The repo
    /// must be readable; then the app copies it and tries a push in the
    /// background.
    pub fn link(&self, server_url: &str, place: &str) -> Result<Linked, RepoProblem> {
        self.shared.teams.team(server_url).map_err(refused)?;
        let address = match self.address_for(place)? {
            Found::Address(address) => address,
            Found::ChooseRemote(remotes) => return Ok(Linked::ChooseRemote(remotes)),
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
        self.shared.delete_copies_no_team_links();
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
        let known = self.shared.known();
        for team in &mut status.teams {
            team.library_repo = linked
                .get(&team.server_url)
                .map(|address| self.status_of(address, &known));
        }
        drop(known);
        if self.shared.has_copies_no_team_links() {
            self.shared.delete_copies_no_team_links();
        }
    }

    /// Waits until no check is running, for tests.
    #[cfg(test)]
    pub(crate) fn wait_until_idle(&self) {
        let started = std::time::Instant::now();
        while self
            .shared
            .known()
            .values()
            .any(|known| known.running.is_some())
        {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(120),
                "a library repo check never ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The address `place` names: a host's address as it is, or, for a
    /// folder (a path or a `file://` URL), the repo the folder leads to.
    fn address_for(&self, place: &str) -> Result<Found, RepoProblem> {
        let address = RepoAddress::parse(place).map_err(refused)?;
        if address.folder_path().is_some_and(|folder| !folder.is_dir()) {
            return Err(refused(format!(
                "There is no folder {}. Check the path, or enter the repo's address.",
                place.trim()
            )));
        }
        self.follow(address, 0)
    }

    /// The repo `address` leads to. A bare repo in a folder is the repo. A
    /// clone in a folder is someone's work, never written by the app
    /// (mx-70700f), so its remote is followed instead, even when the remote
    /// is another clone.
    fn follow(&self, address: RepoAddress, steps: usize) -> Result<Found, RepoProblem> {
        let Some(folder) = address.folder_path().filter(|folder| folder.is_dir()) else {
            // A host, or a folder that is not there: reading it says so.
            return Ok(Found::Address(address));
        };
        if steps >= MAX_CLONE_STEPS {
            return Err(refused(format!(
                "{} leads from clone to clone more than {MAX_CLONE_STEPS} times. Enter the repo's address instead.",
                address.url()
            )));
        }
        match checkout::address_in(&self.shared.git, &folder) {
            Ok(FromFolder::Address(url)) => {
                let next = RepoAddress::parse(&url).map_err(refused)?;
                // A bare repo gives its own folder back.
                if next.folder_path().as_deref() == Some(folder.as_path()) {
                    Ok(Found::Address(address))
                } else {
                    self.follow(next, steps + 1)
                }
            }
            Ok(FromFolder::ChooseRemote(remotes)) => Ok(Found::ChooseRemote(remotes)),
            Err(NoAddress::GitNotInstalled) => Err(git_not_installed(Step::Read)),
            Err(NoAddress::Said(why)) => Err(refused(why)),
        }
    }

    fn repo(&self, address: &RepoAddress) -> LibraryRepo {
        self.shared.repo(address)
    }

    fn address_of(&self, server_url: &str) -> Option<String> {
        self.shared
            .teams
            .library_repos()
            .into_iter()
            .find(|(server, _)| server == server_url)
            .map(|(_, address)| address)
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
                    problem: refused(format!("Can't read the repo: {}", parsed.unwrap_err())),
                },
                copied_at: None,
            };
        };
        let state = known.get(address).cloned().unwrap_or_default();
        let access = match (state.running, state.access) {
            (Some(Running::Check { copying }), _) => RepoAccess::Checking { copying },
            (_, Some(access)) => access,
            (_, None) => RepoAccess::Checking { copying: false },
        };
        LibraryRepoStatus {
            address: address.to_owned(),
            name: parsed.name(),
            host: parsed.host(),
            access,
            copied_at: self
                .repo(&parsed)
                .copied_at()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_millis() as u64),
        }
    }

    /// Checks the repo at `address` on a thread of its own, unless a check
    /// of that address is already running: reads it (unless
    /// `already_read`), makes or updates the app's copy, then tries a push.
    /// A check for another address of the same copy waits for the copy's
    /// lock, then runs with its own address.
    fn check_in_background(&self, address: RepoAddress, already_read: bool) {
        let repo = self.repo(&address);
        let running = Running::Check {
            copying: !repo.is_copied(),
        };
        if !self.shared.begin(address.url(), running) {
            return;
        }
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let access = check(&repo, already_read);
            let write = matches!(
                access,
                RepoAccess::CanReadAndWrite
                    | RepoAccess::CanReadEmpty
                    | RepoAccess::CanReadOnly { .. }
            )
            .then(|| access.clone());
            shared.finish(address.url(), access, write);
        });
    }

    /// Fetches every linked repo once, asking the person nothing, at most
    /// [`START_FETCHES_PER_HOST`] at a time per host.
    fn fetch_all_at_start(&self) {
        let mut by_host: BTreeMap<String, VecDeque<RepoAddress>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for address in self.shared.linked() {
            if seen.insert(address.url().to_owned()) {
                // Marked before any fetch starts, so no row says
                // "Checking" meanwhile, and tests can wait for them.
                self.shared.mark_waiting(address.url());
                by_host
                    .entry(address.host())
                    .or_default()
                    .push_back(address);
            }
        }
        for queue in by_host.into_values() {
            let queue = Arc::new(Mutex::new(queue));
            for _ in 0..START_FETCHES_PER_HOST {
                let (shared, queue) = (self.shared.clone(), queue.clone());
                std::thread::spawn(move || {
                    loop {
                        let next = queue
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .pop_front();
                        let Some(address) = next else { break };
                        shared.fetch_at_start(&address);
                    }
                });
            }
        }
    }
}

impl Shared {
    fn repo(&self, address: &RepoAddress) -> LibraryRepo {
        LibraryRepo::new(
            self.git.clone(),
            &self.folder,
            address.clone(),
            self.limits,
            &self.locks,
        )
    }

    /// Marks a whole check of `url` as running, unless one already is. It
    /// takes over from a start fetch.
    fn begin(&self, url: &str, running: Running) -> bool {
        let mut known = self.known();
        let entry = known.entry(url.to_owned()).or_default();
        if matches!(entry.running, Some(Running::Check { .. })) {
            return false;
        }
        entry.running = Some(running);
        true
    }

    /// A start fetch waiting its turn counts as running.
    fn mark_waiting(&self, url: &str) {
        let mut known = self.known();
        let entry = known.entry(url.to_owned()).or_default();
        if entry.running.is_none() {
            entry.running = Some(Running::StartFetch);
        }
    }

    fn running(&self, url: &str) -> Option<Running> {
        self.known().get(url).and_then(|known| known.running)
    }

    /// Records what a check of `url` found, saves every result, and
    /// deletes a copy unlinked while it was checked.
    fn finish(&self, url: &str, access: RepoAccess, write: Option<RepoAccess>) {
        {
            let mut known = self.known();
            let entry = known.entry(url.to_owned()).or_default();
            entry.running = None;
            entry.access = Some(access);
            if write.is_some() {
                entry.write = write;
            }
            write_results(&self.folder, &known);
        }
        self.delete_copies_no_team_links();
    }

    /// The start fetch of `address`: only makes or updates the copy. It
    /// tries no push, so the last push check's result stands. A whole check
    /// the person asked for meanwhile takes over.
    fn fetch_at_start(&self, address: &RepoAddress) {
        let url = address.url();
        if self.running(url) != Some(Running::StartFetch) {
            return;
        }
        let fetched = self.repo(address).asking_nothing().copy_or_fetch();
        if self.running(url) != Some(Running::StartFetch) {
            return;
        }
        let last_write = self.known().get(url).and_then(|known| known.write.clone());
        let access = match fetched {
            Err(problem) => RepoAccess::CannotRead { problem },
            Ok(()) => last_write.unwrap_or(RepoAccess::CanRead),
        };
        self.finish(url, access, None);
    }

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
    /// else in it, such as a copy stopped halfway. A copy a step is
    /// changing is left: its check deletes it when it ends.
    fn delete_copies_no_team_links(&self) {
        if let Ok(entries) = fs::read_dir(&self.folder) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if self.wanted_folders().contains(&name) {
                    continue;
                }
                if let Err(error) = self.locks.delete_if_idle(&entry.path(), &name) {
                    eprintln!(
                        "kicad-parts-desktop: could not delete the unused library repo copy {} ({error}).",
                        entry.path().display()
                    );
                }
            }
        }
        let linked: BTreeSet<String> = self.linked_addresses().into_iter().collect();
        self.known()
            .retain(|url, known| linked.contains(url) || known.running.is_some());
    }

    fn known(&self) -> MutexGuard<'_, HashMap<String, Known>> {
        self.known.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// The file beside the copies that keeps each address's last results, so
/// a row says what it said before the app restarted. It holds only those
/// results: no password, no key.
fn results_file(folder: &Path) -> PathBuf {
    folder.with_extension("json")
}

/// The last results kept in the results file; none if it cannot be read.
fn read_results(folder: &Path) -> HashMap<String, Known> {
    fs::read(results_file(folder))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Saves every address's last results, through a temporary file, so a
/// crash never leaves half of them. A failure is only logged: the next
/// check finds the same results again.
fn write_results(folder: &Path, known: &HashMap<String, Known>) {
    let file = results_file(folder);
    let saved: BTreeMap<&String, &Known> = known
        .iter()
        .filter(|(_, known)| known.access.is_some())
        .collect();
    let written = serde_json::to_vec_pretty(&saved)
        .map_err(|error| error.to_string())
        .and_then(|json| {
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let temp = file.with_extension("json.tmp");
            fs::write(&temp, json).map_err(|error| error.to_string())?;
            fs::rename(&temp, &file).map_err(|error| error.to_string())
        });
    if let Err(problem) = written {
        eprintln!(
            "kicad-parts-desktop: could not save the library repo results in {} ({problem}).",
            file.display()
        );
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

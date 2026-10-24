//! The teams the person works with. Each team is its own team server, with
//! its own [`TeamCatalog`] and session, side by side: signing in to one
//! team never signs out of another.
//!
//! The app remembers the list (each server address, email, code and
//! colour, see [`saved`]) and what the person last chose in the part list.
//! The part list shows every team's parts at once, through
//! [`Teams::known_parts`]. Every request about one team's parts names its
//! team by server address ([`Teams::team`]), so no team is "the shown
//! one". Anything that works on every team, such as keeping each team's
//! KiCad libraries up to date, uses [`Teams::signed_in`].
//!
//! Each team's sign-in is kept between runs in a [`SignInStore`] (the
//! operating system's credential store in the app), and each team's
//! [`keeper`] carries it on at start and renews it while in use.
//!
//! Each team's last-known parts are kept between runs too, in a file next
//! to the list ([`kept_parts`]), written by the team's [`parts_writer`]. A
//! team that has a kept sign-in shows them at start, marked offline, until
//! its first catch-up replaces them. Signing out and removing the team
//! delete them. A team asked to sign in again shows none, but keeps the
//! file, so signing in again shows its parts at once.

mod colours;
mod keeper;
mod kept_parts;
mod parts_writer;
mod saved;
mod sign_ins;

pub use sign_ins::{MemorySignIns, SignInStore, SignIns};

use crate::team::{KnownParts, TeamCatalog};
use collaboration::ServerAddress;
use desktop_contract::{
    SignInRequest, SignInsKept, TeamChoices, TeamSession, TeamsStatus, team_label,
};
use keeper::{Keeper, KeeperTask};
use parts_writer::{PartsWriter, WriterTask};
use saved::{SavedList, SavedTeam};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

pub struct Teams {
    list_file: PathBuf,
    /// The folder that keeps each team's last-known parts, next to the
    /// list file.
    parts_folder: PathBuf,
    sign_ins: SignIns,
    /// Whether sign-ins outlive the app. Turns to "until closed" if the
    /// store fails.
    kept: Arc<Mutex<SignInsKept>>,
    /// From launch options: they fill in the sign-in form.
    suggested: SavedTeam,
    state: Mutex<TeamList>,
    /// Sign-ins and removals run one at a time, so two sign-ins to the same
    /// new server cannot both add it, and adding a team back waits until
    /// its removal has forgotten the old sign-in.
    one_at_a_time: Mutex<()>,
}

/// One team in the list.
struct Team {
    saved: SavedTeam,
    /// Kept for as long as the team is in the list, so whatever follows the
    /// team (such as its KiCad libraries) keeps following the same one.
    catalog: Arc<TeamCatalog>,
    keeper: Arc<Keeper>,
    /// Keeps the team's last-known parts on disk.
    writer: Arc<PartsWriter>,
    /// Something the person should know about this team, such as a
    /// sign-in the store could not forget.
    note: Option<String>,
}

struct TeamList {
    teams: Vec<Team>,
    /// What the person last chose in the part list, and the team each
    /// feature that works on one team at a time last used.
    choices: TeamChoices,
    /// Why the last sign-in failed, for the sign-in form.
    problem: Option<String>,
    /// Something to say once in the Teams menu, such as a removed team's
    /// sign-in the store could not forget. The next action clears it.
    note: Option<String>,
}

impl TeamList {
    fn find(&self, server_url: &str) -> Option<&Team> {
        self.teams
            .iter()
            .find(|team| team.saved.server_url == server_url)
    }

    fn saved(&self) -> SavedList {
        SavedList {
            teams: self.teams.iter().map(|team| team.saved.clone()).collect(),
            choices: self.choices.clone(),
        }
    }

    /// `choices` with only teams in the list: a team taken off the list is
    /// no longer chosen anywhere, and the part list shows every team.
    fn only_listed(&self, mut choices: TeamChoices) -> TeamChoices {
        if self.find(&choices.filter).is_none() {
            choices.filter.clear();
        }
        choices
            .last_used
            .retain(|_, server| self.find(server).is_some());
        choices
    }
}

impl Teams {
    /// Reads the list of teams from `list_file`, and carries on, in the
    /// background, each team's sign-in kept in `sign_ins`. A suggested
    /// server or email (from launch options) fills in the sign-in form.
    pub fn open(
        list_file: PathBuf,
        sign_ins: SignIns,
        suggested_server: Option<String>,
        suggested_email: Option<String>,
    ) -> Self {
        let mut saved = saved::read(&list_file);
        // Teams saved before colours were (or with a colour edited away)
        // get theirs now, once, in list order.
        let mut coloured = false;
        for at in 0..saved.teams.len() {
            if !colours::is_known(&saved.teams[at].colour) {
                let taken: Vec<String> = saved.teams[..at]
                    .iter()
                    .map(|team| team.colour.clone())
                    .collect();
                saved.teams[at].colour = colours::next_colour(taken.iter().map(String::as_str));
                coloured = true;
            }
        }
        if coloured {
            let _ = saved::write(&list_file, &saved);
        }
        let kept = Arc::new(Mutex::new(sign_ins.kept.clone()));
        let parts_folder = parts_folder_of(&list_file);
        let teams: Vec<Team> = saved
            .teams
            .into_iter()
            .map(|saved| {
                let catalog = Arc::new(TeamCatalog::new());
                // A team the person signed out of is never carried on, even
                // if the store could not forget its sign-in.
                let look_for_kept = !saved.signed_out;
                if look_for_kept {
                    // Shown while its kept sign-in is carried on. The
                    // keeper stops showing them if there is none.
                    if let Some(parts) = kept_parts::read(&parts_folder, &saved.server_url) {
                        catalog.show_kept(parts);
                    }
                }
                let keeper = keeper_for(&saved, &catalog, &sign_ins, &kept, look_for_kept);
                let writer = writer_for(&parts_folder, &saved, &catalog);
                Team {
                    saved,
                    catalog,
                    keeper,
                    writer,
                    note: None,
                }
            })
            .collect();
        Self {
            list_file,
            parts_folder,
            sign_ins,
            kept,
            suggested: SavedTeam {
                server_url: suggested_server.unwrap_or_default(),
                email: suggested_email.unwrap_or_default(),
                ..SavedTeam::default()
            },
            state: Mutex::new(TeamList {
                teams,
                choices: saved.choices,
                problem: None,
                note: None,
            }),
            one_at_a_time: Mutex::new(()),
        }
    }

    /// Signs in to the team at the request's server, adding it to the list
    /// if it is new, with a colour of its own. Other teams stay signed in. Its keeper
    /// then keeps the sign-in for the next run. A failed sign-in adds
    /// nothing; its problem, in plain words, is the error.
    pub fn sign_in(&self, request: SignInRequest) -> Result<(), String> {
        let _one_at_a_time = self.one_at_a_time();
        self.lock().note = None;
        let server_url = match ServerAddress::for_sign_in(&request.server_url) {
            Ok(address) => address.url(),
            Err(refusal) => return self.failed(refusal.to_string()),
        };
        let known = self
            .lock()
            .find(&server_url)
            .map(|team| (team.catalog.clone(), team.keeper.clone()));
        let catalog = match &known {
            Some((catalog, _)) => catalog.clone(),
            None => Arc::new(TeamCatalog::new()),
        };
        // A failure leaves the team as it was: a running session keeps
        // running, and a kept sign-in still being carried on stays, since
        // only the password was wrong.
        if let Err(problem) = catalog.try_sign_in(SignInRequest {
            server_url: server_url.clone(),
            ..request
        }) {
            return self.failed(problem);
        }
        let TeamSession::SignedIn { email, .. } = catalog.status().session else {
            return self.failed("The sign-in ended at once. Try again.".into());
        };
        if let Some((_, keeper)) = &known {
            // The password replaces any kept sign-in still being carried on.
            keeper.signed_in();
        }
        // Parts kept from before show at once, until the catch-up ends.
        if let Some(parts) = kept_parts::read(&self.parts_folder, &server_url) {
            catalog.show_kept(parts);
        }
        let mut list = self.lock();
        // A team keeps its colour; a new one gets the next free colour.
        let colour = match list.find(&server_url) {
            Some(team) => team.saved.colour.clone(),
            None => colours::next_colour(list.teams.iter().map(|team| team.saved.colour.as_str())),
        };
        // The code is read from the team's settings after signing in, so a
        // code known from before is kept until then: the team keeps its
        // name in the meantime.
        let code = Some(catalog.code())
            .filter(|code| !code.is_empty())
            .or_else(|| list.find(&server_url).map(|team| team.saved.code.clone()))
            .unwrap_or_default();
        let saved = SavedTeam {
            server_url: server_url.clone(),
            email,
            code,
            colour,
            signed_out: false,
        };
        list.problem = None;
        match list
            .teams
            .iter_mut()
            .find(|team| team.saved.server_url == server_url)
        {
            Some(team) => {
                team.saved = saved;
                team.note = None;
            }
            None => {
                let keeper = keeper_for(&saved, &catalog, &self.sign_ins, &self.kept, false);
                let writer = writer_for(&self.parts_folder, &saved, &catalog);
                list.teams.push(Team {
                    saved,
                    catalog,
                    keeper,
                    writer,
                    note: None,
                });
            }
        }
        self.save(&list);
        Ok(())
    }

    /// Signs out of one team and forgets its kept sign-in. It stays in the
    /// list, marked signed out, so signing in again needs only the
    /// password, and the next start never carries on a sign-in the store
    /// could not forget. Its kept parts are deleted.
    pub fn sign_out(&self, server_url: &str) {
        let (keeper, writer) = {
            let mut list = self.lock();
            list.problem = None;
            list.note = None;
            let Some(team) = list
                .teams
                .iter_mut()
                .find(|team| team.saved.server_url == server_url)
            else {
                return;
            };
            team.saved.signed_out = true;
            team.note = None;
            let keeper = team.keeper.clone();
            let writer = team.writer.clone();
            self.save(&list);
            (keeper, writer)
        };
        let forgotten = keeper.sign_out();
        // Once the session has ended, so no later write brings them back.
        delete_kept_parts(&writer, server_url);
        if let Err(problem) = forgotten {
            let mut list = self.lock();
            if let Some(team) = list
                .teams
                .iter_mut()
                .find(|team| team.saved.server_url == server_url)
            {
                team.note = Some(format!(
                    "Your sign-in for this team could not be removed from the system's password store ({problem}). The app will not use it again. You can delete it there yourself."
                ));
            }
        }
    }

    /// Signs out of one team, forgets its kept sign-in, deletes its kept
    /// parts, and takes it off the list.
    pub fn remove(&self, server_url: &str) {
        // The sign-in is forgotten while the team is still in the list,
        // and no sign-in runs meanwhile. So adding the team back, even
        // while the store is slow (such as a keyring asking to be
        // unlocked), waits until the old sign-in is gone: the old one can
        // never be forgotten in place of the new one.
        let _one_at_a_time = self.one_at_a_time();
        let Some((keeper, writer)) = self
            .lock()
            .find(server_url)
            .map(|team| (team.keeper.clone(), team.writer.clone()))
        else {
            return;
        };
        // Outside the list's lock, so status polling never waits.
        let forgotten = keeper.sign_out();
        delete_kept_parts(&writer, server_url);
        let mut list = self.lock();
        list.teams
            .retain(|team| team.saved.server_url != server_url);
        list.choices = list.only_listed(list.choices.clone());
        // A sign-in the store could not forget is never used: the team is
        // off the list. The person is told once, to delete it themselves.
        list.note = forgotten.err().map(|problem| {
            format!(
                "Your sign-in for {server_url} could not be removed from the system's password store ({problem}). The app will not use it. You can delete it there yourself."
            )
        });
        self.save(&list);
    }

    /// Remembers what the person chose in the part list and in features
    /// that work on one team at a time, for the next start. A team not in
    /// the list is not kept, so the part list then shows every team.
    pub fn choose(&self, choices: TeamChoices) {
        let mut list = self.lock();
        list.choices = list.only_listed(choices);
        self.save(&list);
    }

    /// The team at `server_url`, signed in or not. Every request about a
    /// team's parts names its team this way, by server address, so it
    /// always reaches the team the person meant.
    pub fn team(&self, server_url: &str) -> Result<Arc<TeamCatalog>, String> {
        self.lock()
            .find(server_url)
            .map(|team| team.catalog.clone())
            .ok_or_else(|| format!("The team at {server_url} is no longer in your teams."))
    }

    /// Every team in the list, signed in or not, by server address, in
    /// list order.
    pub fn every(&self) -> Vec<(String, Arc<TeamCatalog>)> {
        self.lock()
            .teams
            .iter()
            .map(|team| (team.saved.server_url.clone(), team.catalog.clone()))
            .collect()
    }

    /// Every team's last-known parts, in list order: what the part list
    /// shows. A team with none to show, such as one signed out or asked to
    /// sign in again, is left out.
    pub fn known_parts(&self) -> Vec<Arc<KnownParts>> {
        let teams: Vec<(String, String, Arc<TeamCatalog>)> = self
            .lock()
            .teams
            .iter()
            .map(|team| {
                let code = Some(team.saved.code.clone())
                    .filter(|code| !code.is_empty())
                    .unwrap_or_else(|| team.catalog.code());
                (
                    team.saved.server_url.clone(),
                    team_label(&code, &team.saved.server_url),
                    team.catalog.clone(),
                )
            })
            .collect();
        // Outside the list's lock: making a team's parts may read its
        // category names from its server.
        teams
            .into_iter()
            .filter_map(|(server_url, label, catalog)| {
                catalog.last_known_parts(&server_url, &label)
            })
            .collect()
    }

    /// Every team that is signed in, by server address, in list order.
    pub fn signed_in(&self) -> Vec<(String, Arc<TeamCatalog>)> {
        self.lock()
            .teams
            .iter()
            .filter(|team| team.catalog.is_signed_in())
            .map(|team| (team.saved.server_url.clone(), team.catalog.clone()))
            .collect()
    }

    /// Every team in the list and how it is doing, what the person last
    /// chose, and what the sign-in form starts with.
    pub fn status(&self) -> TeamsStatus {
        let mut list = self.lock();
        let mut learned_a_code = false;
        let teams = list
            .teams
            .iter_mut()
            .map(|team| {
                let mut status = team.catalog.status();
                if !status.code.is_empty() && status.code != team.saved.code {
                    // Saved, so the team has its name at the next start,
                    // even before its server is reached.
                    team.saved.code.clone_from(&status.code);
                    learned_a_code = true;
                }
                status.code.clone_from(&team.saved.code);
                status.colour.clone_from(&team.saved.colour);
                status.note.clone_from(&team.note);
                status.server_url.clone_from(&team.saved.server_url);
                status.email.clone_from(&team.saved.email);
                if !team.catalog.is_signed_in() {
                    if let Some(problem) = team.keeper.reaching() {
                        status.session = TeamSession::Reaching { problem };
                    }
                }
                status
            })
            .collect();
        if learned_a_code {
            self.save(&list);
        }
        // The form offers the first team that needs signing in.
        let form = list
            .teams
            .iter()
            .find(|team| !team.catalog.is_signed_in())
            .map(|team| team.saved.clone())
            .unwrap_or_default();
        let pick = |suggested: &str, saved: String| {
            if suggested.is_empty() {
                saved
            } else {
                suggested.to_owned()
            }
        };
        TeamsStatus {
            teams,
            choices: list.choices.clone(),
            kept: self.kept(),
            form_server: pick(&self.suggested.server_url, form.server_url),
            form_email: pick(&self.suggested.email, form.email),
            problem: list.problem.clone(),
            note: list.note.clone(),
        }
    }

    /// Whether sign-ins outlive the app, and if not, why.
    pub fn kept(&self) -> SignInsKept {
        self.kept.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `Some` while the app is carrying on the kept sign-in of the team at
    /// `server_url`: `None` inside during the first try, then why its
    /// server could not be reached.
    pub fn reaching(&self, server_url: &str) -> Option<Option<String>> {
        self.lock().find(server_url)?.keeper.reaching()
    }

    fn one_at_a_time(&self) -> MutexGuard<'_, ()> {
        self.one_at_a_time
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn failed(&self, problem: String) -> Result<(), String> {
        self.lock().problem = Some(problem.clone());
        Err(problem)
    }

    /// Saves the list. A failure is not shown: the sign-in works for now,
    /// and only the next start would ask for the address again.
    fn save(&self, list: &TeamList) {
        let _ = saved::write(&self.list_file, &list.saved());
    }

    fn lock(&self) -> MutexGuard<'_, TeamList> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// The folder that keeps each team's last-known parts: next to the list
/// file, in the app's own data folder.
fn parts_folder_of(list_file: &Path) -> PathBuf {
    list_file
        .parent()
        .unwrap_or(Path::new("."))
        .join(kept_parts::FOLDER)
}

fn writer_for(folder: &Path, saved: &SavedTeam, catalog: &Arc<TeamCatalog>) -> Arc<PartsWriter> {
    Arc::new(PartsWriter::start(WriterTask {
        folder: folder.to_owned(),
        server_url: saved.server_url.clone(),
        catalog: catalog.clone(),
    }))
}

/// Deletes a team's kept parts, once its session has ended. A file that
/// cannot be deleted is logged, not shown: a team signed out or off the
/// list shows no parts anyway, and never reads the file again.
fn delete_kept_parts(writer: &PartsWriter, server_url: &str) {
    if let Err(problem) = writer.delete() {
        eprintln!(
            "kicad-parts-desktop: could not delete the kept parts of {server_url} ({problem})."
        );
    }
}

fn keeper_for(
    saved: &SavedTeam,
    catalog: &Arc<TeamCatalog>,
    sign_ins: &SignIns,
    kept: &Arc<Mutex<SignInsKept>>,
    look_for_kept: bool,
) -> Arc<Keeper> {
    let kept = kept.clone();
    Arc::new(Keeper::start(
        KeeperTask {
            server_url: saved.server_url.clone(),
            catalog: catalog.clone(),
            store: sign_ins.store.clone(),
            store_failed: Arc::new(move |problem| store_failed(&kept, problem)),
        },
        look_for_kept,
    ))
}

/// The store failed: from now on, sign-ins last only until the app closes,
/// and the app says why.
fn store_failed(kept: &Mutex<SignInsKept>, problem: String) {
    *kept.lock().unwrap_or_else(|e| e.into_inner()) = SignInsKept::UntilClosed {
        reason: format!("the system's password store did not keep your sign-in ({problem})"),
    };
}

#[cfg(test)]
mod tests;

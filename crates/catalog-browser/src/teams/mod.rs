//! The teams the person works with. Each team is its own team server, with
//! its own [`TeamCatalog`] and session, side by side: signing in to one
//! team never signs out of another.
//!
//! The app remembers the list (each server address and email, see
//! [`saved`]) and which team the catalog view shows. The catalog view
//! shows one team at a time through [`Teams::shown`]; anything that works
//! on every team, such as keeping each team's KiCad libraries up to date,
//! uses [`Teams::signed_in`].
//!
//! Each team's sign-in is kept between runs in a [`SignInStore`] (the
//! operating system's credential store in the app), and each team's
//! [`keeper`] carries it on at start and renews it while in use.

mod keeper;
mod saved;
mod sign_ins;

pub use sign_ins::{MemorySignIns, SignInStore, SignIns};

use crate::team::TeamCatalog;
use collaboration::ServerAddress;
use desktop_contract::{SignInRequest, SignInsKept, TeamSession, TeamsStatus};
use keeper::{Keeper, KeeperTask};
use saved::{SavedList, SavedTeam};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

pub struct Teams {
    list_file: PathBuf,
    sign_ins: SignIns,
    /// Whether sign-ins outlive the app. Turns to "until closed" if the
    /// store fails.
    kept: Arc<Mutex<SignInsKept>>,
    /// From launch options: they fill in the sign-in form.
    suggested: SavedTeam,
    state: Mutex<TeamList>,
    /// Sign-ins run one at a time, so two sign-ins to the same new server
    /// cannot both add it.
    signing_in: Mutex<()>,
    /// What the catalog view shows while no team is signed in.
    nobody: Arc<TeamCatalog>,
}

/// One team in the list.
struct Team {
    saved: SavedTeam,
    /// Kept for as long as the team is in the list, so whatever follows the
    /// team (such as its KiCad libraries) keeps following the same one.
    catalog: Arc<TeamCatalog>,
    keeper: Arc<Keeper>,
    /// Something the person should know about this team, such as a
    /// sign-in the store could not forget.
    note: Option<String>,
}

struct TeamList {
    teams: Vec<Team>,
    /// The server address of the team the person chose to show.
    chosen: String,
    /// Why the last sign-in failed, for the sign-in form.
    problem: Option<String>,
}

impl TeamList {
    fn find(&self, server_url: &str) -> Option<&Team> {
        self.teams
            .iter()
            .find(|team| team.saved.server_url == server_url)
    }

    /// The team the catalog view shows: the chosen one while it is signed
    /// in, otherwise the first team that is, otherwise the chosen one.
    fn shown(&self) -> Option<&Team> {
        let signed_in = |team: &&Team| team.catalog.is_signed_in();
        let chosen = self.find(&self.chosen);
        chosen
            .filter(signed_in)
            .or_else(|| self.teams.iter().find(signed_in))
            .or(chosen)
    }

    fn saved(&self) -> SavedList {
        SavedList {
            teams: self.teams.iter().map(|team| team.saved.clone()).collect(),
            shown: self.chosen.clone(),
        }
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
        let saved = saved::read(&list_file);
        let kept = Arc::new(Mutex::new(sign_ins.kept.clone()));
        let teams = saved
            .teams
            .into_iter()
            .map(|saved| {
                let catalog = Arc::new(TeamCatalog::new());
                // A team the person signed out of is never carried on, even
                // if the store could not forget its sign-in.
                let look_for_kept = !saved.signed_out;
                let keeper = keeper_for(&saved, &catalog, &sign_ins, &kept, look_for_kept);
                Team {
                    saved,
                    catalog,
                    keeper,
                    note: None,
                }
            })
            .collect();
        Self {
            list_file,
            sign_ins,
            kept,
            suggested: SavedTeam {
                server_url: suggested_server.unwrap_or_default(),
                email: suggested_email.unwrap_or_default(),
                ..SavedTeam::default()
            },
            state: Mutex::new(TeamList {
                teams,
                chosen: saved.shown,
                problem: None,
            }),
            signing_in: Mutex::new(()),
            nobody: Arc::new(TeamCatalog::new()),
        }
    }

    /// Signs in to the team at the request's server, adding it to the list
    /// if it is new, and shows it. Other teams stay signed in. Its keeper
    /// then keeps the sign-in for the next run. A failed sign-in adds
    /// nothing; its problem, in plain words, is the error.
    pub fn sign_in(&self, request: SignInRequest) -> Result<(), String> {
        let _one_at_a_time = self
            .signing_in
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
        let saved = SavedTeam {
            server_url: server_url.clone(),
            email,
            code: catalog.code(),
            signed_out: false,
        };
        let mut list = self.lock();
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
                list.teams.push(Team {
                    saved,
                    catalog,
                    keeper,
                    note: None,
                });
            }
        }
        list.chosen = server_url;
        self.save(&list);
        Ok(())
    }

    /// Signs out of one team and forgets its kept sign-in. It stays in the
    /// list, marked signed out, so signing in again needs only the
    /// password, and the next start never carries on a sign-in the store
    /// could not forget.
    pub fn sign_out(&self, server_url: &str) {
        let keeper = {
            let mut list = self.lock();
            list.problem = None;
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
            self.save(&list);
            keeper
        };
        if let Err(problem) = keeper.sign_out() {
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

    /// Signs out of one team, forgets its kept sign-in, and takes it off
    /// the list.
    pub fn remove(&self, server_url: &str) {
        let removed = {
            let mut list = self.lock();
            let Some(index) = list
                .teams
                .iter()
                .position(|team| team.saved.server_url == server_url)
            else {
                return;
            };
            let removed = list.teams.remove(index);
            self.save(&list);
            removed
        };
        // Signed out outside the lock, so status polling never waits. A
        // sign-in the store could not forget is harmless: the team is off
        // the list, so it is never carried on, and adding the team again
        // replaces it.
        let _ = removed.keeper.sign_out();
    }

    /// Shows the team at `server_url` in the catalog view. No sign-in is
    /// needed: its session is already running.
    pub fn show(&self, server_url: &str) -> Result<(), String> {
        let mut list = self.lock();
        if list.find(server_url).is_none() {
            return Err(format!("{server_url} is not in your list of teams."));
        }
        list.chosen = server_url.to_owned();
        self.save(&list);
        Ok(())
    }

    /// The team the catalog view shows, or a team catalog that is signed
    /// out when no team is. The one-team catalog view reads everything
    /// through this.
    pub fn shown(&self) -> Arc<TeamCatalog> {
        self.lock()
            .shown()
            .map(|team| team.catalog.clone())
            .unwrap_or_else(|| self.nobody.clone())
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

    /// Every team that is signed in, by server address, in list order.
    pub fn signed_in(&self) -> Vec<(String, Arc<TeamCatalog>)> {
        self.lock()
            .teams
            .iter()
            .filter(|team| team.catalog.is_signed_in())
            .map(|team| (team.saved.server_url.clone(), team.catalog.clone()))
            .collect()
    }

    /// Every team in the list and how it is doing, which team the catalog
    /// view shows, and what the sign-in form starts with.
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
        let signed_in = |team: &&Team| team.catalog.is_signed_in();
        let shown = list
            .shown()
            .filter(signed_in)
            .map(|team| team.saved.server_url.clone())
            .unwrap_or_default();
        // The form offers the chosen team if it needs signing in, else the
        // first team that does.
        let needs_signing_in = |team: &&Team| !signed_in(team);
        let form = list
            .find(&list.chosen)
            .filter(needs_signing_in)
            .or_else(|| list.teams.iter().find(needs_signing_in))
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
            shown,
            kept: self.kept(),
            form_server: pick(&self.suggested.server_url, form.server_url),
            form_email: pick(&self.suggested.email, form.email),
            problem: list.problem.clone(),
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

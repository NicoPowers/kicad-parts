//! The teams the person works with. Each team is its own team server, with
//! its own [`TeamCatalog`] and session, side by side: signing in to one
//! team never signs out of another.
//!
//! The app remembers the list (each server address and email, see
//! [`saved`]) and which team the catalog view shows. The catalog view
//! shows one team at a time through [`Teams::shown`]; anything that works
//! on every team, such as keeping each team's KiCad libraries up to date,
//! uses [`Teams::signed_in`].

mod saved;

use crate::team::TeamCatalog;
use collaboration::ServerAddress;
use desktop_contract::{SignInRequest, TeamSession, TeamStatus};
use saved::{SavedList, SavedTeam};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

pub struct Teams {
    list_file: PathBuf,
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
    /// Reads the list of teams from `list_file`. Nobody is signed in yet.
    /// A suggested server or email (from launch options) fills in the
    /// sign-in form.
    pub fn open(
        list_file: PathBuf,
        suggested_server: Option<String>,
        suggested_email: Option<String>,
    ) -> Self {
        let saved = saved::read(&list_file);
        let teams = saved
            .teams
            .into_iter()
            .map(|saved| Team {
                saved,
                catalog: Arc::new(TeamCatalog::new()),
            })
            .collect();
        Self {
            list_file,
            suggested: SavedTeam {
                server_url: suggested_server.unwrap_or_default(),
                email: suggested_email.unwrap_or_default(),
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
    /// if it is new, and shows it. Other teams stay signed in. A failed
    /// sign-in adds nothing; its problem, in plain words, is the error.
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
            .map(|team| team.catalog.clone());
        let catalog = known
            .clone()
            .unwrap_or_else(|| Arc::new(TeamCatalog::new()));
        let status = catalog.sign_in(SignInRequest {
            server_url: server_url.clone(),
            ..request
        });
        let email = match status.session {
            TeamSession::SignedIn { email, .. } => email,
            TeamSession::SignedOut { problem } => {
                return self.failed(problem.unwrap_or_default());
            }
        };
        let mut list = self.lock();
        list.problem = None;
        let saved = SavedTeam {
            server_url: server_url.clone(),
            email,
        };
        match list
            .teams
            .iter_mut()
            .find(|team| team.saved.server_url == server_url)
        {
            Some(team) => team.saved = saved,
            None => list.teams.push(Team { saved, catalog }),
        }
        list.chosen = server_url;
        self.save(&list);
        Ok(())
    }

    /// Signs out of one team. It stays in the list, so signing in again
    /// needs only the password.
    pub fn sign_out(&self, server_url: &str) {
        let catalog = {
            let mut list = self.lock();
            list.problem = None;
            list.find(server_url).map(|team| team.catalog.clone())
        };
        if let Some(catalog) = catalog {
            catalog.sign_out();
        }
    }

    /// Signs out of one team and takes it off the list.
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
        // Signed out outside the lock, so status polling never waits.
        removed.catalog.sign_out();
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

    /// Every team that is signed in, by server address, in list order.
    pub fn signed_in(&self) -> Vec<(String, Arc<TeamCatalog>)> {
        self.lock()
            .teams
            .iter()
            .filter(|team| team.catalog.is_signed_in())
            .map(|team| (team.saved.server_url.clone(), team.catalog.clone()))
            .collect()
    }

    /// The shown team's status. While no team is signed in, the server
    /// address and email fill in the sign-in form: suggested ones first,
    /// then the shown team's.
    pub fn shown_status(&self) -> TeamStatus {
        let list = self.lock();
        let shown = list.shown();
        if let Some(team) = shown.filter(|team| team.catalog.is_signed_in()) {
            return TeamStatus {
                server_url: team.saved.server_url.clone(),
                email: team.saved.email.clone(),
                ..team.catalog.status()
            };
        }
        let saved = shown.map(|team| team.saved.clone()).unwrap_or_default();
        let pick = |suggested: &str, saved: String| {
            if suggested.is_empty() {
                saved
            } else {
                suggested.to_owned()
            }
        };
        TeamStatus {
            server_url: pick(&self.suggested.server_url, saved.server_url),
            email: pick(&self.suggested.email, saved.email),
            session: TeamSession::SignedOut {
                problem: list.problem.clone(),
            },
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_sign_in_adds_no_team_and_saves_no_password() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.json");
        let teams = Teams::open(file.clone(), None, None);
        let problem = teams
            .sign_in(SignInRequest {
                // Port 9 (discard) on loopback is closed on any normal machine.
                server_url: "http://127.0.0.1:9".into(),
                email: "a@example.test".into(),
                password: "secret-password".into(),
            })
            .unwrap_err();
        assert!(!problem.is_empty());
        assert!(!file.exists(), "a failed sign-in saves nothing at all");
        assert_eq!(
            teams.shown_status().session,
            TeamSession::SignedOut {
                problem: Some(problem)
            }
        );
        assert!(teams.signed_in().is_empty());
        assert!(!teams.shown().is_signed_in());
    }

    #[test]
    fn an_address_that_may_not_receive_a_password_is_refused_first() {
        let dir = tempfile::tempdir().unwrap();
        let teams = Teams::open(dir.path().join("team.json"), None, None);
        let problem = teams
            .sign_in(SignInRequest {
                server_url: "http://team.example.com".into(),
                email: "a@example.test".into(),
                password: "secret-password".into(),
            })
            .unwrap_err();
        assert!(
            problem.starts_with("Addresses starting with http://"),
            "{problem}"
        );
    }

    #[test]
    fn suggested_values_fill_in_the_sign_in_form() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.json");
        saved::write(
            &file,
            &SavedList {
                teams: vec![SavedTeam {
                    server_url: "https://parts.example.com".into(),
                    email: "a@example.test".into(),
                }],
                shown: "https://parts.example.com".into(),
            },
        )
        .unwrap();
        let status = Teams::open(file.clone(), None, None).shown_status();
        assert_eq!(
            (status.server_url.as_str(), status.email.as_str()),
            ("https://parts.example.com", "a@example.test")
        );
        let status = Teams::open(file, Some("http://127.0.0.1:1".into()), None).shown_status();
        assert_eq!(
            (status.server_url.as_str(), status.email.as_str()),
            ("http://127.0.0.1:1", "a@example.test")
        );
    }

    #[test]
    fn teams_in_the_list_can_be_shown_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.json");
        let team = |server_url: &str| SavedTeam {
            server_url: server_url.into(),
            email: "a@example.test".into(),
        };
        saved::write(
            &file,
            &SavedList {
                teams: vec![team("https://a.example.com"), team("https://b.example.com")],
                shown: "https://a.example.com".into(),
            },
        )
        .unwrap();
        let teams = Teams::open(file.clone(), None, None);
        teams.show("https://b.example.com").unwrap();
        assert_eq!(saved::read(&file).shown, "https://b.example.com");
        assert!(teams.show("https://c.example.com").is_err());
        teams.remove("https://a.example.com");
        assert_eq!(saved::read(&file).teams, [team("https://b.example.com")]);
        // Nobody is signed in, so the shown team is the chosen one.
        assert_eq!(teams.shown_status().server_url, "https://b.example.com");
    }
}

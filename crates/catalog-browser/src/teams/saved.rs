//! The list of teams the app remembers between runs: each team's server
//! address, the email last used there, its code and colour, and what the
//! person last chose in the part list. It never holds a password or a
//! token.

use collaboration::ServerAddress;
use desktop_contract::TeamChoices;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// One team in the list.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SavedTeam {
    /// The address as the app uses it, such as `https://parts.example.com/acme`.
    #[serde(default)]
    pub(super) server_url: String,
    #[serde(default)]
    pub(super) email: String,
    /// The team's code, such as `MA`, once known. It names the team before
    /// its server is reached.
    #[serde(default)]
    pub(super) code: String,
    /// The team's colour, one of `TEAM_COLOURS`, given once when the team
    /// joined the list, so it stays the same.
    #[serde(default)]
    pub(super) colour: String,
    /// The person signed out of this team. Its kept sign-in, if the store
    /// could not forget it, is never carried on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) signed_out: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SavedList {
    pub(super) teams: Vec<SavedTeam>,
    /// What the person last chose in the part list and in features that
    /// work on one team at a time. (Earlier versions saved the one team
    /// the catalog view showed, as `shown`; it is no longer read.)
    #[serde(default)]
    pub(super) choices: TeamChoices,
}

/// What the file may hold: the list, or the one team that versions before
/// several teams saved.
#[derive(Deserialize)]
#[serde(untagged)]
enum OnDisk {
    List(SavedList),
    OneTeam(SavedTeam),
}

/// The list in `file`; an empty list if there is no file or it cannot be
/// read.
pub(super) fn read(file: &Path) -> SavedList {
    let Some(on_disk) = fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<OnDisk>(&bytes).ok())
    else {
        return SavedList::default();
    };
    let list = match on_disk {
        OnDisk::List(list) => list,
        OnDisk::OneTeam(team) if team.server_url.is_empty() => SavedList::default(),
        OnDisk::OneTeam(team) => SavedList {
            teams: vec![team],
            choices: TeamChoices::default(),
        },
    };
    one_address_per_server(list)
}

/// The list with each address as sign-in writes it now (see
/// `ServerAddress::for_sign_in`), and each server once. An earlier version
/// may have saved `https://Parts.example.com` beside
/// `https://parts.example.com`: they are one team, kept as the first one
/// listed, with a code either one knew.
fn one_address_per_server(list: SavedList) -> SavedList {
    let mut teams: Vec<SavedTeam> = Vec::new();
    for mut team in list.teams {
        team.server_url = address_now(&team.server_url);
        match teams
            .iter_mut()
            .find(|known| known.server_url == team.server_url)
        {
            Some(known) if known.code.is_empty() => known.code = team.code,
            Some(_) => {}
            None => teams.push(team),
        }
    }
    let mut choices = list.choices;
    choices.filter = address_now(&choices.filter);
    for server in choices.last_used.values_mut() {
        *server = address_now(server);
    }
    SavedList { teams, choices }
}

/// `address` as sign-in writes it now; as it is if it is not an address.
fn address_now(address: &str) -> String {
    ServerAddress::for_sign_in(address)
        .map(|address| address.url())
        .unwrap_or_else(|_| address.to_owned())
}

/// Writes the list to `file` through a temporary file, so a crash never
/// leaves half a list.
pub(super) fn write(file: &Path, list: &SavedList) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    let temp = file.with_extension("json.tmp");
    fs::write(&temp, json).map_err(|e| e.to_string())?;
    fs::rename(&temp, file).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_team_an_older_version_saved_becomes_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.json");
        fs::write(
            &file,
            r#"{"server_url":"https://parts.example.com","email":"a@example.test"}"#,
        )
        .unwrap();
        let list = read(&file);
        assert_eq!(list.choices, TeamChoices::default());
        assert_eq!(list.teams.len(), 1);
        assert_eq!(list.teams[0].email, "a@example.test");
        // Nothing saved yet, or a file that cannot be read, is no team.
        fs::write(&file, r#"{"server_url":"","email":""}"#).unwrap();
        assert_eq!(read(&file), SavedList::default());
        fs::write(&file, "not json").unwrap();
        assert_eq!(read(&file), SavedList::default());
        assert_eq!(read(&dir.path().join("missing.json")), SavedList::default());
    }

    #[test]
    fn one_server_saved_under_two_addresses_is_one_team() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.json");
        fs::write(
            &file,
            r#"{"teams":[
                {"server_url":"https://Parts.example.com","email":"a@example.test"},
                {"server_url":"https://beta.example.com","email":"a@example.test"},
                {"server_url":"https://parts.example.com:443","email":"b@example.test","code":"PE"}
            ],"shown":"https://PARTS.example.com",
            "choices":{"filter":"https://PARTS.example.com","last_used":{"add_part":"https://Beta.example.com"}}}"#,
        )
        .unwrap();
        let list = read(&file);
        let addresses: Vec<&str> = list
            .teams
            .iter()
            .map(|team| team.server_url.as_str())
            .collect();
        assert_eq!(
            addresses,
            ["https://parts.example.com", "https://beta.example.com"]
        );
        // The first is kept, with the code the other knew.
        assert_eq!(list.teams[0].email, "a@example.test");
        assert_eq!(list.teams[0].code, "PE");
        // The choices name each team as its address is written now; the
        // shown team earlier versions saved is no longer read.
        assert_eq!(list.choices.filter, "https://parts.example.com");
        assert_eq!(
            list.choices.last_used.get("add_part").map(String::as_str),
            Some("https://beta.example.com")
        );
    }

    #[test]
    fn the_list_reads_back_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config").join("team.json");
        let list = SavedList {
            teams: vec![
                SavedTeam {
                    server_url: "https://parts.example.com/acme".into(),
                    email: "a@example.test".into(),
                    code: "AC".into(),
                    colour: "blue".into(),
                    signed_out: false,
                },
                SavedTeam {
                    server_url: "https://beta.example.com".into(),
                    email: "a@example.test".into(),
                    code: String::new(),
                    colour: "green".into(),
                    signed_out: false,
                },
            ],
            choices: TeamChoices {
                filter: "https://beta.example.com".into(),
                group_by: desktop_contract::GroupBy::Team,
                last_used: [("add_part".to_owned(), "https://beta.example.com".to_owned())].into(),
            },
        };
        write(&file, &list).unwrap();
        assert_eq!(read(&file), list);
    }
}

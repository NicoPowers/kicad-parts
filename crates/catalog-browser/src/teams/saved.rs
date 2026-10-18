//! The list of teams the app remembers between runs: each team's server
//! address and the email last used there, and which team the catalog view
//! shows. It never holds a password or a token.

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
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SavedList {
    pub(super) teams: Vec<SavedTeam>,
    /// The server address of the team the catalog view shows.
    #[serde(default)]
    pub(super) shown: String,
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
    match on_disk {
        OnDisk::List(list) => list,
        OnDisk::OneTeam(team) if team.server_url.is_empty() => SavedList::default(),
        OnDisk::OneTeam(team) => SavedList {
            shown: team.server_url.clone(),
            teams: vec![team],
        },
    }
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
        assert_eq!(list.shown, "https://parts.example.com");
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
    fn the_list_reads_back_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config").join("team.json");
        let list = SavedList {
            teams: vec![
                SavedTeam {
                    server_url: "https://parts.example.com/acme".into(),
                    email: "a@example.test".into(),
                    code: "AC".into(),
                },
                SavedTeam {
                    server_url: "https://beta.example.com".into(),
                    email: "a@example.test".into(),
                    code: String::new(),
                },
            ],
            shown: "https://beta.example.com".into(),
        };
        write(&file, &list).unwrap();
        assert_eq!(read(&file), list);
    }
}

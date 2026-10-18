//! Each team's libraries live in a folder of their own,
//! `kicad-libraries/<CODE>/`, which records the team it belongs to and
//! the team server it was made from. A person may work for several teams,
//! each on its own team server; an update writes only the signed-in team's
//! folder and never one that belongs to another team.
//!
//! The team's ID alone is not enough: every member of a team knows it, so
//! any server the person signs in to could claim it. So a folder takes
//! writes only from the server it records (kicad-parts-092b). If the team
//! really moved, the person says so once, and the folder records the new
//! address.

use crate::plan::Naming;
use collaboration::ServerAddress;
use std::path::{Path, PathBuf};

/// The file in a team's folder that says which team it belongs to and
/// what the next update needs.
pub const STATE_FILE: &str = ".kicad-parts.json";

/// The team an update writes for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Team {
    /// The ID of the team settings record on its server, which stays the
    /// same when the server's address changes.
    pub id: String,
    /// The server's address, as sign-in writes it
    /// (`ServerAddress::for_sign_in(..).url()`). The folder takes writes
    /// only from the server it records.
    pub server: String,
    pub naming: Naming,
}

/// What a team's folder records.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FolderState {
    pub team_id: String,
    /// The team server the libraries were made from. Blank in a folder
    /// made before addresses were recorded.
    pub server: String,
    pub code: String,
    /// When the last update finished (milliseconds since 1970, UTC).
    pub updated_at_ms: Option<u64>,
    /// When the `.kicad_dbl` last changed. A KiCad that started before
    /// then still has the old groups and fields.
    pub layout_changed_at_ms: Option<u64>,
}

/// Why a folder refuses an update for a team.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Refusal {
    /// The folder belongs to another team.
    OtherTeam,
    /// The folder belongs to the team, but was made from another server.
    /// It waits until the person says the team moved.
    OtherServer,
}

impl FolderState {
    /// Why this folder must not take an update for `team`, if it must not.
    ///
    /// A folder with no recorded server, made before servers were
    /// recorded, takes the update and records the server then. That trusts
    /// the first server that claims the team's ID, once: the same trust
    /// every update gave before, and from then on only that server writes.
    pub(crate) fn refusal(&self, team: &Team) -> Option<Refusal> {
        if self.team_id != team.id {
            Some(Refusal::OtherTeam)
        } else if !self.server.is_empty() && !same_server(&self.server, &team.server) {
            Some(Refusal::OtherServer)
        } else {
            None
        }
    }

    fn to_json(&self) -> String {
        let state = serde_json::json!({
            "team_id": self.team_id,
            "server": self.server,
            "code": self.code,
            "updated_at_ms": self.updated_at_ms,
            "layout_changed_at_ms": self.layout_changed_at_ms,
        });
        serde_json::to_string_pretty(&state).expect("JSON values always serialize")
    }
}

/// The folder of the team with `code` under `root`, the `kicad-libraries`
/// folder.
pub fn team_folder(root: &Path, code: &str) -> PathBuf {
    root.join(code)
}

/// What `folder` records, or `None` if it records nothing readable.
pub fn read_state(folder: &Path) -> Option<FolderState> {
    let bytes = std::fs::read(folder.join(STATE_FILE)).ok()?;
    let state: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let text = |name: &str| state[name].as_str().unwrap_or_default().to_owned();
    Some(FolderState {
        team_id: text("team_id"),
        server: text("server"),
        code: text("code"),
        updated_at_ms: state["updated_at_ms"].as_u64(),
        layout_changed_at_ms: state["layout_changed_at_ms"].as_u64(),
    })
}

/// Writes what `folder` records.
pub(crate) fn write_state(folder: &Path, state: &FolderState) -> Result<(), crate::UpdateError> {
    crate::files::write_whole(&folder.join(STATE_FILE), state.to_json().as_bytes())
        .map_err(Into::into)
}

/// Every team folder under `root`, by code, with what each records.
pub fn team_folders(root: &Path) -> Vec<(PathBuf, FolderState)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut folders: Vec<(PathBuf, FolderState)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| read_state(&path).map(|state| (path, state)))
        .collect();
    folders.sort_by(|a, b| a.1.code.cmp(&b.1.code));
    folders
}

/// The team another folder on this computer already uses `code` for, if
/// any: KiCad would refuse two libraries with the same nickname.
pub fn code_used_by_other_team(root: &Path, code: &str, team_id: &str) -> Option<FolderState> {
    read_state(&team_folder(root, code)).filter(|state| state.team_id != team_id)
}

/// Whether `recorded`, a folder's team server, is the server at `server`.
/// Folders made before sign-in wrote every address one way may hold one
/// with capitals or the usual port, such as `https://Parts.example.com:443`,
/// so both are compared as sign-in writes them now.
pub fn same_server(recorded: &str, server: &str) -> bool {
    let as_sign_in_writes = |address: &str| {
        ServerAddress::for_sign_in(address)
            .map(|address| address.url())
            .unwrap_or_else(|_| address.to_owned())
    };
    as_sign_in_writes(recorded) == as_sign_in_writes(server)
}

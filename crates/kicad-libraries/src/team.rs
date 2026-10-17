//! Each team's libraries live in a folder of their own,
//! `kicad-libraries/<CODE>/`, which records the team it belongs to. A
//! person may work for several teams, each on its own team server; an
//! update writes only the signed-in team's folder and never one that
//! belongs to another team.

use crate::plan::Naming;
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
    /// The server's address, to name it to people.
    pub server: String,
    pub naming: Naming,
}

/// What a team's folder records.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FolderState {
    pub team_id: String,
    pub server: String,
    pub code: String,
    /// When the last update finished (milliseconds since 1970, UTC).
    pub updated_at_ms: Option<u64>,
    /// When the `.kicad_dbl` last changed. A KiCad that started before
    /// then still has the old groups and fields.
    pub layout_changed_at_ms: Option<u64>,
}

impl FolderState {
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

//! Each team's parts file: its last-known parts on disk, so a team whose
//! server cannot be reached at start still shows its parts, marked offline
//! (kicad-parts-ed46, ADR 0020).
//!
//! One file per team, in a `parts-files` folder in the app's own data
//! folder. A file holds what the part list needs (each part's values and
//! the category names), the server address it came from, and when it last
//! matched the server. It never holds a password or a token.
//!
//! A file is written whole, through a temporary file, so a crash never
//! leaves half a file. A file that cannot be read, has another version, or
//! names another server is ignored: the next catch-up writes a new one.

use crate::PartValues;
use crate::team::PartsFile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The folder, next to the list of teams, that holds every team's file.
pub(super) const FOLDER: &str = "parts-files";

/// The file layout. A file with another version is ignored, never read in
/// part; change it whenever the layout or `PartValues` changes.
const VERSION: u32 = 1;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OnDisk {
    version: u32,
    /// The team server the parts came from, as sign-in writes addresses.
    server_url: String,
    /// When the parts last matched the team server, in milliseconds since
    /// 1970.
    as_of_ms: u64,
    category_names: BTreeMap<String, String>,
    /// Each part's values, by IPN.
    parts: BTreeMap<String, PartValues>,
}

/// The parts file of the team at `server_url`, in `folder`.
/// Every character but letters, digits, `.` and `-` is written as its code,
/// so two addresses never share a file and every system accepts the name.
pub(super) fn file_for(folder: &Path, server_url: &str) -> PathBuf {
    let name: String = server_url
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch.to_string()
            } else {
                format!("_{:x}_", u32::from(ch))
            }
        })
        .collect();
    folder.join(format!("{name}.json"))
}

/// The parts file of the team at `server_url`, or `None` if there is none
/// it may use. A file that cannot be read, has another version, or names
/// another server is ignored: a team takes parts only from its own server
/// (kicad-parts-092b). Why is logged, not shown: the next catch-up writes
/// a good file, and the person loses nothing.
pub(super) fn read(folder: &Path, server_url: &str) -> Option<PartsFile> {
    let file = file_for(folder, server_url);
    let bytes = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => return ignored(&file, &error.to_string()),
    };
    let on_disk: OnDisk = match serde_json::from_slice(&bytes) {
        Ok(on_disk) => on_disk,
        Err(error) => return ignored(&file, &error.to_string()),
    };
    if on_disk.version != VERSION {
        return ignored(&file, &format!("it has version {}", on_disk.version));
    }
    if on_disk.server_url != server_url {
        return ignored(&file, &format!("it came from {}", on_disk.server_url));
    }
    Some(PartsFile {
        server_url: on_disk.server_url,
        as_of_ms: on_disk.as_of_ms,
        values: on_disk.parts,
        category_names: on_disk.category_names,
    })
}

/// Writes `parts` to its team's file, through a temporary file, so a crash
/// or a full disk never leaves half a file: the old file stays until the
/// new one is whole.
pub(super) fn write(folder: &Path, parts: &PartsFile) -> Result<(), String> {
    fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let on_disk = OnDisk {
        version: VERSION,
        server_url: parts.server_url.clone(),
        as_of_ms: parts.as_of_ms,
        category_names: parts.category_names.clone(),
        parts: parts.values.clone(),
    };
    let json = serde_json::to_vec(&on_disk).map_err(|e| e.to_string())?;
    let file = file_for(folder, &parts.server_url);
    let temp = file.with_extension("json.tmp");
    fs::write(&temp, json).map_err(|e| e.to_string())?;
    fs::rename(&temp, &file).map_err(|e| e.to_string())
}

/// Deletes the parts file of the team at `server_url`. No file is fine.
pub(super) fn delete(folder: &Path, server_url: &str) -> Result<(), String> {
    match fs::remove_file(file_for(folder, server_url)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn ignored(file: &Path, why: &str) -> Option<PartsFile> {
    eprintln!(
        "kicad-parts-desktop: ignored the parts file {}, because {why}. The next catch-up writes them again.",
        file.display()
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACME: &str = "https://parts.example.com/acme";

    /// A folder under `target/`, never the person's own data folder.
    fn folder() -> tempfile::TempDir {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-folders");
        fs::create_dir_all(&target).unwrap();
        tempfile::tempdir_in(target).unwrap()
    }

    fn parts(value: &str) -> PartsFile {
        let values = PartValues {
            category: Some("RES".into()),
            fields: Some(vec![("Value".into(), value.into())]),
            mpn: Some("RC0603FR-0747RL".into()),
            ..PartValues::default()
        };
        PartsFile {
            server_url: ACME.into(),
            as_of_ms: 1_790_000_000_000,
            values: [("RES-0000-0470".to_owned(), values)].into(),
            category_names: [("RES".to_owned(), "Resistors".to_owned())].into(),
        }
    }

    #[test]
    fn a_parts_file_reads_back_as_written() {
        let dir = folder();
        assert_eq!(read(dir.path(), ACME), None, "no file yet");
        write(dir.path(), &parts("47")).unwrap();
        assert_eq!(read(dir.path(), ACME), Some(parts("47")));
        // A newer write replaces the file whole.
        write(dir.path(), &parts("47R")).unwrap();
        assert_eq!(read(dir.path(), ACME), Some(parts("47R")));
    }

    #[test]
    fn each_server_has_its_own_file_with_a_name_every_system_accepts() {
        let dir = Path::new(FOLDER);
        let acme = file_for(dir, ACME);
        assert_eq!(
            acme.file_name().unwrap(),
            "https_3a__2f__2f_parts.example.com_2f_acme.json"
        );
        // Addresses that differ in any character never share a file.
        assert_ne!(acme, file_for(dir, "https://parts.example.com_2f_acme"));
        assert_ne!(acme, file_for(dir, "http://parts.example.com/acme"));
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_whole() {
        let dir = folder();
        write(dir.path(), &parts("47")).unwrap();
        // A folder where the temporary file goes makes the write fail, as
        // a full disk would.
        let temp = file_for(dir.path(), ACME).with_extension("json.tmp");
        fs::create_dir_all(&temp).unwrap();
        assert!(write(dir.path(), &parts("47R")).is_err());
        assert_eq!(read(dir.path(), ACME), Some(parts("47")));
    }

    #[test]
    fn a_parts_file_from_another_server_is_never_used() {
        let dir = folder();
        write(dir.path(), &parts("47")).unwrap();
        // The file of another address, as if renamed or copied there.
        let other = "https://other.example.com";
        fs::copy(file_for(dir.path(), ACME), file_for(dir.path(), other)).unwrap();
        assert_eq!(read(dir.path(), other), None);
    }

    #[test]
    fn a_corrupt_or_unknown_file_is_ignored() {
        let dir = folder();
        let file = file_for(dir.path(), ACME);
        write(dir.path(), &parts("47")).unwrap();
        let good = fs::read_to_string(&file).unwrap();
        for bad in [
            String::from("not json"),
            good[..good.len() / 2].to_owned(),
            good.replace("\"version\":1", "\"version\":2"),
            good.replace("\"mpn\"", "\"main_mpn\""),
        ] {
            fs::write(&file, &bad).unwrap();
            assert_eq!(read(dir.path(), ACME), None, "{bad}");
        }
    }

    #[test]
    fn deleting_a_parts_file_needs_no_file() {
        let dir = folder();
        write(dir.path(), &parts("47")).unwrap();
        delete(dir.path(), ACME).unwrap();
        assert_eq!(read(dir.path(), ACME), None);
        delete(dir.path(), ACME).unwrap();
    }
}

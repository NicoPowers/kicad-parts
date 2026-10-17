//! The SHA-256 of every file in a folder, to show that two folders hold
//! exactly the same bytes, or that a folder did not change. The
//! full-catalog proof uses it three ways: the CSV catalog is the same after
//! the proof as before it, two team servers give byte-identical KiCad
//! libraries, and updating the libraries again writes nothing.

use catalog::sha256_hex;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

/// The folders that make up the CSV catalog, which an import only reads.
pub const CSV_CATALOG_FOLDERS: [&str; 4] = ["database", "symbols", "footprints", "3d-models"];

/// Every file under some folders, by path from the folder they were read
/// from (with `/`), with the SHA-256 of its bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    files: BTreeMap<String, String>,
}

impl Snapshot {
    /// Every file under `folder`, by path from it. Hidden files count too:
    /// a file the proof was not meant to touch is a file. A link is
    /// recorded by where it points, never followed, so a link out of the
    /// folder cannot make the snapshot read outside it.
    pub fn of(folder: &Path) -> io::Result<Self> {
        let mut snapshot = Self::default();
        snapshot.add(folder, folder, "")?;
        Ok(snapshot)
    }

    /// The CSV catalog's folders under `repository` ([`CSV_CATALOG_FOLDERS`]),
    /// by path from it, such as `database/g-res.csv`.
    pub fn of_csv_catalog(repository: &Path) -> io::Result<Self> {
        let mut snapshot = Self::default();
        for folder in CSV_CATALOG_FOLDERS {
            snapshot.add(&repository.join(folder), &repository.join(folder), folder)?;
        }
        Ok(snapshot)
    }

    /// How many files it holds.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The same snapshot without the file at `path`, for a file that is
    /// meant to differ, such as the one that names the team.
    pub fn without(mut self, path: &str) -> Self {
        self.files.remove(path);
        self
    }

    /// What differs from `before`, one plain sentence per file, in path
    /// order. Empty when every file has the same bytes.
    pub fn differences_from(&self, before: &Snapshot) -> Vec<String> {
        let mut differences = Vec::new();
        for (path, hash) in &before.files {
            match self.files.get(path) {
                None => differences.push(format!("{path} is gone.")),
                Some(now) if now != hash => differences.push(format!("{path} changed.")),
                Some(_) => {}
            }
        }
        for path in self.files.keys() {
            if !before.files.contains_key(path) {
                differences.push(format!("{path} is new."));
            }
        }
        differences.sort();
        differences
    }

    fn add(&mut self, base: &Path, folder: &Path, prefix: &str) -> io::Result<()> {
        let mut entries: Vec<fs::DirEntry> = fs::read_dir(folder)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let kind = fs::symlink_metadata(&path)?.file_type();
            let inside = path
                .strip_prefix(base)
                .map_err(io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            let key = if prefix.is_empty() {
                inside
            } else {
                format!("{prefix}/{inside}")
            };
            if kind.is_symlink() {
                let target = fs::read_link(&path)?;
                self.files
                    .insert(key, format!("link to {}", target.display()));
            } else if kind.is_dir() {
                self.add(base, &path, prefix)?;
            } else {
                self.files.insert(key, sha256_hex(fs::read(&path)?));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under `target/`, deleted when dropped.
    fn scratch() -> tempfile::TempDir {
        let base = crate::services::repository_root()
            .join("target")
            .join("test-scratch");
        fs::create_dir_all(&base).unwrap();
        tempfile::Builder::new().tempdir_in(base).unwrap()
    }

    fn write(folder: &Path, path: &str, bytes: &str) {
        let file = folder.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
    }

    /// Every way a file can differ is named, and nothing else is.
    #[test]
    fn it_names_each_changed_new_and_missing_file() {
        let folder = scratch();
        let root = folder.path();
        write(root, "a.txt", "one");
        write(root, "sub/b.txt", "two");
        write(root, "sub/c.txt", "three");
        let before = Snapshot::of(root).unwrap();
        assert_eq!(before.len(), 3);
        assert!(
            Snapshot::of(root)
                .unwrap()
                .differences_from(&before)
                .is_empty()
        );

        write(root, "sub/b.txt", "TWO");
        fs::remove_file(root.join("sub/c.txt")).unwrap();
        write(root, ".hidden", "new");
        assert_eq!(
            Snapshot::of(root).unwrap().differences_from(&before),
            [
                ".hidden is new.",
                "sub/b.txt changed.",
                "sub/c.txt is gone."
            ]
        );
    }

    /// The file that is meant to differ, such as the one naming the team,
    /// can be left out of the comparison; no other file can hide that way.
    #[test]
    fn a_file_meant_to_differ_can_be_left_out() {
        let (one, two) = (scratch(), scratch());
        for (folder, team) in [(one.path(), "team one"), (two.path(), "team two")] {
            write(folder, "MA/.kicad-parts.json", team);
            write(folder, "MA/MA-Parts.sqlite", "rows");
        }
        let a = Snapshot::of(one.path()).unwrap();
        let b = Snapshot::of(two.path()).unwrap();
        assert_eq!(b.differences_from(&a), ["MA/.kicad-parts.json changed."]);
        let left_out = |snapshot: &Snapshot| snapshot.clone().without("MA/.kicad-parts.json");
        assert!(left_out(&b).differences_from(&left_out(&a)).is_empty());
    }

    #[test]
    fn the_csv_catalog_snapshot_names_files_from_the_repository() {
        let repository = scratch();
        for folder in CSV_CATALOG_FOLDERS {
            write(repository.path(), &format!("{folder}/x"), folder);
        }
        write(repository.path(), "target/not-the-catalog", "x");
        let snapshot = Snapshot::of_csv_catalog(repository.path()).unwrap();
        assert_eq!(
            snapshot.files.keys().collect::<Vec<_>>(),
            ["3d-models/x", "database/x", "footprints/x", "symbols/x"]
        );
    }
}

//! The IPNs the CSV catalog folder holds now, for giving a new part its IPN
//! (kicad-parts-3b72).
//!
//! While a team moves from the CSV catalog to the app, someone may add a CSV
//! row while Add part is open: they save a row, or `git pull` brings one.
//! If Add part then gave that IPN to another part, a later import would find
//! two parts with one IPN. So Add part never trusts a list it read earlier:
//!
//! - [`CsvIpns::now`] reads the files again whenever one changed on disk
//!   (another size or time), and otherwise answers from what it read last.
//!   Add part uses it when it opens and while someone types an IPN.
//! - [`CsvIpns::read_fresh`] always reads the files. Saving uses it, just
//!   before the IPN is sent, because a file can change within the same
//!   second and keep its size.
//!
//! Rows nobody has pulled yet are on another computer, so no reading can
//! see them; the guide says to give new IPNs in one place during the move
//! (docs/operations/MOVE_THE_CSV_CATALOG.md).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// The IPNs of the CSV catalog's `database/g-*.csv` files, each with the
/// file it is in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IpnsInCsv {
    /// Each IPN, with the repository path of the first file that has it,
    /// such as `database/g-res.csv`.
    files: BTreeMap<String, String>,
}

impl IpnsInCsv {
    fn add(&mut self, ipn: &str, file: &str) {
        self.files
            .entry(ipn.to_owned())
            .or_insert_with(|| file.to_owned());
    }

    pub fn contains(&self, ipn: &str) -> bool {
        self.files.contains_key(ipn)
    }

    /// The file that has `ipn`, such as `database/g-res.csv`.
    pub fn file_of(&self, ipn: &str) -> Option<&str> {
        self.files.get(ipn).map(String::as_str)
    }

    /// Every IPN, in order.
    pub fn ipns(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

/// What the files looked like when they were read: each file's name, size
/// and time of last change. Another stamp means a file changed, or one was
/// added or removed.
type Stamp = Vec<(String, u64, Option<SystemTime>)>;

/// The CSV catalog's IPNs, read again when its files change.
pub struct CsvIpns {
    root: PathBuf,
    last: Mutex<Option<(Stamp, Arc<IpnsInCsv>)>>,
}

impl CsvIpns {
    /// The IPNs of the CSV catalog under `root` (which holds `database/`).
    /// Nothing is read until they are asked for.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            last: Mutex::new(None),
        }
    }

    /// The IPNs as the files are now, read again only if a file changed
    /// since the last reading. An error says, in plain words, which file
    /// could not be read.
    pub fn now(&self) -> Result<Arc<IpnsInCsv>, String> {
        let stamp = self.stamp()?;
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((read_at, ipns)) = last.as_ref() {
            if *read_at == stamp {
                return Ok(Arc::clone(ipns));
            }
        }
        let ipns = Arc::new(self.read(&stamp)?);
        *last = Some((stamp, Arc::clone(&ipns)));
        Ok(ipns)
    }

    /// The IPNs as the files are now, always read again.
    pub fn read_fresh(&self) -> Result<Arc<IpnsInCsv>, String> {
        let stamp = self.stamp()?;
        let ipns = Arc::new(self.read(&stamp)?);
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some((stamp, Arc::clone(&ipns)));
        Ok(ipns)
    }

    fn database(&self) -> PathBuf {
        self.root.join("database")
    }

    /// The stamp of every `g-*.csv` file, in name order. A folder with no
    /// `database` folder has no CSV catalog, so no IPNs.
    fn stamp(&self) -> Result<Stamp, String> {
        let database = self.database();
        let entries = match fs::read_dir(&database) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(could_not_read("the database folder", &error.to_string())),
        };
        let mut stamp = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| could_not_read("the database folder", &error.to_string()))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !(name.starts_with("g-") && name.ends_with(".csv")) {
                continue;
            }
            let metadata = entry
                .metadata()
                .map_err(|error| could_not_read(&csv_path(&name), &error.to_string()))?;
            if metadata.is_file() {
                stamp.push((name, metadata.len(), metadata.modified().ok()));
            }
        }
        stamp.sort();
        Ok(stamp)
    }

    fn read(&self, stamp: &Stamp) -> Result<IpnsInCsv, String> {
        let mut found = IpnsInCsv::default();
        for (name, _, _) in stamp {
            let path = csv_path(name);
            let ipns = csv_catalog::table_ipns(&self.database().join(name))
                .map_err(|error| could_not_read(&path, &error.to_string()))?;
            for ipn in ipns {
                found.add(&ipn, &path);
            }
        }
        Ok(found)
    }
}

/// The repository path of the CSV file `name`, such as `database/g-res.csv`.
fn csv_path(name: &str) -> String {
    format!("database/{name}")
}

fn could_not_read(what: &str, why: &str) -> String {
    format!(
        "The app could not read {what} of the CSV catalog, so it cannot check which IPNs are free: {why}. Fix the file, then try again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    /// A CSV catalog folder under the workspace's `target/` folder.
    fn catalog(files: &[(&str, &str)]) -> tempfile::TempDir {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        fs::create_dir_all(&target).unwrap();
        let root = tempfile::Builder::new()
            .prefix("csv-ipns-")
            .tempdir_in(target)
            .unwrap();
        fs::create_dir(root.path().join("database")).unwrap();
        for (name, text) in files {
            fs::write(root.path().join("database").join(name), text).unwrap();
        }
        root
    }

    const RES: &str = "\"IPN\",\"MPN\"\n\"RES-0000-0470\",\"RC0603FR-0747RL\"\n";
    const CAP: &str = "IPN,MPN\nCAP-0000-0020,CL10C020CB8NNNC\n";

    #[test]
    fn reads_each_ipn_with_its_file() {
        let root = catalog(&[
            ("g-res.csv", RES),
            ("g-cap.csv", CAP),
            ("substitutes.csv", "IPN,MPN\nRES-0000-9999,X\n"),
        ]);
        let ipns = CsvIpns::new(root.path()).now().unwrap();
        assert_eq!(
            ipns.ipns().collect::<Vec<_>>(),
            ["CAP-0000-0020", "RES-0000-0470"]
        );
        assert_eq!(ipns.file_of("RES-0000-0470"), Some("database/g-res.csv"));
        // substitutes.csv names parts; it gives none an IPN.
        assert!(!ipns.contains("RES-0000-9999"));
    }

    #[test]
    fn a_folder_without_a_csv_catalog_has_no_ipns() {
        let root = catalog(&[]);
        fs::remove_dir(root.path().join("database")).unwrap();
        assert_eq!(
            *CsvIpns::new(root.path()).now().unwrap(),
            IpnsInCsv::default()
        );
    }

    #[test]
    fn a_row_added_on_disk_is_seen_by_the_next_reading() {
        let root = catalog(&[("g-res.csv", RES)]);
        let ipns = CsvIpns::new(root.path());
        assert!(!ipns.now().unwrap().contains("RES-0005-0001"));
        // Someone saves a row, or git pull brings one.
        let file = root.path().join("database/g-res.csv");
        fs::write(&file, format!("{RES}\"RES-0005-0001\",\"ERJ-3EKF1001V\"\n")).unwrap();
        assert!(ipns.now().unwrap().contains("RES-0005-0001"));
        // A new file is seen too.
        fs::write(
            root.path().join("database/g-ind.csv"),
            "IPN\nIND-0001-0001\n",
        )
        .unwrap();
        assert_eq!(
            ipns.now().unwrap().file_of("IND-0001-0001"),
            Some("database/g-ind.csv")
        );
    }

    #[test]
    fn saving_reads_again_even_when_a_change_kept_the_size_and_time() {
        let root = catalog(&[("g-res.csv", RES)]);
        let ipns = CsvIpns::new(root.path());
        assert!(ipns.now().unwrap().contains("RES-0000-0470"));
        let file = root.path().join("database/g-res.csv");
        let changed_at = fs::metadata(&file).unwrap().modified().unwrap();
        // The same number of bytes, and the old time put back.
        fs::write(&file, RES.replace("RES-0000-0470", "RES-0000-0471")).unwrap();
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(changed_at)
            .unwrap();
        // Between saves, the last reading is used: nothing on disk says
        // the file changed.
        assert!(ipns.now().unwrap().contains("RES-0000-0470"));
        // Saving does not rely on that.
        let fresh = ipns.read_fresh().unwrap();
        assert!(fresh.contains("RES-0000-0471"));
        assert!(!fresh.contains("RES-0000-0470"));
        assert!(ipns.now().unwrap().contains("RES-0000-0471"));
    }

    #[test]
    fn a_file_that_cannot_be_read_is_named() {
        let root = catalog(&[("g-res.csv", RES)]);
        // Not UTF-8.
        fs::write(root.path().join("database/g-cap.csv"), b"IPN\n\xff\xfe\n").unwrap();
        let error = CsvIpns::new(root.path()).now().unwrap_err();
        assert!(
            error.starts_with("The app could not read database/g-cap.csv of the CSV catalog"),
            "{error}"
        );
    }
}

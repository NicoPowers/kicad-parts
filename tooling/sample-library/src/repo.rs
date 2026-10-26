//! A throwaway git repo of the sample library.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A copy of the sample library, committed as the first commit of its own
/// git repo, in a temporary folder under `target/test-folders`. The folder
/// is deleted when this is dropped.
///
/// git reads only an empty settings file in the folder, never the
/// developer's settings, hooks or signing (mx-044140), as
/// `Git::isolated` does in catalog-browser.
pub struct SampleRepo {
    folder: tempfile::TempDir,
    path: PathBuf,
}

impl SampleRepo {
    /// A new repo holding the sample library at its first commit, on the
    /// branch `main`.
    pub fn new() -> Self {
        Self::of(&crate::root())
    }

    /// A new repo holding a copy of the library folder `library`.
    pub fn of(library: &Path) -> Self {
        let scratch = native_services::repository_root()
            .join("target")
            .join("test-folders");
        fs::create_dir_all(&scratch).expect("target/test-folders can be made");
        let folder = tempfile::Builder::new()
            .prefix("sample-library-")
            .tempdir_in(&scratch)
            .expect("a folder under target/test-folders");
        let path = folder.path().join("library");
        copy_folder(library, &path);
        fs::write(folder.path().join("gitconfig"), "").expect("an empty git settings file");
        let repo = Self { folder, path };
        repo.git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        repo.commit_all("The sample library");
        repo
    }

    /// The repo's top folder, which holds the library.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder this repo lives in, for other scratch files of a test.
    pub fn scratch(&self) -> &Path {
        self.folder.path()
    }

    /// Runs git in the repo and gives what it printed, trimmed; panics if
    /// git fails.
    pub fn git(&self, args: &[&str]) -> String {
        git_isolated(&self.path, &self.folder.path().join("gitconfig"), args)
    }

    /// Commits every file of the folder as it is, and gives the commit ID.
    pub fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Writes `text` to the repo file `path` (with `/`), making folders.
    pub fn write(&self, path: &str, text: impl AsRef<[u8]>) {
        let file = self.path.join(path);
        fs::create_dir_all(file.parent().expect("a file in a folder")).expect("folders made");
        fs::write(file, text).expect("the file is written");
    }

    /// The text of the repo file `path` (with `/`).
    pub fn text(&self, path: &str) -> String {
        fs::read_to_string(self.path.join(path)).expect("the file is text")
    }
}

impl Default for SampleRepo {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs git in `dir` with `settings` as its only settings file, a fixture
/// identity, no hooks, no signing and no line-ending changes, without a
/// window. Gives what git printed, trimmed; panics if git fails.
pub fn git_isolated(dir: &Path, settings: &Path, args: &[&str]) -> String {
    let hooks = format!(
        "core.hooksPath={}",
        dir.join(".git").join("no-hooks").display()
    );
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            hooks.as_str(),
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", settings)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    let output = native_services::run_hidden("git", &mut command).expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn copy_folder(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("the folder is made");
    for entry in fs::read_dir(from).expect("the folder is readable") {
        let entry = entry.expect("the entry is readable");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("the file is copied");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repo_holds_the_library_at_one_commit_and_is_deleted_after() {
        let repo = SampleRepo::new();
        let top = repo.path().to_path_buf();
        assert!(top.join("symbols/g-res.kicad_sym").is_file());
        assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert_eq!(repo.git(&["branch", "--show-current"]), "main");
        drop(repo);
        assert!(!top.exists());
    }
}

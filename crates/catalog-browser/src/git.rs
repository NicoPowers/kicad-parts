//! Which commit files of the CSV catalog folder were read from, for Add
//! part from our libraries (kicad-parts-37a7): the Catalog records the
//! commit each file came from, so a file must be committed exactly as it
//! is. The same comparison as the CSV import's commit-first rule
//! (tooling/catalog-import services::read_source_commit): the files' bytes,
//! hashed by git without filters, against `HEAD`'s.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

/// The git program the app runs.
const GIT: &str = "git";

/// The commit (`HEAD`, its full ID) that holds every one of `paths`
/// (repository paths under `root`, such as `symbols/g-ics.kicad_sym`)
/// exactly as they are now, or why there is none, in plain words.
pub(crate) fn commit_holding(root: &Path, paths: &[&str]) -> Result<String, String> {
    commit_holding_with(GIT, root, paths)
}

fn commit_holding_with(program: &str, root: &Path, paths: &[&str]) -> Result<String, String> {
    let not_a_repository = || {
        "The CSV catalog folder is not the top folder of a git repository, so the Catalog cannot record which commit the files came from. Add the part from a checkout of the CSV catalog.".to_owned()
    };
    let top =
        git(program, root, &["rev-parse", "--show-toplevel"])?.ok_or_else(not_a_repository)?;
    if !same_folder(Path::new(&top), root) {
        return Err(not_a_repository());
    }
    let commit = git(program, root, &["rev-parse", "--verify", "-q", "HEAD"])?
        .filter(|commit| commit.len() == 40 || commit.len() == 64)
        .ok_or_else(|| {
            "The CSV catalog folder has no commit yet. Commit the files first: the Catalog records the commit each file came from.".to_owned()
        })?;
    if paths.is_empty() {
        return Ok(commit);
    }
    let mut committed: BTreeMap<String, String> = BTreeMap::new();
    // Paths are taken literally, never as patterns.
    let mut args = vec!["--literal-pathspecs", "ls-tree", "-r", "-z", "HEAD", "--"];
    args.extend_from_slice(paths);
    let listed = git(program, root, &args)?.ok_or_else(not_a_repository)?;
    // Each entry is `<mode> <type> <object ID>`, a tab and the path, ending
    // in a NUL.
    for entry in listed.split('\0') {
        if let Some((about, path)) = entry.split_once('\t') {
            if let Some(object) = about.split(' ').nth(2) {
                committed.insert(path.to_owned(), object.to_owned());
            }
        }
    }
    let mut args = vec!["hash-object", "--no-filters", "--"];
    args.extend_from_slice(paths);
    let hashed = git(program, root, &args)?.ok_or_else(not_a_repository)?;
    let objects: Vec<&str> = hashed.lines().collect();
    if objects.len() != paths.len() {
        return Err(not_a_repository());
    }
    let changed: Vec<&str> = paths
        .iter()
        .zip(objects)
        .filter(|(path, object)| committed.get(**path).map(String::as_str) != Some(*object))
        .map(|(path, _)| *path)
        .collect();
    match changed.as_slice() {
        [] => Ok(commit),
        [one] => Err(format!(
            "{one} is not committed as it is. Commit it first, then choose it again: the Catalog records the commit each file came from."
        )),
        many => Err(format!(
            "{} are not committed as they are. Commit them first, then choose them again: the Catalog records the commit each file came from.",
            many.join(", ")
        )),
    }
}

/// Runs git in `root` without a window or a prompt: its standard output
/// without the whitespace at its end when it succeeds, `None` when it
/// fails, or why it could not be started.
fn git(program: &str, root: &Path, args: &[&str]) -> Result<Option<String>, String> {
    let mut command = Command::new(program);
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a GUI app must not flash a console.
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|_| {
        "git was not found, so the app cannot tell which commit the files came from. Install git, then try again.".to_owned()
    })?;
    Ok(output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    }))
}

/// Whether `a` and `b` name the same folder.
fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture_git(root: &Path, args: &[&str]) {
        // No hooks of this computer run in the fixture.
        let hooks = format!("core.hooksPath={}", root.join(".git/no-hooks").display());
        let mut all = vec![
            "-c",
            hooks.as_str(),
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ];
        all.extend_from_slice(args);
        assert!(git(GIT, root, &all).unwrap().is_some(), "git {args:?}");
    }

    #[test]
    fn only_files_committed_as_they_are_have_a_commit() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        fs::create_dir_all(root.join("symbols")).unwrap();
        fs::write(root.join("symbols/g-a.kicad_sym"), "a\n").unwrap();
        let refused = commit_holding(root, &["symbols/g-a.kicad_sym"]).unwrap_err();
        assert!(
            refused.contains("not the top folder of a git repository"),
            "{refused}"
        );
        fixture_git(root, &["init", "-q"]);
        let refused = commit_holding(root, &["symbols/g-a.kicad_sym"]).unwrap_err();
        assert!(refused.contains("has no commit yet"), "{refused}");
        fixture_git(root, &["add", "symbols/g-a.kicad_sym"]);
        fixture_git(root, &["commit", "-q", "-m", "fixture"]);
        let commit = commit_holding(root, &["symbols/g-a.kicad_sym"]).unwrap();
        assert_eq!(commit.len(), 40);
        // Changed, staged or new: not committed as it is.
        fs::write(root.join("symbols/g-a.kicad_sym"), "changed\n").unwrap();
        let refused = commit_holding(root, &["symbols/g-a.kicad_sym"]).unwrap_err();
        assert_eq!(
            refused,
            "symbols/g-a.kicad_sym is not committed as it is. Commit it first, then choose it again: the Catalog records the commit each file came from."
        );
        fixture_git(root, &["add", "symbols/g-a.kicad_sym"]);
        assert!(commit_holding(root, &["symbols/g-a.kicad_sym"]).is_err());
        fs::write(root.join("symbols/g-b.kicad_sym"), "b\n").unwrap();
        let refused =
            commit_holding(root, &["symbols/g-a.kicad_sym", "symbols/g-b.kicad_sym"]).unwrap_err();
        assert!(
            refused.starts_with("symbols/g-a.kicad_sym, symbols/g-b.kicad_sym are not committed"),
            "{refused}"
        );
        // A folder inside the repository is not its top folder.
        assert!(commit_holding(&root.join("symbols"), &[]).is_err());
        // No git: said plainly.
        let missing = commit_holding_with("no-such-git-program", root, &[]).unwrap_err();
        assert!(missing.starts_with("git was not found"), "{missing}");
    }
}

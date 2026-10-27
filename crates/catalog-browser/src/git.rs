//! Which commit files of the local library folder were read from, for Add
//! part from our libraries (kicad-parts-37a7): the Catalog records the
//! commit each file came from, so a file must be committed exactly as it
//! is: the bytes, as git would store them without filters, against
//! `HEAD`'s.
//!
//! The bytes compared are the ones the app already read and will send:
//! their git object ID is worked out here, so a file saved again in KiCad
//! after it was read can never slip through as committed.

use crate::git_program::{Git, NotInstalled};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// The git program the app runs.
const GIT: &str = "git";

/// The commit (`HEAD`, its full ID) that holds every one of `files` (each a
/// repository path under `root`, such as `symbols/g-ics.kicad_sym`, and the
/// bytes read from it) exactly as given, or why there is none, in plain
/// words.
pub(crate) fn commit_holding(root: &Path, files: &[(&str, &[u8])]) -> Result<String, String> {
    commit_holding_with(GIT, root, files)
}

fn commit_holding_with(
    program: &str,
    root: &Path,
    files: &[(&str, &[u8])],
) -> Result<String, String> {
    let not_a_repository = || {
        "The local library folder is not the top folder of a git repository, so the Catalog cannot record which commit the files came from. Add the part from a checkout of the team's library repo.".to_owned()
    };
    let top = match git(program, root, &["rev-parse", "--show-toplevel"])? {
        Ok(top) => top,
        Err(reason) if reason.contains("not a git repository") => {
            return Err(not_a_repository());
        }
        Err(reason) => return Err(could_not_read(&reason)),
    };
    if !same_folder(Path::new(&top), root) {
        return Err(not_a_repository());
    }
    let commit = git(program, root, &["rev-parse", "--verify", "-q", "HEAD"])?
        .ok()
        .filter(|commit| commit.len() == 40 || commit.len() == 64)
        .ok_or_else(|| {
            "The local library folder has no commit yet. Commit the files first: the Catalog records the commit each file came from.".to_owned()
        })?;
    if files.is_empty() {
        return Ok(commit);
    }
    // Older git does not know the option and has only SHA-1 repositories.
    let sha256 = git(program, root, &["rev-parse", "--show-object-format"])?
        .is_ok_and(|format| format == "sha256");
    let mut committed: BTreeMap<String, String> = BTreeMap::new();
    // Paths are taken literally, never as patterns.
    let mut args = vec!["--literal-pathspecs", "ls-tree", "-r", "-z", "HEAD", "--"];
    args.extend(files.iter().map(|(path, _)| *path));
    let listed = git(program, root, &args)?.map_err(|reason| could_not_read(&reason))?;
    // Each entry is `<mode> <type> <object ID>`, a tab and the path, ending
    // in a NUL.
    for entry in listed.split('\0') {
        if let Some((about, path)) = entry.split_once('\t') {
            if let Some(object) = about.split(' ').nth(2) {
                committed.insert(path.to_owned(), object.to_owned());
            }
        }
    }
    let changed: Vec<(&str, &[u8])> = files
        .iter()
        .filter(|(path, bytes)| {
            committed.get(*path).map(String::as_str) != Some(&object_id(bytes, sha256))
        })
        .copied()
        .collect();
    let named: Vec<&str> = changed.iter().map(|(path, _)| *path).collect();
    let mut problem = match named.as_slice() {
        [] => return Ok(commit),
        [one] => format!(
            "{one} is not committed as it is. Commit it first, then choose it again: the Catalog records the commit each file came from."
        ),
        many => format!(
            "{} are not committed as they are. Commit them first, then choose them again: the Catalog records the commit each file came from.",
            many.join(", ")
        ),
    };
    // A repository that changes line endings on commit can never hold
    // KiCad's file as it is, however often it is committed.
    if changed
        .iter()
        .any(|(path, bytes)| converts_line_endings(program, root, path, bytes))
    {
        problem.push_str(
            " This repository changes line endings when files are committed (git's core.autocrlf), so a file with Windows line endings never matches its commit. Add \"/symbols/** -text\", \"/footprints/** -text\" and \"/3d-models/** -text\" to .gitattributes, then commit the files again.",
        );
    }
    Err(problem)
}

/// "git could not read the local library folder: <git's reason>".
fn could_not_read(reason: &str) -> String {
    let reason = reason
        .trim()
        .trim_start_matches("fatal: ")
        .trim_end_matches('.');
    format!("git could not read the local library folder: {reason}.")
}

/// Whether git would change `bytes`' line endings when committing `path`:
/// they have Windows line endings, `core.autocrlf` is on, and no
/// `.gitattributes` rule turns text conversion off for the path.
fn converts_line_endings(program: &str, root: &Path, path: &str, bytes: &[u8]) -> bool {
    if !bytes.windows(2).any(|pair| pair == b"\r\n") {
        return false;
    }
    let autocrlf = git(program, root, &["config", "core.autocrlf"])
        .ok()
        .and_then(Result::ok)
        .is_some_and(|value| value == "true" || value == "input");
    let text_off = git(
        program,
        root,
        &["--literal-pathspecs", "check-attr", "text", "--", path],
    )
    .ok()
    .and_then(Result::ok)
    .is_some_and(|line| line.ends_with(": unset"));
    autocrlf && !text_off
}

/// The ID git gives `bytes` stored as a file (a blob), in a SHA-1 or a
/// SHA-256 repository.
fn object_id(bytes: &[u8], sha256: bool) -> String {
    let mut object = format!("blob {}\0", bytes.len()).into_bytes();
    object.extend_from_slice(bytes);
    if sha256 {
        hex(&Sha256::digest(&object))
    } else {
        hex(&sha1(&object))
    }
}

/// `bytes` as lower-case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// SHA-1, only to name git objects the way git does (not for security).
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (kept, add) in h.iter_mut().zip([a, b, c, d, e]) {
            *kept = kept.wrapping_add(add);
        }
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// How long one local git step for the local library folder may take.
const LOCAL_STEP: Duration = Duration::from_secs(120);

/// Runs git in `root` without a window or a prompt (see [`Git::run`]): its
/// standard output without the whitespace at its end when it succeeds,
/// otherwise the first line of what it said went wrong; or why it could
/// not be started.
fn git(program: &str, root: &Path, args: &[&str]) -> Result<Result<String, String>, String> {
    let ran = Git::named(program)
        .run(Some(root), args, LOCAL_STEP)
        .map_err(|NotInstalled| {
            "git was not found, so the app cannot tell which commit the files came from. Install git, then try again.".to_owned()
        })?;
    Ok(if ran.succeeded {
        Ok(ran.stdout_text().trim_end().to_owned())
    } else if ran.timed_out {
        Err("it did not finish in time".to_owned())
    } else {
        Err(ran.first_problem_line().to_owned())
    })
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

    fn fixture_git(root: &Path, args: &[&str]) -> String {
        fixture_git_with(root, &["core.autocrlf=false"], args)
    }

    /// git in `root` with a fixture identity, no hooks of this computer and
    /// no signing, and the `config` settings given.
    fn fixture_git_with(root: &Path, config: &[&str], args: &[&str]) -> String {
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
        ];
        for setting in config {
            all.extend(["-c", setting]);
        }
        all.extend_from_slice(args);
        git(GIT, root, &all)
            .unwrap()
            .unwrap_or_else(|why| panic!("git {args:?}: {why}"))
    }

    #[test]
    fn object_ids_are_git_s() {
        // `git hash-object` of an empty file and of "hello\n".
        assert_eq!(
            object_id(b"", false),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        assert_eq!(
            object_id(b"hello\n", false),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(
            object_id(b"", true),
            "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"
        );
        // A message longer than one SHA-1 block.
        assert_eq!(
            hex(&sha1(&[b'a'; 1000])),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
    }

    #[test]
    fn only_files_committed_as_they_are_have_a_commit() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        fs::create_dir_all(root.join("symbols")).unwrap();
        fs::write(root.join("symbols/g-a.kicad_sym"), "a\n").unwrap();
        let file = |bytes: &'static [u8]| [("symbols/g-a.kicad_sym", bytes)];
        let refused = commit_holding(root, &file(b"a\n")).unwrap_err();
        assert!(
            refused.contains("not the top folder of a git repository"),
            "{refused}"
        );
        fixture_git(root, &["init", "-q"]);
        let refused = commit_holding(root, &file(b"a\n")).unwrap_err();
        assert!(refused.contains("has no commit yet"), "{refused}");
        fixture_git(root, &["add", "symbols/g-a.kicad_sym"]);
        fixture_git(root, &["commit", "-q", "-m", "fixture"]);
        let commit = commit_holding(root, &file(b"a\n")).unwrap();
        assert_eq!(commit, fixture_git(root, &["rev-parse", "HEAD"]));
        // The bytes read are what count: a file saved again after it was
        // read, even with the committed bytes on disk, is not committed as
        // read.
        let refused = commit_holding(root, &file(b"saved again\n")).unwrap_err();
        assert_eq!(
            refused,
            "symbols/g-a.kicad_sym is not committed as it is. Commit it first, then choose it again: the Catalog records the commit each file came from."
        );
        // Staged but not committed is still not the commit.
        fs::write(root.join("symbols/g-a.kicad_sym"), "changed\n").unwrap();
        fixture_git(root, &["add", "symbols/g-a.kicad_sym"]);
        assert!(commit_holding(root, &file(b"changed\n")).is_err());
        let refused = commit_holding(
            root,
            &[
                ("symbols/g-a.kicad_sym", b"changed\n".as_slice()),
                ("symbols/g-b.kicad_sym", b"new\n".as_slice()),
            ],
        )
        .unwrap_err();
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

    #[test]
    fn git_s_own_reason_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        fixture_git(root, &["init", "-q"]);
        fs::write(
            root.join("a.txt"),
            "a
",
        )
        .unwrap();
        fixture_git(root, &["add", "a.txt"]);
        fixture_git(root, &["commit", "-q", "-m", "fixture"]);
        // The commit's tree is gone: git says why, and so does the app,
        // rather than calling the folder no repository.
        let tree = fixture_git(root, &["rev-parse", "HEAD^{tree}"]);
        let object = root.join(".git/objects").join(&tree[..2]).join(&tree[2..]);
        // git keeps its objects read-only.
        let mut permissions = fs::metadata(&object).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&object, permissions).unwrap();
        fs::remove_file(&object).unwrap();
        let refused = commit_holding(
            root,
            &[(
                "a.txt",
                b"a
"
                .as_slice(),
            )],
        )
        .unwrap_err();
        assert!(
            refused.starts_with("git could not read the local library folder: "),
            "{refused}"
        );
        assert_eq!(
            could_not_read("fatal: detected dubious ownership in repository at 'C:/x'"),
            "git could not read the local library folder: detected dubious ownership in repository at 'C:/x'."
        );
    }

    #[test]
    fn a_repository_that_changes_line_endings_is_said_with_what_to_do() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        fixture_git(root, &["init", "-q"]);
        fs::create_dir_all(root.join("footprints")).unwrap();
        fs::write(root.join("footprints/F.kicad_mod"), "a\r\nb\r\n").unwrap();
        fixture_git(root, &["config", "core.autocrlf", "true"]);
        fixture_git_with(root, &[], &["add", "footprints/F.kicad_mod"]);
        fixture_git_with(root, &[], &["commit", "-q", "-m", "fixture"]);
        let refused = commit_holding(
            root,
            &[("footprints/F.kicad_mod", b"a\r\nb\r\n".as_slice())],
        )
        .unwrap_err();
        assert!(
            refused.contains("changes line endings when files are committed")
                && refused.contains("/footprints/** -text"),
            "{refused}"
        );
        // With the rule, the same bytes commit as they are.
        fs::write(root.join(".gitattributes"), "/footprints/** -text\n").unwrap();
        fixture_git_with(root, &[], &["add", ".gitattributes"]);
        fixture_git_with(
            root,
            &[],
            &["add", "--renormalize", "footprints/F.kicad_mod"],
        );
        fixture_git_with(root, &[], &["commit", "-q", "-m", "keep line endings"]);
        assert!(
            commit_holding(
                root,
                &[("footprints/F.kicad_mod", b"a\r\nb\r\n".as_slice())]
            )
            .is_ok()
        );
    }
}

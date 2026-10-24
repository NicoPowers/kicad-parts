//! Library repos end to end, with bare repos in folders on this computer
//! only. git reads only an empty settings file of the test's own, so the
//! developer's settings, SSH keys and git hosts are never used.

use super::*;
use crate::teams::SignIns;
use std::path::Path;
use std::time::{Duration, Instant};

const ACME: &str = "https://acme.example.test";
const BETA: &str = "https://beta.example.test";
const SYMBOLS: &str = "(kicad_symbol_lib (version 20241209))\n";

struct Fixture {
    dir: tempfile::TempDir,
    git: Git,
    teams: Arc<Teams>,
}

impl Fixture {
    /// Teams ACME and BETA, signed out, with no library repo yet.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let git = Git::isolated(&dir.path().join("gitconfig"));
        let list = dir.path().join("config").join("team.json");
        fs::create_dir_all(list.parent().unwrap()).unwrap();
        fs::write(
            &list,
            format!(
                r#"{{"teams":[
                    {{"server_url":"{ACME}","email":"ada@example.test","code":"AC","colour":"blue","signed_out":true}},
                    {{"server_url":"{BETA}","email":"ada@example.test","code":"BE","colour":"green","signed_out":true}}
                ]}}"#
            ),
        )
        .unwrap();
        let teams = Arc::new(Teams::open(
            list,
            dir.path().join("parts-files"),
            SignIns::until_closed("tests keep no sign-ins"),
            None,
            None,
        ));
        Self { dir, git, teams }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn repos_folder(&self) -> PathBuf {
        self.path("library-repos")
    }

    fn repos(&self) -> LibraryRepos {
        self.repos_with(self.git.clone(), Limits::default())
    }

    fn repos_with(&self, git: Git, limits: Limits) -> LibraryRepos {
        LibraryRepos::start_with(self.repos_folder(), self.teams.clone(), git, limits)
    }

    /// Runs git for the fixture, with an identity and no signing.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let mut all = vec![
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "init.defaultBranch=main",
        ];
        all.extend_from_slice(args);
        let ran = self
            .git
            .run(Some(dir), &all, Duration::from_secs(60))
            .unwrap();
        assert!(ran.succeeded, "git {args:?}: {}", ran.stderr);
        ran.stdout_text().trim().to_owned()
    }

    /// A bare repo, as on a git host, holding one commit with a symbol
    /// library.
    fn host_repo(&self, name: &str) -> PathBuf {
        let bare = self.path(name);
        fs::create_dir_all(&bare).unwrap();
        self.git(&bare, &["init", "-q", "--bare"]);
        let work = self.path(&format!("{name}-work"));
        fs::create_dir_all(work.join("symbols")).unwrap();
        fs::write(work.join("symbols/Acme.kicad_sym"), SYMBOLS).unwrap();
        self.git(&work, &["init", "-q"]);
        self.git(&work, &["add", "."]);
        self.git(&work, &["commit", "-q", "-m", "First symbols"]);
        let bare_text = bare.display().to_string();
        self.git(&work, &["push", "-q", &bare_text, "HEAD:main"]);
        bare
    }

    /// The person's own clone of `bare`.
    fn persons_clone(&self, bare: &Path, name: &str) -> PathBuf {
        let clone = self.path(name);
        let (bare_text, clone_text) = (bare.display().to_string(), clone.display().to_string());
        self.git(self.dir.path(), &["clone", "-q", &bare_text, &clone_text]);
        clone
    }

    /// The library repo of `team` as its row shows it.
    fn status(&self, repos: &LibraryRepos, team: &str) -> Option<LibraryRepoStatus> {
        let mut status = self.teams.status();
        repos.note_status(&mut status);
        status
            .teams
            .into_iter()
            .find(|row| row.server_url == team)
            .and_then(|row| row.library_repo)
    }

    /// The copies in the app's folder.
    fn copies(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.repos_folder())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn summary(problem: &RepoProblem) -> &str {
    &problem.summary
}

#[test]
fn a_team_links_its_repo_from_the_persons_clone_and_can_read_and_write() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let clone = fixture.persons_clone(&bare, "my-checkout");
    let index_before = fs::metadata(clone.join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    let repos = fixture.repos();
    assert_eq!(
        repos.link(ACME, &clone.display().to_string()),
        Ok(Linked::Done)
    );
    repos.wait_until_idle();
    let status = fixture.status(&repos, ACME).unwrap();
    assert_eq!(status.access, RepoAccess::CanReadAndWrite);
    assert_eq!(status.name, "client-library");
    assert_eq!(status.host, "this computer");
    assert!(status.copied_at.is_some());
    // The link holds the clone's origin, never the clone.
    let origin = fixture.git(&clone, &["remote", "get-url", "origin"]);
    assert_eq!(status.address, origin);
    assert!(fixture.status(&repos, BETA).is_none());
    // The app copied the repo into a folder of its own, and never wrote
    // in the person's clone (mx-70700f).
    let address = RepoAddress::parse(&origin).unwrap();
    assert_eq!(fixture.copies(), [address.folder_name()]);
    assert_eq!(
        fs::metadata(clone.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap(),
        index_before
    );
    assert_eq!(fixture.git(&clone, &["status", "--porcelain"]), "");
    // The access check created no branch on the host.
    assert_eq!(fixture.git(&bare, &["branch", "--list"]), "* main");
    // The link is kept for the next start.
    let saved = fs::read_to_string(fixture.path("config/team.json")).unwrap();
    assert!(saved.contains("\"library_repo\""), "{saved}");
}

#[test]
fn a_team_links_its_repo_from_an_address() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let repos = fixture.repos();
    let url = format!(
        "file:///{}",
        bare.display()
            .to_string()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    assert_eq!(repos.link(ACME, &url), Ok(Linked::Done));
    repos.wait_until_idle();
    let status = fixture.status(&repos, ACME).unwrap();
    assert_eq!(status.access, RepoAccess::CanReadAndWrite);
    assert_eq!(status.address, url);
    // A bare repo's folder is its own address.
    assert_eq!(
        repos.link(BETA, &bare.display().to_string()),
        Ok(Linked::Done)
    );
    repos.wait_until_idle();
    assert_eq!(
        fixture.status(&repos, BETA).unwrap().access,
        RepoAccess::CanReadAndWrite
    );
}

#[test]
fn the_app_reads_files_from_its_copy_while_the_host_is_gone() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let repos = fixture.repos();
    repos.link(ACME, &bare.display().to_string()).unwrap();
    repos.wait_until_idle();
    let repo = repos.repo_for(ACME).unwrap();
    let commit = repo.latest_commit().unwrap().unwrap();
    let files = repo.list_files(&commit).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "symbols/Acme.kicad_sym");
    assert_eq!(files[0].size, SYMBOLS.len() as u64);
    // Many files in one git run, a missing one as None.
    assert_eq!(
        repo.read_files(&commit, &["symbols/Acme.kicad_sym", "nope.txt", "symbols"])
            .unwrap(),
        [Some(SYMBOLS.as_bytes().to_vec()), None, None]
    );
    // Only whole commit IDs are read.
    assert!(repo.read_file("--output=x", "a").is_err());
    assert_eq!(
        repo.read_file(&commit, "symbols/Acme.kicad_sym").unwrap(),
        SYMBOLS.as_bytes()
    );
    // The host goes away: checking again says so, and reading still works.
    fs::rename(&bare, fixture.path("moved.git")).unwrap();
    repos.check_again(ACME);
    repos.wait_until_idle();
    let status = fixture.status(&repos, ACME).unwrap();
    let RepoAccess::CannotRead { problem } = &status.access else {
        panic!("{status:?}");
    };
    assert!(
        summary(problem).starts_with("Can't read the repo: the repo was not found."),
        "{problem:?}"
    );
    assert!(status.copied_at.is_some());
    assert_eq!(
        repo.read_file(&commit, "symbols/Acme.kicad_sym").unwrap(),
        SYMBOLS.as_bytes()
    );
    // A file the commit does not hold is said plainly.
    let missing = repo
        .read_file(&commit, "symbols/Other.kicad_sym")
        .unwrap_err();
    assert!(
        missing
            .summary
            .starts_with("Can't read symbols/Other.kicad_sym at commit"),
        "{missing:?}"
    );
}

#[test]
fn fetching_brings_the_copy_up_to_date() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let repos = fixture.repos();
    repos.link(ACME, &bare.display().to_string()).unwrap();
    repos.wait_until_idle();
    let repo = repos.repo_for(ACME).unwrap();
    let first = repo.latest_commit().unwrap().unwrap();
    // A teammate pushes a footprint.
    let work = fixture.path("client-library.git-work");
    fs::create_dir_all(work.join("footprints/Acme.pretty")).unwrap();
    fs::write(
        work.join("footprints/Acme.pretty/R.kicad_mod"),
        "(footprint \"R\")\n",
    )
    .unwrap();
    fixture.git(&work, &["add", "."]);
    fixture.git(&work, &["commit", "-q", "-m", "A footprint"]);
    let bare_text = bare.display().to_string();
    fixture.git(&work, &["push", "-q", &bare_text, "HEAD:main"]);
    repos.check_again(ACME);
    repos.wait_until_idle();
    let second = repo.latest_commit().unwrap().unwrap();
    assert_ne!(first, second);
    assert_eq!(repo.list_files(&second).unwrap().len(), 2);
    // The first commit can still be read: e9a3 pins one.
    assert_eq!(repo.list_files(&first).unwrap().len(), 1);
}

#[test]
fn a_repo_that_refuses_pushes_can_only_be_read() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    // Hosts check write access when a push starts, as this one does.
    let refuse = fixture.path("refuse-push.sh");
    fs::write(
        &refuse,
        "#!/bin/sh\necho \"ERROR: Permission to acme/client-library.git denied to ada.\" >&2\nexit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&refuse, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let git = fixture
        .git
        .clone()
        .with_env("GIT_CONFIG_COUNT", "1")
        .with_env("GIT_CONFIG_KEY_0", "remote.origin.receivepack")
        .with_env(
            "GIT_CONFIG_VALUE_0",
            refuse.display().to_string().replace('\\', "/"),
        );
    let repos = fixture.repos_with(git, Limits::default());
    assert_eq!(
        repos.link(ACME, &bare.display().to_string()),
        Ok(Linked::Done)
    );
    repos.wait_until_idle();
    let status = fixture.status(&repos, ACME).unwrap();
    let RepoAccess::CanReadOnly { problem } = &status.access else {
        panic!("{status:?}");
    };
    assert_eq!(
        summary(problem),
        "Can read, but can't push: permission denied. Check that you may change files in the repo's folder."
    );
    assert!(problem.details.contains("denied to ada"), "{problem:?}");
}

#[test]
fn what_cannot_be_linked_is_said_plainly() {
    let fixture = Fixture::new();
    let repos = fixture.repos();
    let refused = |place: &str| repos.link(ACME, place).unwrap_err().summary;
    let nowhere = fixture.path("nowhere");
    assert!(
        refused(&nowhere.display().to_string()).starts_with("There is no folder"),
        "{}",
        refused(&nowhere.display().to_string())
    );
    let missing = format!(
        "file:///{}/missing.git",
        fixture
            .dir
            .path()
            .display()
            .to_string()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    assert!(
        refused(&missing).starts_with("There is no folder file:///"),
        "{}",
        refused(&missing)
    );
    let plain = fixture.path("plain-folder");
    fs::create_dir_all(&plain).unwrap();
    assert!(
        refused(&plain.display().to_string()).ends_with(
            "is not in a git repo. Choose the folder of your clone of the library, or enter the repo's address."
        ),
        "{}",
        refused(&plain.display().to_string())
    );
    let lonely = fixture.path("no-remote");
    fs::create_dir_all(&lonely).unwrap();
    fixture.git(&lonely, &["init", "-q"]);
    assert!(refused(&lonely.display().to_string()).starts_with("That clone has no remote"));
    assert!(refused("kicad-library").contains("is not a repo address"));
    // A team no longer in the list.
    let gone = repos
        .link("https://gone.example.test", "git@gitlab.com:a/b.git")
        .unwrap_err();
    assert!(
        gone.summary.contains("is no longer in your teams"),
        "{gone:?}"
    );
    // Nothing was linked.
    assert!(fixture.status(&repos, ACME).is_none());
}

#[test]
fn a_clone_with_several_remotes_and_no_origin_asks_which() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let fork = fixture.host_repo("fork.git");
    let clone = fixture.path("checkout");
    fs::create_dir_all(&clone).unwrap();
    fixture.git(&clone, &["init", "-q"]);
    let (bare_text, fork_text) = (bare.display().to_string(), fork.display().to_string());
    fixture.git(&clone, &["remote", "add", "client", &bare_text]);
    fixture.git(&clone, &["remote", "add", "fork", &fork_text]);
    let repos = fixture.repos();
    let Ok(Linked::ChooseRemote(remotes)) = repos.link(ACME, &clone.display().to_string()) else {
        panic!("no choice of remotes");
    };
    assert_eq!(
        remotes,
        [
            RepoRemote {
                name: "client".into(),
                address: bare_text.clone(),
                refused: None,
            },
            RepoRemote {
                name: "fork".into(),
                address: fork_text,
                refused: None,
            },
        ]
    );
    // Choosing one links its address.
    assert_eq!(repos.link(ACME, &remotes[0].address), Ok(Linked::Done));
    // With an origin, origin is used without asking.
    fixture.git(&clone, &["remote", "add", "origin", &bare_text]);
    assert_eq!(
        repos.link(BETA, &clone.display().to_string()),
        Ok(Linked::Done)
    );
    repos.wait_until_idle();
}

#[test]
fn a_host_that_does_not_answer_fails_within_the_time_limit() {
    let fixture = Fixture::new();
    // Takes connections and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().take(4).collect();
        std::thread::sleep(Duration::from_secs(60));
        drop(held);
    });
    let limits = Limits {
        check: Duration::from_secs(2),
        ..Limits::default()
    };
    let repos = fixture.repos_with(fixture.git.clone(), limits);
    let started = Instant::now();
    let problem = repos
        .link(
            ACME,
            &format!("https://127.0.0.1:{port}/client/library.git"),
        )
        .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    // Over https a credential helper may have been asking, so the line
    // says so rather than blaming the connection.
    assert!(!problem.offline, "{problem:?}");
    assert_eq!(
        problem.summary,
        format!(
            "Can't read the repo: git stopped after 2 seconds. git or ssh may be waiting for a passphrase or sign-in the app can't show. Try git ls-remote https://127.0.0.1:{port}/client/library.git in a terminal."
        )
    );
    // A port nobody listens on is refused. Windows tries for about two
    // seconds first, so this step gets a longer limit.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
    let repos = fixture.repos_with(fixture.git.clone(), Limits::default());
    let problem = repos
        .link(
            ACME,
            &format!("https://127.0.0.1:{closed_port}/client/library.git"),
        )
        .unwrap_err();
    assert!(problem.offline, "{problem:?}");
    assert!(
        problem
            .summary
            .starts_with("Can't read the repo: can't reach 127.0.0.1."),
        "{problem:?}"
    );
}

#[test]
fn two_teams_share_one_copy_until_neither_links_it() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let clone = fixture.persons_clone(&bare, "my-checkout");
    let repos = fixture.repos();
    // One team from the person's clone, the other from the address, written
    // differently: one repo, one copy.
    repos.link(ACME, &clone.display().to_string()).unwrap();
    repos.link(BETA, &bare.display().to_string()).unwrap();
    repos.wait_until_idle();
    assert_eq!(fixture.copies().len(), 1, "{:?}", fixture.copies());
    for team in [ACME, BETA] {
        assert_eq!(
            fixture.status(&repos, team).unwrap().access,
            RepoAccess::CanReadAndWrite
        );
    }
    // Unlinking one team keeps the copy the other uses.
    repos.unlink(ACME).unwrap();
    assert!(fixture.status(&repos, ACME).is_none());
    assert_eq!(fixture.copies().len(), 1);
    assert!(repos.repo_for(BETA).unwrap().is_copied());
    // Unlinking the last deletes it, read-only git files too.
    repos.unlink(BETA).unwrap();
    assert_eq!(fixture.copies(), Vec::<String>::new());
}

#[test]
fn copies_no_team_links_are_deleted_at_start_and_when_a_team_goes() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    {
        let repos = fixture.repos();
        repos.link(ACME, &bare.display().to_string()).unwrap();
        repos.wait_until_idle();
    }
    // A copy stopped halfway, and one of a repo no team links.
    fs::create_dir_all(fixture.repos_folder().join("left.partial")).unwrap();
    fs::create_dir_all(fixture.repos_folder().join("gitlab.com_2f_old")).unwrap();
    let repos = fixture.repos();
    repos.wait_until_idle();
    let kept = RepoAddress::parse(&bare.display().to_string())
        .unwrap()
        .folder_name();
    assert_eq!(fixture.copies(), [kept]);
    // Removing the team removes its link, and the copy goes.
    fixture.teams.remove(ACME);
    assert!(fixture.status(&repos, ACME).is_none());
    assert_eq!(fixture.copies(), Vec::<String>::new());
}

#[test]
fn an_empty_repo_can_be_read_but_not_tried_with_a_push() {
    let fixture = Fixture::new();
    let bare = fixture.path("empty.git");
    fs::create_dir_all(&bare).unwrap();
    fixture.git(&bare, &["init", "-q", "--bare"]);
    let repos = fixture.repos();
    repos.link(ACME, &bare.display().to_string()).unwrap();
    repos.wait_until_idle();
    assert_eq!(
        fixture.status(&repos, ACME).unwrap().access,
        RepoAccess::CanReadEmpty
    );
}

#[test]
fn without_git_the_app_says_where_to_get_it() {
    let fixture = Fixture::new();
    let repos = fixture.repos_with(Git::named("no-such-git-program"), Limits::default());
    let problem = repos.link(ACME, "git@gitlab.com:acme/lib.git").unwrap_err();
    assert!(
        problem
            .summary
            .starts_with("Can't read the repo: git is not installed on this computer."),
        "{problem:?}"
    );
    let folder = fixture.path("some-folder");
    fs::create_dir_all(&folder).unwrap();
    let problem = repos.link(ACME, &folder.display().to_string()).unwrap_err();
    assert!(
        problem.summary.contains("git is not installed"),
        "{problem:?}"
    );
}

#[test]
fn an_address_with_a_password_is_never_kept_or_shown() {
    let fixture = Fixture::new();
    let repos = fixture.repos();
    let token = "https://oauth2:glpat-secret@gitlab.com/acme/lib.git";
    assert_eq!(
        repos.link(ACME, token).unwrap_err().summary,
        address::HAS_PASSWORD
    );
    // A clone whose origin holds a token is refused the same way.
    let clone = fixture.path("token-clone");
    fs::create_dir_all(&clone).unwrap();
    fixture.git(&clone, &["init", "-q"]);
    fixture.git(&clone, &["remote", "add", "origin", token]);
    let problem = repos.link(ACME, &clone.display().to_string()).unwrap_err();
    assert!(
        problem.summary.ends_with(address::HAS_PASSWORD),
        "{problem:?}"
    );
    // Among several remotes, it is offered as unusable, its address unseen.
    let bare = fixture.host_repo("client-library.git");
    let several = fixture.path("several");
    fs::create_dir_all(&several).unwrap();
    fixture.git(&several, &["init", "-q"]);
    fixture.git(
        &several,
        &["remote", "add", "client", &bare.display().to_string()],
    );
    fixture.git(&several, &["remote", "add", "work", token]);
    let Ok(Linked::ChooseRemote(remotes)) = repos.link(ACME, &several.display().to_string()) else {
        panic!("no choice of remotes");
    };
    assert_eq!(remotes[1].name, "work");
    assert_eq!(remotes[1].address, "");
    assert_eq!(remotes[1].refused.as_deref(), Some(address::HAS_PASSWORD));
    assert!(remotes[0].refused.is_none());
    // Nothing with the token reached the list of teams.
    let saved = fs::read_to_string(fixture.path("config/team.json")).unwrap();
    assert!(!saved.contains("glpat"), "{saved}");
}

#[test]
fn a_file_address_of_a_clone_links_the_clone_s_remote_never_the_clone() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let clone = fixture.persons_clone(&bare, "my-checkout");
    // A clone of the person's clone: its origin is the first clone.
    let second = fixture.path("second-checkout");
    let (clone_text, second_text) = (clone.display().to_string(), second.display().to_string());
    fixture.git(
        fixture.dir.path(),
        &["clone", "-q", &clone_text, &second_text],
    );
    let repos = fixture.repos();
    let as_url = |path: &Path| {
        format!(
            "file:///{}",
            path.display()
                .to_string()
                .replace('\\', "/")
                .trim_start_matches('/')
        )
    };
    let bare_key = RepoAddress::parse(&bare.display().to_string())
        .unwrap()
        .key();
    for place in [as_url(&clone), as_url(&second), second_text.clone()] {
        assert_eq!(repos.link(ACME, &place), Ok(Linked::Done), "{place}");
        let linked = repos.repo_for(ACME).unwrap();
        assert_eq!(linked.address().key(), bare_key, "{place}");
    }
    // A bare repo's file:// address is kept as given.
    assert_eq!(repos.link(BETA, &as_url(&bare)), Ok(Linked::Done));
    assert_eq!(repos.repo_for(BETA).unwrap().address().url(), as_url(&bare));
    repos.wait_until_idle();
}

#[test]
fn each_team_fetches_and_checks_with_its_own_address() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let path = bare.display().to_string();
    let url = format!(
        "file:///{}",
        path.replace('\\', "/").trim_start_matches('/')
    );
    let repos = fixture.repos();
    repos.link(ACME, &url).unwrap();
    repos.link(BETA, &path).unwrap();
    repos.wait_until_idle();
    assert_eq!(fixture.copies().len(), 1);
    let origin = |repos: &LibraryRepos| {
        let copy = repos.repo_for(BETA).unwrap().folder().to_path_buf();
        fixture.git(&copy, &["remote", "get-url", "origin"])
    };
    // Acme unlinks; Beta's next check goes by Beta's own address.
    repos.unlink(ACME).unwrap();
    repos.check_again(BETA);
    repos.wait_until_idle();
    assert_eq!(origin(&repos), path);
    assert_eq!(
        fixture.status(&repos, BETA).unwrap().access,
        RepoAccess::CanReadAndWrite
    );
}

#[test]
fn a_restart_shows_the_last_results_and_tries_no_push() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    {
        let repos = fixture.repos();
        repos.link(ACME, &bare.display().to_string()).unwrap();
        repos.wait_until_idle();
    }
    // The results are kept beside the copies, never in the list of teams.
    assert!(fixture.path("library-repos.json").is_file());
    // After a restart, a host that now refuses pushes is not asked: the
    // start only fetches, and the last push check's result stands.
    let refuse = fixture.path("refuse-push.sh");
    fs::write(
        &refuse,
        "#!/bin/sh
echo refused >&2
exit 1
",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&refuse, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let git = fixture
        .git
        .clone()
        .with_env("GIT_CONFIG_COUNT", "1")
        .with_env("GIT_CONFIG_KEY_0", "remote.origin.receivepack")
        .with_env(
            "GIT_CONFIG_VALUE_0",
            refuse.display().to_string().replace('\\', "/"),
        );
    let repos = fixture.repos_with(git, Limits::default());
    // At once, before the start fetch ends, the row says what it said.
    assert_eq!(
        fixture.status(&repos, ACME).unwrap().access,
        RepoAccess::CanReadAndWrite
    );
    repos.wait_until_idle();
    assert_eq!(
        fixture.status(&repos, ACME).unwrap().access,
        RepoAccess::CanReadAndWrite
    );
    // Check again tries a push, and finds the refusal.
    repos.check_again(ACME);
    repos.wait_until_idle();
    assert!(matches!(
        fixture.status(&repos, ACME).unwrap().access,
        RepoAccess::CanReadOnly { .. }
    ));
    // A host gone at start is said by the start fetch.
    drop(repos);
    fs::rename(&bare, fixture.path("moved.git")).unwrap();
    let repos = fixture.repos();
    repos.wait_until_idle();
    assert!(matches!(
        fixture.status(&repos, ACME).unwrap().access,
        RepoAccess::CannotRead { .. }
    ));
}

#[test]
fn the_main_branch_is_the_host_s_not_the_first_by_name() {
    let fixture = Fixture::new();
    let bare = fixture.path("empty.git");
    fs::create_dir_all(&bare).unwrap();
    fixture.git(&bare, &["init", "-q", "--bare"]);
    let repos = fixture.repos();
    repos.link(ACME, &bare.display().to_string()).unwrap();
    repos.wait_until_idle();
    let repo = repos.repo_for(ACME).unwrap();
    assert_eq!(repo.latest_commit().unwrap(), None);
    // Commits arrive, on main and on a branch named before it.
    let work = fixture.path("work");
    fs::create_dir_all(&work).unwrap();
    fixture.git(&work, &["init", "-q"]);
    fs::write(
        work.join("a.txt"),
        "a
",
    )
    .unwrap();
    fixture.git(&work, &["add", "."]);
    fixture.git(&work, &["commit", "-q", "-m", "main"]);
    let main = fixture.git(&work, &["rev-parse", "HEAD"]);
    let bare_text = bare.display().to_string();
    fixture.git(&work, &["push", "-q", &bare_text, "HEAD:main"]);
    fs::write(
        work.join("b.txt"),
        "b
",
    )
    .unwrap();
    fixture.git(&work, &["add", "."]);
    fixture.git(&work, &["commit", "-q", "-m", "aside"]);
    fixture.git(&work, &["push", "-q", &bare_text, "HEAD:aaa-aside"]);
    repos.check_again(ACME);
    repos.wait_until_idle();
    assert_eq!(repo.latest_commit().unwrap(), Some(main));
}

#[test]
fn a_clone_whose_origin_is_its_own_folder_is_never_linked() {
    let fixture = Fixture::new();
    let clone = fixture.path("loop-checkout");
    fs::create_dir_all(&clone).unwrap();
    fixture.git(&clone, &["init", "-q"]);
    let clone_text = clone.display().to_string();
    fixture.git(&clone, &["remote", "add", "origin", &clone_text]);
    let repos = fixture.repos();
    let problem = repos.link(ACME, &clone_text).unwrap_err();
    assert!(
        problem.summary.contains("leads from clone to clone"),
        "{problem:?}"
    );
    assert!(fixture.status(&repos, ACME).is_none());
}

#[test]
fn a_relative_remote_is_read_from_the_clone_s_top_folder() {
    let fixture = Fixture::new();
    let bare = fixture.host_repo("client-library.git");
    let clone = fixture.persons_clone(&bare, "my-checkout");
    fixture.git(
        &clone,
        &["remote", "set-url", "origin", "../client-library.git"],
    );
    let inside = clone.join("symbols");
    let repos = fixture.repos();
    // From a folder inside the clone, and with `..` in it: one key.
    assert_eq!(
        repos.link(ACME, &inside.display().to_string()),
        Ok(Linked::Done)
    );
    let bare_key = RepoAddress::parse(&fs::canonicalize(&bare).unwrap().display().to_string())
        .unwrap()
        .key();
    let linked = repos.repo_for(ACME).unwrap().address().key();
    assert_eq!(
        linked.trim_start_matches("//?/"),
        bare_key.trim_start_matches("//?/")
    );
    repos.wait_until_idle();
}

#[test]
fn a_saved_address_that_cannot_be_used_is_never_shown() {
    let fixture = Fixture::new();
    // As an older version could have kept it.
    fixture
        .teams
        .set_library_repo(
            ACME,
            Some("https://oauth2:glpat-secret@gitlab.com/acme/lib.git"),
        )
        .unwrap();
    let repos = fixture.repos();
    let status = fixture.status(&repos, ACME).unwrap();
    assert!(status.unusable);
    assert_eq!(status.address, "");
    assert_eq!(status.name, "This address can't be used");
    assert!(!format!("{status:?}").contains("glpat"));
    repos.unlink(ACME).unwrap();
    assert!(fixture.status(&repos, ACME).is_none());
}

#[test]
fn a_quiet_https_time_out_is_offline() {
    let fixture = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().take(4).collect();
        std::thread::sleep(Duration::from_secs(60));
        drop(held);
    });
    let limits = Limits {
        check: Duration::from_secs(2),
        ..Limits::default()
    };
    let repos = fixture.repos_with(fixture.git.clone(), limits);
    let address =
        RepoAddress::parse(&format!("https://127.0.0.1:{port}/client/library.git")).unwrap();
    // Asking nothing, no sign-in window can be waiting: a slow host.
    let problem = repos
        .shared
        .repo(&address)
        .asking_nothing()
        .can_read()
        .unwrap_err();
    assert!(problem.offline, "{problem:?}");
    assert!(
        problem.summary.contains("did not answer within 2 seconds"),
        "{problem:?}"
    );
}

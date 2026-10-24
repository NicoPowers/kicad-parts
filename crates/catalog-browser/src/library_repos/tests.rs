//! Library repos end to end, with bare repos in folders on this computer
//! only. git reads only an empty settings file of the test's own, so the
//! developer's settings, SSH keys and git hosts are never used.

use super::*;
use crate::teams::SignIns;
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
    assert_eq!(
        repo.list_files(&commit).unwrap(),
        ["symbols/Acme.kicad_sym"]
    );
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
        refused(&missing).starts_with("Can't read the repo: the repo was not found."),
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
                address: bare_text.clone()
            },
            RepoRemote {
                name: "fork".into(),
                address: fork_text
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
    assert!(problem.offline, "{problem:?}");
    assert_eq!(
        problem.summary,
        "Can't read the repo: 127.0.0.1 did not answer within 2 seconds. Check your internet connection, or try again later."
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

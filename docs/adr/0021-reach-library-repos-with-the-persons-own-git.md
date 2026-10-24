# Reach library repos with the person's own git and SSH setup

The person designs for many clients, and each client is a team (ADR 0018). Many clients keep only an ad hoc KiCad library in a git repo, on GitHub, GitLab, Bitbucket or another git server. The person's SSH key can already read and write all of them. So a team links to its library repo, and the app reaches it as the person does in a terminal. Decided 2026-10-24 by best judgement (kicad-parts-ab30).

- **The system's git program.** Every step runs the person's own `git` (`crates/catalog-browser/src/git_program.rs`). So their git settings, `core.sshCommand`, SSH config, agents (the macOS Keychain, 1Password, Pageant, the Windows agent) and credential helpers (such as Git Credential Manager) apply as they are. The app never stores or sees a key or a password. No git library is linked.
- **Never hang, never ask in a terminal.** Every git run has no window, empty input, `GIT_TERMINAL_PROMPT=0` and a time limit. At the limit, the app stops git and every program git started: on Windows with `taskkill /T`, elsewhere by stopping git's own process group. A credential helper's own sign-in window is the person's setup, and is allowed.
- **SSH asks nothing.** For steps that reach a host, when the person has no SSH command of their own, the app sets `GIT_SSH_COMMAND` to `ssh -o BatchMode=yes -o ConnectTimeout=20`. BatchMode makes ssh fail at once about a passphrase or a new host key; agent keys still work. A command of the person's own (`GIT_SSH_COMMAND`, `GIT_SSH`, `core.sshCommand`) or a named SSH kind (`GIT_SSH_VARIANT`, `ssh.variant`) is never changed, because that would break their agent. If it asks something, the time limit stops it. On Git for Windows, plain `ssh` finds Git's own OpenSSH through the same search path as before, so nothing else changes.
- **Adding a repo.** The person enters a folder with their clone, or the repo's address: `git@host:path`, `ssh://`, `https://`, or a bare repo's folder. From a clone, the app uses `origin`, or the only remote, or asks which. It only reads the clone, with `GIT_OPTIONAL_LOCKS=0`, and never writes there (mx-70700f). Linking needs the repo to be readable (`git ls-remote`).
- **One key per repo.** The address is normalised: host in small letters, the scheme's usual port and any `user@` dropped, `.git` and slashes trimmed. So `git@github.com:acme/lib.git` and `https://github.com/acme/lib` are one repo, and teams that link it share one copy.
- **The app's own copy.** A full clone with no checked-out files (`--no-checkout`) in `library-repos/<file-safe key>` in the local data folder, or in `--data-dir`. Files are read from commits, so any commit reads offline, and 3D models are not stored twice. A new copy is made in a `.partial` folder and moved into place when git finishes. Copies no team links are deleted at start, on Remove and after a team is removed.
- **Access check.** Read: `git ls-remote <address> HEAD`. Then copy or fetch. Write: `git push --dry-run --no-verify` of the latest commit to a new branch name that cannot exist yet. Hosts check write access when a push starts, and a dry run creates nothing. The result is one line: "Can read and write.", or the step that failed, why, and what to check. git's own words go under Details. It runs when linking, at start and on Check again.
- **Offline.** Reading uses the app's copy. While the host cannot be reached, the row says so, says when the copy is from, and the person checks again when online.

## Considered options

- **A git library, such as libgit2 or gitoxide.** Rejected: it would need its own SSH and credential handling, so the person's agents, `~/.ssh/config` and credential helpers would not apply, and the app would handle keys.
- **Always set `GIT_SSH_COMMAND`.** Rejected: it would replace a person's own command, such as Windows OpenSSH for 1Password, and break their agent.
- **`SSH_ASKPASS` with `SSH_ASKPASS_REQUIRE=force` pointing at a program that fails.** Not chosen: it needs OpenSSH 8.4 or newer, a helper program shipped with the app, and it overrides a person's own askpass.
- **Accept new host keys (`StrictHostKeyChecking=accept-new`).** Rejected: the person should check a host's key once, themselves.
- **Use the person's clone.** Rejected: the app would change what they work on (mx-70700f).
- **A shallow or partial clone.** Rejected: kicad-parts-e9a3 pins commits, which must read offline.

## Consequences

- `LibraryRepos` (`crates/catalog-browser/src/library_repos/`) holds the links and checks. `LibraryRepo` is the seam later work uses: `can_read`, `copy_or_fetch`, `can_write`, `latest_commit`, `read_file` and `list_files` now. kicad-parts-e9a3 scans through it. kicad-parts-02fb adds "commit on a new branch and push" there.
- `team.json` keeps each team's repo address only.
- git and ssh run with `LC_ALL=C` for steps that reach a host, so their messages match the plain-word rules in `library_repos/problems.rs`.
- If git is not installed, the row says where to get it for the person's system, and the rest of the app works as before.
- Tests use bare repos in folders, a test git that reads only an empty settings file, and a local port that never answers. The journeys run git with an empty settings file of the run's own.

Mulch records: mx-ffee38, mx-a0655b, mx-2a621b, mx-d2814d, mx-044140, mx-4160ed.

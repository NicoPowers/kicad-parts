# Keep each team's last-known parts on disk

The person works for many clients, each a team on its own team server (ADR 0018). If a team's server was down when the app started, that team showed no parts until the server came back. During a run, a team that dropped kept its parts, marked offline. Now an app start does the same. Decided 2026-10-24 by best judgement (kicad-parts-ed46).

- **What is kept.** For each team, what the part list needs: each part's values (`PartValues`) and the category names. With them go the server address, the team code, and when the parts last matched the server. It never holds a password or a token. Each team's KiCad library folder already holds these parts, so the file adds little exposure.
- **Where.** One JSON file per team, in a `known-parts` folder next to the list of teams (`team.json`), in the app's own settings folder. The file name is the server address with every other character written as its code, so two servers never share a file. Never in the repository or a KiCad library folder.
- **When it is written.** Only while the team's parts match its server: the session is live and has caught up. A writer per team (`teams/parts_writer.rs`) writes once the parts have stayed the same for 3 seconds, or after 30 seconds while they keep changing. Searching never writes. While live, it writes again every 10 minutes, so the time it records stays close. Each write goes to a temporary file first, then replaces the old file whole.
- **When it is used.** At start, a team with a kept sign-in shows its kept parts until its first catch-up replaces them. While its server cannot be reached, they are marked offline and read-only, as during a run. A file is used only if it names the team's own server (kicad-parts-092b). A file that cannot be read, has another version, or names another server is ignored and logged. The next catch-up replaces it.
- **When it is deleted.** Signing out of a team, and removing it, delete its file. A team asked to sign in again (its sign-in was rejected or ran out) shows no parts. Its file stays until the person signs out or removes the team, so signing in again shows its parts at once.
- **What the person sees.** The team's row in the Teams menu and the part's offline note say when the parts were last up to date, in local time: "Offline. Parts as of 09:12 yesterday." There are no new settings.

## Considered options

- **The desktop's own SQLite (mx-95dadd).** Not chosen: the desktop has no SQLite of its own yet. One JSON file per team needs no new dependency, and is written and deleted per team. Persisting the replica checkpoint (kicad-parts-d878) may bring one; these files can move there then.
- **Keep the whole replica.** Not chosen: the part list needs only each part's values. The replica and its checkpoint are kicad-parts-d878.
- **Write after every change.** Rejected: a burst of changes from an import or a teammate would write a file of thousands of parts many times a second.
- **Show kept parts for every team in the list.** Rejected: a team with no kept sign-in is signed out, and "offline" would be wrong. A signed-out team shows no parts, as before.

## Consequences

- `TeamCatalog::last_known_parts` reads the kept parts until the first catch-up, then the session's copy. Nothing that shows parts changed (ADR 0019).
- `TeamStatus::parts_as_of` says when the shown parts last matched the server, while they may be out of date. During a run, that is when live updates broke.
- A team that is signed out on a computer whose password store lost its sign-in shows no parts. Its file stays until the person signs out or removes the team.
- The journeys restart the app with a sign-in kept through `--sign-ins=test-file`, a file in the journey's own data folder. Only debug builds offer it, so the app itself keeps tokens only in the system's password store.
- History, offline edits and a faster catch-up from a kept checkpoint are not part of this (kicad-parts-d878).

Mulch records: the records of kicad-parts-ed46.

# Stay signed in to every team through the system's password store

The person designs for many clients in one day, and each client is its own team on its own team server. Signing out of one client to sign in to another, and signing in again after every restart, got in the way. So the app now holds one session per team server, side by side, and keeps each team's sign-in between runs. Decided 2026-10-18 (kicad-parts-f10d). This replaces the part of ADR 0017 and mulch record mx-b959cf that kept the session in memory only.

- **Several teams at once.** Each team in the person's list has its own team catalog, session, live-updates follower and catch-up (`crates/catalog-browser/src/teams/`). The catalog view showed one team at a time, chosen with a team switcher, with no sign-in; ADR 0019 replaces this with one part list over every team. A team whose server cannot be reached shows its last-known parts, marked offline; the other teams are unaffected.
- **What is kept, and where.** The app keeps each team's renewable sign-in, its PocketBase token, never the password. It keeps the token in the operating system's credential store through the `keyring` crate: Windows Credential Manager, the macOS Keychain, or the Secret Service on Linux (GNOME Keyring or KWallet). A token or password is never written to a file. The list file (`team.json`) holds each server address, email and team code.
- **Renewal.** At start, each team's keeper renews the kept token with PocketBase's auth-refresh, and keeps trying while the server cannot be reached. While in use, it renews the token when half its life has passed, and at least every 12 hours. A token the server rejects, for example after the password changed, is forgotten, and only that team asks for the password again.
- **Signing out and removing.** Sign out forgets that team's kept sign-in and keeps the team in the list. Remove, after asking once, also takes it off the list.
- **No credential store.** If none works, such as on Linux with no Secret Service, sign-ins last until the app closes, and the Teams menu says so once, with the reason.

## Considered options

- **Keep sessions in memory only (as before).** Rejected: the person signs in to every client after every restart.
- **Save the password.** Rejected: a token can be renewed and ends when the password changes, so a stolen token is worth less than a password.
- **Write tokens to a file, perhaps encrypted.** Rejected: the key would sit next to the file. The operating system's store protects secrets with the person's own sign-in.
- **Each platform's API by hand.** Rejected: `keyring` covers the three platforms behind one interface. Its Linux backend uses D-Bus, which the desktop already links through Tauri.

## Consequences

- The `keyring` code lives in the desktop app (`apps/desktop/src/sign_ins.rs`), behind the `SignInStore` seam in catalog-browser, so no test can reach the person's real store. Tests use a store in memory, and the journeys start the app with `--sign-ins=memory`.
- A token, like a password, is never sent over plain `http://` to another computer (`PocketBaseClient::with_token`).
- The list of teams is one seam for what comes next: `Teams::signed_in` serves work across every team. Since ADR 0019, `Teams::known_parts` feeds one search over all teams, and `Teams::shown` is gone.
- Each team's KiCad libraries are kept up to date at once, one updater per team, and each writes only its own folder (ADR 0015).
- After a restart with a server that cannot be reached, that team shows the parts the app kept on disk, marked offline, until it is reached (ADR 0020, kicad-parts-ed46).
- PocketBase tokens last 7 days by default. Renewal happens only while the app is open, so a person who does not open the app for 7 days signs in again to each team. While the app is open, renewal keeps every team signed in.
- On macOS, the Keychain ties each saved sign-in to the app's code signature. Until the app is signed, the Keychain may ask once per team, after each update, whether the app may read it.
- Sign out marks the team signed out in the list file. So if the password store cannot forget the sign-in, for example a locked Linux keyring, the next start still does not use it, and the team's row says so.
- One server has one address: the host loses its capitals and the scheme's usual port, so `https://Parts.example.com:443` and `https://parts.example.com` are one team.

Mulch records: mx-ba6203 (replaces the "session in memory only" part of mx-b959cf), mx-c5104a.

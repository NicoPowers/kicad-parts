# Try the team catalog by hand

This starts a throwaway team server on your computer and opens the desktop app connected to it. You can then edit a resistor, see its history, and restore an older revision. Nothing you do changes the CSV catalog or anything else in the repository.

## Build once

From the repository root, in PowerShell or Git Bash:

```
cd apps/desktop-ui
trunk build
cd ../..
cargo build --locked -p kicad-parts-desktop --features custom-protocol
```

You need KiCad 10 installed for the symbol and footprint previews.

## Start it

```
cargo run --locked -p catalog-import -- local-team-server --desktop
```

The command:

1. starts PocketBase on `127.0.0.1` with a random port, with no window and no browser;
2. imports the resistors from `database/g-res.csv`;
3. creates three accounts with new random passwords and prints them;
4. opens the desktop app with the server address and the first email already filled in.

The accounts are:

| Email | Can do |
| --- | --- |
| `editor@example.test` | view and edit parts |
| `second.editor@example.test` | view and edit parts (use it in a second app window to cause a conflict) |
| `viewer@example.test` | only view parts |

## What to try

1. In the app, open **Team catalog: not signed in** at the top right and sign in with the first account's password from the console.
2. Search for `RC0603FR-0747RL` and select it. The right-hand column shows **Details** and **History**.
3. Choose **Edit details**, change a value, give a reason, and save. The part gets a new revision.
4. To see a conflict, keep the first app open and open a second copy pointed at the same server, with its own settings folder:

   ```
   target/debug/kicad-parts-desktop.exe --team-server=<address> --data-dir=<an empty folder>
   ```

   Sign in there as `second.editor@example.test`. In the first app, choose **Edit details** on the part and change something, but do not save yet. In the second app, save a change to the same part. Now save in the first app: it shows where you started, their version and yours, and lets you save your changes on top of theirs. Changes saved in one app appear in the other by themselves.
5. Open **History**, select an older revision, and choose **Restore revision N…**. Restoring adds a new revision with the old details; it never deletes or changes a revision.

## Stop it

Close the desktop app. The command then stops the team server and deletes its data. If you stop the command with Ctrl+C instead, the team server still stops, because it runs in a job that Windows ends with the command.

Without `--desktop`, the command prints the address and accounts and waits for you to press Enter, so you can open the app yourself.

## How signing in works

- The app saves only the server address and your email (in `team.json` in its settings folder). It never saves your password or the sign-in session; after a restart you sign in again.
- For now the app only connects to a team server on this computer (`http://127.0.0.1`, `localhost` or `[::1]`). Live updates use plain HTTP, and a password must not cross a network unencrypted. Connecting to a shared server needs HTTPS first.
- Members marked "can edit catalog" in PocketBase can edit and restore. Everyone else who signs in can only view. Only an administrator can change that mark.

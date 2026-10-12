# Try the team catalog by hand

This starts a throwaway team server on your computer and opens the desktop app connected to it. You can then edit a resistor, replace its footprint with a file, see its history, archive a revision, and review new CSV values that an import did not apply. Nothing you do changes the CSV catalog or anything else in the repository.

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
3. Choose **Edit part**, change a value, give a reason, and save. The part details change, but the part gets no new revision: every revision shares the same details. Only changing the **Symbol** or **Footprint** adds a revision. The revision also shows **Excluded from BOM**, which the team server works out from the symbol and footprints the revision uses, never by hand. It says which mark sets it ("Yes: its symbol says it is not in the BOM"), "No: only some footprint choices say it is not in the BOM" when the choices disagree, and "Not known" when the answer depends on a symbol or footprint the server cannot read: one from KiCad's own library, which the app does not read yet, or one that is not in the Catalog.
4. To see a conflict, keep the first app open and open a second copy pointed at the same server, with its own settings folder:

   ```
   target/debug/kicad-parts-desktop.exe --team-server=<address> --data-dir=<an empty folder>
   ```

   Sign in there as `second.editor@example.test`. In the first app, choose **Edit part** on the part and change something, but do not save yet. In the second app, save a different change to the same part (for example the first app changes the tolerance and the second the value). Now save in the first app: it shows where you started, what they saved and your changes, and lets you save your changes on top of theirs. If both apps make exactly the same change, there is no conflict: the app says "These changes are already saved, so nothing more was saved." Changes saved in one app appear in the other by themselves.
5. To replace the footprint with a file, copy `footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod` to a folder of your own and change it in KiCad's footprint editor. In **Edit part**, choose **Use a file…** under **Footprint** and pick your copy. The app fills in the reference from the file, draws the footprint, and, if the footprint names a 3D model in the Catalog's `3d-models` folder that the Catalog does not have, asks for that file too (leave it out and the footprint shows a missing 3D model). Give a reason and save: the footprint gets a new revision, only this part moves to it, and the message says how many other parts still use the older one. **History** shows the file the new revision came from. A `.kicad_sym` with several symbols asks which one to use. A later import never replaces a footprint changed this way; if the CSV catalog's file for it changes, the part shows the CSV notice, and that value can only be kept.
6. Open **History**. Each entry is one saved change: a change to the part details, a new revision, or an archived revision. Select one to see what changed, who changed it, when and why. Nothing is ever deleted or changed afterwards, and there is no way to copy an older revision back.
7. To take a bad revision out of use, select it in **History** and choose **Archive revision N…**, then give a reason. KiCad and the app stop offering it, so nobody new picks it; designs that already use it keep it. The current revision becomes the newest one that is not archived. **Unarchive revision N…** undoes it. The only revision that is not archived cannot be archived: add a corrected revision first.
8. To see what an import does to a part edited in the app, edit the part's **Description** and **Value**, then type `reimport` in the console and press Enter. The import leaves the edited part as it is, and the app shows nothing: the CSV has no new values. Now give the import new values without touching the repository: copy the `database`, `symbols` and `footprints` folders into an empty folder, change the part's value in the copy's `database/g-res.csv`, and type `reimport <that folder>`. The import still leaves the edited part as it is, and the app shows by itself: "g-res.csv has new values for this part. They were not applied, because the part was edited in this app." Choose **Review CSV values**. Each field whose CSV value differs is listed with its value now and in the CSV. Values the CSV changed come first; values it did not change are marked "(not changed in the CSV)", and accepting one undoes the edit. Tick the ones to accept and choose **Accept the ticked values**: accepting part details adds no revision, and accepting the Symbol or Footprint adds one. The fields you do not tick stay as they are. **Keep current values** accepts none. Either way the notice goes away, and it comes back only if the CSV changes again, even back to what it said before, unless someone already reviewed those exact values. Typing anything else gets a reminder; Enter alone stops the team server.

## Stop it

Close the desktop app. The command then stops the team server and deletes its data. Ctrl+C or closing the console window does the same.

The team server keeps its data in `target/local-team-server/run-<number>` while it runs, never in your system temp folder. The data includes the accounts' password hashes. If the command is killed outright (for example from Task Manager), the team server still stops, because it runs in a job that Windows ends with the command, but the folder stays behind until the next time you start the command, which deletes it.

If the team server will not start and says its data "cannot be upgraded", the data was saved by an older version of the team server: before part details were kept apart from revisions, or before CAD item revisions were numbered like part revisions. That data is never changed. To start fresh, stop the command, delete the `target/local-team-server` folder, and start the command again. If you run PocketBase on a data folder of your own, delete or move that `pb_data` folder instead.

Without `--desktop`, the command prints the address and accounts and waits for you to press Enter or Ctrl+C, so you can open the app yourself.

## How signing in works

- The app saves only the server address and your email (in `team.json` in its settings folder). It never saves your password or the sign-in session; after a restart you sign in again.
- For now the app only connects to a team server on this computer (`http://127.0.0.1`, `localhost` or `[::1]`). Live updates use plain HTTP, and a password must not cross a network unencrypted. Connecting to a shared server needs HTTPS first.
- Members marked "can edit catalog" in PocketBase can edit, archive and review CSV values. Everyone else who signs in can only view. Only an administrator can change that mark, or a member's name, which the history shows as the author of each change.

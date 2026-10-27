# Manual Windows KiCad interoperability checklist

KiCad 10 is a required dependency. The application and automated tests run the installed `kicad-cli` wherever it is useful. This checklist covers what the command line cannot show: how generated data behaves when opened in KiCad's own editors.

## How our code runs kicad-cli

kicad-cli loads your KiCad settings when it starts and saves them again if they changed. If it reads `kicad_common.json` while another KiCad process saves it, it saves defaults over the whole file. That empties your path variables.

So our code never lets kicad-cli use your settings in `%APPDATA%\kicad` or your settings or cache in `%LOCALAPPDATA%\kicad`. Every run gets `KICAD_CONFIG_HOME` and `KICAD_CACHE_HOME` inside a folder of our own:

- **The desktop app** uses `kicad-cli-settings` in its cache folder, next to `previews`.
- **The real-KiCad tests** in `real_kicad.rs` use `kicad-parts-tests\kicad-cli-settings` in the temporary folder.
- **The catalog-browser, local-team and journey tests** use `cache\kicad-cli-settings` in their temporary data folder.

kicad-cli fills that folder with default settings. Previews look the same as with your settings. kicad-cli writes nothing under `Documents\KiCad`, so we leave `KICAD_DOCUMENTS_HOME` unset.

Before each run, our code checks that this folder does not overlap your KiCad folders. If it does, or if the folder cannot be made, kicad-cli does not start. The app then shows the folder and what went wrong.

## Checklist

Use a supported KiCad installation on Windows and a disposable copy of the application output. Do not point the application or a test at the user's KiCad configuration directory.

- Record the application commit/build, Windows version, KiCad version shown in the GUI, and the fixture or generated file under review.
- Generate or export the representative symbol, footprint, board, or library data with the application.
- Open the generated data through KiCad's normal GUI file-open or library-management flow.
- Confirm that KiCad reports no parse, rescue, missing-library, or format-upgrade error.
- Inspect the relevant semantics: symbol pins and fields, footprint pads and geometry, board nets/layers, and referenced 3D models as applicable.
- Save to a disposable copy, close it, reopen it in KiCad, and confirm the same content is present.
- If useful, capture screenshots and a short note with the versions and observed result.
- Record any failure as a follow-up issue. A skipped manual check must not be reported as an automated pass.

When a check can be automated with `kicad-cli`, such as parsing, upgrading, or exporting a file, automate it and drop it from this list.

## KiCad libraries made from the team catalog

The desktop app writes each team's KiCad libraries from the team catalog into `kicad-libraries\<team code>` in its data folder (kicad-parts-f397). `tooling/local-team/tests/kicad_libraries.rs` already checks them with the installed KiCad 10, in a folder whose name is not plain ASCII:

- `kicad-cli sym export svg` and `fp export svg` draw every generated symbol and footprint.
- `kicad-cli sch erc` finds every entry of the database library through the SQLite3 ODBC driver: current, older, archived, and current again after an archive. The schematic saves each entry's exact symbol, and KiCad finds no difference; it tells a wrong copy apart and reports an entry that does not exist.
- `kicad-cli pcb drc` finds a generated footprint by its link, and reports a link to a footprint that does not exist.
- `kicad-cli pcb export step` finds a footprint's 3D model by its link relative to the footprint library after the folder has moved, and reports a model that is not there.

Only KiCad's editors can show the rest. Use KiCad settings you can throw away, or note what you change.

1. **Make the libraries.** Start the team server with `cargo run --locked -p local-team -- server --desktop --team-code MA` and sign in. Open the KiCad settings at the top right. Under **KiCad libraries**, wait for "Up to date". The libraries are deleted when the command stops, so keep it running.
2. **Add them to KiCad.** Copy the symbol libraries. In KiCad, choose **Preferences > Manage Symbol Libraries**, right-click the table on the **Global Libraries** tab, and choose **Paste**. Check that `MA-Parts` and the `MA-` symbol libraries, such as `MA-res-20241209`, appear, with **Show** cleared for the symbol libraries. Choose **OK**. Paste the footprint libraries the same way in **Manage Footprint Libraries**. If pasting does nothing, add each library by hand with the nickname and path shown, and note it.
3. **Place a part.** In the schematic editor, open **Add Symbol**. Check that each category is one group, such as `MA-Parts - Resistors`, and that the `MA-` symbol libraries are not listed. Place `RES-0000-0470_v1`. Check its fields: IPN `RES-0000-0470` without `_v1`, Revision `1`, Value, Manufacturer, MPN and Datasheet. Check that its footprint is `MA-Resistor_SMD:R_0603_1608Metric_v1`.
4. **Look at the footprint and model.** Update the board from the schematic. Check the footprint's pads, and that its 3D model shows in the 3D viewer.
5. **See a new revision arrive.** Keep KiCad open. In the app, change the part's symbol, such as to `Device:R`, which adds revision 2. Wait for "Up to date" in the app, then about 10 seconds more. Close and open **Add Symbol**. Check that `RES-0000-0470_v2` is under Resistors and `RES-0000-0470_v1` is under **Older revisions**, without restarting KiCad. Note how long it took.
6. **Archive a revision.** In the app, archive revision 2. Check that `RES-0000-0470_v2` moves to **Archived revisions** and `_v1` back to Resistors. KiCad can hide only a whole library, so archived revisions stay listed under that group.
7. **Update from Library keeps the revision.** In the schematic that holds `_v1`, run **Tools > Update Symbols from Library**. Check that the symbol stays `RES-0000-0470_v1` with revision 1's symbol, and that only the part details changed. Do the same while `_v1` is under **Older revisions**: category fields, such as Tolerance, should stay as they were, because that group has only the fields every category shares. Then run **Tools > Update Footprints from Library** in the board, and check that the footprint stays `R_0603_1608Metric_v1`.
8. **A new group needs a restart.** Add a part with a symbol in a category that had none. The app says the groups or fields changed. Check that the new group appears only after KiCad restarts.
9. **Safe while KiCad is open.** With KiCad open and a part placed, save several changes in the app quickly. Check that KiCad shows no load error and that the placed part still updates from the library.
10. **Files saved into the libraries.** Open a generated footprint in the footprint editor and save it. Check that KiCad refuses, because the file is read-only. KiCad still treats the footprint library as one it can change, because it checks the `.pretty` folder, not the files. So save the footprint under a new name in the same library. Check that this works, and that the app then names that file as one that did not come from the team catalog and leaves it alone. Delete the file afterwards.
11. **A moved folder.** Close KiCad, move the team's folder, change the library paths in KiCad's tables to the new place, and open the board again. Check that the footprint's 3D model still shows.

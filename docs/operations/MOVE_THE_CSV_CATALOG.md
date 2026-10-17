# Move your CSV catalog into the team catalog

This guide is for the engineer who looks after a team's CSV catalog: the `database/g-*.csv` files and the `symbols/`, `footprints/` and `3d-models/` libraries. Before the team moves to the team catalog, you run one command. It shows that the move loses nothing, and that KiCad gets what the CSV catalog meant it to get.

The command only reads the CSV catalog. It never changes a file in `database/`, `symbols/`, `footprints/` or `3d-models/`, and it checks that at the end.

## Before you start

You need Rust and KiCad 10 installed, and a clone of this repository with your CSV catalog in it.

Commit your CSV catalog first. A team catalog only takes files that are committed, so everyone can check out exactly what was imported. If you only want to try it, add `--allow-uncommitted-changes` to the command.

## Run the proof

From the repository root, in PowerShell or Git Bash:

```
cargo run --locked -p catalog-import -- full-pilot
```

It takes about a minute. The command:

1. **Imports every CSV file** into a new team server that runs only on your computer, one file after another, as your team would. Then it imports them all again, to show a second import changes nothing.
2. **Checks that nothing is lost.** It compares every field of every row with the CSV files, one by one. It also checks `substitutes.csv`, every symbol, footprint and 3D model your rows use, and that no IPN is in two CSV files.
3. **Makes the KiCad libraries** your team will use, such as `MA-Parts`.
4. **Makes them again** from a second, separate team server, and checks both are the same, byte for byte.
5. **Compares them with the CSV catalog's own KiCad library**, `database/parts.sqlite`.
6. **Checks that the CSV catalog is unchanged.**

The team servers and libraries are deleted when the command ends.

## Read the report

The first line says **PASSED** or **FAILED**. Then each step has a short section.

- **Import** says how many files and rows were imported, and what the library check found. "Not in the Catalog yet" lists symbols and footprints in your libraries that no row uses. The desktop app's **Library check** view shows them.
- **Nothing lost** counts the fields it checked and the CAD files stored with their license. It lists the **import problems**: things that could not become part of the catalog, such as a footprint whose 3D model file is missing. An import problem keeps the value, so nothing is dropped. Fix them when you can, in the CSV catalog.
- **Not imported, and why** lists the few things the import never takes in, each with its reason. For example, the old `update_db.sh` script left a table out, but the import reads the CSV files themselves.
- **KiCad libraries** says how many entries KiCad gets. Each part revision is one entry, such as `RES-0000-0470_v1`.
- **Compared with the CSV catalog's own KiCad library** lists every difference, grouped, each with its reason. For example, supplier part numbers and prices are not sent to KiCad, and a bare board with no symbol gets no entry. Add `--every-difference` to see each difference on its own line.
- **Time** says how long each step took.

## If the proof fails

The end of the report lists each thing that was lost or differs without a reason, with the file and row it comes from.

1. **Fix the CSV catalog.** Most failures name a row or a library file. Correct it, as you would before any import.
2. **Commit** your change.
3. **Run the proof again** until it says PASSED.

If a difference is right and meant, ask a developer to add it to the list of meant differences, with its reason. That list is in `tooling/catalog-import/src/csv_kicad_library/intended.rs`. A difference never passes without a written reason.

## After it passes

Your CSV catalog can move. To try the team catalog with all of it, follow [Try the team catalog by hand](TRY_THE_TEAM_CATALOG.md).

# GitPLM Parts Project
This repository has been forked from [git-plm/parts](https://github.com/git-plm/parts).  Check it out for excellent info on IPN numbering schemes.  All that info has been stripped out in this fork to focus on the KiCad parts database and its day-to-day usage.

## Try the new desktop app
The new app (Rust and Tauri, in `apps/`, `crates/` and `tooling/`) is a prototype. It reads the CSV catalog into a team server that runs only on your computer, so you can try it without changing anything in the repository. You need Rust, [trunk](https://trunkrs.dev/) and KiCad 10.

Build it once, and again after each update:

```bash
cd apps/desktop-ui
trunk build
cd ../..
cargo build --locked -p kicad-parts-desktop --features custom-protocol
```

Start a team server with every CSV file imported and open the app:

```bash
cargo run --locked -p catalog-import -- local-team-server --desktop
```

The console prints three accounts and their passwords, which are new each time. Sign in at the top right of the app. Add `--csv g-res,g-cap` to import only some CSV files, which is faster. Close the app to stop the server and delete its data.

To try the app without the CSV catalog, add `--sample` instead. The team server then holds the 16 made-up parts of the [sample library](fixtures/sample-library/README.md), set up without any CSV import, and the app reads that library's folder.

See [Try the team catalog by hand](docs/operations/TRY_THE_TEAM_CATALOG.md) for what to try.

To check that the whole CSV catalog can move into a team catalog without losing anything, see [Check that your CSV catalog can move to a team catalog](docs/operations/MOVE_THE_CSV_CATALOG.md).

### Checks before a change is done
Build the desktop app as above, then run these from the repository root. All must pass.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -q -p kicad-parts-desktop-ui --target wasm32-unknown-unknown --locked -- -D warnings
cargo test --workspace --locked --no-fail-fast
npm run js-tests --prefix apps/desktop/journeys
cargo run -q --locked -p csv-catalog-fixture -- check .
for f in database/g-*.csv; do n=$(basename "$f" .csv); cargo run -q --locked -p catalog-import -- pilot . "$n"; done
cargo run -q --locked -p catalog-import -- full-pilot
npm run journeys --prefix apps/desktop/journeys
ml doctor
```

The JavaScript tests check the 3D view's camera rules and the team server's own rules, under the journeys' pinned Node.js. The pilots check each CSV file on its own, in an empty team catalog. The full pilot checks the whole CSV catalog together, and the KiCad libraries made from it.

## API credentials (optional)
Some scripts use DigiKey and Mouser APIs. To use them, copy the example secrets file and add your keys:

```bash
cp secrets.env.example secrets.env
```

Then edit `secrets.env` and replace the placeholders with your DigiKey client ID/secret and Mouser API key. Do not commit `secrets.env`; it is gitignored.

## KiCad Parts Manager GUI (older Python app)
The repository also includes the older Python desktop GUI for editing part CSVs, managing alternates, and generating the SQLite database.

### Features
- Category browser for all `database/g-*.csv` files
- Spreadsheet-like editing with undo/redo
- Smart add-part form for RES/CAP/IND (closest standard-value snapping + IPN generation)
- Supplier search against DigiKey and Mouser
- Per-part substitutes stored in `database/substitutes.csv`
- BOM export (wide and long formats)
- SQLite database generation from all CSV categories

### Setup
From the repository root:

```bash
python -m venv .venv
. .venv/bin/activate  # On Windows PowerShell use: .venv\\Scripts\\Activate.ps1
pip install -r gui/requirements.txt
```

### Run

```bash
python gui/main.py
```

### Tests
The GUI includes a test suite covering the SI parser, standard-value snapping, IPN generation, validators, CSV round-trip, SQLite generation, substitutes, and search logic. To run:

```bash
pip install pytest
cd gui
python -m pytest tests/ -v
```

### Supplier columns
All `database/g-*.csv` category files include:
- `DigiKey_PN`
- `Mouser_PN`

These can be set manually or populated from the Supplier Search dialog in the GUI.

## Debugging broken `*.kicad_dbl` files
Sometimes when you modify the `#gplm.kicad_dbl` file, there is a typo and KiCad will no longer load it and does not give you any helpful debugging messages. You can use the [`jq`](https://github.com/jqlang/jq) command line utility to quickly find errors in the file, since the `kicad_dbl` format appears to be JSON.

`jq . \#gplm.kicad_dbl`

## Adding New Parts
If the symbol and footprint already exist, adding a new part is simple as:

1. If you are adding a new part category, create a new `csv` file, then edit `update_db.sh` to add to the list of imports.
2. Add a line to one of the `csv` files. The `csv` files should be sorted by `IPN`. This ensures the `IPN` is unique (which is the lib/db key), and merge operations are simpler if the file is always sorted.
3. run `update_db.sh` (or use the GUI "Generate DB" button)
4. No need to restart KiCad 9

If you need to add a symbol, footprint or 3D model, put it here:

- A symbol goes in the library for its part category, `symbols/g-<category>.kicad_sym` (for example `g-res.kicad_sym`).
- A footprint goes in the library for its package, `footprints/g-<KiCad footprint library>.pretty`, named like the KiCad library where that kind of footprint lives (for example `g-Resistor_SMD.pretty` or `g-Package_SO.pretty`).
- A 3D model goes next to its footprint's library, in `3d-models/g-<footprint library>.3dshapes/`, named like its footprint (for example `3d-models/g-Package_SO.3dshapes/TI_SO-PowerPAD-8.step`).

## Implementation details
The IPN (Internal Part Number) format used is specified in [this document](partnumbers.md).

So we use the following flow:

`CSV -> Sqlite3 -> ODBC -> KiCad`

This might seem overly complex, but it is actually pretty easy as SQLite3 can import `csv` files, so no additional tooling is required. See the [`envsetup.sh`](envsetup.sh) file for how this is done.

`csv` files can be easily edited in [LibreOffice](https://www.libreoffice.org/) or [VisiData](https://www.visidata.org/). **Note, in LibreOffice make sure you import CSV files with character set as `UTF-8` (`UTF-7`, which seems to be the default, will cause bad things to happen)**

A separate `csv` file is used for each [part category](https://github.com/git-plm/gitplm/blob/main/partnumbers.md#three-letter-category-code) (ex: `IND`, `RES`, `CAP`, etc.). There are several reasons for this:

Initially, this part database will be optimized for low-cost rapid prototyping at places like [JLCPCB](https://jlcpcb.com/) and [Seeed Studio](https://www.seeedstudio.com/fusion_pcb.html) using parts from:

- https://jlcpcb.com/Parts
- https://www.seeedstudio.com/opl.html

(this may not work out so the approach may change)

## Directories
- `database` - CSV files and the SQLite3 database file
- `symbols` - custom KiCad symbols
- `footprints` - custom KiCad footprints

## Guidelines
### Reference designators
* [KiCad Library Conventions](https://klc.kicad.org/symbol/s6/s6.1/)
* [Wikipedia](https://en.wikipedia.org/wiki/Reference_designator)
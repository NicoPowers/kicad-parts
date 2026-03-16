# GitPLM Parts Project
This repository has been forked from [git-plm/parts](https://github.com/git-plm/parts).  Check it out for excellent info on IPN numbering schemes.  All that info has been stripped out in this fork to focus on the KiCad parts database and its day-to-day usage.

## API credentials (optional)
Some scripts use DigiKey and Mouser APIs. To use them, copy the example secrets file and add your keys:

```bash
cp secrets.env.example secrets.env
```

Then edit `secrets.env` and replace the placeholders with your DigiKey client ID/secret and Mouser API key. Do not commit `secrets.env`; it is gitignored.

## Library providers (provider-owned aggregate)
The GUI supports multiple provider-owned repositories (symbols, footprints, 3D models, design blocks, and `g-*.csv` data), then generates one aggregate workspace:

- `library-providers.example.yaml` is committed as a template.
- `library-providers.yaml` is local and gitignored.
- `kicad` is a read-only reference source (default symbols/footprints), not an IPN-owning provider.
- Aggregate folders are generated from provider mappings:
  - `symbols/`
  - `footprints/`
  - `3d-models/`
  - `design-blocks/`
  - `database/g-*.csv`
  - `database/provider-lib-table-fragments.txt`

Provider onboarding flow in the GUI:

1. Click `Providers` in the toolbar.
2. Add a provider Git URL.
3. App tries SSH first (`git ls-remote`), then HTTPS fallback using system Git credentials / Git Credential Manager.
4. App scans the repo and suggests symbol/footprint/3D/design-block/database folders.
5. Set a unique 2-3 letter provider prefix (for IPN ownership), confirm mappings, then save.
6. App rebuilds aggregate links and merged CSVs, then regenerates `database/parts.sqlite`.

### Multi-user workflow (open source + per-user providers)

Each clone/fork of this repository uses the same open-source app code, but keeps provider data local per user:

- Committed/shared files: app code, tests, docs, templates (`library-providers.example.yaml`), and KiCad reference submodule declarations.
- Local/per-user files: `library-providers.yaml`, `libs/providers/` checkouts, aggregate folders (`symbols/`, `footprints/`, `3d-models/`, `design-blocks/`), merged `database/g-*.csv`, and `database/provider-lib-table-fragments.txt`.
- On startup, if configured provider checkouts are missing, the app prompts to clone them or continue with available providers.
- Result: anyone can clone/fork this repo and maintain their own provider list without leaking private client repositories or merged part data.

## KiCad Parts Manager GUI
The repository now includes a cross-platform desktop GUI for editing part CSVs, managing alternates, and generating the SQLite database.

### Features
- Category browser for all `database/g-*.csv` files
- Spreadsheet-like editing with undo/redo
- Smart add-part form for RES/CAP/IND (closest standard-value snapping + provider-prefixed IPN generation)
- Supplier search against DigiKey and Mouser
- Per-part substitutes stored in `database/substitutes.csv`
- BOM export (wide and long formats)
- Aggregate SQLite generation from merged provider CSV categories
- Provider-based library indexing with provider-prefixed library nicknames
- Cross-provider part sharing utility (with write-access checks)

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

If you need to add a symbol or footprint, add to the matching `g-XXX.kicad_sym`, or `g-XXX.pretty` libraries.

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
- `database` - committed DB config/scripts plus per-user generated merged CSVs and SQLite output
- `symbols` - generated per-user provider aggregate symbol libraries (gitignored)
- `footprints` - generated per-user provider aggregate footprint libraries (gitignored)
- `3d-models` - generated per-user provider aggregate 3D model links (gitignored)
- `design-blocks` - generated per-user provider aggregate KiCad design blocks (gitignored)
- `libs/providers` - per-user provider repo checkouts (gitignored)

## Guidelines
### Reference designators
* [KiCad Library Conventions](https://klc.kicad.org/symbol/s6/s6.1/)
* [Wikipedia](https://en.wikipedia.org/wiki/Reference_designator)
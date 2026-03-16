# KiCad Parts Manager -- Architecture & Developer Reference

This document describes the full codebase structure, module responsibilities,
data flows, and conventions used throughout the project. Use it as a reference
when adding features or debugging existing behaviour.

---

## Table of Contents

- [High-Level Overview](#high-level-overview)
- [Directory Layout](#directory-layout)
- [Data Flow](#data-flow)
- [Database Layer (`database/`)](#database-layer-database)
  - [CSV Files](#csv-files)
  - [Category Codes](#category-codes)
  - [SQLite Generation](#sqlite-generation)
  - [KiCad Database Library (`#gplm.kicad_dbl`)](#kicad-database-library-gplmkicad_dbl)
  - [Substitutes](#substitutes)
- [KiCad Libraries](#kicad-libraries)
  - [Git Submodules](#git-submodules)
  - [Local Libraries](#local-libraries)
  - [Library Indexing](#library-indexing)
  - [Copy-to-Local Flow](#copy-to-local-flow)
  - [3D Models](#3d-models)
- [GUI Application (`gui/`)](#gui-application-gui)
  - [Entry Point](#entry-point)
  - [Module Map](#module-map)
  - [Main Window (`main_window.py`)](#main-window-main_windowpy)
  - [Smart Add-Part Form (`smart_form.py`)](#smart-add-part-form-smart_formpy)
  - [Generic Add-Part Dialog (`dialogs.py`)](#generic-add-part-dialog-dialogspy)
  - [SI Value Parsing & Formatting (`si_parser.py`)](#si-value-parsing--formatting-si_parserpy)
  - [Standard Values & Snapping (`standard_values.py`)](#standard-values--snapping-standard_valuespy)
  - [IPN Generation (`ipn.py`)](#ipn-generation-ipnpy)
  - [CSV I/O (`csv_manager.py`, `csv_model.py`)](#csv-io-csv_managerpy-csv_modelpy)
  - [Schema Discovery (`schema.py`)](#schema-discovery-schemapy)
  - [Supplier API Integration (`supplier_api.py`, `supplier_dialog.py`)](#supplier-api-integration-supplier_apipy-supplier_dialogpy)
  - [Library Viewer (`lib_viewer.py`)](#library-viewer-lib_viewerpy)
  - [Library Sync (`lib_sync.py`)](#library-sync-lib_syncpy)
  - [Submodule Manager (`submodule_manager.py`)](#submodule-manager-submodule_managerpy)
  - [BOM Export (`bom_export.py`)](#bom-export-bom_exportpy)
  - [Error Handling (`error_handler.py`)](#error-handling-error_handlerpy)
  - [Validation (`validators.py`)](#validation-validatorspy)
  - [Search (`search.py`)](#search-searchpy)
  - [Substitutes Store (`substitutes.py`)](#substitutes-store-substitutespy)
  - [Symbol Units (`symbol_units.py`)](#symbol-units-symbol_unitspy)
  - [Copy Prompt Dialog (`copy_prompt_dialog.py`)](#copy-prompt-dialog-copy_prompt_dialogpy)
  - [SQLite Generator (`db_generator.py`)](#sqlite-generator-db_generatorpy)
- [Test Suite (`gui/tests/`)](#test-suite-guitests)
- [Configuration Files](#configuration-files)
- [IPN Format Reference](#ipn-format-reference)
- [Adding a New Category](#adding-a-new-category)
- [Adding a New Feature -- Checklist](#adding-a-new-feature----checklist)

---

## High-Level Overview

This project manages an electronics parts database for KiCad EDA. The core data
lives in CSV files under `database/`. A cross-platform PyQt6 GUI lets users
browse, edit, and add parts. The GUI can also generate an SQLite database that
KiCad connects to via its Database Libraries (ODBC) feature.

```
CSV files  -->  GUI (edit / add / search)  -->  SQLite  -->  KiCad ODBC
                    |
                    +-->  Supplier APIs (DigiKey, Mouser)
                    +-->  KiCad library preview (symbols, footprints)
                    +-->  BOM export
```

---

## Directory Layout

```
kicad-parts/
├── database/                   # Shared DB config + per-user generated aggregate outputs
│   ├── g-*.csv                 # Generated merged categories (gitignored)
│   ├── substitutes.csv         # Alternate MPN records
│   ├── #gplm.kicad_dbl        # KiCad database library descriptor
│   ├── update_db.sh            # Shell script for SQLite generation
│   ├── provider-lib-table-fragments.txt  # Generated KiCad table helper text (gitignored)
│   └── parts.sqlite            # Generated SQLite database (gitignored)
│
├── symbols/                    # Generated per-user provider aggregate symbol links (gitignored)
│   └── <PREFIX>/*.kicad_sym
│
├── footprints/                 # Generated per-user provider aggregate footprint links (gitignored)
│   └── <PREFIX>/*.pretty/
│       └── *.kicad_mod
│
├── 3d-models/                  # Generated per-user provider aggregate 3D links (gitignored)
├── design-blocks/              # Generated per-user provider aggregate design blocks (gitignored)
│
├── logos/                      # Supplier logos used by GUI assets
│
├── references/                 # API response reference payloads / fixtures
│
├── libs/                       # Git submodules (reference libraries)
│   ├── kicad-symbols/          # KiCad official symbol libraries
│   ├── kicad-footprints/       # KiCad official footprint libraries
│   ├── kicad-library-utils/    # KiCad s-expression parsing utilities
│   └── providers/              # Per-user provider repo checkouts (gitignored)
│
├── gui/                        # PyQt6 GUI application
│   ├── main.py                 # Application entry point
│   ├── requirements.txt        # Python dependencies
│   ├── app/                    # Application package
│   │   ├── __init__.py
│   │   ├── main_window.py      # MainWindow class + delegates
│   │   ├── smart_form.py       # Smart add-part dialog (RES/CAP/IND)
│   │   ├── dialogs.py          # Generic add-part dialog
│   │   ├── si_parser.py        # SI value parsing + formatting
│   │   ├── standard_values.py  # E12/E24/E96 series + snapping
│   │   ├── ipn.py              # IPN parsing + generation
│   │   ├── csv_manager.py      # CSV read/write/backup
│   │   ├── csv_model.py        # Qt table model for CSV data
│   │   ├── schema.py           # Category schema discovery
│   │   ├── supplier_api.py     # DigiKey/Mouser API client
│   │   ├── supplier_dialog.py  # Supplier search dialog
│   │   ├── kicad_lib.py        # Library indexing + search
│   │   ├── lib_viewer.py       # 2D symbol/footprint preview
│   │   ├── lib_sync.py         # Copy symbols/footprints to local
│   │   ├── submodule_manager.py# Git submodule init/update
│   │   ├── bom_export.py       # BOM export (wide/long formats)
│   │   ├── error_handler.py    # Global exception handler + dialog
│   │   ├── validators.py       # Cell and IPN validation
│   │   ├── search.py           # Row search utility
│   │   ├── substitutes.py      # SubstitutesStore (substitutes.csv)
│   │   ├── symbol_units.py     # Multi-unit symbol helpers
│   │   ├── copy_prompt_dialog.py # Copy-to-local confirmation dialog
│   │   └── db_generator.py     # SQLite generator from CSVs
│   │
│   └── tests/                  # pytest test suite
│       ├── conftest.py         # Path setup for `app.*` imports
│       ├── test_csv_manager.py
│       ├── test_db_generator.py
│       ├── test_ipn.py
│       ├── test_kicad_lib.py
│       ├── test_lib_sync.py
│       ├── test_lib_viewer.py
│       ├── test_schema.py
│       ├── test_search.py
│       ├── test_si_parser.py
│       ├── test_standard_values.py
│       ├── test_submodule_manager.py
│       ├── test_supplier_api.py
│       ├── test_substitutes.py
│       └── test_validators.py
│
├── .gitmodules                 # Submodule declarations
├── .gitignore
├── secrets.env.example         # Template for API credentials
├── partnumbers.md              # IPN format specification
├── README.md                   # Setup and usage instructions
└── ARCHITECTURE.md             # This file
```

---

## Data Flow

```
                              ┌──────────────┐
                              │  g-*.csv     │  (source of truth)
                              └──────┬───────┘
                                     │
                  ┌──────────────────┼──────────────────┐
                  │                  │                   │
           ┌──────▼──────┐   ┌──────▼──────┐   ┌───────▼──────┐
           │  GUI tables  │   │ update_db.sh│   │  db_generator │
           │  (PyQt6)     │   │ (shell)     │   │  (Python)     │
           └──────┬───────┘   └──────┬──────┘   └───────┬──────┘
                  │                  │                   │
                  │           ┌──────▼──────────────────▼┐
                  │           │     parts.sqlite          │
                  │           └──────────┬────────────────┘
                  │                      │
                  │              ┌───────▼───────┐
                  │              │  KiCad ODBC   │
                  │              │  (#gplm.dbl)  │
                  │              └───────────────┘
                  │
           ┌──────▼───────┐
           │  BOM export   │  (.csv)
           └──────────────┘
```

- **CSVs are the single source of truth.** The GUI reads and writes them.
- **SQLite is a derived artifact** regenerated from CSVs on demand.
- **KiCad reads the SQLite** through its Database Libraries feature, configured
  by `#gplm.kicad_dbl`.

---

## Database Layer (`database/`)

### CSV Files

Each category has a file named `g-{code}.csv`. Every CSV has at minimum an
`IPN` column. Most also include `MPN`, `Manufacturer`, `Description`, `Symbol`,
`Footprint`, `Datasheet`, `LCSC`, `DigiKey_PN`, and `Mouser_PN`.

Many categories now also include supplier pricing fields:
`DigiKey_Price`, `Mouser_Price`, `Price_Range`, and `Price_LastSynced_UTC`.
These columns are populated by supplier assignment/price-sync workflows in the
GUI and persisted directly in category CSVs.

Category-specific columns:

| Category | Extra Columns |
|----------|---------------|
| `g-res.csv` (Resistors) | `Value`, `Tolerance` |
| `g-cap.csv` (Capacitors) | `Value`, `Voltage`, `Material`, `Tolerance` |
| `g-ind.csv` (Inductors) | `Value`, `Current` |
| `g-dio.csv` (Diodes) | `Value`, `Current`, `Voltage` |
| `g-trs.csv` (Transistors) | `Value`, `Type`, `max(Vce)`, `max(Ic)` |
| `g-osc.csv` (Oscillators) | `Frequency`, `Stability`, `Load` |
| `g-opt.csv` (Opto) | `Color`, `I-forward-max`, `V-forward`, `Wavelength` |
| `g-reg.csv` (Regulators) | `Voltage`, `Current` |
| `g-pwr.csv` (Power) | `Voltage`, `Current` |
| `g-mec.csv` (Mechanical) | `Type` |
| `g-rvr.csv` (Variable Res) | `Resistance`, `Tolerance`, `Turns` |
| `g-rfm.csv` (RF) | `Form` |
| `g-swi.csv` (Switches) | `Form` |
| `g-ana.csv` (Analog ICs) | `Sim_Library`, `Sim_Name`, `Sim_Device`, `Sim_Pins` |
| `g-art.csv` (Artwork) | `Comments` |
| `g-pcb.csv` (PCBs) | `Project link` |

The `Value` column for passive components (RES, CAP, IND) stores a
human-readable SI notation string (e.g., `"4.7k"`, `"10n"`, `"222n"`). The
search system parses SI values numerically so that searching `"0.01u"` matches
a part stored as `"10n"`.

### Category Codes

| Code | Description |
|------|-------------|
| `ana` | Analog ICs: op-amps, comparators, ADC/DAC |
| `art` | Artwork items: fiducials, test points, graphics |
| `cap` | Capacitors |
| `con` | Connectors |
| `cpd` | Circuit protection devices |
| `dio` | Diodes |
| `ics` | Integrated circuits (general) |
| `ind` | Inductors and transformers |
| `mcu` | Microcontrollers and related modules |
| `mec` | Mechanical: screws, standoffs, spacers, etc. |
| `mpu` | Processors, SoMs, and SBCs |
| `opt` | Optoelectronics and optical components |
| `osc` | Oscillators and crystals |
| `pcb` | Printed circuit boards (bare boards) |
| `pwr` | Power components (relays, etc.) |
| `reg` | Voltage/current regulators |
| `res` | Resistors |
| `rfm` | RF modules and RF components |
| `rvr` | Variable resistors / trimmers / potentiometers |
| `swi` | Switches |
| `trs` | Transistors and FETs |

### SQLite Generation

Two paths to generate `parts.sqlite`:

1. **Shell**: `cd database && bash update_db.sh` (uses `sqlite3` CLI)
2. **Python**: The GUI's "Generate DB" button calls `db_generator.generate_sqlite()`.

Both iterate over `g-*.csv` files, create one table per file, and import rows.
The `GPLMLIBS` variable in `update_db.sh` lists all category codes.

### KiCad Database Library (`#gplm.kicad_dbl`)

This JSON file maps SQLite tables to KiCad library entries. Each category
section specifies `table`, `key` (always `IPN`), `symbols`, `footprints`, and a
`fields` array that maps CSV columns to KiCad schematic fields. KiCad reads
this file and connects to `parts.sqlite` via ODBC.

### Substitutes

`substitutes.csv` stores alternate MPNs for a given IPN. Headers:
`IPN,MPN,Manufacturer,Datasheet,Supplier,SupplierPN`. The GUI shows these in a
sub-panel below the main table.

---

## KiCad Libraries

### Git Submodules

Built-in reference libraries are still available through submodules under `libs/`:

| Submodule | Source |
|-----------|--------|
| `libs/kicad-symbols` | `https://gitlab.com/kicad/libraries/kicad-symbols.git` |
| `libs/kicad-footprints` | `https://gitlab.com/kicad/libraries/kicad-footprints.git` |
| `libs/kicad-library-utils` | `https://gitlab.com/kicad/libraries/kicad-library-utils.git` |

These are shallow-cloned (`--depth 1`) and updated on app start via
`SubmoduleWorker` using provider-config-driven submodule path resolution.
The `kicad-library-utils` submodule provides Python modules for parsing
`.kicad_sym` and `.kicad_mod` s-expression files, used by the 2D preview widgets.

3D models (`kicad-packages3D`) are **not** submoduled due to size. They are
resolved in this order when copying a footprint to local:
1. local `libs/kicad-packages3D` (if present in the workspace)
2. remote KiCad GitLab raw URL fallback.

### Provider Model

- Provider mappings are loaded from local `library-providers.yaml` when present.
- `library-providers.example.yaml` is committed as a template/default reference.
- `kicad` is a read-only reference source and does not own IPNs.
- Providers include path mappings for symbols/footprints/3D and optional git
  metadata (`repo_url`, auth mode, verification timestamp).
- On startup, missing provider checkouts are detected; the app can clone them
  immediately or continue in a degraded mode with warnings.
- This keeps the project open-source and multi-user friendly: each user can
  clone/fork the app and maintain their own provider list/checkouts locally.

### Local Libraries

- **Symbols**: `symbols/*.kicad_sym`
- **Footprints**: `footprints/*.pretty/*.kicad_mod`
- **3D Models**: `3d-models/*.step`

Local libraries take priority over reference (submodule) libraries during
resolution. The intent is that parts always reference local copies, ensuring
stability across KiCad updates.

### Library Indexing

`KiCadLibraryIndex` (in `kicad_lib.py`) indexes local + all configured external
providers. It provides:

- `rebuild()` -- re-scans all library directories
- `entries(kind)` -- returns all `LibraryEntry` items for "symbol" or "footprint"
- `search(query, kind)` -- prefix + contains substring search
- `fuzzy_match(tokens, kind)` -- multi-keyword scored search
- `resolve(name, kind)` -- exact `lib:name` lookup returning file path

Each `LibraryEntry` has: `name` (e.g. `"g-pas:R_US"`), `source`, provider
metadata (`provider_id`, `provider_name`), `kind`, `lib_path`, `file_path`.

### Copy-to-Local Flow

When a user selects a symbol or footprint from a non-local provider:

1. `_maybe_copy_external_reference()` in `main_window.py` detects non-local source.
2. `CopyPromptDialog` lets the user pick a target local library.
3. `copy_symbol()` or `copy_footprint()` in `lib_sync.py` performs the copy.
4. For footprints, associated 3D models are fetched via `fetch_3d_model()`,
   first checking provider-mapped 3D roots, then built-in KiCad fallback.
5. Symbol namespaces are rewritten to the target library name.
6. The library index is rebuilt.

### 3D Models

3D model paths inside `.kicad_mod` files use the `${KICAD8_3DMODEL_DIR}`
variable. When copying a footprint locally, `lib_sync.py` rewrites these paths
to point to `3d-models/` and downloads the `.step` files on demand from the
KiCad GitLab repository.

---

## GUI Application (`gui/`)

### Entry Point

```python
# gui/main.py
app = QApplication(sys.argv)
install_global_handler(app)          # global exception dialogs
workspace_root = Path(__file__).resolve().parents[1]
window = MainWindow(workspace_root)
window.show()
app.exec()
```

### Module Map

```
main.py
  └── MainWindow (main_window.py)
        ├── CsvTableModel (csv_model.py)       ← Qt model for table data
        ├── CompleterDelegate                   ← cell editor with autocomplete + browse
        ├── SmartAddPartDialog (smart_form.py)  ← RES/CAP/IND add form
        ├── GenericAddPartDialog (dialogs.py)   ← all other categories
        ├── SupplierSearchDialog (supplier_dialog.py)
        │     └── SearchWorker → SupplierApiClient (supplier_api.py)
        ├── SymbolViewer / FootprintViewer (lib_viewer.py)
        │     └── kicad-library-utils (s-expression parser)
        ├── SubmoduleWorker (submodule_manager.py)
        ├── KiCadLibraryIndex (kicad_lib.py)
        ├── SubstitutesStore (substitutes.py)
        ├── BOM export (bom_export.py)
        └── SQLite generation (db_generator.py)
```

### Main Window (`main_window.py`)

The central orchestrator. Key responsibilities:

- **Sidebar**: category list (uppercase labels, tooltips from `CATEGORY_DESCRIPTIONS`)
- **Table**: `QTableWidget` backed by `CsvTableModel`, with `CompleterDelegate`
  providing autocomplete for Symbol/Footprint columns and browse ("...") buttons
- **Undo/Redo**: `QUndoStack` with `SetCellCommand`, `InsertRowCommand`,
  `DeleteRowCommand`
- **Preview dock**: `SymbolViewer` + `FootprintViewer` + unit picker combo box
- **Toolbar actions**: Add Part, Save, Generate DB, Update Libraries, Search,
  Filter Column, Duplicate Row, Delete Row, Export BOM, Undo, Redo
- **Supplier pricing columns**: `DigiKey_Price`, `Mouser_Price`,
  `Price_LastSynced_UTC` are stored in CSV but hidden in the main table;
  `Price_Range` is visible and color-coded for stale/missing sync state
- **Search**: SI-aware search across all categories (parses SI values for
  numeric equivalence)
- **Filter**: per-column filter with SI-aware comparison for the `Value` column
- **Header display**: `HEADER_DISPLAY_NAMES` maps CSV header names to
  user-friendly display names (e.g., `DigiKey_PN` -> `DigiKey PN`)
- **IPN tooltips**: `IPN_TOOLTIPS` provides category-specific IPN format
  explanations on the column header
- **Read-only IPN**: for `res`, `cap`, `ind` categories the IPN column is locked
- **Context menu workflows**: single-row and multi-row actions support:
  - Open datasheet / supplier links
  - Search similar specs / exact MPN
  - Batch "Assign DigiKey + Mouser PN"
  - Batch "Sync Prices"
  - "Search in KiCad libs" when a symbol/footprint reference is not local

`CompleterDelegate` creates composite editors for Symbol/Footprint columns:
a `QLineEdit` with autocomplete plus a "..." browse button that opens a
`QFileDialog`. Selected references are normalized through the copy-to-local flow.

### Smart Add-Part Form (`smart_form.py`)

Used for resistors, capacitors, and inductors. Features:

- **Mounting type selector**: SMT / Through-Hole, dynamically updates package
  presets
- **Ideal value input**: parsed via `parse_si_value()` (handles `4k7`, `10n`,
  `0.01u`, etc.)
- **Snap to standard**: `snap_resistor()` / `snap_capacitor()` / `snap_inductor()`
  from `standard_values.py`
- **IPN generation**: `generate_resistor_ipn()` / `generate_capacitor_ipn()` /
  `generate_inductor_ipn()` from `ipn.py`
- **Value formatting**: `format_si_value()` produces the human-readable SI
  string stored in the `Value` column
- **Symbol/Footprint**: editable fields with autocomplete, browse buttons, and
  fuzzy match buttons
- **Supplier search**: "Find Supplier Part" button opens `SupplierSearchDialog`

### Generic Add-Part Dialog (`dialogs.py`)

Used for all non-passive categories. Dynamically builds a form from the CSV
headers. Includes:

- **Mounting type selector** (if Symbol/Footprint columns exist)
- **Symbol/Footprint fields** with autocomplete, browse, and match buttons
- **Find Supplier Part** button (if `supplier_dialog_factory` is provided)
- **Fuzzy matching** via `_context_tokens()` which combines category, mounting
  type, and field values into search tokens

### SI Value Parsing & Formatting (`si_parser.py`)

Pure Python, no GUI dependencies. Two main functions:

- `parse_si_value(text) -> float`: Handles `25k`, `4k7`, `0.01u`, `22pF`,
  `100`, etc. Strips unit suffixes (Ω, F, H, W, V, A). Supports inline
  notation (`4k7` = 4700).
- `format_si_value(value) -> str`: Picks the SI prefix giving a coefficient
  between 1 and 999, preferring integers over decimals. Examples:
  `4700 -> "4.7k"`, `22e-12 -> "22p"`, `0.01e-6 -> "10n"`.
- `parse_power_rating(text) -> (float, str)`: Parses power ratings, handling
  fractional notation (`1/10`), SI prefixes, and watt suffixes.

### Standard Values & Snapping (`standard_values.py`)

Contains E12, E24, and E96 series base values. Key functions:

- `snap_to_nearest(value, series) -> SnapResult`: finds the closest standard
  value, returning the snapped value, a 4-character code, and percent error
- `snap_resistor(value_ohms, tolerance)`: uses E96 for 1%, E24 for 5%
- `snap_capacitor(value_farads, tolerance)`: uses E24 for 5%, E12 otherwise
- `snap_inductor(value_henry)`: uses E12
- `encode_resistor_e96(ohms) -> str`: 4-digit resistance code for IPN VVVV
- `encode_capacitor_code(farads) -> str`: 4-digit capacitance code for IPN VVVV

### IPN Generation (`ipn.py`)

IPN format: `PP-CCC-NNNN-VVVV` where:
- `PP` = 2-3 letter provider prefix
- `CCC` = 3-letter uppercase category code
- `NNNN` = 4-digit sequence number (zero-padded)
- `VVVV` = 4-character value/variant code

Functions:
- `parse_ipn(value) -> ParsedIPN | None`
- `is_valid_ipn(value) -> bool`
- `collect_all_ipns(database_dir) -> set[str]`
- `generate_resistor_ipn(prefix, existing, ohms)`: encodes resistance into VVVV
- `generate_capacitor_ipn(prefix, existing, farads)`: encodes capacitance into VVVV
- `generate_inductor_ipn(prefix, existing)`: sequential
- `generate_sequential_ipn(prefix, ccc, existing)`: for all other categories

### CSV I/O (`csv_manager.py`, `csv_model.py`)

**`csv_manager.py`**:
- `CsvDocument`: dataclass holding `path`, `headers`, `rows`, `quote_all`
- `read_csv(path) -> CsvDocument`: reads with auto-detected quoting
- `write_csv(document, make_backup)`: writes back, sorted by IPN, with optional
  `.bak` backup

**`csv_model.py`**:
- `CsvTableModel(QAbstractTableModel)`: wraps headers + rows for Qt table view
- Emits `dirtyChanged` signal for unsaved-changes tracking
- Supports `setData`, `set_cell`, `insert_row`, `delete_row`, `duplicate_row`,
  `sort`
- Highlights duplicate IPNs in red and invalid cells with pink background

### Schema Discovery (`schema.py`)

- `CategorySchema`: frozen dataclass with `key`, `csv_path`, `headers`
- `discover_category_schemas(database_dir)`: scans for `g-*.csv` files, reads
  headers
- `NUMERIC_HINTS`: set of column names treated as numeric
  (`Value`, `Current`, `Voltage`, etc.)
- `URL_COLUMNS`: set of column names rendered as links
  (`Datasheet`, `LCSC`, `DigiKey_PN`, `Mouser_PN`)
- `PRICE_COLUMNS`: supplier pricing/sync metadata fields
  (`DigiKey_Price`, `Mouser_Price`, `Price_Range`, `Price_LastSynced_UTC`)
- `REQUIRED_COLUMNS`: `IPN`, `Description`, `Symbol`, `Footprint`

### Supplier API Integration (`supplier_api.py`, `supplier_dialog.py`)

**`supplier_api.py`**:
- `SupplierPart`: dataclass for search results (source, mpn, manufacturer,
  description, datasheet, digikey_pn, mouser_pn, price, quantity_available,
  product_url)
- `SupplierApiClient`: loads credentials from `secrets.env`, handles OAuth2
  for DigiKey and API key auth for Mouser (`MOUSER_SEARCH_API_KEY`)
- API calls are logged to `logs/supplier_api.log` for diagnostics
- `search_digikey_keyword()`: keyword search via DigiKey Product Information API
- `search_digikey_product_details()` and `search_digikey_product_pricing()`:
  exact MPN-oriented DigiKey lookups
- `search_mouser_keyword()`: keyword search via Mouser Search API
- `search_mouser_partnumber()`: exact/relaxed Mouser part-number lookup
- `search_by_mpn()`: parallel DigiKey + Mouser MPN lookup
- `resolve_supplier_pns()`: derives `DigiKey_PN`, `Mouser_PN`, and price fields
  from an MPN
- `fetch_supplier_prices()`: refreshes price fields using existing supplier PNs
- `search_all()`: parallelized keyword search across both providers

**`supplier_dialog.py`**:
- `SupplierSearchDialog`: hybrid search dialog with:
  - local inventory group (`My Parts`) ranked by confidence score
  - optional remote fallback (DigiKey/Mouser) when local confidence is not high
  - explicit "Search DigiKey + Mouser" override
  - editable local-result cells for quick in-dialog correction of
    MPN/Manufacturer/Description/supplier PN fields
- `SearchWorker(QThread)`: async supplier lookup for UI responsiveness
- `PriceSyncWorker(QThread)`: async price refresh for one selected local row
- `PnAssignWorker` and `PriceSyncBatchWorker`: parallelized batch workflows
  (up to 4 workers) for assigning supplier PNs and syncing prices
- `PnAssignProgressDialog` and `PriceSyncProgressDialog`: modal batch progress
  dialogs with per-row update logs
- Context menu actions include open links, "Search similar specs", and
  "Search same MPN"

### Library Viewer (`lib_viewer.py`)

Provides 2D preview rendering using `QPainter`:

- `SymbolViewer`: renders KiCad symbols from `.kicad_sym` files using the
  s-expression parser from `kicad-library-utils`. Supports multi-unit symbols
  with a unit picker. Includes version-patching fallback for format mismatches.
  Scales text with zoom, hides labels for dense symbols (>80 pins).
- `FootprintViewer`: renders footprints from `.kicad_mod` files. Shows pad
  numbers, scales labels with zoom, hides for dense footprints (>120 pads).
- Both inherit from `_BaseViewer` which provides a `clear()` method to reset
  to "No item selected".

### Library Sync (`lib_sync.py`)

Handles copying KiCad items from reference submodules to local libraries:

- `copy_symbol(src, symbol_name, dest)`: extracts a symbol block from the
  source `.kicad_sym`, rewrites the namespace, and appends to the destination
- `copy_footprint(src, dest_dir, local_3d_dir)`: copies the `.kicad_mod` file,
  downloads associated 3D models, rewrites model paths
- `fetch_3d_model(model_ref, local_3d_dir)`: downloads `.step` files from
  KiCad's GitLab CDN

### Submodule Manager (`submodule_manager.py`)

- `SUBMODULE_PATHS`: tuple of the three submodule relative paths
- `submodules_ready(repo_root)`: checks if all submodules are initialized
- `ensure_submodules(repo_root)`: runs `git submodule update --init --depth 1`
- `update_submodules(repo_root)`: runs `git submodule update --remote --depth 1`
- `submodule_heads(repo_root)`: returns short SHA for each submodule HEAD
- `SubmoduleWorker(QThread)`: runs ensure/update in background, emits
  `completed(ok, output)` signal

### BOM Export (`bom_export.py`)

Two export formats:

- `export_bom_wide()`: one row per part, substitute columns appended as
  `Alt1_MPN`, `Alt1_Manufacturer`, etc.
- `export_bom_long()`: one row per part + substitute combination (normalized)

Both use `SubstitutesStore` to include alternate MPNs.

### Error Handling (`error_handler.py`)

- `ErrorDialog`: modal dialog showing error message, full traceback, and a
  "Copy Error" button
- `show_error_dialog(parent, title, message, details)`: convenience function
- `install_global_handler(app)`: installs custom `sys.excepthook` and
  `threading.excepthook` to catch unhandled exceptions and show them as dialogs
  instead of crashing

### Validation (`validators.py`)

- `is_valid_url(value)`: checks for `http://` or `https://` scheme
- `validate_cell(column, value)`: returns error string if IPN is invalid,
  Datasheet URL is malformed, or required column is empty
- `find_duplicate_values(rows, column)`: returns set of row indices with
  duplicate values (used to highlight duplicate IPNs)

### Search (`search.py`)

- `SearchHit`: dataclass with `category`, `ipn`, `mpn`, `description`
- `search_rows(category, rows, query)`: substring search across IPN, MPN,
  Description fields
- `LocalSearchHit` / `LocalSearchSummary`: scored local search result model with
  confidence tiers (`none`, `low`, `medium`, `high`)
- `rank_rows()` and `search_local_inventory()`: score rows using exact/partial
  string match, token overlap, similarity ratio, and SI-value equivalence
- `should_search_remote()`: controls remote fallback based on local confidence
- `search_components()`: orchestrates unified local+remote search flow

`main_window.py` and `supplier_dialog.py` use this module to decide when to
show local-only results versus when to fetch supplier APIs.

### Substitutes Store (`substitutes.py`)

- `SubstituteRecord`: dataclass for one substitute entry
- `SubstitutesStore`: loads/saves `substitutes.csv`, provides `by_ipn()` and
  `add()` methods

### Symbol Units (`symbol_units.py`)

- `item_matches_variant(item, unit, demorgan)`: checks if a symbol graphic item
  belongs to the selected unit/demorgan variant
- `unit_options_from_symbol(symbol)`: returns list of `(unit_number, label)`
  tuples for the unit picker combo box

### Copy Prompt Dialog (`copy_prompt_dialog.py`)

- `CopyPromptDialog`: modal dialog asking the user to confirm copying a
  reference library item into a local library, with a combo box to select the
  target library and an optional preview widget

### SQLite Generator (`db_generator.py`)

- `generate_sqlite(database_dir, out_db, progress_cb)`: iterates over all
  `g-*.csv` files, creates one SQLite table per file (all columns TEXT), and
  imports rows. Accepts an optional progress callback for the GUI progress
  dialog.

---

## Test Suite (`gui/tests/`)

Run tests from the `gui/` directory:

```bash
cd gui
python -m pytest tests/ -v
```

**`conftest.py`** adds `gui/` to `sys.path` so tests can import `app.*`, and
declares a `live` pytest marker for opt-in supplier API integration tests.

| Test File | Module Under Test | What It Tests |
|-----------|-------------------|---------------|
| `test_csv_manager.py` | `csv_manager` | Read/write CSV, quoting, backup, IPN sorting |
| `test_db_generator.py` | `db_generator` | SQLite table creation from CSVs |
| `test_ipn.py` | `ipn` | IPN parsing, validation, all generation functions |
| `test_kicad_lib.py` | `kicad_lib` | Library indexing, search, fuzzy match |
| `test_lib_sync.py` | `lib_sync` | Symbol/footprint copy, namespace rewriting |
| `test_lib_viewer.py` | `symbol_units` | Unit option generation, variant matching |
| `test_schema.py` | `schema` | Schema discovery, column type inference |
| `test_search.py` | `search` | Row search hits |
| `test_si_parser.py` | `si_parser` | SI value parsing (all notations), power ratings |
| `test_standard_values.py` | `standard_values` | E-series snapping, encoding |
| `test_submodule_manager.py` | `submodule_manager` | Submodule readiness checks |
| `test_supplier_api.py` | `supplier_api` | API parsing, provider-specific edge cases, MPN resolution, optional live supplier integration |
| `test_substitutes.py` | `substitutes` | Store load/save, by_ipn lookup |
| `test_validators.py` | `validators` | URL validation, cell validation, duplicates |

Tests are designed to run without PyQt6 (no GUI dependencies) except where
mocked. The `conftest.py` handles import path setup.

---

## Configuration Files

| File | Purpose |
|------|---------|
| `secrets.env` | API credentials (gitignored). Copy from `secrets.env.example`. |
| `secrets.env.example` | Template showing required keys: `DIGIKEY_CLIENT_ID`, `DIGIKEY_CLIENT_SECRET`, `MOUSER_SEARCH_API_KEY` |
| `library-providers.yaml` | Local provider mappings (gitignored), including provider prefix, symbols/footprints/3D/design-block/database paths. Each user keeps their own file. |
| `library-providers.example.yaml` | Committed provider mapping template (includes `kicad` read-only reference source plus sample IPN-owning provider layout). |
| `.gitmodules` | Declares the three KiCad library submodules |
| `.gitignore` | Ignores per-user generated/provider paths (`libs/providers/`, `symbols/`, `footprints/`, `3d-models/`, `design-blocks/`, `database/g-*.csv`, `database/provider-lib-table-fragments.txt`, `database/parts.sqlite`), `secrets.env`, `library-providers.yaml`, caches and logs |
| `gui/requirements.txt` | `PyQt6`, `requests`, `python-dotenv`, `openpyxl`, `pytest` |

---

## IPN Format Reference

Format: `PP-CCC-NNNN-VVVV`

- **PP**: provider prefix (2-3 uppercase letters, e.g., `SL`, `MA`)
- **CCC**: 3-letter uppercase category code (e.g., `RES`, `CAP`, `IND`, `TRS`)
- **NNNN**: 4-digit sequence number, zero-padded (e.g., `0001`)
- **VVVV**: 4-character value/variant code

For passives, VVVV encodes the component value:
- **Resistors**: E96-encoded resistance (e.g., `0470` for 47 ohms)
- **Capacitors**: pF-notation code (e.g., `0020` for 2pF)
- **Inductors**: sequential (`0001`, `0002`, ...)

For all other categories, VVVV defaults to `0001` (sequential).

See `partnumbers.md` for the full specification.

---

## Adding a New Category

1. **Create the CSV**: `database/g-{code}.csv` with at minimum:
   `IPN,MPN,Manufacturer,Description,Symbol,Footprint,Datasheet,LCSC,DigiKey_PN,Mouser_PN`
   Add category-specific columns as needed. If the category participates in the
   supplier pricing workflow, also include:
   `DigiKey_Price,Mouser_Price,Price_Range,Price_LastSynced_UTC`.

2. **Update `update_db.sh`**: Add the code to the `GPLMLIBS` variable
   (alphabetical order).

3. **Update `CATEGORY_DESCRIPTIONS`** in `gui/app/main_window.py`: add a
   `"code": "Description"` entry.

4. **Update `#gplm.kicad_dbl`**: Add a new library section mapping the SQLite
   table columns to KiCad fields.

5. **Run tests**: `cd gui && python -m pytest tests/ -v` to verify nothing
   is broken.

---

## Adding a New Feature -- Checklist

1. **Identify affected modules** using the module map above.
2. **Write or update tests** in `gui/tests/` for any new logic.
3. **Keep GUI-independent logic separate** from PyQt6 code (e.g.,
   `si_parser.py`, `ipn.py`, `standard_values.py` have no GUI imports).
4. **Use `show_error_dialog()`** for user-facing errors instead of letting
   exceptions propagate.
5. **Use `QThread`** for any long-running operations (API calls, git commands,
   file I/O) to keep the UI responsive.
6. **Update `schema.py`** if adding new column type hints (`NUMERIC_HINTS`,
   `URL_COLUMNS`, `REQUIRED_COLUMNS`).
7. **Update `#gplm.kicad_dbl`** if CSV column changes affect KiCad field
   mapping.
8. **Run the full test suite** before committing.
9. **Update this document** if the change introduces new modules, data flows,
   or conventions.

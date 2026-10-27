# The CSV catalog is gone, and new teams start with the starting categories

The app is a proof of concept with no users, so nothing had to move or stay compatible. The user decided on 2026-10-26 that the repository's CSV catalog goes: `database/` (with `#gplm.kicad_dbl`), `symbols/`, `footprints/` and `3d-models/`. In the end every team, MA included, gets its parts from its own linked library repo (kicad-parts-ff3c). This ADR records what went with it and what replaced the CSV catalog where a kept feature needed it (kicad-parts-e257). It supersedes ADR 0008 and ADR 0009.

- **Removed.** The CSV import into a team server: its route, import runs, re-imports and the import state. The CSV review, import problems and CSV rows that share a part's IPN. CSV values filling in team parts, and the list of CSV-only parts while signed out. The per-file pilots, `full-pilot`, the `csv-catalog` crate and `csv-catalog-fixture`. The library check's frozen copy of today's CSV catalog. `local-team-server --csv`. The old Python app, which only edited the CSV files, and the KiCad library submodules only it used. Journeys 08 and 24.
- **Migrations.** No data had to be kept, so the migrations no longer make what only the import used: `catalog_csv_reviews`, `catalog_import_problems`, the CSV fields of audit entries, and the `csv-import` and `csv-review` Change kinds. A new server starts cleanly; an old data folder must start again empty.
- **Starting categories.** Only the CSV import made categories. Now migration `1792700000_starting_categories.js` sets up the 21 starting categories in one Change, the first time a server starts. Their codes, names, rules and fields are `STARTING_CATEGORIES` in `crates/catalog/src/category.rs`. The fields came from the CSV files' columns. The server reads the same list from `pb_hooks/catalog_starting_categories.json`, and a test keeps the two in step. A superuser still sets up other categories with `POST /api/catalog/categories`.
- **Edit part's symbol library.** When a file replaces one of KiCad's own symbols, Edit part suggested the library of the CSV file the part came from. Now it suggests the part's category's symbol library, such as `g-res`, as Add part does.
- **The local library folder.** `--catalog-root` stays, as the local library folder: the folder whose `g-` libraries the library check, Add part from our libraries, the Libraries browser's Ours and the 3D view read, through `SourceProfile::own()`. It has no default any more. With none set, those features say there is no local library folder yet and point to linking the team's library repo in the Teams menu. Tests and journeys pass the sample library.
- **The library check** compares the local library folder with each CAD item's current revision in the Catalog. A footprint that differs from the Catalog's is to fix; one the Catalog has unchanged is clean-up; one it lacks is a warning.
- **The category guess** is still measured on the 178 parts of the old catalog, from their clues kept in `fixtures/category-guess`.

## Considered options

- **Fold every migration into one.** Rejected: editing the few that made CSV-only collections was smaller, and kept the tests of older data being refused.
- **Set up the starting categories in a hook when the server starts.** Rejected: PocketBase runs migrations after bootstrap hooks, so a fresh server had no tables yet. A migration runs once, at the right time.
- **Keep the repository as the default library folder.** Rejected: it has no libraries now, so every feature would look empty without saying why.

## Consequences

- Making the team's linked library repo its local library folder, and loosening the team server's `g-` naming rule for client libraries, belong to kicad-parts-ff3c.
- ADR 0001's import oracle, ADR 0011's import half, ADR 0019's CSV rows and marks, ADR 0022's import and pilots, and ADR 0023's CSV leftovers no longer apply.
- `catalog-import` keeps its name, though it imports nothing now: it starts local team servers and writes KiCad libraries.

Mulch records: the records of kicad-parts-e257.

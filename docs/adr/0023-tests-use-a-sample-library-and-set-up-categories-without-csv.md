# Tests use a sample library and set up categories without a CSV file

The CSV catalog (`database/g-*.csv`, `#gplm.kicad_dbl`, `symbols/`, `footprints/`, `3d-models/`) is about to be deleted (kicad-parts-e257). Until now most tests and journeys read it, so deleting it would break tests of features that stay. Decided 2026-10-26 by best judgement (kicad-parts-fbc2).

- **A sample library.** `fixtures/sample-library` holds a small made-up library: 6 symbol libraries, 4 footprint libraries, 2 of our own 3D models, lib tables, and `parts.toml` with 7 categories and 16 parts. Every file is real KiCad 10, saved by `kicad-cli`. Each part is there for a reason, which the folder's README lists. It sits beside `fixtures/import`, the repo's other fixture folder, not in a new `tests/fixtures`.
- **Our own naming, no new profile.** The sample library uses the Catalog's names: `g-` libraries and 3D model links into `${GITPLM_PARTS}/3d-models/g-<library>.3dshapes/`. The team server only accepts these, and Add part from our libraries reads our layout through `SourceProfile::own()` (ADR 0022). So the sample library is passed as the catalog root, as the CSV catalog was, and no profile refactor was needed.
- **Linking.** Rust tests take the folder from `sample_library::root()`. A test that needs a git repo gets a throwaway copy, committed under `target/test-folders`, through `SampleRepo`. Git runs there with no global or system settings, a fixture identity, no hooks and no signing. Journeys pass `--catalog-root` to the app; `sampleLibraryCopy` makes a committed copy with a change.
- **Seeding a team server.** `sample_library::seed` sets up the categories, then adds each part the way Add part saves one, with its symbol, footprint and 3D model files. It never uses the CSV import. `catalog-import local-team-server --sample` does the same for people and journeys; `--csv` stays for the CSV features.
- **Setting up a category without a CSV file.** Only the CSV import could make a category. A new route, `POST /api/catalog/categories`, lets the team server's superuser set one up: a Change of kind `category`, revision 1 of its details, and an audit entry. It refuses a category the Catalog already has. The app offers no screen for it yet. Migration `1792600000_category_changes.js` adds the kind; undoing it is refused once such a Change exists, because the older version cannot read it.
- **Seeding leaves remembered matches.** Adding parts through the new-part route records remembered matches, as a person confirming a category does. So a guess in a seeded team says "people chose this category 3 times for reference letter C", not the starting rule. The rules that guess a category are in code (`STARTING_CATEGORIES`), not in the CSV files.
- **An empty CSV catalog.** `csv_catalog::load` returns no parts when the root has no `database/` folder, and the header then shows no CSV words. This keeps the app working on the sample library and after kicad-parts-e257.

## Considered options

- **Keep reading the CSV catalog in tests.** Rejected: deleting it would break tests of features that stay.
- **Seed team servers with a CSV import of sample CSV files.** Rejected: the CSV import goes with kicad-parts-e257.
- **Write records straight into PocketBase.** Rejected: it would skip the team server's own rules and could save data no person could.
- **A sample library with client-style names.** Rejected for now: the team server refuses those names, so Add part could not save its parts.

## Consequences

- Deleting the four CSV catalog folders breaks only tests of features kicad-parts-e257 removes: the CSV import, CSV reviews, import problems, CSV values filling team parts, the pilots, `csv-catalog-fixture`, and the library check of today's CSV catalog. Tests of those stay in `*_csv.rs` files, `pocketbase_import.rs` and the import crates.
- The symbol library Add part suggests for a part that uses one of KiCad's symbols still comes from the CSV file only. kicad-parts-e257 must rework it.
- Signed out, with no team, the part list says "No parts match.", which is wrong when there is nothing to search. kicad-parts-e257 should say why the list is empty.
- The library check and some Add part text still name the CSV catalog and imports.

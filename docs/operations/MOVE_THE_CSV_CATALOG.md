# Check that your CSV catalog can move to a team catalog

This guide is for the engineer who looks after a team's CSV catalog: the `database/g-*.csv` files and the `symbols/`, `footprints/` and `3d-models/` libraries. One command, the full-catalog proof, shows what moving them into a team catalog would do. It shows that nothing is lost, and that KiCad gets what the CSV catalog meant it to get.

The proof does not move anything yet. It imports a copy into throwaway team servers on your computer, checks the result and deletes it. It only reads the CSV catalog: it never changes a file in `database/`, `symbols/`, `footprints/` or `3d-models/`, and it checks that at the end.

## Before you start

You need Rust and a clone of this repository with your CSV catalog in it. The first run downloads PocketBase, the team server, so it needs the internet once. You do not need KiCad for the proof.

Commit your CSV catalog first. A team catalog only takes files that are committed, so everyone can check out exactly what was imported. If you only want to try it, add `--allow-uncommitted-changes` to the command.

## Run the proof

From the repository root, in PowerShell or Git Bash:

```
cargo run --locked -p catalog-import -- full-pilot
```

It takes about a minute. The proof:

1. **Imports every CSV file** into a new team server, one file after another, as your team would. Then it imports them all again, to show a second import changes nothing.
2. **Checks that nothing is lost.** It compares every field of every row with the CSV files, one by one. It also checks `substitutes.csv`, every symbol, footprint and 3D model your rows use, and that no IPN is in two CSV files.
3. **Makes the KiCad libraries** your team would use, such as `MA-Parts`.
4. **Makes them again** from a second, separate team server, and checks both are the same, byte for byte.
5. **Compares them with the CSV catalog's own KiCad library**, `database/parts.sqlite`.
6. **Checks that the CSV catalog is unchanged.**

The team servers and libraries are deleted when the proof ends.

## Read the report

The first line says **PASSED** or **FAILED**. Then each step has a short section.

- **Import** says how many files and rows were imported, and what the library check found. "Not in the Catalog yet" counts symbols and footprints in your libraries that no row uses. The desktop app's **Library check** view lists them.
- **Nothing lost** counts the fields it checked and the CAD files stored with their license. It lists the **import problems**: things that could not become part of the catalog, such as a footprint whose 3D model file is missing. An import problem keeps the value, so nothing is dropped. Fix them in the CSV catalog when you can.
- **Not imported, and why** lists the few things the import never takes in, each with its reason. For example, the old `update_db.sh` script left a table out, but the import reads the CSV files themselves.
- **KiCad libraries** says how many entries KiCad gets. Each part revision is one entry, such as `RES-0000-0470_v1`.
- **Compared with the CSV catalog's own KiCad library** (built from today's CSV files) lists every difference, grouped, each with its reason. For example, supplier part numbers and prices are not sent to KiCad, and a bare board with no symbol gets no entry. Add `--every-difference` to see each difference on its own line.
- **Time** says how long each step took.

## If the proof fails

The end of the report lists each thing that was lost or differs without a reason, with the file and row it comes from.

1. **Fix the CSV catalog.** Most failures name a row or a library file. Correct it, as you would before any import.
2. **Record your change in the CSV catalog fixture.** The proof compares everything with this fixture, a record of every file in the CSV catalog. Each time you change a file in `database/`, `symbols/`, `footprints/` or `3d-models/` on purpose, record it again:

   ```
   cargo run --locked -p csv-catalog-fixture -- write .
   ```

   If you forget, the proof stops at the start and tells you to.
3. **Commit** your change together with `fixtures/import/csv-catalog/manifest.json`.
4. **Run the proof again** until it says PASSED.

`database/parts.sqlite` is a frozen copy that nothing can build again, so it keeps your old values. The proof compares with your CSV files instead. It lists each place where `parts.sqlite` is older as `parts-sqlite-is-older`, which is fine once the fixture records the change.

If a difference is right and meant, ask a developer to add it to the list of meant differences, with its reason. That list is in `tooling/catalog-import/src/csv_kicad_library/intended.rs`. A difference never passes without a written reason.

## Give new IPNs in one place while you move

While your team moves from the CSV catalog to a team catalog, people can add parts in both. Two people can then give one IPN to two different parts. To stop that, choose one place for new IPNs and tell the whole team:

- **Add new parts in the app.** Add part offers the next free IPN. It checks the team catalog and the CSV files in your folder. It checks the CSV files again just before it saves, and if a row took the IPN in the meantime, it saves nothing and offers the next free IPN.
- **Stop adding rows to the CSV files.** If someone must add a row, they commit and push it at once, and everyone pulls before they add a part.

The app only sees the CSV files in your own folder. It cannot see a row that is still on someone else's computer, not yet pushed or pulled.

If two parts get one IPN anyway, the import finds it. A CSV row is a different part when its IPN belongs to a part added in the app and the row names none of that part's MPNs. A row with no MPN counts as a different part too, because nothing shows it is the same part. The import then:

- leaves the app's part exactly as it is;
- records the import problem `ipn-given-to-another-part`, which keeps the whole row and says which part has the IPN;
- shows "A CSV row has this IPN for another part" on that part in the app.

No CSV review can take such a row. **Give the CSV row another IPN**, commit, and import again. If the row and the part really are one part, give them the same MPN instead.

Add part also cannot use an IPN that a row of the latest import has, even a row that could not become a part, such as a row with too many fields. The team server refuses it.

## Not done yet

- **No import into your team's own server yet.** The proof only imports into throwaway servers. Importing into a team server that keeps running is kicad-parts-0694.
- **Footprint alternatives.** A row can list more than one footprint, such as `g-Package_DFN_QFN:WQFN20_RUK_TEX-L;WQFN20_RUK_TEX`. Only the first, with its library name, becomes part of the team's KiCad libraries. The others stay as written, as KiCad footprint filters. Keeping them as real alternatives is kicad-parts-d608.
- **The LCSC part number in KiCad.** The CSV catalog's own KiCad library showed LCSC on some parts. The team's KiCad libraries leave all supplier data out. Whether KiCad parts should show the LCSC number is kicad-parts-ed4a.

## After it passes

To try the team catalog with your whole CSV catalog, follow [Try the team catalog by hand](TRY_THE_TEAM_CATALOG.md).

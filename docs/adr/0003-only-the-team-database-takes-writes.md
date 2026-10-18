# Only the team server's database takes writes

Each database has one job. The team server's PocketBase database is the only one people change. Each desktop keeps a copy that applies the server's numbered Changes in order. Files that KiCad reads, such as a database library, are generated again from those records. We sync records, never SQLite files, and no SQLite file has two writers.

## Consequences

- The desktop copy is in `crates/catalog/src/replica.rs`. It ignores a Change it has already applied, so nothing is missed or applied twice. At sign-in it reads every part's current state, then applies the Changes saved meanwhile, so sign-in does not slow down as the history grows.
- Every desktop must hear of every Change and make what it saved current. So the server's write guards keep both directions in step. A part moves only to what the Change in the same transaction saved. Everything a Change saves for a part moves the part: new details, a new revision or an archive entry. A transaction saves at most one Change, and every new record names it. A refused write is a programming error, so it answers 500 and is logged.
- The desktop writes the KiCad database library with `crates/kicad-libraries`, from one thread, holding a lock file so no second writer runs. It changes the SQLite database in place, in one transaction, so each table KiCad reads is whole. KiCad's background refresh reads one table at a time, so during one refresh a moving row can show in two groups or in none; its links still resolve, because KiCad then looks in every table.

Mulch records: mx-95dadd, mx-0350e0, mx-ab5e0a.
